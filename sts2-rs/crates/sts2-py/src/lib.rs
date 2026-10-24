//! Python module: `sts2_rs`.
//!
//! Built with maturin. After `maturin develop` in the active venv,
//! `import sts2_rs` works alongside `import sts2_env`.
//!
//! Phase 0 surface (bead sts2-rs-r5i): `CombatState` is constructible with a
//! seed, exposes `get_state()` returning the bridge-shape dict, and accepts
//! `apply(action_dict)` calls. `__deepcopy__` / `__copy__` / `__repr__` are
//! implemented so Python ergonomics work end-to-end.
//!
//! Per the plan (`docs/plans/2026-05-06-rust-porting-plan-v2.1-refined.md`
//! line 67), the Phase-0 done-when condition is:
//!
//! > A Python script can do `import sts2_rs;
//! > cs = sts2_rs.CombatState(seed=0, ...); print(cs.get_state())` and the
//! > result deserializes via `BridgeReplayTrace.from_dict` without raising.
//!
//! Phase 1+ fills in real `apply` semantics, `legal_actions`, `state_hash`,
//! and `pending_choice`.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};
use smallvec::SmallVec;
use std::collections::HashSet;

use sts2_content::cards::dispatch as card_dispatch;
use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_content::monster_dispatch::{build_ai_with_ascension, MonsterSpawnArgs, SpawnPosition};
#[cfg(test)]
use sts2_core::cards::CardType;
use sts2_core::cards::{Card, PileKind, VarKey};
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    apply as core_apply, eval_features as core_eval_features,
    eval_features_wide as core_eval_features_wide,
    evaluate_in_progress_raw as core_evaluate_in_progress_raw, legal_actions as core_legal_actions,
    parity, state_hash as core_state_hash, terminal_value as core_terminal_value, Action, CardUid,
    ChoiceKind, ChoiceOption, CombatPhase, CombatState as CoreCombatState, Creature, CreatureUid,
    IntentHistory, MonsterId, MonsterKind, PendingChoice, PlayerCombatState, Side, TurnEvents,
    EVAL_FEATURE_NAMES, EVAL_FEATURE_NAMES_WIDE, EVAL_FEATURE_WEIGHTS,
};
use sts2_mcts::{
    hint as mcts_hint, par_search_with_policy, LeafEval, Mcts, MctsConfig, SharedPolicy,
    TerminalEvalMode,
};
use sts2_wire::emit::mcts::RecommendParams;
#[cfg(test)]
use sts2_wire::emit::names::card_type_python_name;
use sts2_wire::emit::names::choice_kind_python_name;

// The `#[global_allocator]` (mimalloc) and its purge setting.
pub mod allocator;

// The bundled `EmbeddingValueNet` policy prior was REMOVED on 2026-08-15.
//
// It was an Expert-Iteration net trained on an all-Ironclad corpus,
// `include_bytes!`-baked into the extension and applied by default. Measured
// on the 168-position book (`logs/2026-08-15-policy-prior/FINDINGS.md`) it
// ranked actions barely above random on the one character it was gated to
// (top-1 0.200 against a 0.154 uniform expectation) and *below* random on the
// other four (Necrobinder 0 of 34). Turning it on cost Ironclad **21.3pp** of
// gold-action agreement at 1000 sims, 24 paired seeds, p < 1e-4 — the search
// stopped searching and played the prior's top pick (median visit ratio 509).
// `experiments/necro-vs-ic-lift` had independently measured -1.9pp win rate
// for Ironclad and ~-21pp for Necrobinder.
//
// Search now always uses `SharedPolicy::uniform()` with `use_puct_prior =
// false`, which is documented byte-identical to the plain `Mcts::new` /
// `par_search` path. The generic PUCT machinery it rode on (`LearnedPolicyPrior`,
// `MctsConfig::use_puct_prior`) is deliberately KEPT in `sts2-mcts`: it is
// what a future Gumbel/Sequential-Halving root schedule would need, it is
// exercised by `corpus_gen --policy-prior-weights`, and the same FINDINGS run
// measured the PUCT branch itself as free over `c_puct` in [2.5, 10]. What was
// removed is the artifact and the code that applied it, not the mechanism.

pub mod combat_surrogate;
pub mod heuristic_agent;
pub mod mcts_combat_agent;
pub mod ppo_agent;
/// `tract`-backed twin of `ppo.onnx_policy.OnnxPolicy` — scores an
/// already-built observation, so it covers every surface the Python
/// builders emit.
pub mod ppo_policy;
/// Wire-driven PPO run policy — the advisor lane's non-combat decisions,
/// built and scored entirely in Rust.
pub mod ppo_wire;
pub mod run_manager;
pub mod state_dict;

pub use crate::run_manager::PyRunManager;

/// `serde_json::Value` → Python object: the ONE conversion every dict this
/// crate hands out now goes through. The dicts themselves are built in
/// [`sts2_wire::emit`], so a wasm workbench and this module read the same
/// shape from the same function (parity by construction).
pub(crate) fn value_to_py<'py>(
    py: Python<'py>,
    value: &serde_json::Value,
) -> PyResult<Bound<'py, PyAny>> {
    pythonize::pythonize(py, value)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("pythonize failed: {e}")))
}

/// [`value_to_py`] for a value known to be an object.
pub(crate) fn value_to_pydict<'py>(
    py: Python<'py>,
    value: &serde_json::Value,
) -> PyResult<Bound<'py, PyDict>> {
    let obj = value_to_py(py, value)?;
    obj.downcast_into::<PyDict>().map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!("emitter returned a non-dict: {e}"))
    })
}

/// [`value_to_py`] for a value known to be an array.
pub(crate) fn value_to_pylist<'py>(
    py: Python<'py>,
    value: &serde_json::Value,
) -> PyResult<Bound<'py, PyList>> {
    let obj = value_to_py(py, value)?;
    obj.downcast_into::<PyList>().map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!("emitter returned a non-list: {e}"))
    })
}

/// [`sts2_wire::emit::EmitError`] → `ValueError`, the exception the
/// hand-built `PyDict` code raised for the same faults.
pub(crate) fn emit_err(e: sts2_wire::emit::EmitError) -> PyErr {
    pyo3::exceptions::PyValueError::new_err(e.to_string())
}

pub fn build_seeded_state(seed: u64, ascension: u8) -> CoreCombatState {
    CoreCombatState {
        rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),
        run_rng: sts2_core::run_rng_set::RunRngSet::new((seed) as i32),
        ascension,
        master_deck: Vec::new(),
        last_added_card: None,
        is_elite: false,
        is_boss: false,
        act_index: 1,
        ..CoreCombatState::default()
    }
}

fn parse_encounter(name: &str) -> Result<Vec<(MonsterKind, IntentHistory)>, String> {
    match name {
        // STS2-native encounter names. The legacy STS1-stub arms
        // (Cultist / JawWorm / AcidSlimeS / GreenLouse / Sentries)
        // were removed in bead sts2-rs-v1u5 along with the underlying
        // `MonsterKind` variants. Python scripts that still reference
        // those names need to migrate to the STS2-native analogues
        // below.
        "Nibbit" => Ok(vec![(MonsterKind::Nibbit, IntentHistory::default())]),
        "Mawler" => Ok(vec![(MonsterKind::Mawler, IntentHistory::default())]),
        "LeafSlimeS" => Ok(vec![(MonsterKind::LeafSlimeS, IntentHistory::default())]),
        "TwigSlimeS" => Ok(vec![(MonsterKind::TwigSlimeS, IntentHistory::default())]),
        "ShrinkerBeetle" => Ok(vec![(
            MonsterKind::ShrinkerBeetle,
            IntentHistory::default(),
        )]),
        "ShrinkerBeetles" => Ok(vec![
            (MonsterKind::ShrinkerBeetle, IntentHistory::default()),
            (MonsterKind::ShrinkerBeetle, IntentHistory::default()),
        ]),
        other => Err(format!(
            "unknown encounter {other:?}; supported: \
             Nibbit, Mawler, LeafSlimeS, TwigSlimeS, ShrinkerBeetle, ShrinkerBeetles"
        )),
    }
}

fn build_encounter_state(
    seed: u64,
    encounter: &str,
    ascension: u8,
) -> Result<CoreCombatState, String> {
    // Bead sts2-rs-ov4 (P6-1): populate the PlayCard dispatch registry
    // before the first `Action::PlayCard` can fire. Idempotent.
    card_dispatch::install_default_registry();

    let monsters = parse_encounter(encounter)?;

    // Build the runtime card arena from the starter-deck CardSpecs.
    // Each CardSpec yields one `Card` runtime instance with a fresh UID;
    // the UID is also pushed onto the draw pile. Before bead sts2-rs-y97
    // (vertical-slice P6 expansion) this loop only filled the draw pile —
    // `state.cards` stayed empty and `Action::PlayCard` couldn't look up
    // card properties. Now `state.cards[uid - 1]` resolves to the full
    // runtime card (id / cost / damage / block / target_type).
    let deck = ironclad_starter_deck();
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    let mut cards_arena: Vec<Card> = Vec::with_capacity(deck.len());
    for (idx, spec) in deck.iter().enumerate() {
        let raw = u32::try_from(idx + 1).expect("deck size fits in u32");
        let uid = CardUid::new(raw).expect("non-zero");
        draw.push(uid);
        cards_arena.push(Card {
            uid,
            card_id: spec.id,
            cost: spec.cost,
            card_type: spec.card_type,
            target_type: spec.target_type,
            upgraded: false,
            base_damage: spec.base_damage,
            base_block: spec.base_block,
            ..Card::pod_defaults()
        });
    }

    let player = PlayerCombatState {
        hand: SmallVec::new(),
        draw,
        discard: SmallVec::new(),
        exhaust: SmallVec::new(),
        play: SmallVec::new(),
        energy: 0,
        max_energy: 3,
        stars: 0,
        current_hp: 80,
        max_hp: 80,
        block: 0,
        powers: SmallVec::new(),
        // The deck built above is always `ironclad_starter_deck()` (this
        // helper has no character parameter), so the player character must
        // say so. This used to be load-bearing for the character-gated
        // policy prior (GH#294); that prior is gone, but an accurate
        // character on a state that genuinely is Ironclad is still correct
        // and other surfaces read it.
        character: Some(sts2_core::CharacterId::Ironclad),
        potions: smallvec::SmallVec::new(),
        potion_slot_indices: smallvec::SmallVec::new(),
        potion_slots: 0,
        orbs: sts2_core::OrbSlots::default(),
        creature_uid: None,
        pet_uid: None,
        shivs_played_this_turn: 0,
        cards_played_this_turn: [0; 5],
        powered_hits_this_turn: 0,
        powered_hits_by_target_this_turn: smallvec::SmallVec::new(),
        powered_hits_at_card_play_start: 0,
        attack_command_suppressed_this_play: false,
        vigor_latch_card: None,
        generated_cards_this_combat: 0,
        skills_played_this_turn: 0,
        card_plays_started_this_turn: 0,
        cards_first_in_series_this_turn: 0,
        request_end_turn_after_play: false,
        retain_this_turn: smallvec::SmallVec::new(),
        sly_this_turn: smallvec::SmallVec::new(),
        sly_this_combat: smallvec::SmallVec::new(),
        pending_retain_count: 0,
        stars_gained_this_turn: 0,
        pinned_top: smallvec::SmallVec::new(),
        pinned_bottom: SmallVec::new(),
        deferred_draw_order: false,
    };

    let rng = sts2_core::dotnet_rng::Rng::new((seed) as i32);
    let mut run_rng = sts2_core::run_rng_set::RunRngSet::new(seed as i32);
    let mut creatures = Vec::new();
    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    let mut taken_hp: HashSet<i32> = HashSet::new();

    for (i, (kind, history)) in monsters.into_iter().enumerate() {
        let uid_val = u32::try_from(i + 1).expect("monster index fits u32");
        let uid = CreatureUid::new(uid_val).expect("non-zero");
        let hp = kind.starting_hp_unique(&mut run_rng.niche, &taken_hp, ascension);
        taken_hp.insert(hp);
        creatures.push(Creature {
            id: uid.0,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: SmallVec::new(),
            species: Some(kind),
            monster_id: MonsterId(kind.python_name()),
            intent: None,
            intent_history: history,
            ai: None,
        });
        enemy_uids.push(uid);
    }

    let mut state = CoreCombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng,
        run_rng,
        creatures,
        cards: cards_arena.into(),
        powers: Vec::new().into(),
        relics: Vec::new().into(),
        potions: Vec::new(),
        player,
        allies: SmallVec::new(),
        enemy_uids,
        pending_choice: None,
        resolving_card: None,
        pending_play: None,
        pending_turn_setup_resume: false,
        pending_draw_resume: None,
        pending_snecko_cost_randomize: false,
        pending_cluster_resume: None,
        pending_hand_draw_resume: None,
        pending_relic_hand_draw_resume: None,
        pending_wlp_retain_resume: false,
        pending_end_turn_resume: None,
        pending_autoplay_resume: None,
        autoplay_inline_resolve: false,
        pending_curse_choice: None,
        pending_potion_hook_resume: None,
        pending_enemy_exec_resume: None,
        combat_log: TurnEvents::default(),
        swipe_stolen: Vec::new(),
        escaped_creatures: 0,
        heist_recovered_gold: 0,
        event_combat_timed_out: false,
        gold: 0,
        is_over: false,
        player_won: false,
        ascension,
        master_deck: Vec::new(),
        last_added_card: None,
        is_elite: false,
        is_boss: false,
        act_index: 1,
        active_attack: None,
        energy_wasted: 0,
        cards_drawn: 0,
        pending_run_gold_loss: 0,
        last_drawn_card: None,
    };

    // Apply on-spawn self-buffs that were skipped because we built
    // creatures by direct push rather than via `spawn_enemy`.
    // enemy_uids has already been moved into `state` above, so iterate
    // its copy via `state.enemy_uids.clone()`.
    for uid in state.enemy_uids.clone() {
        state.apply_creature_spawn_buffs(uid);
    }

    start_combat(&mut state);
    start_player_turn(&mut state);
    Ok(state)
}

/// Build a fully-initialized in-combat [`CoreCombatState`] from a scenario
/// spec (character, deck, HP, relics, potions, encounter, ascension), ready to
/// receive `legal_actions()` / `apply()` mid-turn-1.
///
/// The construction now lives in [`sts2_wire::scenario::build_scenario_state`]
/// so the browser SimLab sandbox rebuilds byte-identical boards; this is the
/// thin oracle-side delegate every PyO3 caller here still routes through.
#[allow(clippy::too_many_arguments)]
fn build_scenario_state_impl(
    seed: u64,
    character: &str,
    deck_ids: &[String],
    hp: i32,
    max_hp: i32,
    relic_ids: &[String],
    potion_ids: &[String],
    encounter: &str,
    ascension: u8,
    monster_override: Option<&[String]>,
    monster_move_states: Option<&[Option<String>]>,
) -> Result<CoreCombatState, String> {
    sts2_wire::scenario::build_scenario_state(
        seed,
        character,
        deck_ids,
        hp,
        max_hp,
        relic_ids,
        potion_ids,
        encounter,
        ascension,
        monster_override,
        monster_move_states,
    )
}

fn run_playout(state: &mut CoreCombatState) -> (bool, u32) {
    let mut turns = 0u32;
    while !state.is_over && turns < 1000 {
        core_apply(state, Action::EndTurn);
        turns += 1;
    }
    (state.player_won, turns)
}

/// Play `state` to terminal with the expert heuristic policy, returning
/// `(player_won, final_hp, actions)`.
///
/// This is the **baseline arm** for search-vs-baseline A/Bs. It exists
/// because [`run_playout`] (behind the `rollouts` pyfunction) only ever
/// applies [`Action::EndTurn`] — it plays no cards, loses essentially every
/// fight, and is therefore useless as an A/B denominator: a "lift over
/// baseline" computed against it collapses to the search arm's own win
/// rate, identically for every character.
///
/// Uses the same `pick_action_heuristic` (softmax over expert play
/// priority) that MCTS uses to drive its rollouts, so "MCTS minus this"
/// isolates the contribution of *search* over the policy search already
/// rolls out with.
fn play_heuristic_to_terminal(
    state: &CoreCombatState,
    seed: u64,
    max_actions: u32,
) -> (bool, i32, u32) {
    use rand::SeedableRng;
    let mut s = state.clone();
    let mut rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(seed);
    let mut actions = 0u32;
    while !core_terminal_value(&s).0 && actions < max_actions {
        let legal = core_legal_actions(&s);
        if legal.is_empty() {
            let _ = core_apply(&mut s, Action::EndTurn);
            actions += 1;
            continue;
        }
        let idx = sts2_mcts::expert::pick_action_heuristic(&mut rng, &s, &legal);
        let _ = core_apply(&mut s, legal[idx].clone());
        actions += 1;
    }
    (s.player_won, s.player.current_hp, actions)
}

/// Expert-heuristic playout to terminal — the A/B baseline arm.
///
/// See [`play_heuristic_to_terminal`]. Prefer this over `rollouts` for any
/// measurement that needs a *playing* baseline rather than a do-nothing one.
/// The arena's `STS2_RUNAI_POTION_SWAP` resolver, verbatim
/// (`sts2_runai::policy::resolve_potion_swap_flag_from_env`): `true` only
/// when the swap seam is on. `PpoRunEnv` passes this as
/// `interactive_potion_swap` so training and arena RunManagers can never
/// disagree on the flag (plan 2026-08-20-ppo-potion-bootstrap P2 — ONE
/// source of truth).
#[pyfunction]
fn resolve_potion_swap_flag() -> bool {
    sts2_runai::policy::resolve_potion_swap_flag_from_env()
}

/// Whether a surfaced full-belt swap is decided by the OC value rule rather
/// than by the run policy — `sts2_runai::policy::potion_swap_is_rule_side_from_env`,
/// verbatim. `PpoRunEnv._advance` gates its rule-side branch on this so it
/// mirrors `MetaWithRules::choose` exactly; the two must agree or G1 breaks.
#[pyfunction]
fn potion_swap_is_rule_side() -> bool {
    sts2_runai::policy::potion_swap_is_rule_side_from_env()
}

#[pyfunction]
#[pyo3(signature = (state, seed = 0, max_actions = 2000))]
fn heuristic_play_to_terminal(
    py: Python<'_>,
    state: &PyCombatState,
    seed: u64,
    max_actions: u32,
) -> PyResult<PyObject> {
    let starting_hp = state.inner.player.current_hp;
    let (won, final_hp, actions) = play_heuristic_to_terminal(&state.inner, seed, max_actions);
    let d = pyo3::types::PyDict::new(py);
    d.set_item("player_won", won)?;
    d.set_item("final_hp", final_hp)?;
    d.set_item("starting_hp", starting_hp)?;
    d.set_item("dmg_taken", starting_hp - final_hp)?;
    d.set_item("actions", actions)?;
    d.set_item("timed_out", actions >= max_actions)?;
    Ok(d.into())
}

/// Decode a bridge-shape action dict into a core [`Action`].
///
/// Extracted from [`PyCombatState::apply`] so the decoder can be reused
/// without going through a `PyCombatState`. Reads the same dict shape that [`PyCombatState::legal_actions`] emits and
/// decodes the per-variant fields (`card_index`, `target_index`, `slot`,
/// `index`) into the matching [`Action`] variant. An unknown or missing
/// `"action"` value raises `ValueError` rather than silently ending the
/// turn — masking that bug previously produced false-positive `STATE_DRIFT`
/// in the parity-fuzz harness.
fn parse_bridge_action(dict: &Bound<'_, PyDict>) -> PyResult<Action> {
    let action_kind: String = dict
        .get_item("action")?
        .ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err("apply: missing required key 'action'")
        })?
        .extract()
        .map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "apply: 'action' must be a string ({e})"
            ))
        })?;

    // Extract an `Option<u8>` from key `name`. Missing key, explicit
    // `None`, or `null` all map to `None`; any present scalar is
    // cast via `usize -> u8` to mirror the `legal_actions` emit.
    let opt_u8 = |name: &str| -> PyResult<Option<u8>> {
        match dict.get_item(name)? {
            Some(v) if !v.is_none() => {
                let raw: usize = v.extract().map_err(|e| {
                    pyo3::exceptions::PyValueError::new_err(format!(
                        "apply: '{name}' must be a non-negative integer ({e})"
                    ))
                })?;
                let narrowed = u8::try_from(raw).map_err(|_| {
                    pyo3::exceptions::PyValueError::new_err(format!(
                        "apply: '{name}'={raw} out of range for u8"
                    ))
                })?;
                Ok(Some(narrowed))
            }
            _ => Ok(None),
        }
    };
    let req_u8 = |name: &str| -> PyResult<u8> {
        let raw: usize = dict
            .get_item(name)?
            .ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "apply: missing required key '{name}'"
                ))
            })?
            .extract()
            .map_err(|e| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "apply: '{name}' must be a non-negative integer ({e})"
                ))
            })?;
        u8::try_from(raw).map_err(|_| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "apply: '{name}'={raw} out of range for u8"
            ))
        })
    };

    match action_kind.as_str() {
        "play" => Ok(Action::PlayCard {
            hand_index: req_u8("card_index")?,
            target: opt_u8("target_index")?,
        }),
        "end_turn" => Ok(Action::EndTurn),
        // GH #303: `"potion"` is the back-compat spelling of
        // `"drink_potion"` and now parses straight to the canonical
        // variant, so nothing downstream has to know the alias exists.
        // (`Action::UsePotion` also delegates in `sts2_core::apply` — the
        // net for anyone constructing the variant directly.)
        //
        // `slot` is the ENGINE index here: the engine keeps `potions`
        // dense, so slot i is the i-th held potion. The bridge/mod
        // dialect uses the same key for the ABSOLUTE belt slot
        // (`SafeGetPotionAtSlot`, empties included) — callers crossing
        // that boundary must remap first, which is what
        // `realgame_mcts._remap_potion_slot` exists to do.
        "potion" | "drink_potion" => {
            // The bridge spells DISCARD as the same verb plus
            // `discard: true` (`sts2_realgame.discard_potion`,
            // `realgame_random`). The engine has no combat-level discard
            // action, so there is nothing faithful to decode this to —
            // and pre-GH#303 it was masked by `UsePotion` no-op'ing.
            // Now that the verb really drinks, silently accepting the
            // flag would turn "throw it away" into "drink it": a state
            // divergence, not a lost no-op. Fail loud, same as an
            // unknown verb.
            if matches!(dict.get_item("discard")?, Some(v) if v.is_truthy()?) {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "apply: 'discard' is not supported — the engine has no \
                     potion-discard action, and decoding it as a drink would \
                     diverge from the world that emitted it",
                ));
            }
            Ok(Action::DrinkPotion {
                potion_slot: req_u8("slot")?,
                target: opt_u8("target_index")?,
            })
        }
        "choose" => {
            let mut sel: SmallVec<[u8; 4]> = SmallVec::new();
            // Prefer the full `selection` index list (multi-pick: HIDDEN_DAGGERS
            // `DiscardFromHand` min=max=2 ⇒ `[0,1]`); fall back to a single
            // `index` for back-compat single-pick callers.
            match dict.get_item("selection")? {
                Some(v) if !v.is_none() => {
                    let raw: Vec<usize> = v.extract().map_err(|e| {
                        pyo3::exceptions::PyValueError::new_err(format!(
                            "apply: 'selection' must be a list of non-negative integers ({e})"
                        ))
                    })?;
                    for r in raw {
                        let narrowed = u8::try_from(r).map_err(|_| {
                            pyo3::exceptions::PyValueError::new_err(format!(
                                "apply: 'selection' entry {r} out of range for u8"
                            ))
                        })?;
                        sel.push(narrowed);
                    }
                }
                _ => sel.push(req_u8("index")?),
            }
            Ok(Action::ResolveChoice { selection: sel })
        }
        other => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "apply: unknown action kind '{other}'"
        ))),
    }
}

/// Python wrapper around [`sts2_core::CombatState`].
///
/// Phase 0 is a thin construct/inspect surface. The real action dispatch and
/// observation accessors land in Phase 1+.
#[pyclass(name = "CombatState", module = "sts2_rs")]
#[derive(Debug, Clone)]
pub struct PyCombatState {
    pub(crate) inner: CoreCombatState,
    /// Stored so `__repr__` can show the seed without reaching into the RNG
    /// (which doesn't expose its seed).
    seed: u64,
    /// Wire-side trust flag for enemy intent damage. `true` for states
    /// constructed from registry / encounter builders (no wire ingest);
    /// possibly `false` for states built via `state_from_dict` when the
    /// wire dict's Attack intents lack damage data and the optional
    /// `label` field couldn't be salvaged. Surfaced through
    /// `mcts_recommend` as `intent_damage_known` so consumers know
    /// when the win/loss split is unreliable. Bead `sts2-rs-bpt4`.
    intent_damage_known: bool,
}

impl PyCombatState {
    /// Borrow the inner [`CoreCombatState`] (Rust-side only — not
    /// exposed to Python). Used by [`crate::state_dict::state_to_dict_py`]
    /// to serialize without re-exposing the `inner` field.
    pub(crate) fn inner_ref(&self) -> &CoreCombatState {
        &self.inner
    }

    /// Crate-internal constructor used by [`crate::run_manager::PyRunManager`]
    /// to expose the in-flight combat state during `Phase::Combat`.
    /// Bead `sts2-rs-8xlqe`.
    pub(crate) fn from_inner(inner: CoreCombatState, seed: u64) -> Self {
        Self {
            inner,
            seed,
            intent_damage_known: true,
        }
    }
}

#[pymethods]
impl PyCombatState {
    #[new]
    #[pyo3(signature = (seed = 0, deck = None, relics = None, encounter = None, ascension = 0))]
    fn new(
        seed: u64,
        deck: Option<&Bound<'_, PyAny>>,
        relics: Option<&Bound<'_, PyAny>>,
        encounter: Option<&Bound<'_, PyAny>>,
        ascension: u8,
    ) -> PyResult<Self> {
        let _ = (deck, relics);
        let enc_str: Option<String> = match encounter {
            Some(e) => Some(e.extract()?),
            None => None,
        };
        let inner = if let Some(ref name) = enc_str {
            build_encounter_state(seed, name, ascension)
                .map_err(pyo3::exceptions::PyValueError::new_err)?
        } else {
            build_seeded_state(seed, ascension)
        };
        Ok(Self {
            inner,
            seed,
            intent_damage_known: true,
        })
    }

    /// Whether the wire-side enemy intents this state was built from
    /// carry trustworthy damage values. `True` for states built via
    /// the encounter-name constructor (no wire ingest); possibly
    /// `False` for `state_from_dict` results when one or more Attack
    /// intents had `amount` missing/0 *and* no parseable `label`
    /// salvage. MCTS rollouts will still tick those intents but they
    /// resolve as 0-damage no-ops, biasing the continuous Q toward
    /// "safe" — downstream consumers should treat the win/loss split
    /// as low-confidence when this is `False`. Bead `sts2-rs-bpt4`.
    #[getter]
    fn intent_damage_known(&self) -> bool {
        self.intent_damage_known
    }

    /// Pre-weight feature-vector consumed by the Texel tuning harness
    /// (bead `sts2-rs-uxyf` / vsqc.2). Returns the 12 base scalars in
    /// the canonical `EVAL_FEATURE_NAMES` column order — the Python
    /// harness fits per-feature weights such that
    /// `sigmoid(K · np.dot(features, weights)) ≈ outcome`. Column
    /// names are exposed via [`Self::feature_names`] for header
    /// alignment.
    fn feature_vector(&self) -> Vec<f32> {
        core_eval_features(&self.inner).to_vec()
    }

    /// Canonical column names for [`Self::feature_vector`]. The
    /// corpus generator (vsqc.1) writes these as the NPZ header so
    /// the Texel harness (vsqc.3) can parse the file without
    /// guessing column order. Bead `sts2-rs-uxyf` (vsqc.2).
    #[staticmethod]
    fn feature_names() -> Vec<String> {
        EVAL_FEATURE_NAMES
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    /// Current `const f32` weights paired with [`Self::feature_names`].
    /// The Texel tuner consumes these as the *starting point* for
    /// Adam optimization. After fit, the new weights are written
    /// back to the Rust `const f32` slots (bead `sts2-rs-p8gq` /
    /// vsqc.4). Bead `sts2-rs-uxyf` (vsqc.2).
    #[staticmethod]
    fn feature_weights() -> Vec<f32> {
        EVAL_FEATURE_WEIGHTS.to_vec()
    }

    /// Wide-schema feature vector (~80 floats) for the Wave-6
    /// Expert Iteration path. Bead `sts2-rs-qctu.1`. Sister to
    /// [`Self::feature_vector`] (12 floats, Texel-fitted). Column
    /// order is pinned by [`Self::feature_names_wide`]; see the
    /// `sts2_core::eval_wide` module docstring for the layout.
    fn feature_vector_wide(&self) -> Vec<f32> {
        core_eval_features_wide(&self.inner).to_vec()
    }

    /// Canonical column names for [`Self::feature_vector_wide`].
    /// Consumed by the Wave-6 corpus generator + the
    /// `AlphaZero`-style training script when writing NPZ headers
    /// so column drift surfaces as a parse error rather than a
    /// silent feature mismatch. Bead `sts2-rs-qctu.1`.
    #[staticmethod]
    fn feature_names_wide() -> Vec<String> {
        EVAL_FEATURE_NAMES_WIDE
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    /// Raw, undivided, unclamped evaluator score (the same value
    /// `evaluate_in_progress_raw` returns in Rust). Used by the
    /// Texel harness as a regression check: `feature_vector` dotted
    /// with `feature_weights` must equal this value to within f32
    /// noise before any tuning round runs (bead `sts2-rs-uxyf` /
    /// vsqc.2 acceptance criterion).
    fn evaluate_in_progress_score(&self) -> f32 {
        core_evaluate_in_progress_raw(&self.inner)
    }

    /// Serialize the current state into a Python dict matching the bridge
    /// JSON shape (`combat_state_to_bridge_state`). The result deserializes
    /// via Python's `BridgeReplayTrace.from_dict` without raising.
    fn get_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let value = parity::combat_state_to_bridge_state(&self.inner);
        pythonize::pythonize(py, &value)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("pythonize failed: {e}")))
    }

    /// Apply an action expressed as a Python dict in bridge action shape.
    ///
    /// Reads the same dict shape that [`Self::legal_actions`] emits and
    /// decodes the per-variant fields (`card_index`, `target_index`,
    /// `slot`, `index`) into the matching [`Action`] variant. An unknown
    /// or missing `"action"` value raises `ValueError` rather than
    /// silently ending the turn — masking that bug previously produced
    /// false-positive `STATE_DRIFT` in the parity-fuzz harness. The
    /// underlying `sts2_core::apply` returns a `StepResult` that is
    /// dropped because this binding doesn't surface it yet.
    #[pyo3(signature = (action))]
    fn apply(&mut self, action: &Bound<'_, PyAny>) -> PyResult<()> {
        let dict = action.downcast::<PyDict>().map_err(|_| {
            pyo3::exceptions::PyValueError::new_err("apply: expected a dict in bridge action shape")
        })?;
        let mapped = parse_bridge_action(dict)?;
        let _ = core_apply(&mut self.inner, mapped);
        Ok(())
    }

    /// Run the engine's structural invariant suite against the current
    /// (UNCLAMPED) internal state and return the first violation message, or
    /// `None` when all invariants hold.
    ///
    /// item#8 (parity invariant gate): `state_to_dict` clamps hp/block/energy
    /// on emit, so the Python parity harness can never see an internal
    /// corruption through the wire surface — a differential diff against C#
    /// stays "clean" even if BOTH engines (or just Rust) reached a physically
    /// impossible state. This exposes the same `check_combat_invariants` suite
    /// the libfuzzer `combat_step` target runs (HP/block/energy bounds,
    /// PowerStore index integrity, deck-partition conservation, hand cap,
    /// enemy-uid↔creature mirror, termination consistency) so the harness can
    /// assert it after every `apply()` — catching shared-wrong-behavior the
    /// differential test alone would pass.
    fn check_invariants(&self, ctx: &str) -> Option<String> {
        sts2_core::invariants::check_combat_invariants(&self.inner, ctx).err()
    }

    /// Enumerate the legal actions for the current state as a list of
    /// bridge-shape dicts.
    ///
    /// Shape contract (mirrors the `sts2-rl-agent` rollout bridge):
    ///   * `{"action": "play", "card_index": i, "target_index": j | None}`
    ///   * `{"action": "end_turn"}`
    ///   * `{"action": "choose", "index": i}` — emitted iff a
    ///     `pending_choice` is open; in that case no other actions
    ///     are legal.
    ///   * `{"action": "potion", "slot": i, "target_index": j | None}`
    ///
    /// Empty list when the state is terminal — MCTS callers treat this
    /// as the leaf signal.
    fn legal_actions<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        crate::value_to_pylist(py, &sts2_wire::emit::legal_actions(&self.inner))
    }

    /// Deterministic structural hash of the current state, suitable as
    /// an MCTS transposition-table key. Stable across runs of the same
    /// compiled binary; **not** stable across pin bumps (Rust stdlib
    /// changes or new hashed fields may shift the value).
    fn state_hash(&self) -> u64 {
        core_state_hash(&self.inner)
    }

    /// Return the open pending-choice description, or `None` when no
    /// choice is currently pending.
    ///
    /// The dict shape mirrors the Python bridge payload at
    /// `sts2-rl-agent@01c094d:sts2_env/parity/bridge_replay.py:303-316`
    /// (always `type = "card_select"`, with `cards`, `min_select`,
    /// `max_select`). Per bead sts2-rs-ck2, the Rust port also exposes
    /// the [`ChoiceKind`] discriminant via a `kind` field — Python has
    /// only one `PendingCardChoice` class so the discriminant lives in
    /// the `prompt` / `source_pile` strings, but the Rust enum maps
    /// every flavor explicitly so callers can branch without prompt
    /// parsing.
    ///
    /// Shape:
    ///
    /// ```text
    /// {
    ///   'type': 'card_select',          # always — matches Python bridge
    ///   'kind': 'put_back',             # Rust-only: ChoiceKind discriminant
    ///   'cards': [{'id': 'Headbutt', 'type': 'Attack', 'upgraded': False},
    ///             ...],
    ///   'min_select': 1,
    ///   'max_select': 1,
    ///   'can_skip': False,
    /// }
    /// ```
    fn pending_choice<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        match sts2_wire::emit::pending_choice(&self.inner) {
            None => Ok(None),
            Some(v) => Ok(Some(crate::value_to_pydict(py, &v)?)),
        }
    }

    /// Whether a card is SUSPENDED mid-play — its `OnPlay` opened a screen and
    /// the rest of the card (`AfterCardPlayed`, replays, disposal) is parked
    /// waiting for the pick.
    ///
    /// This is the one piece of state a reconstruction can never carry: no wire
    /// dialect expresses it, and `parse_state_dict` refuses a dict that even
    /// claims to. A driver therefore has to know when NOT to rebuild — see
    /// `CombatMirror.reconcile`, which verifies observables against a parked
    /// mirror instead of replacing it.
    fn has_pending_play(&self) -> bool {
        self.inner.pending_play.is_some()
    }

    /// Set the player's current and max HP in place, nothing else
    /// (sts2-agent fork, A4d). An evaluator normalizes HP before a search
    /// (raised-HP states saturate its score); a dump-and-rebuild round trip
    /// to do it also rebuilt what the wire can't carry exactly (a Buff or
    /// Debuff intent's power and amount), so the state changed beyond HP.
    ///
    /// Both stores of the player's HP are set: the `PlayerCombatState`
    /// scalars and, when the combat has one, the player's unified `Creature`
    /// record, which the search reads first (`player_hp_block` in
    /// sts2-mcts). Setting only the scalars left the search seeing the old HP.
    ///
    /// Raises on `max_hp < 1` or `hp` outside `1..=max_hp`.
    fn set_player_hp(&mut self, hp: i32, max_hp: i32) -> PyResult<()> {
        if max_hp < 1 || hp < 1 || hp > max_hp {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "set_player_hp: need 1 <= hp <= max_hp, got hp {hp}, max_hp {max_hp}"
            )));
        }
        self.inner.player.max_hp = max_hp;
        self.inner.player.current_hp = hp;
        if let Some(uid) = self.inner.player.creature_uid {
            if let Some(c) = self.inner.creatures.iter_mut().find(|c| c.id == uid.0) {
                c.max_hp = max_hp;
                c.current_hp = hp;
            }
        }
        Ok(())
    }

    /// `true` when a turn-transition walk is SUSPENDED on a player choice —
    /// Well-Laid Plans' retain select inside the end-of-turn flush, a
    /// Stampede auto-play's mid-transition screen, a suspended turn setup /
    /// cluster / hand draw, or the KnowledgeDemon curse walk. None of these
    /// resume markers is expressible on the wire, so a mirror that rebuilds
    /// here deletes the rest of the transition and lands a full round behind
    /// (55 of the 230 player-side events on the v28wr census: mirror round
    /// N−1, energy 0 vs the world's refilled 3). The mirror keeps the
    /// carried state instead, exactly as it does for a mid-play card.
    fn has_suspended_walk(&self) -> bool {
        self.inner.pending_wlp_retain_resume
            || self.inner.pending_end_turn_resume.is_some()
            || self.inner.pending_turn_setup_resume
            || self.inner.pending_cluster_resume.is_some()
            || self.inner.pending_autoplay_resume.is_some()
            || self.inner.pending_draw_resume.is_some()
            || self.inner.pending_hand_draw_resume.is_some()
            || self.inner.pending_relic_hand_draw_resume.is_some()
            || self.inner.pending_enemy_exec_resume.is_some()
    }

    /// `true` when a DRINK is suspended on a player choice — the potion's
    /// card-select is open and `Hook.AfterPotionUsed` has not fired yet.
    ///
    /// The same "unexpressible on the wire" class as
    /// [`Self::has_pending_play`] and [`Self::has_suspended_walk`], and it was
    /// missing from both. C# `PotionModel.OnUseWrapper` (PotionModel.cs:291-342)
    /// `await`s `OnUse` — including the Attack/Skill/Power/Colorless choose-a-card
    /// screen — before firing `Hook.AfterPotionUsed` at `:338`, so the engine
    /// parks the deferred dispatch on
    /// [`sts2_core::CombatState::pending_potion_hook_resume`]. Nothing on the
    /// wire carries that, so a mirror that rebuilds while a potion screen is
    /// open silently drops the hook: `ReptileTrinket`'s +3 temporary Strength
    /// and `BeltBuckle` never fire, and the suspended choice itself is deleted
    /// along with them.
    ///
    /// Kept separate from `has_suspended_walk` rather than folded into it: a
    /// drink happens inside the player's own play phase, which both sides
    /// agree on, so a caller must NOT excuse `$.phase` / `$.side` from the
    /// comparison the way a genuine turn-transition walk has to.
    fn has_pending_potion_hook(&self) -> bool {
        self.inner.pending_potion_hook_resume.is_some()
    }

    // ── Persistent-combat-mirror surface ───────────────────────────────
    //
    // These five methods exist for one caller: a `CombatMirror` that keeps
    // ONE engine state alive for a whole fight while the C# DLL (or the
    // live game) is the world. Per decision it rebuilds every observable
    // from the wire and carries forward only what the wire cannot express.
    // Everything here fails loud — a mirror that silently patches over a
    // shape mismatch is worse than no mirror, because its search then
    // plans against a board that never existed.

    /// Apply an action and return which [`sts2_core::StepResult`] the
    /// engine reported, as a lowercase string.
    ///
    /// [`Self::apply`] drops the result. The mirror cannot: an
    /// `"invalid_target"` means the engine and the world disagree about
    /// what is legal, which is a divergence to crash on, and
    /// `"combat_won"` / `"combat_lost"` let the driver check the engine's
    /// verdict against what the world actually did next.
    #[pyo3(signature = (action))]
    fn apply_step(&mut self, action: &Bound<'_, PyAny>) -> PyResult<&'static str> {
        let dict = action.downcast::<PyDict>().map_err(|_| {
            pyo3::exceptions::PyValueError::new_err(
                "apply_step: expected a dict in bridge action shape",
            )
        })?;
        let mapped = parse_bridge_action(dict)?;
        Ok(match core_apply(&mut self.inner, mapped) {
            sts2_core::StepResult::Advanced => "advanced",
            sts2_core::StepResult::PendingChoice => "pending_choice",
            sts2_core::StepResult::CombatWon => "combat_won",
            sts2_core::StepResult::CombatLost => "combat_lost",
            sts2_core::StepResult::InvalidTarget => "invalid_target",
        })
    }

    /// The `shuffle` RNG stream's cursor.
    ///
    /// Every pile shuffle — the empty-draw auto-reshuffle included —
    /// consumes from this one stream, so a jump across an `advance` means
    /// the engine rolled an order the world rolled independently. Downstream
    /// observables are then *unverifiable* rather than wrong, which is the
    /// mirror's one principled non-crash (lightspeed's
    /// `empty_deck_shuffle_count` guard). It over-tags — a deliberate
    /// shuffle counts too — and that errs toward suppressing a crash we
    /// could not have justified anyway.
    const fn shuffle_stream_counter(&self) -> u32 {
        self.inner.run_rng.shuffle.counter()
    }

    /// First-in-series card plays the player has made this turn — EchoForm's
    /// `num >= Amount` gate (GH #368.1).
    ///
    /// Exposed because it is hidden state no producer emits: a rebuild reads
    /// 0 and the gate re-opens, so a diagnostic needs to be able to ask what
    /// the mirror actually carries. See `transplant_hidden`, which carries it.
    const fn cards_first_in_series_this_turn(&self) -> u32 {
        self.inner.player.cards_first_in_series_this_turn
    }

    /// The player's own knowledge of their draw pile, as draw-pile
    /// indices (the same encoding `state_to_dict` uses).
    ///
    /// `{"pinned_top": [i, …], "pinned_bottom": [i, …],
    ///   "deferred_draw_order": bool}`. A mirror's `deferred_draw_order`
    /// must always be `False` — it holds concrete truth, and only the
    /// private search clone is put on the belief representation.
    fn draw_pile_knowledge<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        d.set_item(
            "pinned_top",
            pin_positions(&self.inner, &self.inner.player.pinned_top),
        )?;
        d.set_item(
            "pinned_bottom",
            pin_positions(&self.inner, &self.inner.player.pinned_bottom),
        )?;
        d.set_item("deferred_draw_order", self.inner.player.deferred_draw_order)?;
        Ok(d)
    }

    /// Carry the unobservable half of `carried` onto `self`.
    ///
    /// `self` is a fresh reconstruction: every observable already came
    /// from reality. What reality does not report — and what therefore
    /// has to survive from the previous decision or be lost — is:
    ///
    /// * the RNG cursors (legacy stream + every `RunRngSet` stream);
    /// * each enemy's `intent_history` (the wire carries an i32 summary
    ///   that `state_from_dict` cannot rebuild, so a fresh state's
    ///   history is empty and the move AI re-rolls from scratch);
    /// * the draw-pile pins and the belief flag;
    /// * the per-combat latch a relic keeps in `counter`, for exactly those
    ///   relics the wire did not disclose one for (`carry_relic_counters`).
    ///
    /// Pins are translated BY POSITION, not by uid: `state_from_dict`
    /// reallocates every `CardUid` as it walks the piles, so the carried
    /// mirror's uids are meaningless here, and the pins are dropped when the
    /// two draw piles disagree (a reshuffle between decisions) rather than
    /// failing the whole transplant — the RNG cursors are still valid.
    ///
    /// `enemy_slots[i]` names the CARRIED slot holding the history for this
    /// state's enemy `i`, `None` for a newly-spawned one. It exists because
    /// the two worlds stop agreeing on slot numbering the moment one reaps a
    /// corpse the other keeps; see `combat_mirror.enemy_slot_key`. Omitting
    /// it falls back to slot-for-slot, which then requires equal rosters.
    /// `carry_rng` decides who owns the RNG cursors. Default `true` is the
    /// original behaviour: the wire says nothing about the run's dice, so the
    /// carried mirror's own streams are the only continuity there is. Pass
    /// `false` when the reconstruction was built from a wire carrying
    /// `$.rng.run_seed` — those streams ARE the world's, and overwriting them
    /// with the mirror's would throw away the one thing that makes an enemy
    /// intent predictable instead of guessed.
    ///
    /// `carry_relic_counters` names the relics whose `counter` must survive
    /// from the carried mirror. A relic's `counter` is not always a displayed
    /// tally — it is also where a one-shot per-combat latch lives (Vambrace's
    /// "the first card-granted block this combat doubles"). Both producers
    /// only ever emit a counter for relics that SHOW one, so an undisclosed
    /// latch reset to armed on every reconcile and the mirror doubled a second
    /// card's block: 89 of the 123 `$.player.block` records on the v14 sweep
    /// carried Vambrace, against an 11% base rate. Pass the relics the wire
    /// left at `None`; the ones it did disclose keep reality's value.
    #[pyo3(signature = (carried, enemy_slots = None, carry_rng = true, carry_relic_counters = None, carry_power_counters = None))]
    fn transplant_hidden(
        &mut self,
        carried: &Self,
        enemy_slots: Option<Vec<Option<usize>>>,
        carry_rng: bool,
        carry_relic_counters: Option<Vec<String>>,
        carry_power_counters: Option<Vec<String>>,
    ) -> PyResult<()> {
        let (fresh_draw, carried_draw) = (
            self.inner.player.draw.len(),
            carried.inner.player.draw.len(),
        );
        let fresh_enemies = self.inner.enemy_uids.len();
        let slots: Vec<Option<usize>> = if let Some(v) = enemy_slots {
            if v.len() != fresh_enemies {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "transplant_hidden: enemy_slots has {} entries for {fresh_enemies} enemies",
                    v.len()
                )));
            }
            v
        } else {
            let carried_enemies = carried.inner.enemy_uids.len();
            if fresh_enemies != carried_enemies {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "transplant_hidden: enemy roster disagrees — carried mirror has \
                     {carried_enemies}, the reconstruction has {fresh_enemies}, and no \
                     enemy_slots mapping was supplied to align them"
                )));
            }
            (0..fresh_enemies).map(Some).collect()
        };

        if carry_rng {
            self.inner.rng = carried.inner.rng.clone();
            self.inner.run_rng = carried.inner.run_rng.clone();
        } else {
            // The reconstruction's `run_rng` came from the world; only the
            // search-side legacy stream has no wire representation and still
            // needs carrying.
            self.inner.rng = carried.inner.rng.clone();
        }

        for (slot, carried_slot) in slots.iter().enumerate() {
            let Some(carried_slot) = *carried_slot else {
                continue; // a newly-spawned enemy has no history to carry
            };
            let Some(&src_uid) = carried.inner.enemy_uids.get(carried_slot) else {
                continue;
            };
            let dst_uid = self.inner.enemy_uids[slot];
            let Some(src) = carried
                .inner
                .creatures
                .iter()
                .find(|c| c.id == src_uid.0)
                .map(|c| (c.intent_history, c.ai.clone()))
            else {
                continue;
            };
            let (history, src_ai) = src;
            if let Some(dst) = self.inner.creatures.iter_mut().find(|c| c.id == dst_uid.0) {
                dst.intent_history = history;
                // The move machine's per-instance history — `state_log`,
                // `counter`, the `performed_*` flags, a pending
                // `SetMoveImmediate` — is not expressible on any wire, so a
                // rebuilt AI starts blank and mispredicts the very next
                // telegraph. `current` is NOT carried: it rides the wire as
                // `move_state`, so reality wins on it.
                if let (Some(dst_ai), Some(src_ai)) = (dst.ai.as_mut(), src_ai.as_ref()) {
                    dst_ai.transplant_instance_state_from(src_ai);
                }
            }
        }

        // Pins travel as draw-pile POSITIONS, so they are only translatable
        // when both sides agree on the pile. When they don't — a reshuffle
        // between decisions is the common case — the pins are simply
        // unreconstructable and are dropped. They are the player's own
        // knowledge of an order that no longer exists, so dropping them loses
        // nothing real; refusing the whole transplant would also have thrown
        // away the RNG cursors, which ARE still valid.
        if fresh_draw == carried_draw {
            let translate = |pins: &[CardUid]| -> smallvec::SmallVec<[CardUid; 4]> {
                pins.iter()
                    .filter_map(|uid| carried.inner.player.draw.iter().position(|d| d == uid))
                    .map(|idx| self.inner.player.draw[idx])
                    .collect()
            };
            self.inner.player.pinned_top = translate(&carried.inner.player.pinned_top);
            self.inner.player.pinned_bottom = translate(&carried.inner.player.pinned_bottom);
        }
        self.inner.player.deferred_draw_order = carried.inner.player.deferred_draw_order;
        // First-in-series card plays this turn — EchoForm's `num >= Amount`
        // gate (GH #368.1). No producer emits it, so the reconstruction always
        // reads 0 and the gate re-opens on every reconcile. Carried here
        // rather than derived, because the count includes plays made BEFORE
        // the EchoForm power existed — Echo Form is itself a power card, so
        // its own play is the usual such case, and a per-power counter
        // seeded at apply time cannot see it (that residual was 9 of the 28
        // remaining positive enemy-HP desyncs on the post-fix DLL sweep).
        self.inner.player.cards_first_in_series_this_turn =
            carried.inner.player.cards_first_in_series_this_turn;
        // Started card plays this turn — the Sloth/Ringing LEGALITY gate
        // (GH #370). `state_from_dict` already seeded this exactly when a
        // Sloth power was on the wire (`SlothPower.DisplayAmount` is the live
        // count), and that reading is authoritative, so only fall back to the
        // mirror's own accumulation when the wire had no Sloth to disclose it.
        // Ringing needs the count just as much and has no carrier of its own:
        // C# scans `History.CardPlaysStarted` and stores nothing on the power.
        let sloth_disclosed_the_count = self.inner.player.powers.iter().any(|puid| {
            self.inner
                .powers
                .get_by_id(puid.0)
                .is_some_and(|p| p.kind == sts2_core::powers::PowerKind::Sloth)
        });
        if !sloth_disclosed_the_count {
            self.inner.player.card_plays_started_this_turn =
                carried.inner.player.card_plays_started_this_turn;
        }
        // The rest of the GH #370 accumulator class. Each is a plain per-turn
        // or per-combat tally that content reads as a gate and no producer
        // emits, so a rebuild re-zeroes it and the gate re-opens every
        // reconcile. Audited read-site by read-site against the C#; only the
        // ones with a LIVE consumer are carried.
        //
        //  * `shivs_played_this_turn` — `PhantomBladesPower` gives the FIRST
        //    Shiv each turn `+Amount` and gates on this being 0
        //    (`PhantomBladesPower.cs:53-72`, C# `CardPlaysFinished == 0`).
        //    Rebuilt at 0, EVERY Shiv reads as the first and takes the bonus —
        //    and Shivs arrive in volume.
        //  * `generated_cards_this_combat` — Regent's Supermassive scales its
        //    damage directly off it (`regent/supermassive.rs:24`), so a
        //    reconstruction hits for the floor.
        //  * `stars_gained_this_turn` — Regent's Radiate scales off it
        //    (`regent/radiate.rs:11`); same shape.
        //
        // Deliberately NOT carried, each verified rather than assumed:
        // `powered_hits_this_turn` / `powered_hits_by_target_this_turn` have no
        // live consumer left — GangUp is `MultiplayerOnly` and C# excludes the
        // owner's own hits, so the solo body is a flat 5. `skills_played_this_turn`
        // is documented-dead. The within-a-play scratch fields cannot span a
        // reconcile at all, because a graded decision never lands mid-play.
        self.inner.player.shivs_played_this_turn = carried.inner.player.shivs_played_this_turn;
        self.inner.player.generated_cards_this_combat =
            carried.inner.player.generated_cards_this_combat;
        self.inner.player.stars_gained_this_turn = carried.inner.player.stars_gained_this_turn;

        // The Sly / Retain uid lists are NOT carried. They are seeded from the
        // wire in `state_from_dict` (the `retain_turn` / `sly_turn` / `sly`
        // card flags, GH #370), and a carried uid could not be honoured
        // anyway: the rebuild re-allocates every `CardUid` from its own pile
        // walk, so a uid from the previous mirror names a different card.
        // Nothing to guard, then — unlike the Sloth counter above, there is no
        // fallback branch that could fight the derivation.
        //
        // `pending_retain_count` is likewise left alone, and here the reason is
        // that ZERO is already the right answer at every rebuild:
        //  * outside the end-turn chain it is always 0 — `invariants.rs:446`
        //    asserts exactly that at each round boundary;
        //  * inside the chain it is drained from the live `WellLaidPlansPower`
        //    amount (`combat_loop.rs:2500-2513`), so a rebuilt mirror
        //    re-derives it from the power rather than needing history;
        //  * and at the one graded decision the chain can suspend on — the
        //    Well-Laid-Plans `RetainAtTurnEnd` select — it has just been set
        //    to 0 (`combat_loop.rs:2580`) because the open choice now
        //    represents the whole budget. Carrying it would restore a value
        //    the engine deliberately spent.
        //
        // `vigor_latch_card` DOES need carrying. C# `VigorPower.BeforeAttack`
        // latches `commandToModify` on the first powered attack so that only
        // that card's later commands keep the bonus; the latch clears when
        // Vigor is spent. Lost across a rebuild, a second command in the same
        // play re-takes the additive boost. It names a card in the PLAY pile
        // (`damage.rs:973` stores `play.last()`), which the wire ships in
        // order, so translate it by position exactly as the draw-pile pins are
        // translated above — and drop it when the piles disagree, since a
        // latch pointing at the wrong card is worse than none.
        // Length equality alone is a weak guard on a pile this short, so the
        // card at the translated index must also BE the same card.
        self.inner.player.vigor_latch_card = carried.inner.player.vigor_latch_card.and_then(|c| {
            if self.inner.player.play.len() != carried.inner.player.play.len() {
                return None;
            }
            let idx = carried.inner.player.play.iter().position(|p| *p == c)?;
            let dst = *self.inner.player.play.get(idx)?;
            let same_card = |s: &Self, uid: sts2_core::CardUid| {
                s.inner
                    .cards
                    .get(uid.0.get() as usize - 1)
                    .map(|k| k.card_id)
            };
            (same_card(self, dst) == same_card(carried, c)).then_some(dst)
        });

        for name in carry_relic_counters.unwrap_or_default() {
            let kind = crate::state_dict::relic_kind_from_python_name(&name).ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "transplant_hidden: unknown relic {name:?} in carry_relic_counters"
                ))
            })?;
            let Some(counter) = carried
                .inner
                .relics
                .iter()
                .find(|r| r.kind == kind)
                .map(|r| r.counter)
            else {
                continue; // the carried mirror never had it (a mid-combat pickup)
            };
            if let Some(dst) = self.inner.relics.iter_mut().find(|r| r.kind == kind) {
                dst.counter = counter;
            }
        }

        // `state_from_dict` builds every state with `TurnEvents::default()`
        // and the wire has no representation for it (state_dict.rs "not in
        // the wire format"), so each reconcile re-zeroed the history counters
        // C# reads out of `CombatManager.History`. Spite (`LostHpThisTurn`)
        // and TearAsunder (`1 + DamageReceivedEntry count`) read them as HIT
        // COUNTS, so the mirror predicted 1 hit forever — 79 of the 252
        // `$.encounter[*].hp` divergences on the v27wr sweep, with deltas
        // matching `-(repeat-1) x per_hit` in closed form. The carried
        // mirror was itself produced by (previous transplant -> exactly one
        // stepped action), so its log holds the running accumulation and
        // re-carrying keeps it continuous across the whole combat; the
        // turn-boundary `reset_per_turn` fires inside whichever step crosses
        // it, preserving the per-turn/combat-wide split for free.
        //
        // Only uid-free state travels: `newly_dead_this_turn`,
        // `last_attack_skill_played` and `card_play_log` hold uids the
        // reconstruction has just reallocated, and
        // `chains_bound_card_played_this_turn` IS on the wire, so reality
        // must keep winning on it. `unmovable_card_block_plays` is safe:
        // its tags are `player_card_plays_this_combat` sequence numbers,
        // which travel with it, so the fresh state's future plays continue
        // the same numbering — Unmovable rebuilt without it doubled EVERY
        // block card instead of only the turn's first (30 of the 140
        // on-play `$.player.block` divergences on the v27wr sweep).
        {
            let src = &carried.inner.combat_log;
            let dst = &mut self.inner.combat_log;
            dst.cards_drawn_by_effect_this_turn = src.cards_drawn_by_effect_this_turn;
            dst.status_cards_drawn_this_turn = src.status_cards_drawn_this_turn;
            dst.doom_applied_by_player_this_turn = src.doom_applied_by_player_this_turn;
            dst.osty_attacks_this_turn = src.osty_attacks_this_turn;
            dst.cards_exhausted_this_turn = src.cards_exhausted_this_turn;
            dst.hp_lost_this_turn = src.hp_lost_this_turn;
            dst.unblocked_damage_received_this_combat = src.unblocked_damage_received_this_combat;
            dst.energy_spent_this_turn = src.energy_spent_this_turn;
            dst.zero_cost_attacks_played_this_turn = src.zero_cost_attacks_played_this_turn;
            dst.card_plays_finished_this_turn = src.card_plays_finished_this_turn;
            dst.card_plays_finished_last_player_turn = src.card_plays_finished_last_player_turn;
            dst.cards_discarded_this_turn = src.cards_discarded_this_turn;
            dst.lightning_orbs_channeled_this_combat = src.lightning_orbs_channeled_this_combat;
            dst.player_card_plays_this_combat = src.player_card_plays_this_combat;
            dst.ethereal_cards_played_this_combat = src.ethereal_cards_played_this_combat;
            dst.exhaust_sweep_ethereal = src.exhaust_sweep_ethereal;
            dst.joss_paper_ethereal_count = src.joss_paper_ethereal_count;
            dst.block_zero_at_turn_end = src.block_zero_at_turn_end;
            dst.block_before_late_turn_end = src.block_before_late_turn_end;
            dst.unmovable_card_block_plays
                .clone_from(&src.unmovable_card_block_plays);
            // The HistoryCourse slots, with the uid component dropped — the
            // reconstruction reallocated every CardUid, and
            // `listener_history_course` already falls back to
            // `card_pod_from_id` for a stale uid, which is exactly why
            // `None` is safe. Left uncarried, the relic was permanently
            // inert in the mirror (30 v28wr events — run 1056's world kept
            // auto-playing last turn's Battle Trance dupe: +3 hand, -3
            // draw, +NO_DRAW the mirror never saw).
            dst.last_attack_skill_played = src
                .last_attack_skill_played
                .map(|(id, up, _uid)| (id, up, None));
            dst.prev_round_last_attack_skill_played = src
                .prev_round_last_attack_skill_played
                .map(|(id, up, _uid)| (id, up, None));
        }

        // Per-card hidden `combat_vars`. The C# producers emit no
        // `combat_vars` at all (0 occurrences across the 1484-record v27wr
        // forensic log), so every rebuild wiped ThrummingHatchet's
        // `ReturnIfPlayedRound` stamp (33 records / 64 fields — the largest
        // named pile signature), Sown/Swift-style first-play marker bits in
        // `Generic`, `GeneratedInCombat`, `EtherealThisCombat`, and every
        // scaling accumulator (Rampage / Claw / RitualDagger). Carry them
        // by pile+position, exactly as the draw-pile pins are: a pile whose
        // length changed (or whose card at the position changed identity)
        // is unreconstructable and drops its carries. Wire-owned vars are
        // excluded — the three cost banks and the two wire-cost stamps
        // belong to the `persistent_cost` channel, and
        // `WitherFakeUpgradeLevel` is re-derived from wire `base_damage` —
        // and a var the fresh card already carries (the Rust dialect
        // round-trips `combat_vars`) keeps the wire's value: reality wins.
        {
            const WIRE_OWNED: &[VarKey] = &[
                VarKey::StompCostReductionThisTurn,
                VarKey::PinpointCostReductionThisTurn,
                VarKey::SneckoCostDeltaThisTurn,
                VarKey::WireTransientCostThisTurn,
                VarKey::WireCostExpiringOnPlay,
                VarKey::WireCostStable,
                VarKey::WitherFakeUpgradeLevel,
            ];
            let pile_pairs = [
                (&self.inner.player.hand[..], &carried.inner.player.hand[..]),
                (&self.inner.player.draw[..], &carried.inner.player.draw[..]),
                (
                    &self.inner.player.discard[..],
                    &carried.inner.player.discard[..],
                ),
                (
                    &self.inner.player.exhaust[..],
                    &carried.inner.player.exhaust[..],
                ),
                (&self.inner.player.play[..], &carried.inner.player.play[..]),
            ];
            let mut copies: Vec<(CardUid, Vec<(VarKey, i32)>)> = Vec::new();
            for (fresh_pile, carried_pile) in pile_pairs {
                if fresh_pile.len() != carried_pile.len() {
                    continue;
                }
                for (&fresh_uid, &carried_uid) in fresh_pile.iter().zip(carried_pile.iter()) {
                    let Some(src) = carried.inner.cards.iter().find(|c| c.uid == carried_uid)
                    else {
                        continue;
                    };
                    if src.combat_vars.is_empty() {
                        continue;
                    }
                    let Some(dst) = self.inner.cards.iter().find(|c| c.uid == fresh_uid) else {
                        continue;
                    };
                    if dst.card_id != src.card_id {
                        continue;
                    }
                    let vars: Vec<(VarKey, i32)> = src
                        .combat_vars
                        .iter()
                        .filter(|(k, v)| {
                            *v != 0 && !WIRE_OWNED.contains(k) && dst.combat_var(*k) == 0
                        })
                        // Rampage's own escalation is wire-owned: C# grows
                        // `Damage.BaseValue` itself (`Rampage.cs:54`), so the
                        // reconstructed card's `base_damage` already carries
                        // the growth and re-injecting the var double-counts —
                        // MCTS planned 37 where the game dealt 23 (v34wr seed
                        // 1020 act-1 boss, round-4 agent 2). The SAME var on
                        // any other card is Momentum's stash, which C# keeps
                        // in the enchantment model (never on the wire), so
                        // that carry must stay.
                        .filter(|(k, _)| {
                            !(*k == VarKey::RampageDamage
                                && dst.card_id == sts2_core::cards::CardId::Rampage)
                        })
                        .copied()
                        .collect();
                    if !vars.is_empty() {
                        copies.push((fresh_uid, vars));
                    }
                }
            }
            for (uid, vars) in copies {
                if let Some(card) = self.inner.cards.iter_mut().find(|c| c.uid == uid) {
                    for (k, v) in vars {
                        card.set_combat_var(k, v);
                    }
                }
            }
        }

        // Power counters the wire structurally cannot disclose. The C#
        // producers emit `counter` only where a display badge exists; a
        // power whose accumulator lives in InternalData (Juggling's
        // attacks-this-turn — 26 v28wr hand records where the 3rd-attack
        // clone never fired; Possess*'s stolen-stat totals) arrives as
        // `{name, amount}` and every rebuild re-zeroed it. Same contract as
        // `carry_relic_counters`: the caller names only the powers the wire
        // left undisclosed, so a disclosed counter keeps reality's value.
        // The player's powers are split across the legacy list and the
        // creature mirror (d03 migration), so read/write the union; enemies
        // align through the same `slots` mapping as the AI carry.
        for name in carry_power_counters.unwrap_or_default() {
            let Some(kind) = crate::state_dict::power_kind_from_python_name(&name) else {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "transplant_hidden: unknown power {name:?} in carry_power_counters"
                )));
            };
            let read = |st: &CoreCombatState, uids: &[sts2_core::PowerUid]| -> Option<i32> {
                uids.iter().find_map(|pu| {
                    st.powers
                        .get_by_id(pu.0)
                        .filter(|p| p.kind == kind && p.counter != 0)
                        .map(|p| p.counter)
                })
            };
            let write = |st: &mut CoreCombatState, uids: &[sts2_core::PowerUid], c: i32| {
                for pu in uids {
                    if let Some(p) = st.powers.get_mut_by_id(pu.0) {
                        if p.kind == kind {
                            p.counter = c;
                            return;
                        }
                    }
                }
            };
            // Player: legacy list + creature mirror, first non-zero wins.
            let carried_player_uids: Vec<sts2_core::PowerUid> = carried
                .inner
                .player
                .powers
                .iter()
                .copied()
                .chain(
                    carried
                        .inner
                        .player
                        .creature_uid
                        .and_then(|cu| carried.inner.creatures.iter().find(|c| c.id == cu.0))
                        .map(|c| c.powers.clone())
                        .unwrap_or_default(),
                )
                .collect();
            if let Some(c) = read(&carried.inner, &carried_player_uids) {
                let fresh_player_uids: Vec<sts2_core::PowerUid> = self
                    .inner
                    .player
                    .powers
                    .iter()
                    .copied()
                    .chain(
                        self.inner
                            .player
                            .creature_uid
                            .and_then(|cu| self.inner.creatures.iter().find(|cr| cr.id == cu.0))
                            .map(|cr| cr.powers.clone())
                            .unwrap_or_default(),
                    )
                    .collect();
                write(&mut self.inner, &fresh_player_uids, c);
            }
            // Enemies, through the slot alignment.
            for (slot, carried_slot) in slots.iter().enumerate() {
                let Some(carried_slot) = *carried_slot else {
                    continue;
                };
                let (Some(&src_uid), Some(&dst_uid)) = (
                    carried.inner.enemy_uids.get(carried_slot),
                    self.inner.enemy_uids.get(slot),
                ) else {
                    continue;
                };
                let Some(src_powers) = carried
                    .inner
                    .creatures
                    .iter()
                    .find(|c| c.id == src_uid.0)
                    .map(|c| c.powers.clone())
                else {
                    continue;
                };
                if let Some(c) = read(&carried.inner, &src_powers) {
                    let Some(dst_powers) = self
                        .inner
                        .creatures
                        .iter()
                        .find(|cr| cr.id == dst_uid.0)
                        .map(|cr| cr.powers.clone())
                    else {
                        continue;
                    };
                    write(&mut self.inner, &dst_powers, c);
                }
            }
        }

        // `Power.applier` for the applier-gated debuffs. `state_from_dict`
        // hardcodes `applier: None` (no wire field carries it), so
        // Shrink/Hex/Dampen/Constrict's `AfterDeath` removal — gated on
        // `p.applier == Some(dead)` — could never fire in a reconstruction:
        // the mirror kept a dead ShrinkerBeetle's permanent Shrink for the
        // rest of the fight (21 v28wr `$.player.powers` events). Copy the
        // carried instance's applier onto the fresh player-side power of the
        // same kind, remapped through the slot alignment; nothing else reads
        // `applier` on these kinds, so a missing mapping is inert.
        {
            use sts2_core::PowerKind as PK;
            const APPLIER_KINDS: [PK; 4] = [PK::Shrink, PK::Hex, PK::Dampen, PK::Constrict];
            let mut uid_map: Vec<(sts2_core::CreatureUid, sts2_core::CreatureUid)> = Vec::new();
            for (slot, carried_slot) in slots.iter().enumerate() {
                if let Some(cs) = carried_slot {
                    if let (Some(&src), Some(&dst)) = (
                        carried.inner.enemy_uids.get(*cs),
                        self.inner.enemy_uids.get(slot),
                    ) {
                        uid_map.push((src, dst));
                    }
                }
            }
            let carried_player_uids: Vec<sts2_core::PowerUid> = carried
                .inner
                .player
                .powers
                .iter()
                .copied()
                .chain(
                    carried
                        .inner
                        .player
                        .creature_uid
                        .and_then(|cu| carried.inner.creatures.iter().find(|c| c.id == cu.0))
                        .map(|c| c.powers.clone())
                        .unwrap_or_default(),
                )
                .collect();
            let fresh_player_uids: Vec<sts2_core::PowerUid> = self
                .inner
                .player
                .powers
                .iter()
                .copied()
                .chain(
                    self.inner
                        .player
                        .creature_uid
                        .and_then(|cu| self.inner.creatures.iter().find(|cr| cr.id == cu.0))
                        .map(|cr| cr.powers.clone())
                        .unwrap_or_default(),
                )
                .collect();
            for kind in APPLIER_KINDS {
                let Some(src_applier) = carried_player_uids.iter().find_map(|pu| {
                    carried
                        .inner
                        .powers
                        .get_by_id(pu.0)
                        .filter(|p| p.kind == kind)
                        .and_then(|p| p.applier)
                }) else {
                    continue;
                };
                let Some(mapped) = uid_map
                    .iter()
                    .find(|(src, _)| *src == src_applier)
                    .map(|(_, dst)| *dst)
                else {
                    continue;
                };
                for pu in &fresh_player_uids {
                    if let Some(p) = self.inner.powers.get_mut_by_id(pu.0) {
                        if p.kind == kind && p.applier.is_none() {
                            p.applier = Some(mapped);
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Open a [`PendingChoice`] whose options came from the WORLD rather
    /// than from the engine's own derivation.
    ///
    /// This is the mirror's single path for mid-combat selections. The
    /// engine has already executed the same card and is parked on its own
    /// derived choice; rather than trusting that derivation to match what
    /// the game is showing (an all-or-nothing match gate that failed 263
    /// times in 60 runs), the driver re-opens the choice over the live
    /// option list and demotes the engine's derivation to an assertion.
    ///
    /// Each option is `{"id": str, "upgraded": bool, "pile": str,
    /// "index": int}` where `pile` ∈ `hand|draw|discard|exhaust|play`.
    /// `pile`/`index` name the concrete card the option refers to and
    /// must be given together; omitting both makes a SYNTHETIC option
    /// (no `card_uid`), which is only meaningful for the generative kinds
    /// — see [`choice_kind_is_generative`].
    ///
    /// Raises on: an unknown kind or card id, a half-specified location,
    /// an out-of-range index, `min_select > max_select`, a `min_select`
    /// larger than the option count, or a choice already being open (all
    /// of which are driver bugs, not game states).
    #[pyo3(signature = (kind, options, min_select, max_select, can_skip = false, replace_parked = false))]
    fn open_injected_choice(
        &mut self,
        kind: &str,
        options: &Bound<'_, PyList>,
        min_select: u8,
        max_select: u8,
        can_skip: bool,
        replace_parked: bool,
    ) -> PyResult<()> {
        let mut carried_post = None;
        if let Some(parked) = &self.inner.pending_choice {
            if !replace_parked {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "open_injected_choice: a choice is already open — resolve or drop it first",
                ));
            }
            // The caller is a mirror PARKED on its own derived list, swapping
            // in the world's. That is the plan's normal path ("record the
            // parked choice's kind and arity … re-open the choice via
            // open_injected_choice"), and it is only reachable once the mirror
            // stopped being rebuilt mid-play — before that the rebuild cleared
            // the choice as a side effect and this could never trigger.
            //
            // Clearing the choice alone is the point: `pending_play` (the
            // suspended rest of the card) must SURVIVE, which is the whole
            // reason the mirror is still parked here. And the parked choice
            // itself is half of that suspended tail: `post_effect` is what
            // runs AFTER the pick (Brand's GainStrength, BurningPact's Draw,
            // Scavenge's EnergyNextTurn), so swapping the option list must
            // not amputate it — dropping it here was 60 of the 188
            // `$.player.powers` divergences on the v27wr sweep (a Brand that
            // exhausted its card and never granted its Strength).
            carried_post = parked.post_effect.map(|pe| (parked.kind, pe));
            self.inner.pending_choice = None;
        }
        let kind = choice_kind_from_python_name(kind).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "open_injected_choice: unknown choice kind {kind:?}"
            ))
        })?;
        // Only the SAME screen keeps the tail — a genuinely different
        // injected kind must not inherit a foreign card's post-effect.
        let carried_post = carried_post.and_then(|(pk, pe)| (pk == kind).then_some(pe));
        if min_select > max_select {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "open_injected_choice: min_select ({min_select}) > max_select ({max_select})"
            )));
        }

        let mut parsed: Vec<ChoiceOption> = Vec::with_capacity(options.len());
        for (i, item) in options.iter().enumerate() {
            let d = item.downcast::<PyDict>().map_err(|_| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "open_injected_choice: options[{i}] must be a dict"
                ))
            })?;
            parsed.push(self.parse_injected_option(d, i)?);
        }
        if usize::from(min_select) > parsed.len() {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "open_injected_choice: min_select ({min_select}) exceeds the {} options offered",
                parsed.len()
            )));
        }
        // `max_select` gets the same bound. Both drivers clamp before
        // calling, which is exactly why this has to be checked HERE: this
        // method is the only thing standing between an unclamped caller and
        // a choice whose arity the option list cannot satisfy.
        if usize::from(max_select) > parsed.len() {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "open_injected_choice: max_select ({max_select}) exceeds the {} options offered",
                parsed.len()
            )));
        }

        self.inner.pending_choice = Some(PendingChoice {
            kind,
            options: parsed,
            min_select,
            max_select,
            can_skip,
            post_effect: carried_post,
            picked: smallvec::SmallVec::new(),
        });
        Ok(())
    }

    /// Mark the OPEN choice as a turn-start screen the world raised from a
    /// relic's `AfterPlayerTurnStart`, so resolving it also runs the rest of
    /// turn setup.
    ///
    /// Mirror-fidelity round-4 defect #13 (Gambling Chip). C#
    /// `Hook.AfterPlayerTurnStart` **awaits** `GamblingChip`'s
    /// `CardSelectCmd.FromHandForDiscard`, so the DLL's board at that decision
    /// is a MID-turn-setup snapshot: the hand is drawn and energy is reset, but
    /// every later relic on the `AfterPlayerTurnStart` pass (MercuryHourglass),
    /// the whole `AfterPlayerTurnStartLate` pass (BloodVial) and the outer
    /// loop's `Hook.AfterSideTurnStart` (Akabeko, Lantern) have NOT run — they
    /// resolve only once the player answers. A mirror seeded from that wire and
    /// handed a bare `discard_from_hand_and_draw` therefore never ran that
    /// batch at all, and stayed short a Vigor 8 / +1 energy / −3-to-all / heal 2
    /// for the rest of the combat.
    ///
    /// This parks [`sts2_core::PendingClusterResume`] with `relic_start` just
    /// past `after_relic`'s equip slot, which is exactly the state the engine
    /// parks itself when its OWN GamblingChip raises the screen. The resolve
    /// step (`apply_step({"action": "choose", ...})`) then runs the remaining
    /// relic listeners and the whole turn-start tail in C# order.
    ///
    /// Args:
    ///     after_relic: python relic id (e.g. `"GAMBLING_CHIP"`) of the relic
    ///         whose hook the world is blocked inside.
    ///
    /// Returns:
    ///     `True` when the marker was armed. `False` — a no-op — when no choice
    ///     is open or the player does not own `after_relic` (the driver's
    ///     screen-recognition was wrong, and parking a resume against a board
    ///     that never suspended would fire the batch twice).
    ///
    /// Raises:
    ///     ValueError: `after_relic` is not a known relic id.
    fn arm_turn_start_resume(&mut self, after_relic: &str) -> PyResult<bool> {
        let kind =
            crate::state_dict::relic_kind_from_python_name(after_relic).ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "arm_turn_start_resume: unknown relic id {after_relic:?}"
                ))
            })?;
        if self.inner.pending_choice.is_none() {
            return Ok(false);
        }
        let Some(idx) = self.inner.relics.iter().position(|r| r.kind == kind) else {
            return Ok(false);
        };
        let next = u8::try_from(idx.saturating_add(1)).unwrap_or(u8::MAX);
        self.inner.pending_cluster_resume = Some(sts2_core::PendingClusterResume {
            snapshot: self.inner.player_power_uids(),
            idx: 0,
            autoplay: sts2_core::PendingAutoplay::default(),
            side_turn_start_fired: false,
            mayhem_phase: false,
            relic_start: Some(next),
        });
        Ok(true)
    }

    /// The POWER twin of [`Self::arm_turn_start_resume`] — GH #355.
    ///
    /// `arm_turn_start_resume` closes the case where a RELIC's
    /// `AfterPlayerTurnStart` awaits a card select. In v0.107.1 both relics
    /// that do so (GamblingChip, ChoicesParadox) are hard-gated to
    /// `TurnNumber <= 1`, so that surface can only ever fire on turn 1. The
    /// generalization to "any turn" is not another relic: it is the three
    /// POWERS whose `AfterPlayerTurnStart` awaits one with no turn gate at
    /// all — `TyrannyPower` (exhaust N), `ToolsOfTheTradePower` (discard N)
    /// and `EntropyPower` (transform N). A mirror seeded from the wire at
    /// one of those screens is a mid-setup snapshot exactly as the
    /// Gambling-Chip one was, and answering the bare selection left every
    /// later power in the cluster, the whole relic `AfterPlayerTurnStart`
    /// bus and `Hook.AfterSideTurnStart` unrun for the rest of the combat —
    /// the mirror then reads a full round behind the world.
    ///
    /// This parks the same [`sts2_core::PendingClusterResume`] the engine
    /// parks for itself when its OWN Tyranny/Tools suspends the cluster
    /// (`combat_loop::run_after_player_turn_start_cluster`): a no-op
    /// auto-play at the suspending power's index, so the resume advances to
    /// `idx + 1` and runs the rest of the walk plus the turn-start tail.
    /// `side_turn_start_fired` is false because that walk is a LATER step
    /// than the cluster — it fires at its normal spot on resume.
    ///
    /// Args:
    ///     after_power: python power id (e.g. `"TYRANNY"`) of the power
    ///         whose turn-start hook the world is blocked inside.
    ///
    /// Returns:
    ///     `True` when the marker was armed. `False` — a no-op — when no
    ///     choice is open or the player does not carry `after_power`
    ///     (the driver's screen-recognition was wrong, and parking a resume
    ///     against a board that never suspended would fire the batch twice).
    ///
    /// Raises:
    ///     ValueError: `after_power` is not a known power id.
    fn arm_turn_start_resume_after_power(&mut self, after_power: &str) -> PyResult<bool> {
        let kind =
            crate::state_dict::power_kind_from_python_name(after_power).ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "arm_turn_start_resume_after_power: unknown power id {after_power:?}"
                ))
            })?;
        if self.inner.pending_choice.is_none() {
            return Ok(false);
        }
        let snapshot = self.inner.player_power_uids();
        let Some(idx) = snapshot.iter().position(|uid| {
            self.inner
                .powers
                .get_by_id(uid.0)
                .is_some_and(|p| p.kind == kind)
        }) else {
            return Ok(false);
        };
        self.inner.pending_cluster_resume = Some(sts2_core::PendingClusterResume {
            snapshot,
            idx,
            autoplay: sts2_core::PendingAutoplay::default(),
            side_turn_start_fired: false,
            mayhem_phase: false,
            relic_start: None,
        });
        Ok(true)
    }

    /// Read-only attack-damage preview for the live hand.
    ///
    /// For each hand card that deals base damage, returns the fully-modified
    /// per-hit damage it would deal each LIVING enemy *right now* — Strength /
    /// Vulnerable / Weak / relic-additive folded in via the exact
    /// [`sts2_core::damage::compute_modified_damage`] stage `play_card` runs
    /// (C# `Hook.ModifyDamage`). Nothing is mutated; this is a pure display
    /// aid so the workbench can show "9" on a Strike aimed at a Vulnerable
    /// target instead of the base "6", matching the live number the game UI
    /// shows.
    ///
    /// The outer list is aligned with the hand (one entry per hand card, same
    /// order as `state_to_dict(..)["deck"]["hand"]`). Each inner dict maps an
    /// enemy's FULL encounter-array index — the position in `enemy_uids` that
    /// `state_to_dict` labels each enemy with, and the `target_index` the play
    /// path expects — to the modified per-hit damage. Cards with no base damage
    /// (skills/powers/0-damage) and dead enemy slots are omitted (empty dict).
    ///
    /// Each card is scored against [`staged_for_play`] — the modifier
    /// chain is read as if that card were mid-dispatch — so play-gated
    /// listeners fire. Without it Lethality (`+Amount%` on the turn's first
    /// powered Attack, gated on a non-empty Play pile) was silently skipped and
    /// a Strike under Lethality 50 previewed its bare 6 instead of 9 (GH #520).
    ///
    /// The inner dicts are keyed by INT, which JSON cannot spell, so the
    /// emitter ([`sts2_wire::emit::card_damage_preview`]) returns a typed
    /// `Vec<BTreeMap<usize, i32>>` and `PyO3` converts it natively.
    fn card_damage_preview(&self) -> Vec<std::collections::BTreeMap<usize, i32>> {
        sts2_wire::emit::card_damage_preview(&self.inner)
    }

    /// Read-only block preview for the live hand — the block half of
    /// [`Self::card_damage_preview`].
    ///
    /// For each hand card with base block, returns the block it would actually
    /// grant if played right now, with Dexterity / Frail / Fasten folded in via
    /// [`sts2_core::card_effects::modify_player_block_move`] — the same
    /// `Hook.ModifyBlock` stage `gain_block_player` runs. `None` for cards with
    /// no base block, so the list is hand-aligned and a caller can skip a slot
    /// without index arithmetic.
    ///
    /// The workbench had no block counterpart to the damage preview, so a
    /// Defend under Dexterity 5 rendered its printed "5 Block" while granting
    /// 10 — the display gap that made Necrobinder's summon cards look like the
    /// better mitigation (GH #520, bug report `wb-b5c453e74f1b80e3`).
    fn card_block_preview(&self) -> Vec<Option<i32>> {
        sts2_wire::emit::card_block_preview(&self.inner)
    }

    /// Return `(is_terminal, value)` where `value ∈ {-1.0, 0.0, 1.0}`
    /// is the MCTS reward from the player's perspective.
    fn terminal_value<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let (term, val) = core_terminal_value(&self.inner);
        let term_obj: Py<PyAny> = term.into_pyobject(py)?.to_owned().into_any().unbind();
        let val_obj: Py<PyAny> = val.into_pyobject(py)?.into_any().unbind();
        PyTuple::new(py, [term_obj, val_obj])
    }

    /// Explicit deep-clone method (plan §175 `PyO3` surface gate —
    /// bead [`sts2-rs-oe1ul`]). Python callers prefer `state.clone()`
    /// over `copy.copy(state)` or `copy.deepcopy(state)` in
    /// performance-critical loops where the import + dispatch
    /// overhead of the `copy` module matters; this avoids both.
    ///
    /// Semantically equivalent to `__copy__` / `__deepcopy__` since
    /// `CombatState` contains no `Arc`/`Rc` aliases (per plan §State
    /// and the rationale on `__deepcopy__` below) — a shallow `Clone`
    /// is observationally a deep clone.
    #[pyo3(name = "clone")]
    fn py_clone(&self) -> Self {
        self.clone()
    }

    /// Support `copy.copy(cs)` — same as a clone of the inner state.
    fn __copy__(&self) -> Self {
        self.clone()
    }

    /// Support `copy.deepcopy(cs, memo)` — same as a clone of the inner
    /// state. Since `CombatState` contains no `Arc`/`Rc` aliases (plan
    /// §State), shallow `clone()` is observationally equivalent to deep.
    #[pyo3(signature = (_memo))]
    fn __deepcopy__(&self, _memo: &Bound<'_, PyAny>) -> Self {
        self.clone()
    }

    fn __repr__(&self) -> String {
        format!(
            "CombatState(round={}, phase={:?}, seed={})",
            self.inner.round, self.inner.phase, self.seed
        )
    }
}

/// Positions of `pins` within `state`'s draw pile, in pin order.
///
/// Stale pins (a uid no longer physically in `draw`) drop out, matching
/// the retain-sweep every consumer performs before trusting a pin list.
fn pin_positions(state: &CoreCombatState, pins: &[CardUid]) -> Vec<usize> {
    pins.iter()
        .filter_map(|uid| state.player.draw.iter().position(|d| d == uid))
        .collect()
}

/// Inverse of [`choice_kind_python_name`]. Built by scanning the forward
/// table rather than hand-maintaining a second one, so the two cannot
/// drift as `ChoiceKind` grows.
fn choice_kind_from_python_name(s: &str) -> Option<ChoiceKind> {
    ChoiceKind::ALL
        .iter()
        .copied()
        .find(|k| choice_kind_python_name(*k) == s)
}

/// Does this choice kind present ENGINE-GENERATED content?
///
/// The distinction the persistent combat mirror needs. A *deterministic*
/// kind's options are concrete cards out of the player's own piles: the
/// engine and the live game must derive the same list, and a mismatch is
/// a real divergence to crash on. A *generative* kind's options were
/// rolled — the engine rolled its own three colorless cards and the game
/// rolled a different three — so a mismatch is expected and says nothing
/// about fidelity. The mirror injects the live options either way; this
/// only decides whether the engine's own derivation is held to them.
///
/// Only the three `AddGenerated*` flavors qualify. `CardSelect` — the
/// catch-all default, which also backs generated card rewards — is
/// deliberately NOT listed: if it turns out to open on rolled content
/// mid-combat, the crash will say so and it can be reclassified with
/// evidence. Guessing generous here would silently disable the check.
///
/// Raises on an unknown kind rather than defaulting either way.
#[pyfunction]
fn choice_kind_is_generative(kind: &str) -> PyResult<bool> {
    let k = choice_kind_from_python_name(kind).ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "choice_kind_is_generative: unknown choice kind {kind:?}"
        ))
    })?;
    Ok(matches!(
        k,
        ChoiceKind::AddGeneratedToHand
            | ChoiceKind::AddGeneratedRetainedToHand
            | ChoiceKind::AddGeneratedFreeToHand
            // The KnowledgeDemon curse screen's options are TOKENS the boss
            // conjured (Disintegration vs MindRot/Sloth/WasteAway) — they
            // live in no pile, so a locator can never resolve them and an
            // option-list mismatch says nothing about pile fidelity. The
            // resolver reads the picked index + `pending_curse_choice`, not
            // the option's board position.
            | ChoiceKind::MonsterCurseSelect
    ))
}

/// Install the process-level locked-card set from a list of C# epoch ids
/// (`"REGENT5_EPOCH"`), replacing any previous one. GH #367.
///
/// These are the epochs the save has NOT revealed — the direct analogue of the
/// `!unlockState.IsEpochRevealed<E>()` arms in each pool's
/// `FilterThroughEpochs`. Every consumer of a character card pool then prunes
/// them: combat generation, card rewards, shop stock.
///
/// An empty list restores the default (`UnlockState.all`), which is what the
/// C# headless oracle builds — so the engine and DLL arms should leave this
/// alone, and only the LIVE arm, playing against a real save, installs
/// anything. Returns the number of cards now locked.
///
/// Raises on an unrecognised epoch id rather than silently ignoring it: a
/// typo'd id that no-ops would present as a mysterious residual cursor drift,
/// which is exactly the class of bug this closes.
#[pyfunction]
fn set_locked_epochs(epoch_ids: Vec<String>) -> PyResult<usize> {
    let mut epochs = Vec::with_capacity(epoch_ids.len());
    for id in &epoch_ids {
        let e = sts2_content::unlocks::Epoch::from_id(id).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "set_locked_epochs: unknown epoch id {id:?}"
            ))
        })?;
        epochs.push(e);
    }
    sts2_content::unlocks::set_locked_epochs(&epochs);
    Ok(sts2_content::unlocks::locked_cards().len())
}

/// Every C# epoch id this engine gates cards behind, in declaration order.
/// GH #374.
///
/// The wire carries what the live save has REVEALED
/// (`UnlockState.ToSerializable().UnlockedEpochs`), and C# knows many epochs
/// this engine does not model (Relic1-5, Silent1/3/4/6, Underdocks, Darv, …).
/// Inverting revealed → locked therefore needs *this* engine's coverage, and a
/// consumer that hardcoded its own copy of the 20 ids would be one more table
/// to drift. Ask the producer instead.
///
/// So `locked = known_epochs() - revealed`, which is exactly what
/// [`set_locked_epochs`] wants.
#[pyfunction]
fn known_epochs() -> Vec<String> {
    sts2_content::unlocks::Epoch::ALL
        .into_iter()
        .map(|e| e.id().to_string())
        .collect()
}

/// Every currently-locked card id, as registry names. Diagnostic surface for
/// the mirror's own reporting — a pool-size desync should be able to say what
/// the engine thinks is locked. GH #367.
#[pyfunction]
fn locked_cards() -> Vec<String> {
    sts2_content::unlocks::locked_cards()
        .into_iter()
        .map(|c| c.to_python_name().to_string())
        .collect()
}

/// Every card id the GENERIC removal offer will not show, as registry names.
///
/// Both bridges fan a shop's single `remove_card` option out into one
/// candidate per deck card, and that fan-out has to match what the game will
/// actually put on the screen: `CardSelectCmd.FromDeckForRemoval`
/// (`CardSelectCmd.cs:629`) gates on `CardModel.IsRemovable`, which in
/// v0.107.1 is `!Keywords.Contains(CardKeyword.Eternal)` and nothing else. So
/// Ascender's Bane and Greed are never offered, and basic Strike/Defend
/// always are.
///
/// Exposed as a LIST, not a predicate, so a caller builds one frozenset per
/// process instead of crossing the FFI boundary once per deck card.
///
/// Deliberately NOT the legacy `CardId::is_removable`, which ALSO dropped
/// basics, did not describe v0.107.1, and is exactly what hand-copying into
/// the bridges got wrong -- the bridge lists offered `remove:ASCENDERS_BANE`
/// (which no screen can show) while withholding the Strike-thinning line
/// entirely. That predicate was deleted on 2026-08-26 so a sixth copy of it
/// cannot appear.
///
/// Spec-only, so it cannot see an enchantment that grants Eternal; pair it
/// with [`eternal_granting_enchantments`] wherever the caller has the live
/// card.
#[pyfunction]
fn non_removable_cards() -> Vec<String> {
    sts2_core::cards::CardId::all()
        .iter()
        .copied()
        .filter(|&c| !sts2_content::cards::spec_index::card_is_removable_csharp(c))
        .map(|c| c.to_python_name().to_string())
        .collect()
}

/// Enchantments that make ANY card non-removable, as canonical UPPER_SNAKE
/// wire names.
///
/// C# reads `CardModel.Keywords`, the card's LIVE keyword set, so removability
/// is not a property of the card id alone: `TezcatarasEmber.OnEnchant` calls
/// `Card.AddKeyword(CardKeyword.Eternal)` (`TezcatarasEmber.cs:18`), and
/// `NutritiousSoup` applies that enchantment to every basic Strike in the
/// deck. A bridge fan-out that filters only on [`non_removable_cards`] will
/// therefore offer Strikes the removal screen withholds.
///
/// Derived, not restated: each enchantment is applied to a probe card and the
/// engine's own instance predicate is asked. Adding an Eternal-granting
/// enchantment to the engine extends this list with no edit here.
#[pyfunction]
fn eternal_granting_enchantments() -> Vec<String> {
    sts2_core::cards::EnchantmentId::ALL
        .iter()
        .copied()
        .filter(|&e| {
            let mut probe = sts2_core::cards::Card::pod_defaults();
            probe.card_id = sts2_core::cards::CardId::StrikeIronclad;
            probe.set_enchantment(e, 1);
            !sts2_content::cards::spec_index::card_is_removable_instance(&probe)
        })
        .map(|e| e.python_name().to_string())
        .collect()
}

/// Arm or disarm the engine's damage-source trace. GH #368.1.
///
/// A mirror desync reports the *magnitude* of an HP divergence and nothing
/// about its cause, and the reconstruction is re-imposed one decision later,
/// so the evidence expires. Arming this makes every HP-loss event record who
/// dealt it; [`take_damage_trace`] drains the buffer.
///
/// Arming CLEARS any buffered rows, so a caller always starts from a known
/// window. Off by default.
///
/// **Thread-scoped.** Arming affects the calling thread only, which is what
/// keeps MCTS rollout workers from buffering rows nobody drains. A step that
/// runs on a different thread than the arming call traces empty.
#[pyfunction]
fn set_damage_trace(on: bool) {
    sts2_core::damage_trace::set_armed(on);
}

/// Drain the damage-trace buffer for THIS thread, oldest first.
///
/// Each row is a dict: `seq`, `source` (innermost dispatcher frame),
/// `source_stack` (outermost first), `dealer`, `target_uid`, `target_id`,
/// `final_damage`, `blocked`, `hp_loss`, `hp_before`, `hp_after`, `turn`.
///
/// An empty `source` is a finding, not a gap in the data: it means the damage
/// came from a call site no dispatcher frame covers yet.
///
/// Check [`damage_trace_dropped`] alongside this: a non-zero count means the
/// rows returned are a prefix of the window, not the whole of it.
#[pyfunction]
fn take_damage_trace(py: Python<'_>) -> PyResult<Vec<Py<PyDict>>> {
    sts2_core::damage_trace::take()
        .into_iter()
        .map(|e| {
            let d = PyDict::new(py);
            d.set_item("seq", e.seq)?;
            d.set_item("source", e.source)?;
            d.set_item("source_stack", e.source_stack)?;
            d.set_item("dealer", e.dealer)?;
            d.set_item("target_uid", e.target_uid)?;
            d.set_item("target_id", e.target_id)?;
            d.set_item("final_damage", e.final_damage)?;
            d.set_item("blocked", e.blocked)?;
            d.set_item("hp_loss", e.hp_loss)?;
            d.set_item("hp_before", e.hp_before)?;
            d.set_item("hp_after", e.hp_after)?;
            d.set_item("turn", e.turn)?;
            Ok(d.unbind())
        })
        .collect()
}

/// Damage-trace rows discarded since the last arm because this thread's
/// buffer filled (cap 4096). Non-zero means `take_damage_trace` returned a
/// PREFIX. Normally 0 — one combat step produces a few dozen rows.
#[pyfunction]
fn damage_trace_dropped() -> u32 {
    sts2_core::damage_trace::dropped()
}

impl PyCombatState {
    /// Decode one `open_injected_choice` option dict.
    ///
    /// `pile` + `index` are the concrete-card locator; they must appear
    /// together or not at all. A half-specified locator is a driver bug
    /// that would otherwise silently produce a synthetic option and send
    /// a pile-select resolver looking for a card that isn't there.
    fn parse_injected_option(&self, d: &Bound<'_, PyDict>, i: usize) -> PyResult<ChoiceOption> {
        let err = |msg: String| pyo3::exceptions::PyValueError::new_err(msg);
        let raw_id: String = d
            .get_item("id")?
            .ok_or_else(|| {
                err(format!(
                    "open_injected_choice: options[{i}] is missing 'id'"
                ))
            })?
            .extract()
            .map_err(|_| {
                err(format!(
                    "open_injected_choice: options[{i}].id must be a string"
                ))
            })?;
        let card_id = state_dict::card_id_from_python_name(&raw_id).ok_or_else(|| {
            err(format!(
                "open_injected_choice: options[{i}].id {raw_id:?} is not a known card"
            ))
        })?;
        let upgraded: bool = match d.get_item("upgraded")? {
            Some(v) if !v.is_none() => v.extract().map_err(|_| {
                err(format!(
                    "open_injected_choice: options[{i}].upgraded must be a bool"
                ))
            })?,
            _ => false,
        };

        let pile_name: Option<String> = match d.get_item("pile")? {
            Some(v) if !v.is_none() => Some(v.extract().map_err(|_| {
                err(format!(
                    "open_injected_choice: options[{i}].pile must be a string"
                ))
            })?),
            _ => None,
        };
        let index: Option<usize> = match d.get_item("index")? {
            Some(v) if !v.is_none() => Some(v.extract().map_err(|_| {
                err(format!(
                    "open_injected_choice: options[{i}].index must be a non-negative integer"
                ))
            })?),
            _ => None,
        };

        let (pile, index) = match (pile_name, index) {
            (None, None) => {
                return Ok(ChoiceOption {
                    card_id,
                    upgraded,
                    card_uid: None,
                    source_pile: None,
                })
            }
            (Some(p), Some(idx)) => (p, idx),
            (p, idx) => {
                return Err(err(format!(
                    "open_injected_choice: options[{i}] gives {} without {} — a card locator \
                     needs both",
                    if p.is_some() { "'pile'" } else { "'index'" },
                    if idx.is_some() { "'pile'" } else { "'index'" },
                )))
            }
        };

        let (kind, uids) = match pile.as_str() {
            "hand" => (PileKind::Hand, self.inner.player.hand.as_slice()),
            "draw" => (PileKind::Draw, self.inner.player.draw.as_slice()),
            "discard" => (PileKind::Discard, self.inner.player.discard.as_slice()),
            "exhaust" => (PileKind::Exhaust, self.inner.player.exhaust.as_slice()),
            "play" => (PileKind::Play, self.inner.player.play.as_slice()),
            other => {
                return Err(err(format!(
                    "open_injected_choice: options[{i}].pile {other:?} must be one of \
                     hand|draw|discard|exhaust|play"
                )))
            }
        };
        let uid = *uids.get(index).ok_or_else(|| {
            err(format!(
                "open_injected_choice: options[{i}].index {index} is out of range for a \
                 {}-card {pile} pile",
                uids.len()
            ))
        })?;

        // The locator names a card; that card had better be the card the
        // option claims to be, or the mirror and the world are pointing at
        // different boards.
        //
        // A uid sitting in a pile but absent from `state.cards` is itself a
        // broken state — the pile is a list of handles INTO that arena, so a
        // dangling one means the check silently passes on an option nothing
        // can resolve. `if let Some(..)` used to do exactly that; be
        // explicit instead.
        let Some(card) = self.inner.cards.iter().find(|c| c.uid == uid) else {
            return Err(err(format!(
                "open_injected_choice: options[{i}] locates {pile}[{index}] = {uid:?}, which is \
                 not in the card arena — the mirror's piles reference a card it does not have"
            )));
        };
        if card.card_id != card_id || card.upgraded != upgraded {
            return Err(err(format!(
                "open_injected_choice: options[{i}] says {raw_id:?} (upgraded={upgraded}) \
                 at {pile}[{index}], but the mirror has {:?} (upgraded={}) there",
                card.card_id.to_python_name(),
                card.upgraded,
            )));
        }

        Ok(ChoiceOption {
            card_id,
            upgraded,
            card_uid: Some(uid),
            source_pile: Some(kind),
        })
    }
}

/// Render an [`Action`] as a bridge-shape Python dict — the shape
/// `PyCombatState::legal_actions` emits, built by
/// [`sts2_wire::emit::combat_action_to_value`].
pub(crate) fn action_to_pydict<'py>(
    py: Python<'py>,
    action: &Action,
) -> PyResult<Bound<'py, PyDict>> {
    crate::value_to_pydict(py, &sts2_wire::emit::combat_action_to_value(action))
}

/// Python wrapper around [`sts2_mcts::Mcts`].
///
/// Phase 8 (bead sts2-rs-69p): thin surface for driving MCTS from Python.
/// `Mcts(state, iters, c_puct, seed)` builds an engine over a clone of
/// `state.inner`; `search()` returns `{"action", "visits", "value"}`;
/// `get_pv()` returns the principal variation as a list of action dicts.
#[pyclass(name = "Mcts", module = "sts2_rs")]
pub struct PyMcts {
    inner: Mcts<SharedPolicy>,
    /// Inputs retained so `search()` can run a root-parallel
    /// `par_search_with_policy` when `threads > 1` (the engine's
    /// parallel path is a free function over these, not a method on the
    /// single-tree `inner`).
    root_state: CoreCombatState,
    config: MctsConfig,
    policy: SharedPolicy,
    leaf_eval: LeafEval,
    threads: u32,
}

impl std::fmt::Debug for PyMcts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyMcts").finish()
    }
}

#[pymethods]
impl PyMcts {
    #[new]
    #[pyo3(signature = (state, iters = 1000, c_puct = std::f32::consts::SQRT_2, seed = 0, max_rollout_depth = 200, leaf_eval = None, threads = 1))]
    fn new(
        state: &PyCombatState,
        iters: u32,
        c_puct: f32,
        seed: u64,
        max_rollout_depth: u32,
        leaf_eval: Option<&str>,
        threads: u32,
    ) -> PyResult<Self> {
        let policy = SharedPolicy::uniform();
        let cfg = MctsConfig {
            c_puct,
            max_simulations: iters,
            max_rollout_depth,
            seed,
            use_puct_prior: false,
            ..MctsConfig::default()
        };
        // Resolve the optional leaf-evaluator override for A/B work (e.g. the
        // full-run win-rate harness comparing the old narrow Texel leaf
        // against the wide L2 champion). `None` keeps the production default
        // (`LeafEval::TexelWide`). Only the two unit-variant leaves are
        // exposed here — runtime/learned leaves carry weights and are not
        // selectable by string.
        let eval = match leaf_eval {
            None => LeafEval::default(),
            Some("texel") => LeafEval::Texel,
            Some("texel-wide" | "texel_wide") => LeafEval::TexelWide,
            Some(other) => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "leaf_eval must be 'texel' or 'texel-wide', got {other:?}"
                )))
            }
        };
        let inner = Mcts::with_policy(state.inner.clone(), cfg, policy.clone())
            .with_leaf_eval(eval.clone());
        Ok(Self {
            inner,
            root_state: state.inner.clone(),
            config: cfg,
            policy,
            leaf_eval: eval,
            threads: threads.max(1),
        })
    }

    /// Run `max_simulations` and return a dict with `action`, `visits`,
    /// `value`. Each entry in `visits` carries the per-action breakdown:
    /// `action`, `visits`, `value` (mean), `wins`, `losses`, `ties`, plus
    /// derived probabilities `p_win`, `p_tie`, `p_loss`.
    fn search<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        // Run the search off the GIL: the rollouts are pure-Rust and touch no
        // Python objects, so releasing it lets other Python threads make
        // progress (mirrors `recommend`/`search_tree` below) and makes a
        // thread-pool driver — not just the process pool — viable.
        //
        // `threads > 1` runs root-parallel `par_search_with_policy`: the sim
        // budget (`config.max_simulations`) is SPLIT across workers and the
        // root-child stats merged (same total sims, faster wall-clock). The
        // single-tree `inner` path is used for `threads <= 1`.
        let (action, outcomes) = if self.threads > 1 {
            let root_state = self.root_state.clone();
            let config = self.config;
            let policy = self.policy.clone();
            let leaf_eval = self.leaf_eval.clone();
            let workers = self.threads;
            py.allow_threads(move || {
                let r = par_search_with_policy(root_state, config, workers, policy, leaf_eval);
                (r.best_action, r.outcomes)
            })
        } else {
            let inner = &mut self.inner;
            py.allow_threads(move || {
                let action = inner.search();
                let outcomes = inner.root_outcomes();
                (action, outcomes)
            })
        };
        let action_dict = action_to_pydict(py, &action)?;
        let visits = PyList::empty(py);
        let mut best_value = 0.0_f32;
        for o in &outcomes {
            let entry = PyDict::new(py);
            entry.set_item("action", action_to_pydict(py, &o.action)?)?;
            entry.set_item("visits", o.visits)?;
            entry.set_item("value", o.mean_value)?;
            entry.set_item("wins", o.wins)?;
            entry.set_item("losses", o.losses)?;
            entry.set_item("ties", o.ties)?;
            let vdenom = f64::from(o.visits.max(1));
            entry.set_item("p_win", f64::from(o.wins) / vdenom)?;
            entry.set_item("p_tie", f64::from(o.ties) / vdenom)?;
            entry.set_item("p_loss", f64::from(o.losses) / vdenom)?;
            visits.append(entry)?;
            if o.action == action {
                best_value = o.mean_value;
            }
        }
        let out = PyDict::new(py);
        out.set_item("action", action_dict)?;
        out.set_item("visits", visits)?;
        out.set_item("value", best_value)?;
        Ok(out)
    }

    /// Return the principal variation from the root.
    fn get_pv<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let pv = self.inner.get_pv();
        let list = PyList::empty(py);
        for a in &pv {
            list.append(action_to_pydict(py, a)?)?;
        }
        Ok(list)
    }
}

#[pyfunction]
fn version() -> &'static str {
    sts2_core::version()
}

/// The run-AI's expected-HP-cost tables (`[projection]` in the embedded
/// `data/runai/heuristic_v.toml`), as a plain dict.
///
/// Exists for the live-game advisor, which draws the champion's projected
/// route on the map HUD and has to walk HP forward between hops — the
/// champion only rest-seeks when it believes it is hurt, so a frozen-HP walk
/// never draws a campfire. That lane reconstructs its obs from the bridge
/// envelope and has **no `RunManager`**, so it cannot reach
/// `PyRunManager::path_plan` or `projection::project_room` (both need a
/// `VisibleRunView`). The tables are the part that transfers.
///
/// Deliberately the tables and not a `project_room` wrapper: the rest-site
/// branch needs `act_progress()` for its heal-vs-smith choice and the event
/// branch needs the remaining-event pool, neither of which exists without a
/// view. Deliberately a getter and not a Python-side copy: this TOML is
/// `include_str!`'d into the binary, so a second copy in the advisor would be
/// a source of truth nothing gates.
#[pyfunction]
fn projection_hp_table(py: Python<'_>) -> PyResult<PyObject> {
    let t = &sts2_runai::eval::Weights::embedded().projection;
    let d = PyDict::new(py);
    d.set_item("monster_hp_loss", t.monster_hp_loss.to_vec())?;
    d.set_item("elite_hp_loss", t.elite_hp_loss.to_vec())?;
    d.set_item("event_hp_loss", t.event_hp_loss)?;
    d.set_item("unknown_hp_loss", t.unknown_hp_loss)?;
    d.set_item("rest_heal_frac", t.rest_heal_frac)?;
    Ok(d.into())
}

/// Git commit this extension was built from (stamped by `build.rs` into
/// `STS2_BUILD_GIT`), or `"unknown"` when git was unavailable at build
/// time. The parity harness's `assert_wheel_fresh` compares this to the
/// source tree's `HEAD` to reject a wheel built from a stale checkout —
/// the staleness mode the mtime + content probes can't detect. Bead
/// `sts2-rs-ybtmu` follow-up.
#[pyfunction]
fn build_git() -> &'static str {
    env!("STS2_BUILD_GIT")
}

/// Content hash of sts2-py's own source, INCLUDING the `include_bytes!`
/// value/policy-net data, stamped at build time. See `sts2_core::SRC_HASH`.
pub const SRC_HASH: &str = env!("STS2_PY_SRC_HASH");

/// Aggregate content hash of the whole engine: the compiled-in `SRC_HASH` const
/// of every workspace crate linked into this extension plus the workspace
/// inputs (`STS2_WORKSPACE_HASH` = the workspace Cargo.toml + Cargo.lock +
/// rust-toolchain.toml), combined in `sts2_srchash::ENGINE_CRATES` order.
/// Compiled INTO the `.so`, so it reflects
/// exactly what each crate was last *compiled* from — a stale dependency rlib
/// carries its old per-crate hash. The parity harness compares this to
/// [`recompute_src_hash`] to prove the loaded engine matches its source
/// (issue #15, bead `sts2-rs-cg5ci`).
#[pyfunction]
fn src_hash() -> String {
    sts2_srchash::combine(&[
        sts2_core::SRC_HASH,
        sts2_content::SRC_HASH,
        sts2_mcts::SRC_HASH,
        SRC_HASH,
        sts2_runai::SRC_HASH,
        sts2_wire::SRC_HASH,
        sts2_cli::SRC_HASH,
        sts2_vocab::SRC_HASH,
        env!("STS2_WORKSPACE_HASH"),
    ])
}

/// Recompute the aggregate engine hash from the on-disk source at `rs_root`
/// (the sts2-rs checkout root). Uses the SAME `sts2_srchash` code as the
/// build-time stamp, so on a clean tree this equals [`src_hash`]; a mismatch
/// means the loaded `.so` was not compiled from the source at `rs_root`.
#[pyfunction]
fn recompute_src_hash(rs_root: &str) -> String {
    sts2_srchash::recompute_engine_hash(std::path::Path::new(rs_root))
}

/// How this extension was compiled, as stamped by `scripts/pgo_wheel.sh`.
///
/// The stamps are compile-time env vars read with `option_env!`, which rustc
/// records as env dependencies, so cargo recompiles this crate when one
/// changes. Only `pgo_wheel.sh` sets them: `STS2_WHEEL_PGO_PROFDATA_SHA256`
/// (the sha256 of the merged `.profdata`) on its `-Cprofile-use` build,
/// `STS2_WHEEL_PGO_INSTRUMENTED` on its `-Cprofile-generate` build, and
/// `STS2_WHEEL_TARGET_CPU` with `--v3`. Every other build (`wheel-venv.sh`,
/// maturin, uv, cargo) reports a plain, portable build. None of them reaches
/// [`src_hash`], so a plain and a PGO build of one source share it and both
/// pass the freshness gate. The PPO trainer refuses a wheel whose `pgo` is
/// false (`sts2-pycli/python/ppo/wheel_guard.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildInfo {
    /// A `-Cprofile-use` build: true only when the profdata stamp is set and
    /// the instrumented stamp is not.
    pub pgo: bool,
    /// The `-Cprofile-generate` build that collects the profile.
    pub pgo_instrumented: bool,
    pub profdata_sha256: Option<&'static str>,
    /// `None` = no `-Ctarget-cpu`, i.e. a portable build.
    pub target_cpu: Option<&'static str>,
}

impl BuildInfo {
    /// The stamps this extension was compiled with.
    pub fn current() -> Self {
        Self::from_stamps(
            option_env!("STS2_WHEEL_PGO_PROFDATA_SHA256"),
            option_env!("STS2_WHEEL_PGO_INSTRUMENTED"),
            option_env!("STS2_WHEEL_TARGET_CPU"),
        )
    }

    /// An empty value counts as unset, so `VAR= maturin develop` stamps
    /// nothing.
    fn from_stamps(
        profdata_sha256: Option<&'static str>,
        instrumented: Option<&'static str>,
        target_cpu: Option<&'static str>,
    ) -> Self {
        let set = |v: Option<&'static str>| v.filter(|s| !s.is_empty());
        let profdata_sha256 = set(profdata_sha256);
        let pgo_instrumented = set(instrumented).is_some();
        Self {
            pgo: profdata_sha256.is_some() && !pgo_instrumented,
            pgo_instrumented,
            profdata_sha256,
            target_cpu: set(target_cpu),
        }
    }
}

/// [`BuildInfo`] as a dict, plus [`src_hash`] and [`build_git`], so a run can
/// record which engine build it ran on.
#[pyfunction]
fn build_info(py: Python<'_>) -> PyResult<PyObject> {
    let info = BuildInfo::current();
    let d = PyDict::new(py);
    d.set_item("pgo", info.pgo)?;
    d.set_item("pgo_instrumented", info.pgo_instrumented)?;
    d.set_item("profdata_sha256", info.profdata_sha256)?;
    d.set_item("target_cpu", info.target_cpu)?;
    d.set_item("src_hash", src_hash())?;
    d.set_item("build_git", build_git())?;
    Ok(d.into())
}

/// Port of C# `StringHelper.GetDeterministicHashCode`.
///
/// Exposed to Python so the parity-fuzz harness can derive the same
/// uint seed that C# `RunRngSet(stringSeed)` uses internally
/// (`Seed = (uint)GetDeterministicHashCode(stringSeed)`), letting both
/// engines start combat from byte-identical RNG state. Bead `sts2-rs-ykmk9`.
#[pyfunction]
fn deterministic_hash_code(text: &str) -> i32 {
    sts2_core::deterministic_hash_code(text)
}

/// Roll the run's act list from a display seed — the port of C#
/// `ActModel.GetRandomList` off the dedicated `act_selection` stream
/// (`StartRunLobby.BeginRunLocally`). Returns the three act id slugs in
/// index order, e.g. `["UNDERDOCKS", "HIVE", "GLORY"]`.
///
/// This exposes the SELECTION only. Underdocks is ported (#553) and
/// `RunManager(act1=...)` walks it; `RunManager.act1` reports the resolved
/// act, which is what a caller building a run should read.
/// `run_parity.py --act-parity` compares this against the C# oracle
/// seed-for-seed, which is the gate that would have caught the Vantom /
/// Lagavulin Matriarch divergence.
///
/// `underdocks_unlocked` mirrors `UnlockState.IsEpochRevealed<UnderdocksEpoch>()`.
/// `force_undiscovered` mirrors `TestMode.IsOff && !isMultiplayer`: leave it
/// false to match the headless oracle (pure seed roll), true to model a real
/// singleplayer save where an undiscovered Underdocks is force-picked.
#[pyfunction]
#[pyo3(signature = (seed, underdocks_unlocked=false, underdocks_discovered=false, force_undiscovered=false))]
fn roll_acts(
    seed: &str,
    underdocks_unlocked: bool,
    underdocks_discovered: bool,
    force_undiscovered: bool,
) -> Vec<String> {
    let unlocks = sts2_core::ActSelectionUnlocks {
        underdocks_unlocked,
        underdocks_discovered,
        force_undiscovered,
    };
    sts2_core::act_selection::roll_acts_for_seed(seed, &unlocks)
        .iter()
        .map(|a| a.id().to_string())
        .collect()
}

/// `n` **pass-every-turn** playouts from `state`, reporting the win rate.
///
/// WARNING — this is NOT a heuristic-agent baseline. [`run_playout`] applies
/// only [`Action::EndTurn`]: it plays no cards, so the player stands still
/// and dies in nearly every fight. The win rate this returns is therefore
/// ~0% for essentially any real encounter, for every character.
///
/// Do NOT use it as the denominator of a search-vs-baseline A/B — "lift over
/// a baseline that never wins" is just the search arm's own win rate. Use
/// [`heuristic_play_to_terminal`] for a baseline that actually plays.
/// (A 780-fight Necro-vs-IC lift panel was voided by exactly this mistake;
/// it scored 0.0% baseline in every cell and produced a tight, entirely
/// spurious CI.)
///
/// Its legitimate use is as a cheap engine-throughput / termination smoke
/// check, where "does the fight end at all" is the question.
#[pyfunction]
#[pyo3(signature = (state, n = 1000))]
fn rollouts(state: &PyCombatState, n: u64) -> PyResult<PyObject> {
    let mut wins = 0u64;
    let mut losses = 0u64;
    let mut total_turns = 0u64;

    for offset in 0..n {
        let mut s = state.inner.clone();
        s.rng = sts2_core::dotnet_rng::Rng::new(state.seed.wrapping_add(offset) as i32);
        let (won, turns) = run_playout(&mut s);
        if won {
            wins += 1;
        } else {
            losses += 1;
        }
        total_turns += u64::from(turns);
    }

    #[allow(clippy::cast_precision_loss)]
    let win_rate = wins as f64 / n as f64;
    #[allow(clippy::cast_precision_loss)]
    let avg_turns = total_turns as f64 / n as f64;

    Python::with_gil(|py| {
        let dict = PyDict::new(py);
        dict.set_item("wins", wins)?;
        dict.set_item("losses", losses)?;
        dict.set_item("n", n)?;
        dict.set_item("win_rate", win_rate)?;
        dict.set_item("avg_turns", avg_turns)?;
        Ok(dict.into())
    })
}

/// Parse the wire `hidden_info` string into a [`sts2_mcts::HiddenInfoMode`].
///
/// Shared by `mcts_recommend` and `mcts_play_to_terminal` so the two cannot
/// drift. Rejects an unrecognized value rather than falling back: a typo in
/// an A/B arm's config must fail loudly, not silently run the wrong search.
pub fn parse_hidden_info(mode: &str, fn_name: &str) -> PyResult<sts2_mcts::HiddenInfoMode> {
    sts2_wire::emit::mcts::parse_hidden_info(mode, fn_name)
        .map_err(pyo3::exceptions::PyValueError::new_err)
}

/// Resolve the search constants that TRAVEL WITH a hidden-information mode.
///
/// `c_puct` and `draw_widening_c` are not free knobs: the deployed BeliefDraw
/// honest search is the triple (beliefdraw, `belief_draw_c_puct(sims)`, 2.0)
/// (the F2 retune, `sts2-rs-09x5n`), and pairing BeliefDraw with the engine's
/// `SQRT_2` / no-widening defaults is a mistuned search, not a neutral one.
/// `None` from Python therefore means "whatever this mode deploys" rather than
/// a fixed number, so flipping the default mode cannot leave the constants
/// behind. An explicit value always wins — an ablation still gets what it asks
/// for.
///
/// Pure and public so a test can pin the pairing without running a search.
pub fn resolve_search_constants(
    hidden_info: sts2_mcts::HiddenInfoMode,
    c_puct: Option<f32>,
    draw_widening_c: Option<f32>,
    sims: u32,
) -> (f32, Option<f32>) {
    let honest = hidden_info == sts2_mcts::HiddenInfoMode::BeliefDraw;
    let c = c_puct.unwrap_or_else(|| {
        if honest {
            sts2_runai::policy::belief_draw_c_puct(sims)
        } else {
            std::f32::consts::SQRT_2
        }
    });
    let dw = draw_widening_c.or(honest.then_some(sts2_mcts::BELIEF_DRAW_DRAW_WIDENING_C));
    (c, dw)
}

/// Build one per-action search config for [`mcts_play_to_terminal`].
///
/// Pure and public so a test can pin that the DEPLOYED honest triple
/// (beliefdraw / c_puct 2.5 / draw_widening_c 2.0) actually lands in the
/// config — it previously inherited `MctsConfig::default()`'s Clairvoyant.
#[allow(clippy::too_many_arguments)]
pub fn play_to_terminal_config(
    sims: u32,
    max_rollout_depth: u32,
    seed: u64,
    c_puct: f32,
    hidden: sts2_mcts::HiddenInfoMode,
    draw_widening_c: f32,
    action_count: u32,
) -> MctsConfig {
    MctsConfig {
        c_puct,
        max_simulations: sims,
        max_rollout_depth,
        seed: seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(u64::from(action_count)),
        terminal_eval_mode: TerminalEvalMode::RichBoth,
        use_puct_prior: false,
        hidden_info: hidden,
        draw_widening_c,
        ..MctsConfig::default()
    }
}

/// Drive a single combat from `state` to terminal under MCTS-recommended
/// actions and return `(player_won, final_hp, turns, actions, wall_ms)`.
///
/// Mirrors `play_one_fight` in `closed_loop_harness.rs` but exposes the
/// loop to Python so replay-driver probes can measure a proper
/// MCTS-driven winrate from a captured boss-entry state — instead of
/// reading the root-search value, which only reflects ~1 search worth
/// of signal, not actual playout outcome.
///
/// `max_actions` caps the per-fight action budget (matches the
/// closed-loop harness's `max_actions_per_fight` default of 2000).
///
/// `max_rollout_depth` defaults to 200 to MATCH THE DEPLOYED Texel
/// combat search (run-context MCTS rolls out to `COMBAT_ROLLOUT_DEPTH`
/// = 200, reaching true terminals). Do not lower this for A/B panels: a
/// shallow (e.g. depth-10) Texel rolls out to a heuristic leaf instead
/// of the real fight end and conserves HP far worse (measured +12pp win
/// rate and ~7 HP/fight better at depth 200 on identical states), so any
/// arm A/B'd against a depth-10 Texel is scored against a hobbled
/// baseline.
///
/// `hidden_info` / `draw_widening_c` default to the historical values
/// (`clairvoyant`, 1.0) so pre-existing callers (`boss_only.py`,
/// `boss_sweep_parallel.py`) are byte-identical. Pass
/// `hidden_info="beliefdraw", draw_widening_c=2.0, c_puct=2.5` to replay
/// under the DEPLOYED honest search — required on reconstructed real-DLL
/// states, whose RNG cursors are faked, so a clairvoyant root would plan
/// against a fictional future and overstate how easily the fight is won.
///
/// Search always uses the uniform prior — the bundled learned prior was
/// removed on 2026-08-15 (see the note at the top of this file). The
/// `use_prior` argument is gone rather than accepted-and-ignored, so a caller
/// that still passes it fails loudly instead of quietly getting something
/// other than what it asked for.
///
/// This closes a replay-symmetry hazard rather than opening one: `use_prior`
/// used to default to `True` here and `False` in `mcts_recommend`, so a
/// replay (e.g. the #301 lethal-fight autopsy) could easily run a STRONGER
/// search than the sweep that produced the death it was explaining, winning
/// more fights than the sweep did and biasing every verdict toward
/// "blunder-shaped". Both entry points are now uniform and cannot diverge.
#[pyfunction]
#[pyo3(signature = (
    state,
    sims = 1000,
    max_rollout_depth = 200,
    seed = 0,
    c_puct = std::f32::consts::SQRT_2,
    max_actions = 2000,
    hidden_info = "clairvoyant",
    draw_widening_c = 1.0,
))]
#[allow(clippy::too_many_arguments)]
fn mcts_play_to_terminal(
    state: &PyCombatState,
    sims: u32,
    max_rollout_depth: u32,
    seed: u64,
    c_puct: f32,
    max_actions: u32,
    hidden_info: &str,
    draw_widening_c: f32,
) -> PyResult<PyObject> {
    let hidden = parse_hidden_info(hidden_info, "mcts_play_to_terminal")?;
    let mut s = state.inner.clone();
    let starting_hp = s.player.current_hp;
    // NOTE: do NOT call start_combat / start_player_turn here. The
    // PyCombatState we receive is already an in-progress combat
    // (captured via RunManager::combat_state() or constructed at a
    // mid-fight checkpoint). start_combat would reset round/energy/
    // stars and break the captured state.

    let start = std::time::Instant::now();
    let mut action_count: u32 = 0;
    let mut turn_count: u32 = 0;
    let mut last_round = s.round;
    let policy = SharedPolicy::uniform();

    while !core_terminal_value(&s).0 && action_count < max_actions {
        let acts = core_legal_actions(&s);
        if acts.is_empty() {
            let _ = core_apply(&mut s, Action::EndTurn);
            action_count += 1;
            continue;
        }
        let cfg = play_to_terminal_config(
            sims,
            max_rollout_depth,
            seed,
            c_puct,
            hidden,
            draw_widening_c,
            action_count,
        );
        let mut mcts = Mcts::with_policy(s.clone(), cfg, policy.clone());
        let recommended = mcts.search();
        let _ = core_apply(&mut s, recommended);
        action_count += 1;
        if s.round != last_round {
            turn_count += 1;
            last_round = s.round;
        }
    }

    let wall_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let is_terminal = core_terminal_value(&s).0;
    let final_hp = s.player.current_hp.max(0);
    let player_won = is_terminal && final_hp > 0;
    let dmg_taken = (starting_hp - final_hp).max(0);

    Python::with_gil(|py| {
        let d = PyDict::new(py);
        d.set_item("player_won", player_won)?;
        d.set_item("final_hp", final_hp)?;
        d.set_item("starting_hp", starting_hp)?;
        d.set_item("dmg_taken", dmg_taken)?;
        d.set_item("turns", turn_count)?;
        d.set_item("actions", action_count)?;
        d.set_item("wall_ms", wall_ms)?;
        d.set_item("timed_out", !is_terminal)?;
        Ok(d.into())
    })
}

/// Build a per-action score dict consumed by the Python hint adapter.
///
/// Shape:
/// ```text
/// {
///   'action': {<bridge action dict>},
///   'visits': u32,
///   'value': f32,           # Q = (wins - losses) / visits
///   'wins': u32,
///   'losses': u32,
///   'ties': u32,
///   'p_win': f64,           # wins / visits
///   'p_tie': f64,
///   'p_loss': f64,
///   'bucket': str,          # WIN_PROB_BUCKETS label for p_win
///   'low_confidence': bool, # visits < LOW_CONFIDENCE_VISIT_FRACTION * max_visits
///   'q_saturated': bool,    # global flag — Q spread < Q_SATURATION_EPSILON
///   'is_best': bool,        # set on exactly one row (Q-then-visits tie-break)
/// }
/// ```
#[allow(clippy::too_many_arguments)]
pub(crate) fn action_score_pydict<'py>(
    py: Python<'py>,
    action: &Action,
    visits: u32,
    wins: u32,
    losses: u32,
    ties: u32,
    value_sum: f32,
    low_confidence: bool,
    q_saturated: bool,
    is_best: bool,
) -> PyResult<Bound<'py, PyDict>> {
    crate::value_to_pydict(
        py,
        &sts2_wire::emit::mcts::action_score_value(
            action,
            visits,
            wins,
            losses,
            ties,
            value_sum,
            low_confidence,
            q_saturated,
            is_best,
        ),
    )
}

/// `potion_charge_hp` (a list of at most 8 finite HP-equivalents, by the
/// engine's dense belt slot; missing slots 0) → `RecommendParams`' array.
fn potion_charge_slots(charges: Option<Vec<f32>>) -> PyResult<Option<[f32; 8]>> {
    let Some(charges) = charges else { return Ok(None) };
    if charges.len() > 8 || charges.iter().any(|c| !c.is_finite()) {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "mcts_recommend: potion_charge_hp must be at most 8 finite values, got {charges:?}"
        )));
    }
    let mut out = [0.0_f32; 8];
    out[..charges.len()].copy_from_slice(&charges);
    Ok(Some(out))
}

/// `belt_value` (a dict: `base`, the belt's potion ids; `table`, a list of
/// `[potion ids, hp]` giving every sub-multiset of `base` once; `fallback`,
/// `{potion id: hp}` for a potion beyond the base; `default`, hp for a kind
/// without a fallback) → the engine's [`sts2_core::belt_value::BeltValue`].
fn parse_belt_value(d: &Bound<'_, PyDict>) -> PyResult<std::sync::Arc<sts2_core::belt_value::BeltValue>> {
    use pyo3::exceptions::PyValueError;
    let kind = |name: String| {
        sts2_wire::state_dict::potion_kind_from_python_name(&name)
            .ok_or_else(|| PyValueError::new_err(format!("mcts_recommend: belt_value: unknown potion {name:?}")))
    };
    let kinds = |names: Vec<String>| names.into_iter().map(kind).collect::<PyResult<Vec<_>>>();
    let get = |key: &str| {
        d.get_item(key)?
            .ok_or_else(|| PyValueError::new_err(format!("mcts_recommend: belt_value needs {key:?}")))
    };
    let base = kinds(get("base")?.extract::<Vec<String>>()?)?;
    let table = get("table")?
        .extract::<Vec<(Vec<String>, f32)>>()?
        .into_iter()
        .map(|(k, v)| Ok((kinds(k)?, v)))
        .collect::<PyResult<Vec<_>>>()?;
    let fallback = get("fallback")?
        .extract::<std::collections::BTreeMap<String, f32>>()?
        .into_iter()
        .map(|(k, v)| Ok((kind(k)?, v)))
        .collect::<PyResult<Vec<_>>>()?;
    let default = get("default")?.extract::<f32>()?;
    sts2_core::belt_value::BeltValue::new(base, table, fallback, default)
        .map(std::sync::Arc::new)
        .map_err(|e| PyValueError::new_err(format!("mcts_recommend: {e}")))
}

/// The victory eval's potion price λ (HP-equivalents per held potion) in
/// effect for this process: `STS2_RUNAI_POTION_WEIGHT`, else 8. Read once
/// and latched, by this call or the first search (sts2-agent fork).
#[pyfunction]
fn potion_weight() -> f32 {
    sts2_core::victory_potion_weight()
}

/// MCTS recommendation surface for the Python hint adapter.
///
/// Returns a list of per-action `ActionScore` dicts ranked by Q (mean
/// value) descending, with visit count as the secondary tiebreaker —
/// matching the sort the CLI hint formatter uses. The top row has
/// `is_best=True`.
///
/// `potion_charge_hp` (sts2-agent fork): a potion carry-over charge in
/// HP-equivalents per dense belt slot (at most 8 values); each drink's
/// reported `q` is lowered by its slot's charge before ranking, and the
/// result carries `potion_charge_hp`. Pair it with a zero victory-eval
/// potion price (`STS2_RUNAI_POTION_WEIGHT=0`, see `potion_weight()`).
///
/// `belt_value` (sts2-agent fork): the search's value of the potion belt,
/// carried on its root copy and used by every evaluation in the search (win
/// terminals and both depth-cap leaves) instead of `potions · λ`, so a line
/// pays for a drink wherever it drinks; see `parse_belt_value` for its shape.
/// A returned `pv` always starts with the `is_best` action, else is empty.
///
/// # Modes
///
/// * `fair=False` (default) — run ONE [`Mcts::search`] from `state` and
///   read root visit/win/loss/tie counters per child. Cheap; suffers
///   the usual UCB selection bias (visit counts drift to the best-
///   looking branch, low-Q children get under-explored).
///
/// * `fair=True` — for each legal action at `state`, clone the state,
///   apply the action, and run an INDEPENDENT MCTS from the resulting
///   post-action state with `sims` simulations. Each per-action search
///   reports its own root-level win/loss/tie aggregate (summed across
///   the children of the post-action root). Free of the main-search
///   UCB bias; answers "win% if I commit to this action and play
///   well from there." Mirrors `run_fair_eval` in
///   `sts2-cli/src/agent/human.rs`.
///
/// # GIL
///
/// The actual MCTS work runs inside `py.allow_threads(...)` so multiple
/// Python threads can call this concurrently. Verified by the
/// `gil_release` integration test.
#[pyfunction]
#[pyo3(signature = (
    state,
    *,
    sims = 1000,
    c_puct = None,
    depth = 200,
    seed = 0,
    threads = 8,
    fair = false,
    rollout_policy = "heuristic",
    hidden_info = "beliefdraw",
    draw_widening_c = None,
    common_random_numbers = false,
    endturn_widening_c = None,
    endturn_widening_alpha = None,
    potion_charge_hp = None,
    belt_value = None,
))]
#[allow(clippy::too_many_arguments)]
fn mcts_recommend<'py>(
    py: Python<'py>,
    state: &PyCombatState,
    sims: u32,
    c_puct: Option<f32>,
    depth: u32,
    seed: u64,
    threads: u32,
    fair: bool,
    rollout_policy: &str,
    hidden_info: &str,
    draw_widening_c: Option<f32>,
    common_random_numbers: bool,
    endturn_widening_c: Option<f32>,
    endturn_widening_alpha: Option<f32>,
    potion_charge_hp: Option<Vec<f32>>,
    belt_value: Option<&Bound<'py, PyDict>>,
) -> PyResult<Bound<'py, PyDict>> {
    // The search config, the ranking and the dict shape all live in
    // `sts2_wire::emit::mcts` — the same code the browser workbench runs, so
    // the two hosts cannot drift. This wrapper only maps the Python-facing
    // arguments and releases the GIL.
    //
    // `hidden_info` defaults to `beliefdraw`, the DEPLOYED honest search —
    // the mode `sts2_runai::policy::resolve_hidden_info` picks with
    // `STS2_RUNAI_HONEST` unset, i.e. what every agent that plays a run uses.
    // It defaulted to the leaky `clairvoyant` for byte-stability, and that
    // default is exactly how omitting the argument shipped leaky advice: the
    // workbench recorder dropped the keyword and searched every recorded
    // decision with a root that could see the draw order.
    //
    // `endturn_widening_c` / `endturn_widening_alpha` are ablation knobs:
    // `None` keeps what the mode deploys (`MctsConfig::apply_mode_dialect`,
    // the run agent's PR #467 values under beliefdraw — GH #758).
    let hidden_info = parse_hidden_info(hidden_info, "mcts_recommend")?;
    let (c_puct, draw_widening_c) =
        resolve_search_constants(hidden_info, c_puct, draw_widening_c, sims);
    let params = RecommendParams {
        sims,
        depth,
        c_puct,
        seed,
        threads,
        fair,
        rollout_policy: sts2_wire::emit::mcts::parse_rollout_policy(
            rollout_policy,
            "mcts_recommend",
        )
        .map_err(pyo3::exceptions::PyValueError::new_err)?,
        hidden_info,
        draw_widening_c,
        endturn_widening_c,
        endturn_widening_alpha,
        common_random_numbers,
        potion_charge_hp: potion_charge_slots(potion_charge_hp)?,
        belt_value: belt_value.map(parse_belt_value).transpose()?,
    };
    let root_state = state.inner.clone();
    // Forward the wire-side trust flag so `--mcts-log` consumers can tell
    // when the win/loss split was computed against 0-damage attack intents
    // (i.e. the continuous Q over-states safety). Bead `sts2-rs-bpt4`.
    let intent_damage_known = state.intent_damage_known;
    // All MCTS work happens off-GIL so concurrent Python threads can
    // actually parallelize.
    let out = py.allow_threads(move || {
        sts2_wire::emit::mcts::recommend_value(&root_state, &params, intent_damage_known)
    });
    crate::value_to_pydict(py, &out)
}

/// Run a single (non-fair) MCTS search and serialize the **entire search
/// tree** for the agent search-tree inspector.
///
/// Returns `{nodes, root, pv, sims, c_puct}`; the node shape is documented
/// on [`sts2_wire::emit::mcts::search_tree_value`], which builds it — the
/// same function the browser workbench's inspector calls. Config mirrors
/// `mcts_recommend` (single tree, single thread) so the tree matches the
/// hints shown elsewhere; `hidden_info` / `draw_widening_c` are plumbed for
/// the same reason `mcts_recommend` plumbs them (without them the inspector
/// silently ran the leaky `Clairvoyant` default while the hint panel beside
/// it ran the deployed honest `beliefdraw` search), and so are the
/// `endturn_widening_*` ablation knobs. Defaults stay at `mcts_recommend`'s,
/// which are now the deployed honest ones.
#[pyfunction]
#[pyo3(signature = (
    state,
    *,
    sims = 1000,
    c_puct = None,
    depth = 200,
    seed = 0,
    rollout_policy = "heuristic",
    hidden_info = "beliefdraw",
    draw_widening_c = None,
    endturn_widening_c = None,
    endturn_widening_alpha = None,
))]
#[allow(clippy::too_many_arguments)]
fn mcts_search_tree<'py>(
    py: Python<'py>,
    state: &PyCombatState,
    sims: u32,
    c_puct: Option<f32>,
    depth: u32,
    seed: u64,
    rollout_policy: &str,
    hidden_info: &str,
    draw_widening_c: Option<f32>,
    endturn_widening_c: Option<f32>,
    endturn_widening_alpha: Option<f32>,
) -> PyResult<Bound<'py, PyDict>> {
    let hidden_info = parse_hidden_info(hidden_info, "mcts_search_tree")?;
    let (c_puct, draw_widening_c) =
        resolve_search_constants(hidden_info, c_puct, draw_widening_c, sims);
    let params = RecommendParams {
        sims,
        depth,
        c_puct,
        seed,
        rollout_policy: sts2_wire::emit::mcts::parse_rollout_policy(
            rollout_policy,
            "mcts_search_tree",
        )
        .map_err(pyo3::exceptions::PyValueError::new_err)?,
        hidden_info,
        draw_widening_c,
        endturn_widening_c,
        endturn_widening_alpha,
        ..RecommendParams::default()
    };
    let root_state = state.inner.clone();
    let out =
        py.allow_threads(move || sts2_wire::emit::mcts::search_tree_value(&root_state, &params));
    crate::value_to_pydict(py, &out)
}

#[pymodule]
pub fn sts2_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // First, before anything else runs in the extension: mimalloc must never
    // purge, or spaced advisor calls fault the search tree back in (A1b).
    allocator::never_purge();
    // Bead `sts2-rs-7hhd`: install the card-effect dispatch registry at
    // module import time. Without this, any state built via
    // `state_from_dict` (or any other path that bypasses
    // `build_encounter_state`) would have no card handlers registered —
    // every `Action::PlayCard` would silently no-op, MCTS rollouts would
    // never see the player damage an enemy, and `wins=0` for every
    // action (see the bead repro). Idempotent.
    card_dispatch::install_default_registry();
    // Install the relic + potion dispatch registries too. Without these,
    // run-side relic `after_obtained` listeners (Precise Scissors et al.
    // removing cards, MawBank gold, …) and potion effects silently no-op
    // through the `RunManager` run loop — only card effects were wired at
    // module import. `ensure_installed` is idempotent. Mirrors the CLI host
    // (`sts2-cli/src/lib.rs`).
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();

    m.add_function(wrap_pyfunction!(resolve_potion_swap_flag, m)?)?;
    m.add_function(wrap_pyfunction!(potion_swap_is_rule_side, m)?)?;
    m.add_function(wrap_pyfunction!(non_removable_cards, m)?)?;
    m.add_function(wrap_pyfunction!(eternal_granting_enchantments, m)?)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(build_git, m)?)?;
    m.add_function(wrap_pyfunction!(projection_hp_table, m)?)?;
    m.add_function(wrap_pyfunction!(src_hash, m)?)?;
    m.add_function(wrap_pyfunction!(recompute_src_hash, m)?)?;
    m.add_function(wrap_pyfunction!(build_info, m)?)?;
    m.add_function(wrap_pyfunction!(deterministic_hash_code, m)?)?;
    m.add_function(wrap_pyfunction!(roll_acts, m)?)?;
    m.add_function(wrap_pyfunction!(rollouts, m)?)?;
    m.add_function(wrap_pyfunction!(mcts_play_to_terminal, m)?)?;
    m.add_function(wrap_pyfunction!(heuristic_play_to_terminal, m)?)?;
    m.add_class::<PyCombatState>()?;
    m.add_class::<PyMcts>()?;
    m.add_class::<crate::run_manager::PyRunManager>()?;
    m.add_class::<crate::heuristic_agent::PyHeuristicRunAgent>()?;
    m.add_class::<crate::mcts_combat_agent::PyMctsCombatAgent>()?;
    m.add_class::<crate::combat_surrogate::PyCombatSurrogate>()?;
    m.add_class::<crate::ppo_agent::PyPpoRunAgent>()?;
    m.add_function(wrap_pyfunction!(crate::ppo_agent::ppo_observe_json, m)?)?;
    // Bead sts2-rs-92sk (hint-T1): wire-format parsing + registry
    // introspection. The Python adapter (`sts2-cli/python/sts2_hint/`)
    // catches `UnsupportedStateError` to suppress hints loudly without
    // crashing combat.
    let py = m.py();
    m.add(
        "UnsupportedStateError",
        py.get_type::<state_dict::UnsupportedStateError>(),
    )?;
    m.add("WIRE_VERSION", state_dict::WIRE_VERSION)?;
    m.add_function(wrap_pyfunction!(state_dict::parse_state_dict_py, m)?)?;
    m.add_function(wrap_pyfunction!(state_dict::list_registry_names, m)?)?;
    m.add_function(wrap_pyfunction!(potion_weight, m)?)?;
    // GH#286: per-id (Debug, python_name) forms for the vocab round-trip test.
    m.add_function(wrap_pyfunction!(state_dict::list_registry_id_forms, m)?)?;
    // Bead sts2-rs-osyf (hint-T2): wire-format → CoreCombatState
    // constructor. Returns a `PyCombatState` whose `legal_actions`
    // reflects the injected mid-turn snapshot.
    m.add_function(wrap_pyfunction!(state_from_dict_py, m)?)?;
    // Parity-fuzz T7: scenario-shaped state constructor matching the C#
    // headless `init_scenario` command (Task 7 of
    // `docs/plans/2026-05-23-parity-fuzz-harness.md`).
    m.add_function(wrap_pyfunction!(build_scenario_state_py, m)?)?;
    // Workbench MCTS recorder: load a `.run-source` scenario TOML into
    // a `CombatState` via the same path `sts2-fight-replay` uses.
    m.add_function(wrap_pyfunction!(combat_state_from_scenario_toml, m)?)?;
    // SimLab sandbox catalog: forceable monster move states + the encounter
    // registry, so the position editor's dropdowns come from the engine.
    m.add_function(wrap_pyfunction!(monster_moves, m)?)?;
    m.add_function(wrap_pyfunction!(list_encounters, m)?)?;
    // Bead sts2-rs-cosb (hint-T4): MCTS recommendation surface +
    // win-probability bucket table.
    m.add_function(wrap_pyfunction!(mcts_recommend, m)?)?;
    m.add_function(wrap_pyfunction!(mcts_search_tree, m)?)?;
    // Persistent-combat-mirror surface (the rest are `PyCombatState`
    // methods; this one is a pure classification of a choice kind).
    m.add_function(wrap_pyfunction!(choice_kind_is_generative, m)?)?;
    m.add_function(wrap_pyfunction!(set_locked_epochs, m)?)?;
    m.add_function(wrap_pyfunction!(known_epochs, m)?)?;
    m.add_function(wrap_pyfunction!(locked_cards, m)?)?;
    m.add_function(wrap_pyfunction!(set_damage_trace, m)?)?;
    m.add_function(wrap_pyfunction!(take_damage_trace, m)?)?;
    m.add_function(wrap_pyfunction!(damage_trace_dropped, m)?)?;
    let buckets = PyList::empty(py);
    for (lo, label) in mcts_hint::WIN_PROB_BUCKETS {
        let row = PyTuple::new(
            py,
            [
                lo.into_pyobject(py)?.into_any(),
                label.into_pyobject(py)?.into_any(),
            ],
        )?;
        buckets.append(row)?;
    }
    m.add("WIN_PROB_BUCKETS", buckets)?;
    // Bead sts2-rs-wfjn (hint-T3): inverse — CoreCombatState → wire
    // dict. Round-trips through `state_from_dict` modulo documented
    // ignored fields (RNG cursor, combat_log, hook listeners, intent
    // history).
    m.add_function(wrap_pyfunction!(state_dict::state_to_dict_py, m)?)?;
    // Wire-driven PPO run policy: build the observation from a bridge dict
    // (no `RunManager` exists over the bridge) and optionally score it with
    // the character's champion, all without `numpy`/`onnxruntime`.
    m.add_function(wrap_pyfunction!(ppo_wire::ppo_wire_obs, m)?)?;
    m.add_function(wrap_pyfunction!(ppo_wire::ppo_wire_decide, m)?)?;
    // The `OnnxPolicy.distribution` twin: same contract, `tract` instead of
    // `onnxruntime`, so the advisor bundle needs neither it nor `numpy`.
    m.add_class::<crate::ppo_policy::PyPpoNetPolicy>()?;
    m.add(
        "PpoBudgetExceeded",
        py.get_type::<ppo_policy::PpoBudgetExceeded>(),
    )?;
    Ok(())
}

/// Build a [`PyCombatState`] from a wire-format dict (the same dict
/// shape that [`state_dict::parse_state_dict_py`] validates).
///
/// Bead `sts2-rs-osyf` (hint-T2). The returned object is the canonical
/// `sts2_rs.CombatState` — callers can drive `.legal_actions()`,
/// `.pending_choice()`, `.terminal_value()`, etc.
///
/// # Errors
///
/// * `ValueError` — schema violation (missing key, bad type, hp >
///   `max_hp`, etc.).
/// * `UnsupportedStateError` — wire dict references a card / monster /
///   power / relic id not yet modelled by the Rust hint engine, or is
///   in a mid-resolution state (`active_attack`, `pending_choice`,
///   `pending_play` non-`None`).
#[pyfunction]
#[pyo3(name = "state_from_dict")]
fn state_from_dict_py(d: &Bound<'_, PyDict>) -> PyResult<PyCombatState> {
    // Parse first so we can read the intent_damage_known aggregate before
    // building the inner state; both halves live in `state_dict::` and
    // `state_from_dict` is just `parse_state_dict` + `state_from_dict_impl`.
    let parsed = state_dict::parse_state_dict(d)?;
    let intent_damage_known = parsed.intent_damage_known;
    let inner = state_dict::state_from_dict_impl(&parsed)?;
    // Re-derive a stable seed for `PyCombatState.seed` — used only by
    // `__repr__`. Re-extract from the dict's `rng.seed` field if
    // present, else 0.
    let seed = d
        .get_item("rng")?
        .and_then(|rng| {
            rng.downcast::<PyDict>().ok().and_then(|rd| {
                rd.get_item("seed")
                    .ok()
                    .flatten()
                    .and_then(|s| s.extract::<u64>().ok())
            })
        })
        .unwrap_or(0);
    // Validate structural invariants before handing the state to
    // Python — fail loud here rather than letting a subtle invariant
    // violation surface as a panic deep inside `apply`.
    if let Err(why) = inner.assert_invariants() {
        return Err(state_dict::UnsupportedStateError::new_err(format!(
            "$: state_from_dict produced inconsistent state: {why}"
        )));
    }
    Ok(PyCombatState {
        inner,
        seed,
        intent_damage_known,
    })
}

/// Build a [`PyCombatState`] from a parity-fuzz `Scenario` (Task 7 of
/// `docs/plans/2026-05-23-parity-fuzz-harness.md`).
///
/// Surfaces a 1:1 counterpart to the C# headless `init_scenario` command
/// so the Python driver can pass identical inputs to both engines. The
/// returned state is mid-turn-1: `legal_actions()` is non-empty, the
/// player has drawn an opening hand, the enemy intent is rolled.
///
/// Encounter ids accept the canonical `UPPER_SNAKE` form from
/// `sts2-content/src/encounters/act1_overgrowth.rs` (e.g.
/// `SHRINKER_BEETLE_WEAK`, `VANTOM_BOSS`); deck/relic ids accept the
/// canonical Python-style names from the content registry
/// (`STRIKE_IRONCLAD`, `BURNING_BLOOD`, …).
///
/// # Errors
///
/// * `ValueError` — unknown card/relic/encounter id, invalid hp, or
///   `character != "Ironclad"` (v1 only supports IC).
#[pyfunction]
#[pyo3(name = "build_scenario_state")]
#[pyo3(signature = (
    seed,
    character,
    deck,
    hp,
    max_hp,
    relics,
    encounter,
    ascension = 0,
    monsters = None,
    potions = None,
    move_states = None,
))]
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn build_scenario_state_py(
    seed: u64,
    character: &str,
    deck: Vec<String>,
    hp: i32,
    max_hp: i32,
    relics: Vec<String>,
    encounter: &str,
    ascension: u8,
    monsters: Option<Vec<String>>,
    potions: Option<Vec<String>>,
    move_states: Option<Vec<Option<String>>>,
) -> PyResult<PyCombatState> {
    let potions = potions.unwrap_or_default();
    let inner = build_scenario_state_impl(
        seed,
        character,
        &deck,
        hp,
        max_hp,
        &relics,
        &potions,
        encounter,
        ascension,
        monsters.as_deref(),
        move_states.as_deref(),
    )
    .map_err(pyo3::exceptions::PyValueError::new_err)?;
    Ok(PyCombatState {
        inner,
        seed,
        intent_damage_known: true,
    })
}

/// Seed used when realizing a scenario fixture into a `CombatState`.
/// Mirrors `RecorderConfig::default().seed` in `fight_replay.rs`'s
/// `record_from_fixture` path (the CLI defaults the combat seed to 1),
/// so a fixture loaded here matches one loaded by `sts2-fight-replay`.
const SCENARIO_FIXTURE_SEED: u64 = 1;

/// Load a `.run-source` scenario TOML into a core [`CoreCombatState`],
/// reusing the exact path the `sts2-fight-replay` CLI uses
/// (`load_scenario_file` + `scenario_to_combat_state`).
///
/// Like the CLI replay path (`replay/record.rs`), the state is driven
/// through `start_combat` + `start_player_turn` before it is returned,
/// so the player's opening hand is drawn and energy is granted. Without
/// this the loaded state sits at round 0 with an empty hand and the only
/// legal action `end_turn`, which never advances — the workbench MCTS
/// recorder would otherwise spin forever.
fn load_combat_state_from_toml(path: &std::path::Path) -> Result<CoreCombatState, String> {
    let scenario = sts2_cli::sim::scenario::toml_file::load_scenario_file(path)
        .map_err(|e| format!("load_scenario_file: {e}"))?;
    let mut state = sts2_cli::scenario_to_combat_state(&scenario, SCENARIO_FIXTURE_SEED)
        .map_err(|e| format!("scenario_to_combat_state: {e}"))?;
    let _ = start_combat(&mut state);
    let _ = start_player_turn(&mut state);
    Ok(state)
}

/// Python: `combat_state_from_scenario_toml(path) -> CombatState`.
///
/// Loads a `.run-source` scenario TOML fixture into a `CombatState`
/// for the workbench MCTS recorder, via the same loader the
/// `sts2-fight-replay` binary uses.
#[pyfunction]
fn combat_state_from_scenario_toml(path: &str) -> PyResult<PyCombatState> {
    let inner = load_combat_state_from_toml(std::path::Path::new(path))
        .map_err(pyo3::exceptions::PyValueError::new_err)?;
    Ok(PyCombatState::from_inner(inner, SCENARIO_FIXTURE_SEED))
}

// ---------------------------------------------------------------------------
// Sandbox catalog surface (SimLab position editor)
// ---------------------------------------------------------------------------

/// One move a monster's AI can be parked on: the engine state id the wire's
/// `move_state` accepts, whether it is the AI's opening state, and the head
/// intent it telegraphs (`None` for a move that telegraphs nothing).
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterMoveInfo {
    pub id: &'static str,
    pub initial: bool,
    pub intent: Option<sts2_core::Intent>,
}

/// Every `Move` state of `kind`'s AI, in state-table order.
///
/// The catalog behind the SimLab sandbox's "intent" dropdown: these are the
/// only ids `MonsterAi::force_current_state_by_name` accepts, so a position
/// edited against this list always rebuilds. Built with default spawn args in
/// the `alone`/`front` slot — group-dependent movesets (Nibbit's BUTT/SLICE/
/// HISS chains) are the union of what the state table declares, which is the
/// same table every slot shares; only the *opening* state differs by slot.
///
/// # Errors
///
/// An unknown monster name (PascalCase or UPPER_SNAKE both accepted).
pub fn monster_moves_impl(kind: &str, ascension: u8) -> Result<Vec<MonsterMoveInfo>, String> {
    let species = crate::state_dict::monster_kind_from_python_name(kind)
        .ok_or_else(|| format!("unknown monster {kind:?}"))?;
    let position = SpawnPosition {
        alone: true,
        front: true,
        back: false,
        index: 0,
    };
    let ai = build_ai_with_ascension(species, MonsterSpawnArgs::default(), position, ascension);
    let out = ai
        .states
        .iter()
        .filter_map(|(id, st)| match st {
            sts2_core::monster_ai::MonsterState::Move(mv) => Some(MonsterMoveInfo {
                id,
                initial: ai.initial == *id,
                intent: mv.intents.first().copied(),
            }),
            _ => None,
        })
        .collect();
    Ok(out)
}

/// Python: `monster_moves(kind, ascension=0) -> [{id, initial, intent}]`.
///
/// `intent` is the bare wire intent (`{kind, amount?, times?}`) or `None`.
#[pyfunction]
#[pyo3(signature = (kind, ascension = 0))]
fn monster_moves(py: Python<'_>, kind: &str, ascension: u8) -> PyResult<Vec<Py<PyDict>>> {
    let moves =
        monster_moves_impl(kind, ascension).map_err(pyo3::exceptions::PyValueError::new_err)?;
    let mut out = Vec::with_capacity(moves.len());
    for mv in moves {
        let d = PyDict::new(py);
        d.set_item("id", mv.id)?;
        d.set_item("initial", mv.initial)?;
        match mv.intent {
            Some(intent) => {
                d.set_item(
                    "intent",
                    crate::value_to_pydict(
                        py,
                        &sts2_wire::emit::combat::bare_intent_to_value(intent),
                    )?,
                )?;
            }
            None => d.set_item("intent", py.None())?,
        }
        out.push(d.unbind());
    }
    Ok(out)
}

/// One row of the encounter catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncounterInfo {
    pub id: &'static str,
    pub display: &'static str,
    pub tier: &'static str,
    /// 1-based act number the table belongs to.
    pub act: u8,
    /// The act table's name (`Overgrowth`, `Underdocks`, `Hive`, `Glory`).
    pub act_name: &'static str,
    /// Canonical roster as `MonsterKind::python_name()` strings.
    pub monsters: Vec<&'static str>,
}

/// Every encounter across the four act tables, table order.
///
/// Replaces the hand-kept dropdown list the SimLab UI used to carry: the
/// registry is the source of truth, and `build_scenario_state` resolves
/// every id here through the same `encounters::find`.
#[must_use]
pub fn list_encounters_impl() -> Vec<EncounterInfo> {
    use sts2_content::encounters::{
        act1_overgrowth, act1_underdocks, act2_hive, act3_glory, EncounterDef, EncounterTier,
    };
    fn tier_name(t: EncounterTier) -> &'static str {
        match t {
            EncounterTier::Weak => "Weak",
            EncounterTier::Normal => "Normal",
            EncounterTier::Elite => "Elite",
            EncounterTier::Boss => "Boss",
            // `EncounterTier` is #[non_exhaustive]; a future tier is still
            // listed rather than dropping its encounters from the catalog.
            _ => "Unknown",
        }
    }
    let tables: [(&'static [EncounterDef], u8, &'static str); 4] = [
        (act1_overgrowth::ACT1_ENCOUNTERS, 1, "Overgrowth"),
        (act1_underdocks::ACT1_UNDERDOCKS_ENCOUNTERS, 1, "Underdocks"),
        (act2_hive::ACT2_ENCOUNTERS, 2, "Hive"),
        (act3_glory::ACT3_ENCOUNTERS, 3, "Glory"),
    ];
    let mut out = Vec::new();
    for (table, act, act_name) in tables {
        for def in table {
            out.push(EncounterInfo {
                id: def.id,
                display: def.display,
                tier: tier_name(def.tier),
                act,
                act_name,
                monsters: def
                    .canonical_roster()
                    .iter()
                    .map(|(k, _)| k.python_name())
                    .collect(),
            });
        }
    }
    out
}

/// Python: `list_encounters() -> [{id, display, tier, act, act_name, monsters}]`.
#[pyfunction]
fn list_encounters(py: Python<'_>) -> PyResult<Vec<Py<PyDict>>> {
    let mut out = Vec::new();
    for e in list_encounters_impl() {
        let d = PyDict::new(py);
        d.set_item("id", e.id)?;
        d.set_item("display", e.display)?;
        d.set_item("tier", e.tier)?;
        d.set_item("act", e.act)?;
        d.set_item("act_name", e.act_name)?;
        d.set_item("monsters", e.monsters)?;
        out.push(d.unbind());
    }
    Ok(out)
}

#[cfg(test)]
mod sandbox_catalog_tests {
    use super::*;

    #[test]
    fn monster_moves_lists_forceable_move_states() {
        let moves = monster_moves_impl("Nibbit", 0).expect("Nibbit is a known monster");
        assert!(!moves.is_empty());
        // Every listed id must be accepted by the reconstruction path a
        // sandbox position rides in on.
        let position = SpawnPosition {
            alone: true,
            front: true,
            back: false,
            index: 0,
        };
        for mv in &moves {
            let mut ai = build_ai_with_ascension(
                MonsterKind::Nibbit,
                MonsterSpawnArgs::default(),
                position,
                0,
            );
            assert!(
                ai.force_current_state_by_name(mv.id),
                "{} is not a forceable move state",
                mv.id
            );
        }
        // UPPER_SNAKE spelling (the wire dialect) resolves to the same table.
        assert_eq!(monster_moves_impl("NIBBIT", 0).unwrap(), moves);
        assert!(monster_moves_impl("NotAMonster", 0).is_err());
    }

    #[test]
    fn monster_moves_carries_the_head_intent() {
        let moves = monster_moves_impl("FuzzyWurmCrawler", 0).unwrap();
        assert!(
            moves.iter().any(|m| m.intent.is_some()),
            "a crawler telegraphs something"
        );
    }

    #[test]
    fn list_encounters_covers_every_act_table() {
        let all = list_encounters_impl();
        assert_eq!(all.len(), sts2_content::encounters::n_encounters());
        let vantom = all
            .iter()
            .find(|e| e.id == "VANTOM_BOSS")
            .expect("act-1 boss listed");
        assert_eq!((vantom.tier, vantom.act), ("Boss", 1));
        assert!(!vantom.monsters.is_empty());
        assert!(all.iter().any(|e| e.act == 3));
        // Ids are globally unique — the dropdown keys on them.
        let mut ids: Vec<&str> = all.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
    }
}

#[cfg(test)]
mod heuristic_baseline_tests {
    use super::*;

    /// The expert heuristic must actually PLAY CARDS, i.e. beat the
    /// pass-every-turn `run_playout` on a fight that is trivially winnable.
    ///
    /// This is the regression that motivated the function: a
    /// search-vs-baseline A/B whose baseline is `rollouts` (EndTurn spam)
    /// measures nothing, because that baseline loses ~100% of every fight
    /// for every character — a denominator of 0 makes "lift" just the
    /// search's own win rate.
    #[test]
    fn heuristic_beats_end_turn_spam_on_a_trivial_fight() {
        // A full IC starter deck vs a single weak monster: winnable by any
        // agent that plays Strikes, unwinnable by one that only passes.
        let toml = r#"
schema = 2
[player]
class = "Ironclad"
hp = 80
max_hp = 80
deck = ["STRIKE_IRONCLAD", "STRIKE_IRONCLAD", "STRIKE_IRONCLAD", "STRIKE_IRONCLAD", "STRIKE_IRONCLAD", "DEFEND_IRONCLAD", "DEFEND_IRONCLAD", "DEFEND_IRONCLAD", "DEFEND_IRONCLAD", "BASH"]
relics = []
[[monsters]]
kind = "NIBBIT"
"#;
        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!("sts2py_heur_base-{pid}-{ts}.toml"));
        std::fs::write(&path, toml).unwrap();
        let base = load_combat_state_from_toml(&path).expect("loads");
        let _ = std::fs::remove_file(&path);
        assert!(
            !core_terminal_value(&base).0,
            "fixture must be a live fight, not an already-terminal state"
        );

        let mut passive_wins = 0;
        let mut heuristic_wins = 0;
        for seed in 0..8u64 {
            let st = base.clone();

            let mut passive = st.clone();
            passive.rng = sts2_core::dotnet_rng::Rng::new(seed as i32);
            if run_playout(&mut passive).0 {
                passive_wins += 1;
            }

            if play_heuristic_to_terminal(&st, seed, 2000).0 {
                heuristic_wins += 1;
            }
        }
        assert_eq!(
            passive_wins, 0,
            "run_playout is EndTurn spam; if it starts winning, it is no longer \
             the do-nothing baseline this test contrasts against"
        );
        assert!(
            heuristic_wins >= 6,
            "expert heuristic won only {heuristic_wins}/8 trivial fights — it is \
             not playing cards, so it cannot serve as an A/B baseline"
        );
    }
}

#[cfg(test)]
mod src_hash_tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    fn canonical(path: &Path) -> PathBuf {
        path.canonicalize()
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn rs_root() -> PathBuf {
        canonical(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
    }

    /// Every crate's build-time stamp hashes the inputs the runtime recompute
    /// reads. If one build script hashed a different set, a fresh wheel would
    /// be reported stale, or a stale one fresh.
    #[test]
    fn src_hash_equals_a_recompute_of_this_checkout() {
        assert_eq!(src_hash(), recompute_src_hash(rs_root().to_str().unwrap()));
    }

    /// `sts2_srchash::ENGINE_CRATES` names every workspace crate this extension
    /// links (the closure of sts2-py's normal path dependencies, minus the
    /// hasher). A crate missing from it is invisible to `src_hash`: a `.so`
    /// built before an edit to it passes every freshness check.
    #[test]
    fn engine_crates_are_what_sts2_rs_links() {
        let out = std::process::Command::new(env!("CARGO"))
            .args([
                "metadata",
                "--format-version",
                "1",
                "--no-deps",
                "--offline",
            ])
            .arg("--manifest-path")
            .arg(rs_root().join("Cargo.toml"))
            .output()
            .expect("run cargo metadata");
        assert!(
            out.status.success(),
            "cargo metadata: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let meta: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        // Package dir -> (name, the dirs of its normal path dependencies).
        let mut packages: BTreeMap<PathBuf, (String, Vec<PathBuf>)> = BTreeMap::new();
        for pkg in meta["packages"].as_array().unwrap() {
            let manifest = Path::new(pkg["manifest_path"].as_str().unwrap());
            let deps = pkg["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|dep| dep["kind"].is_null())
                .filter_map(|dep| dep["path"].as_str())
                .map(|dir| canonical(Path::new(dir)))
                .collect();
            let name = pkg["name"].as_str().unwrap().to_string();
            packages.insert(canonical(manifest.parent().unwrap()), (name, deps));
        }
        let mut linked = BTreeSet::new();
        let mut todo = vec![canonical(Path::new(env!("CARGO_MANIFEST_DIR")))];
        while let Some(dir) = todo.pop() {
            let (name, deps) = packages.get(&dir).unwrap_or_else(|| {
                panic!(
                    "path dependency {} is not a workspace member",
                    dir.display()
                )
            });
            if linked.insert(name.as_str()) {
                todo.extend(deps.iter().cloned());
            }
        }
        linked.remove("sts2-srchash");
        let listed: BTreeSet<&str> = sts2_srchash::ENGINE_CRATES.into_iter().collect();
        assert_eq!(
            linked, listed,
            "update sts2_srchash::ENGINE_CRATES and src_hash()"
        );
    }
}

#[cfg(test)]
mod build_info_tests {
    use super::*;

    /// The test binary is built the way `just sim-engine`, maturin and uv
    /// build the wheel: without `pgo_wheel.sh`'s stamps. Such a build must
    /// never claim PGO, or the trainer's guard would wave it through.
    #[test]
    fn a_default_build_reports_a_plain_portable_build() {
        assert_eq!(
            BuildInfo::current(),
            BuildInfo {
                pgo: false,
                pgo_instrumented: false,
                profdata_sha256: None,
                target_cpu: None,
            }
        );
        Python::with_gil(|py| {
            let d = build_info(py).unwrap();
            let d = d.downcast_bound::<PyDict>(py).unwrap();
            let get = |k: &str| d.get_item(k).unwrap().expect(k);
            assert!(!get("pgo").extract::<bool>().unwrap());
            assert!(!get("pgo_instrumented").extract::<bool>().unwrap());
            assert!(get("profdata_sha256").is_none());
            assert!(get("target_cpu").is_none());
            // The same value the freshness gate compares, so a run's record
            // names the source it trained on.
            assert_eq!(get("src_hash").extract::<String>().unwrap(), src_hash());
        });
    }

    #[test]
    fn only_the_profile_use_stamp_claims_pgo() {
        let sha = Some("ab12");
        let pgo = BuildInfo::from_stamps(sha, None, Some("x86-64-v3"));
        assert!(pgo.pgo && !pgo.pgo_instrumented);
        assert_eq!(pgo.profdata_sha256, sha);
        assert_eq!(pgo.target_cpu, Some("x86-64-v3"));

        // The instrumented build runs the profile workload; it must not pass
        // for a PGO build even if a profdata stamp leaked into its env.
        let instrumented = BuildInfo::from_stamps(sha, Some("1"), None);
        assert!(!instrumented.pgo && instrumented.pgo_instrumented);

        // `VAR= cmd` sets an empty value: that is unset, not a stamp.
        let empty = BuildInfo::from_stamps(Some(""), Some(""), Some(""));
        assert_eq!(empty, BuildInfo::from_stamps(None, None, None));
        assert!(!empty.pgo);
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    // ── search-constant pairing ────────────────────────────────────────

    /// The honest triple travels together: flipping the default mode to
    /// BeliefDraw without carrying `c_puct` / `draw_widening_c` with it would
    /// deploy a MISTUNED honest search, which is worse than either endpoint.
    #[test]
    fn beliefdraw_defaults_carry_the_f2_retune() {
        let (c, dw) =
            resolve_search_constants(sts2_mcts::HiddenInfoMode::BeliefDraw, None, None, 1000);
        assert!((c - sts2_runai::policy::belief_draw_c_puct(1000)).abs() < 1e-6);
        assert!(dw.is_some_and(|v| (v - 2.0).abs() < 1e-6));
        // The constant is read off the BUDGET, not pinned: below 500 sims the
        // curve steps down, which is the whole point of resolving it here.
        let (low, _) =
            resolve_search_constants(sts2_mcts::HiddenInfoMode::BeliefDraw, None, None, 200);
        assert!((low - sts2_runai::policy::belief_draw_c_puct(200)).abs() < 1e-6);
        assert!(low < c, "the curve must step DOWN below 500 sims/tree");
    }

    /// Every other mode keeps the engine defaults byte-identical, and an
    /// explicit value always wins so an ablation gets what it asked for.
    #[test]
    fn other_modes_keep_engine_defaults_and_explicit_wins() {
        let (c, dw) =
            resolve_search_constants(sts2_mcts::HiddenInfoMode::Clairvoyant, None, None, 1000);
        assert!((c - std::f32::consts::SQRT_2).abs() < 1e-6);
        assert!(dw.is_none());
        let (c, dw) = resolve_search_constants(
            sts2_mcts::HiddenInfoMode::BeliefDraw,
            Some(1.25),
            Some(0.5),
            1000,
        );
        assert!((c - 1.25).abs() < 1e-6);
        assert!(dw.is_some_and(|v| (v - 0.5).abs() < 1e-6));
    }

    // ── Persistent-combat-mirror surface ───────────────────────────────

    /// A small Ironclad-vs-Nibbit state built through the same wire path
    /// the mirror uses, so the tests below exercise the real
    /// reconstruction (fresh uids and all) rather than a hand-built
    /// state that could never come off the wire.
    fn mirror_fixture(py: Python<'_>) -> PyCombatState {
        let card = |id: &str, idx: i32| -> Bound<'_, PyDict> {
            let d = PyDict::new(py);
            d.set_item("id", id).unwrap();
            d.set_item("stable_idx", idx).unwrap();
            d.set_item("kind", "card").unwrap();
            d.set_item("upgraded", false).unwrap();
            d
        };
        let hand = PyList::empty(py);
        hand.append(card("STRIKE_IRONCLAD", 1)).unwrap();
        hand.append(card("DEFEND_IRONCLAD", 2)).unwrap();
        let draw = PyList::empty(py);
        draw.append(card("BASH", 3)).unwrap();
        draw.append(card("STRIKE_IRONCLAD", 4)).unwrap();
        draw.append(card("DEFEND_IRONCLAD", 5)).unwrap();

        let deck = PyDict::new(py);
        deck.set_item("draw_pile", draw).unwrap();
        deck.set_item("discard_pile", PyList::empty(py)).unwrap();
        deck.set_item("hand", hand).unwrap();
        deck.set_item("exhaust_pile", PyList::empty(py)).unwrap();

        let player = PyDict::new(py);
        player.set_item("class", "ironclad").unwrap();
        player.set_item("hp", 60).unwrap();
        player.set_item("max_hp", 80).unwrap();
        player.set_item("block", 0).unwrap();
        player.set_item("energy", 3).unwrap();
        player.set_item("max_energy", 3).unwrap();
        player.set_item("energy_regen_per_turn", 3).unwrap();
        player.set_item("relics", PyList::empty(py)).unwrap();
        player.set_item("powers", PyList::empty(py)).unwrap();
        player.set_item("potions", PyList::empty(py)).unwrap();

        let nibbit = PyDict::new(py);
        nibbit.set_item("kind", "Nibbit").unwrap();
        nibbit.set_item("uid", 1_i64).unwrap();
        nibbit.set_item("hp", 40).unwrap();
        nibbit.set_item("max_hp", 48).unwrap();
        nibbit.set_item("block", 0).unwrap();
        nibbit.set_item("powers", PyList::empty(py)).unwrap();
        nibbit.set_item("intents", PyList::empty(py)).unwrap();
        nibbit
            .set_item("intent_history", PyList::empty(py))
            .unwrap();
        nibbit.set_item("is_dead", false).unwrap();
        nibbit.set_item("is_escaped", false).unwrap();
        let enc = PyList::empty(py);
        enc.append(nibbit).unwrap();

        let d = PyDict::new(py);
        d.set_item("wire_version", 1).unwrap();
        d.set_item("phase", "PlayerPlayPhase").unwrap();
        d.set_item("round", 1).unwrap();
        d.set_item("side", "Player").unwrap();
        d.set_item("is_elite", false).unwrap();
        d.set_item("turn", 1).unwrap();
        d.set_item("combat_id", 0_i64).unwrap();
        d.set_item("player", player).unwrap();
        d.set_item("deck", deck).unwrap();
        d.set_item("encounter", enc).unwrap();

        state_from_dict_py(&d).expect("fixture must parse")
    }

    fn option_dict<'py>(
        py: Python<'py>,
        id: &str,
        pile: Option<(&str, usize)>,
    ) -> Bound<'py, PyDict> {
        let d = PyDict::new(py);
        d.set_item("id", id).unwrap();
        d.set_item("upgraded", false).unwrap();
        if let Some((p, i)) = pile {
            d.set_item("pile", p).unwrap();
            d.set_item("index", i).unwrap();
        }
        d
    }

    /// `transplant_hidden` carries exactly the unobservable fields —
    /// A relic's `counter` is also where a one-shot per-combat latch lives,
    /// GH #370, the rest of the class. Each of these has a live content
    /// consumer that reads it as a gate, and no producer emits any of them.
    #[test]
    fn transplant_hidden_carries_the_remaining_gating_accumulators() {
        Python::with_gil(|py| {
            let mut carried = mirror_fixture(py);
            carried.inner.player.shivs_played_this_turn = 2;
            carried.inner.player.generated_cards_this_combat = 4;
            carried.inner.player.stars_gained_this_turn = 3;

            let mut fresh = mirror_fixture(py);
            fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect("shapes agree");

            assert_eq!(
                fresh.inner.player.shivs_played_this_turn, 2,
                "PhantomBlades gates its bonus on this being 0; re-zeroed, \
                 every Shiv of the turn reads as the first one"
            );
            assert_eq!(
                fresh.inner.player.generated_cards_this_combat, 4,
                "Supermassive scales its damage off this count"
            );
            assert_eq!(
                fresh.inner.player.stars_gained_this_turn, 3,
                "Radiate scales off this count"
            );
        });
    }

    /// GH #370. Ringing gates card legality on "has any play started this
    /// turn", and C# stores that nowhere — `RingingPower.ShouldPlay` scans
    /// `History.CardPlaysStarted`. With no wire carrier the mirror's own
    /// accumulation is the only source, so the transplant must carry it or a
    /// Ringing-afflicted hand reads playable all turn.
    #[test]
    fn transplant_hidden_carries_the_started_play_count_when_no_sloth_discloses_it() {
        Python::with_gil(|py| {
            let mut carried = mirror_fixture(py);
            carried.inner.player.card_plays_started_this_turn = 3;
            let mut fresh = mirror_fixture(py);
            fresh.inner.player.card_plays_started_this_turn = 0;
            fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect("shapes agree");
            assert_eq!(
                fresh.inner.player.card_plays_started_this_turn, 3,
                "no producer emits this count without a Sloth on the board; \
                 dropping it leaves Ringing's gate stuck open for the turn"
            );
        });
    }

    /// ...but when a Sloth IS up, `state_from_dict` has already seeded the
    /// count from `DisplayAmount`, and that reading is the world's. Carrying
    /// the mirror's own number over it would re-introduce exactly the drift
    /// the wire is there to correct.
    #[test]
    fn transplant_hidden_defers_to_a_sloth_disclosed_started_play_count() {
        Python::with_gil(|py| {
            let mut carried = mirror_fixture(py);
            carried.inner.player.card_plays_started_this_turn = 3;

            let mut fresh = mirror_fixture(py);
            let pid = std::num::NonZeroU32::new(77).unwrap();
            fresh.inner.powers.push(sts2_core::Power {
                id: pid,
                kind: sts2_core::powers::PowerKind::Sloth,
                amount: 2,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            fresh.inner.player.powers.push(sts2_core::PowerUid(pid));
            // What `state_from_dict` would have written from `display`.
            fresh.inner.player.card_plays_started_this_turn = 1;

            fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect("shapes agree");
            assert_eq!(
                fresh.inner.player.card_plays_started_this_turn, 1,
                "`SlothPower.DisplayAmount` is authoritative; the carry is a \
                 fallback for the no-Sloth case, not an override"
            );
        });
    }

    /// Move the whole hand (STRIKE, then DEFEND) into the play pile — the
    /// shape a mirror has mid-play, which is the only time the Vigor latch is
    /// live. Two cards with DIFFERENT ids, so a position slip is detectable.
    fn move_hand_into_play(s: &mut PyCombatState) {
        while !s.inner.player.hand.is_empty() {
            let uid = s.inner.player.hand.remove(0);
            s.inner.player.play.push(uid);
        }
    }

    /// GH #370. C# `VigorPower.BeforeAttack` latches the first powered attack
    /// so only THAT card's later commands keep the additive boost. Dropped
    /// across a rebuild, a second command in the same play re-takes it.
    #[test]
    fn transplant_hidden_translates_the_vigor_latch_by_play_pile_position() {
        Python::with_gil(|py| {
            let mut carried = mirror_fixture(py);
            let mut fresh = mirror_fixture(py);
            move_hand_into_play(&mut carried);
            move_hand_into_play(&mut fresh);
            // The SECOND card, so the assertion cannot pass on a hardcoded
            // head-of-pile.
            carried.inner.player.vigor_latch_card = Some(carried.inner.player.play[1]);

            fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect("shapes agree");

            assert_eq!(
                fresh.inner.player.vigor_latch_card,
                Some(fresh.inner.player.play[1]),
                "the latch names a card in the play pile, which the wire ships \
                 in order — so it translates by position like the draw pins"
            );
        });
    }

    /// A latch pointing at the wrong card is worse than no latch: it would
    /// exempt the real Vigor card and boost an unrelated one.
    #[test]
    fn transplant_hidden_drops_a_vigor_latch_the_play_pile_cannot_place() {
        Python::with_gil(|py| {
            let mut carried = mirror_fixture(py);
            move_hand_into_play(&mut carried);
            carried.inner.player.vigor_latch_card = Some(carried.inner.player.play[0]);

            // The play pile emptied between decisions (the play resolved).
            let mut fresh = mirror_fixture(py);
            fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect("shapes agree");

            assert_eq!(
                fresh.inner.player.vigor_latch_card, None,
                "the piles disagree, so the position is untranslatable"
            );
        });
    }

    /// Equal lengths are not enough on a pile this short — the card at the
    /// translated index must also BE the same card.
    #[test]
    fn transplant_hidden_drops_a_vigor_latch_whose_position_changed_hands() {
        Python::with_gil(|py| {
            let mut carried = mirror_fixture(py);
            move_hand_into_play(&mut carried);
            carried.inner.player.vigor_latch_card = Some(carried.inner.player.play[0]);

            // Same length, same cards — reordered. Position 0 now names
            // DEFEND where the carried mirror latched STRIKE.
            let mut fresh = mirror_fixture(py);
            move_hand_into_play(&mut fresh);
            fresh.inner.player.play.swap(0, 1);

            fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect("shapes agree");

            assert_eq!(
                fresh.inner.player.vigor_latch_card, None,
                "translating onto a different card would exempt the real Vigor \
                 card and boost an unrelated one"
            );
        });
    }

    /// and the producers only disclose a counter for relics that SHOW one.
    /// An undisclosed latch must survive the reconstruction, or Vambrace
    /// re-arms every decision and doubles a second card's block.
    #[test]
    fn transplant_hidden_carries_an_undisclosed_relic_latch() {
        Python::with_gil(|py| {
            let push = |s: &mut PyCombatState, counter: i32| {
                s.inner.relics.push(sts2_core::Relic {
                    id: std::num::NonZeroU32::new(100).unwrap(),
                    kind: sts2_core::RelicKind::Vambrace,
                    counter,
                    is_wax: false,
                    is_melted: false,
                });
            };

            // Named: the carried mirror's "already used" latch wins.
            let mut carried = mirror_fixture(py);
            push(&mut carried, 1);
            let mut fresh = mirror_fixture(py);
            push(&mut fresh, 0);
            fresh
                .transplant_hidden(&carried, None, true, Some(vec!["VAMBRACE".into()]), None)
                .expect("shapes agree");
            assert_eq!(
                fresh.inner.relics[0].counter, 1,
                "an undisclosed latch must survive the reconstruction"
            );

            // Not named — the wire DID disclose it, so reality keeps its value.
            let mut fresh = mirror_fixture(py);
            push(&mut fresh, 0);
            fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect("shapes agree");
            assert_eq!(
                fresh.inner.relics[0].counter, 0,
                "a disclosed counter is reality's, not the mirror's"
            );
        });
    }

    /// The monster move machine keeps per-instance history no wire carries —
    /// `counter`, the `performed_*` flags, `state_log`, a pending
    /// `SetMoveImmediate` — and every one of them feeds the NEXT transition.
    /// A reconstruction starts them blank, so the mirror telegraphs a
    /// different move than the game from an identical board. Measured on the
    /// oracle with `cursor_drifts == 0`, i.e. with randomness ruled out:
    /// TwigSlimeM `Attack 11` vs `StatusCard`, Wriggler `Attack 6` vs `Buff`.
    ///
    /// `current` is the one field the wire DOES carry (`move_state`), so it
    /// must NOT be carried — reality wins on it like every other observable.
    #[test]
    fn transplant_hidden_carries_the_move_machines_instance_state() {
        Python::with_gil(|py| {
            let mut carried = mirror_fixture(py);
            let mut fresh = mirror_fixture(py);
            let Some(src) = carried.inner.creatures[0].ai.as_mut() else {
                return; // fixture monster has no AI; nothing to assert
            };
            src.set_counter(7);
            let carried_current = src.current_state_id();

            let fresh_current = fresh.inner.creatures[0]
                .ai
                .as_ref()
                .map(sts2_core::MonsterAi::current_state_id);
            fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect("shapes agree");

            let ai = fresh.inner.creatures[0]
                .ai
                .as_ref()
                .expect("ai still present");
            assert_eq!(
                ai.counter(),
                7,
                "the move machine's per-instance counter must survive the rebuild"
            );
            assert_eq!(
                Some(ai.current_state_id()),
                fresh_current,
                "`current` rides the wire as `move_state`, so the reconstruction keeps its own"
            );
            let _ = carried_current;
        });
    }

    /// A name the engine cannot resolve means producer and engine disagree
    /// about the relic vocabulary; silently skipping it would leave the latch
    /// re-armed while the report claimed it had been carried.
    #[test]
    fn transplant_hidden_refuses_an_unknown_relic_name() {
        Python::with_gil(|py| {
            let carried = mirror_fixture(py);
            let mut fresh = mirror_fixture(py);
            let err = fresh
                .transplant_hidden(&carried, None, true, Some(vec!["VAMBRAAACE".into()]), None)
                .expect_err("an unknown relic must be refused");
            assert!(
                err.to_string().contains("VAMBRAAACE"),
                "the error must name the relic, got: {err}"
            );
        });
    }

    /// Rampage's escalation is wire-owned (C# grows `Damage.BaseValue`
    /// itself, so the reconstructed card's `base_damage` already includes
    /// it) and must NOT be re-injected — the double-count had MCTS planning
    /// 37 damage where the game dealt 23 (v34wr seed 1020). The same var on
    /// any OTHER card is Momentum's stash, which never crosses the wire and
    /// must keep being carried.
    #[test]
    fn transplant_hidden_skips_rampage_var_on_rampage_only() {
        use sts2_core::cards::{CardId, VarKey};
        Python::with_gil(|py| {
            let mut carried = mirror_fixture(py);
            let rampage_uid = carried.inner.player.draw[0];
            let momentum_uid = carried.inner.player.draw[1];
            let ridx = rampage_uid.0.get() as usize - 1;
            let midx = momentum_uid.0.get() as usize - 1;
            carried.inner.cards[ridx].card_id = CardId::Rampage;
            carried.inner.cards[ridx].set_combat_var(VarKey::RampageDamage, 14);
            carried.inner.cards[midx].set_combat_var(VarKey::RampageDamage, 6);

            let mut fresh = mirror_fixture(py);
            let f_rampage = fresh.inner.player.draw[0].0.get() as usize - 1;
            let f_momentum = fresh.inner.player.draw[1].0.get() as usize - 1;
            fresh.inner.cards[f_rampage].card_id = CardId::Rampage;

            fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect("shapes agree");

            assert_eq!(
                fresh.inner.cards[f_rampage].combat_var(VarKey::RampageDamage),
                0,
                "Rampage's own growth is baked into the wire base_damage; \
                 carrying the var double-counts"
            );
            assert_eq!(
                fresh.inner.cards[f_momentum].combat_var(VarKey::RampageDamage),
                6,
                "Momentum's stash on a non-Rampage card never crosses the \
                 wire and must survive the rebuild"
            );
        });
    }

    /// RNG cursors, intent history, pins — and translates the pins by
    /// POSITION, because the reconstruction reallocates every uid.
    #[test]
    fn transplant_hidden_carries_pins_by_position() {
        Python::with_gil(|py| {
            let mut carried = mirror_fixture(py);
            // Pin the Bash on top and the last Defend to the bottom, then
            // burn some RNG so the cursors are visibly non-default.
            let bash = carried.inner.player.draw[0];
            let defend = carried.inner.player.draw[2];
            sts2_core::cards::pin_top(&mut carried.inner.player, bash);
            sts2_core::cards::pin_bottom(&mut carried.inner.player, defend);
            for _ in 0..5 {
                let _ = carried.inner.run_rng.shuffle.next_int(0, 100);
            }
            let carried_cursor = carried.inner.run_rng.shuffle.counter();
            assert_ne!(carried_cursor, 0, "fixture must move the cursor");

            let mut fresh = mirror_fixture(py);
            assert!(fresh.inner.player.pinned_top.is_empty());
            // Distinct arenas: the uids cannot be carried verbatim.
            assert_ne!(
                fresh.inner.player.draw[0].get(),
                0,
                "sanity: fresh state has real uids"
            );

            fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect("shapes agree");

            let k = fresh.draw_pile_knowledge(py).unwrap();
            let top: Vec<usize> = k
                .get_item("pinned_top")
                .unwrap()
                .unwrap()
                .extract()
                .unwrap();
            let bottom: Vec<usize> = k
                .get_item("pinned_bottom")
                .unwrap()
                .unwrap()
                .extract()
                .unwrap();
            assert_eq!(top, vec![0], "the pinned top card keeps its position");
            assert_eq!(bottom, vec![2]);
            assert_eq!(
                fresh.shuffle_stream_counter(),
                carried_cursor,
                "RNG cursors must survive the transplant"
            );
        });
    }

    /// A differently-sized draw pile drops the PINS and keeps everything
    /// else.
    ///
    /// Pins are translated by draw-pile position, so a size mismatch makes
    /// them untranslatable — but that is the ordinary consequence of a
    /// reshuffle between decisions, not a reason to throw away the RNG
    /// cursors too. The pile difference itself is reported by the mirror's
    /// own observable diff, which is where it belongs.
    #[test]
    fn transplant_hidden_drops_untranslatable_pins() {
        Python::with_gil(|py| {
            let mut carried = mirror_fixture(py);
            let top = carried.inner.player.draw[0];
            sts2_core::cards::pin_top(&mut carried.inner.player, top);
            for _ in 0..5 {
                let _ = carried.inner.run_rng.shuffle.next_int(0, 100);
            }
            let cursor = carried.inner.run_rng.shuffle.counter();

            let mut fresh = mirror_fixture(py);
            fresh.inner.player.draw.pop();
            fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect("a shorter draw pile must not fail the transplant");

            assert!(
                fresh.inner.player.pinned_top.is_empty(),
                "pins into a pile that no longer exists must be dropped"
            );
            assert_eq!(
                fresh.inner.run_rng.shuffle.counter(),
                cursor,
                "the RNG cursors are still valid and must survive"
            );
        });
    }

    /// Without an alignment, a roster mismatch is refused — the fallback is
    /// slot-for-slot, and slots only line up when both sides agree.
    #[test]
    fn transplant_hidden_refuses_an_unaligned_roster() {
        Python::with_gil(|py| {
            let carried = mirror_fixture(py);
            let mut fresh = mirror_fixture(py);
            fresh.inner.enemy_uids.pop();

            let err = fresh
                .transplant_hidden(&carried, None, true, None, None)
                .expect_err("an unaligned roster must be refused");
            assert!(
                err.to_string().contains("enemy roster disagrees"),
                "error must name the mismatch, got: {err}"
            );
        });
    }

    /// With an alignment, the same mismatch transplants cleanly: each of this
    /// state's enemies names the carried slot holding its history, and a
    /// `None` entry is a newly-spawned enemy with none to carry.
    #[test]
    fn transplant_hidden_honors_an_explicit_alignment() {
        Python::with_gil(|py| {
            let carried = mirror_fixture(py);
            let mut fresh = mirror_fixture(py);
            fresh.inner.enemy_uids.pop();
            fresh
                .transplant_hidden(&carried, Some(vec![]), true, None, None)
                .expect("an explicit alignment must be honored");

            let mut newly_spawned = mirror_fixture(py);
            newly_spawned
                .transplant_hidden(&carried, Some(vec![None]), true, None, None)
                .expect("a `None` slot is a newly-spawned enemy, not an error");
        });
    }

    /// `open_injected_choice` opens exactly the options it was given, and
    /// resolves the pile locators to concrete cards.
    #[test]
    fn open_injected_choice_uses_the_supplied_options() {
        Python::with_gil(|py| {
            let mut cs = mirror_fixture(py);
            let opts = PyList::empty(py);
            opts.append(option_dict(py, "STRIKE_IRONCLAD", Some(("hand", 0))))
                .unwrap();
            opts.append(option_dict(py, "DEFEND_IRONCLAD", Some(("hand", 1))))
                .unwrap();

            cs.open_injected_choice("make_free_in_hand", &opts, 1, 1, false, false)
                .expect("choice opens");

            let pc = cs.inner.pending_choice.as_ref().expect("choice is open");
            assert_eq!(pc.kind, ChoiceKind::MakeFreeInHand);
            assert_eq!(pc.min_select, 1);
            assert_eq!(pc.max_select, 1);
            assert_eq!(pc.options.len(), 2);
            assert_eq!(pc.options[0].card_uid, Some(cs.inner.player.hand[0]));
            assert_eq!(pc.options[1].card_uid, Some(cs.inner.player.hand[1]));

            // And it is answerable through the normal action surface.
            let action = PyDict::new(py);
            action.set_item("action", "choose").unwrap();
            action.set_item("index", 0_u8).unwrap();
            cs.apply_step(action.as_any()).expect("choice resolves");
            assert!(
                cs.inner.pending_choice.is_none(),
                "resolving must close the choice"
            );
        });
    }

    /// `pending_choice()` is the exact inverse of `open_injected_choice`:
    /// feeding its own option dicts straight back re-opens the identical
    /// choice.
    ///
    /// This is what lets a driver hold a live option list to a concrete
    /// board POSITION rather than a card name — names repeat (two Strikes),
    /// positions don't — and it is how the oracle test injects without a
    /// name-matching heuristic standing between the engine and itself.
    #[test]
    fn pending_choice_locators_round_trip_through_injection() {
        Python::with_gil(|py| {
            let mut cs = mirror_fixture(py);
            let opts = PyList::empty(py);
            // The draw pile holds two distinct cards plus a duplicate of a
            // hand card, so a name-only round-trip would be ambiguous here.
            opts.append(option_dict(py, "BASH", Some(("draw", 0))))
                .unwrap();
            opts.append(option_dict(py, "STRIKE_IRONCLAD", Some(("draw", 1))))
                .unwrap();
            cs.open_injected_choice("put_back", &opts, 1, 1, true, false)
                .expect("choice opens");

            let emitted = cs
                .pending_choice(py)
                .expect("pending_choice succeeds")
                .expect("a choice is open");
            let cards: Vec<Bound<'_, PyDict>> = emitted
                .get_item("cards")
                .unwrap()
                .unwrap()
                .downcast::<PyList>()
                .unwrap()
                .iter()
                .map(|c| c.downcast::<PyDict>().unwrap().clone())
                .collect();
            let locator = |d: &Bound<'_, PyDict>| -> (String, usize) {
                (
                    d.get_item("pile").unwrap().unwrap().extract().unwrap(),
                    d.get_item("index").unwrap().unwrap().extract().unwrap(),
                )
            };
            assert_eq!(locator(&cards[0]), ("draw".to_owned(), 0));
            assert_eq!(locator(&cards[1]), ("draw".to_owned(), 1));

            // Feed the emitted dicts back verbatim onto a twin state.
            let round_tripped = PyList::empty(py);
            for c in &cards {
                round_tripped.append(c).unwrap();
            }
            let mut twin = mirror_fixture(py);
            twin.open_injected_choice("put_back", &round_tripped, 1, 1, true, false)
                .expect("emitted options re-open unchanged");
            let a = cs.inner.pending_choice.as_ref().unwrap();
            let b = twin.inner.pending_choice.as_ref().unwrap();
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.options.len(), b.options.len());
            for (x, y) in a.options.iter().zip(&b.options) {
                assert_eq!(x.card_id, y.card_id);
                assert_eq!(x.upgraded, y.upgraded);
                assert_eq!(x.source_pile, y.source_pile);
            }
        });
    }

    /// A synthetic option — no `card_uid`, the shape a generative kind
    /// produces — emits no locator, so a driver can tell "rolled content"
    /// from "a card on the board" without guessing.
    #[test]
    fn synthetic_options_emit_no_locator() {
        Python::with_gil(|py| {
            let mut cs = mirror_fixture(py);
            let opts = PyList::empty(py);
            opts.append(option_dict(py, "STRIKE_IRONCLAD", None))
                .unwrap();
            cs.open_injected_choice("add_generated_to_hand", &opts, 1, 1, false, false)
                .expect("choice opens");

            let emitted = cs.pending_choice(py).unwrap().unwrap();
            let card = emitted
                .get_item("cards")
                .unwrap()
                .unwrap()
                .downcast::<PyList>()
                .unwrap()
                .get_item(0)
                .unwrap();
            let card = card.downcast::<PyDict>().unwrap();
            assert!(card.get_item("pile").unwrap().is_none());
            assert!(card.get_item("index").unwrap().is_none());
        });
    }

    /// `replace_parked` is the only way to re-open over a live choice.
    ///
    /// Every other test in this file passes `false`, so without this the flag's
    /// contract is entirely unasserted: refuse by default (a driver that did
    /// not mean to replace a live choice is a bug), swap the list when the
    /// caller says so.
    ///
    /// The other half of the contract — that clearing the choice leaves
    /// `pending_play` intact, which is the whole reason the flag exists — is
    /// asserted in `test_combat_mirror.py`'s
    /// `test_replacing_a_parked_choice_keeps_the_suspended_card`, where a real
    /// Armaments play produces a real suspended tail. `CardPlayFinish`'s fields
    /// are private, so there is no honest way to stage one here.
    #[test]
    fn replace_parked_is_required_to_re_open_over_a_live_choice() {
        Python::with_gil(|py| {
            let mut cs = mirror_fixture(py);
            let one = |id: &str, at: usize| {
                let l = PyList::empty(py);
                l.append(option_dict(py, id, Some(("hand", at)))).unwrap();
                l
            };

            cs.open_injected_choice(
                "make_free_in_hand",
                &one("STRIKE_IRONCLAD", 0),
                1,
                1,
                false,
                false,
            )
            .expect("the mirror's own derivation opens");

            assert!(
                cs.open_injected_choice(
                    "make_free_in_hand",
                    &one("DEFEND_IRONCLAD", 1),
                    1,
                    1,
                    false,
                    false
                )
                .is_err(),
                "a choice is already open — replacing it has to be asked for"
            );

            cs.open_injected_choice(
                "make_free_in_hand",
                &one("DEFEND_IRONCLAD", 1),
                1,
                1,
                false,
                true,
            )
            .expect("replace_parked swaps the list in");

            let pc = cs
                .inner
                .pending_choice
                .as_ref()
                .expect("the world's screen is open");
            assert_eq!(pc.options.len(), 1);
            assert_eq!(
                pc.options[0].card_uid,
                Some(cs.inner.player.hand[1]),
                "the world's option won, not the mirror's"
            );
        });
    }

    /// Every way of getting an injected choice wrong is a driver bug and
    /// raises. None of these are game states the mirror should tolerate.
    #[test]
    fn open_injected_choice_rejects_malformed_input() {
        Python::with_gil(|py| {
            let mut cs = mirror_fixture(py);
            let good = |pile: Option<(&str, usize)>| {
                let l = PyList::empty(py);
                l.append(option_dict(py, "STRIKE_IRONCLAD", pile)).unwrap();
                l
            };

            let cases: Vec<(&str, PyErr)> = vec![
                (
                    "unknown choice kind",
                    cs.open_injected_choice(
                        "no_such_kind",
                        &good(Some(("hand", 0))),
                        1,
                        1,
                        false,
                        false,
                    )
                    .unwrap_err(),
                ),
                (
                    "out of range",
                    cs.open_injected_choice(
                        "make_free_in_hand",
                        &good(Some(("hand", 9))),
                        1,
                        1,
                        false,
                        false,
                    )
                    .unwrap_err(),
                ),
                (
                    "min_select",
                    cs.open_injected_choice("make_free_in_hand", &good(None), 2, 1, false, false)
                        .unwrap_err(),
                ),
                (
                    "exceeds",
                    cs.open_injected_choice("make_free_in_hand", &good(None), 2, 2, false, false)
                        .unwrap_err(),
                ),
            ];
            for (needle, err) in cases {
                let msg = err.to_string();
                assert!(
                    msg.contains(needle),
                    "expected an error mentioning {needle:?}, got: {msg}"
                );
            }

            // A locator that names the wrong card: hand[1] is a Defend.
            let liar = PyList::empty(py);
            liar.append(option_dict(py, "STRIKE_IRONCLAD", Some(("hand", 1))))
                .unwrap();
            let err = cs
                .open_injected_choice("make_free_in_hand", &liar, 1, 1, false, false)
                .unwrap_err();
            assert!(
                err.to_string().contains("but the mirror has"),
                "a mislocated option must report both sides, got: {err}"
            );

            // Half a locator is worse than none — it would silently make a
            // synthetic option a pile resolver cannot use.
            let half = PyList::empty(py);
            let d = PyDict::new(py);
            d.set_item("id", "STRIKE_IRONCLAD").unwrap();
            d.set_item("pile", "hand").unwrap();
            half.append(d).unwrap();
            let err = cs
                .open_injected_choice("make_free_in_hand", &half, 1, 1, false, false)
                .unwrap_err();
            assert!(
                err.to_string().contains("needs both"),
                "a half-specified locator must be refused, got: {err}"
            );

            // Opening over an already-open choice is a lost-track bug.
            cs.open_injected_choice(
                "make_free_in_hand",
                &good(Some(("hand", 0))),
                1,
                1,
                false,
                false,
            )
            .expect("first open succeeds");
            let err = cs
                .open_injected_choice(
                    "make_free_in_hand",
                    &good(Some(("hand", 0))),
                    1,
                    1,
                    false,
                    false,
                )
                .unwrap_err();
            assert!(err.to_string().contains("already open"));
        });
    }

    /// The generative/deterministic split, and that an unknown kind is an
    /// error rather than a silent default in either direction.
    #[test]
    fn choice_kind_generativity_is_explicit() {
        assert!(choice_kind_is_generative("add_generated_to_hand").unwrap());
        assert!(choice_kind_is_generative("add_generated_free_to_hand").unwrap());
        assert!(choice_kind_is_generative("add_generated_retained_to_hand").unwrap());
        for deterministic in [
            "put_back",
            "make_free_in_hand",
            "exhaust_from_hand",
            "discard_from_hand",
            "card_select",
        ] {
            assert!(
                !choice_kind_is_generative(deterministic).unwrap(),
                "{deterministic} draws its options from the player's own piles"
            );
        }
        assert!(choice_kind_is_generative("not_a_kind").is_err());
    }

    /// The string table round-trips in both directions for every variant,
    /// so `open_injected_choice` can name any choice the engine can open.
    #[test]
    fn choice_kind_names_round_trip() {
        for k in ChoiceKind::ALL.iter().copied() {
            let name = choice_kind_python_name(k);
            assert_eq!(
                choice_kind_from_python_name(name),
                Some(k),
                "{name:?} must decode back to {k:?}"
            );
        }
    }

    /// GH #303: the back-compat `"potion"` verb must decode to the
    /// canonical `DrinkPotion` variant. It used to decode to
    /// `Action::UsePotion`, whose apply arm was a silent no-op — the
    /// drink vanished and the caller was told it landed.
    #[test]
    fn potion_verb_decodes_to_drink_potion() {
        Python::with_gil(|py| {
            for verb in ["potion", "drink_potion"] {
                let d = PyDict::new(py);
                d.set_item("action", verb).unwrap();
                d.set_item("slot", 1_u8).unwrap();
                d.set_item("target_index", 2_u8).unwrap();
                assert_eq!(
                    parse_bridge_action(&d).expect("verb decodes"),
                    Action::DrinkPotion {
                        potion_slot: 1,
                        target: Some(2),
                    },
                    "'{verb}' must decode to the canonical variant",
                );
            }
        });
    }

    /// GH #303 follow-up: the bridge's potion-DISCARD dict must not
    /// decode to a drink. Same verb, same `slot` key, plus
    /// `discard: true` (`sts2_realgame.discard_potion`). Before the
    /// alias landed this dict was harmlessly swallowed by the
    /// `UsePotion` no-op; decoding it as `DrinkPotion` would now fire
    /// the potion's effect the world never fired.
    #[test]
    fn potion_discard_is_rejected_not_silently_drunk() {
        Python::with_gil(|py| {
            for verb in ["potion", "drink_potion"] {
                let d = PyDict::new(py);
                d.set_item("action", verb).unwrap();
                d.set_item("slot", 0_u8).unwrap();
                d.set_item("discard", true).unwrap();
                let err = parse_bridge_action(&d)
                    .expect_err("'{verb}' + discard must not decode to a drink");
                assert!(
                    err.to_string().contains("discard"),
                    "error should name the offending key, got: {err}",
                );
            }

            // `discard: false` is the plain drink the bridge sends when
            // it bothers to spell the flag out — must still work.
            let d = PyDict::new(py);
            d.set_item("action", "potion").unwrap();
            d.set_item("slot", 0_u8).unwrap();
            d.set_item("discard", false).unwrap();
            assert_eq!(
                parse_bridge_action(&d).expect("discard=false is a normal drink"),
                Action::DrinkPotion {
                    potion_slot: 0,
                    target: None,
                },
            );
        });
    }

    #[test]
    fn combat_state_from_scenario_toml_loads_a_fixture() {
        let toml = r#"
schema = 2
[player]
class = "Ironclad"
hp = 50
max_hp = 80
deck = ["STRIKE_IRONCLAD", "DEFEND_IRONCLAD", "BASH"]
relics = ["BURNING_BLOOD"]
[[monsters]]
kind = "VANTOM"
"#;
        // Unique per-process + per-nanosecond suffix so parallel
        // worktree agents don't race on the same /tmp path (mirrors
        // `toml_file.rs`'s `write_tmp` helper).
        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!("sts2py_fixture_test-{pid}-{ts}.toml"));
        std::fs::write(&path, toml).unwrap();
        let inner = load_combat_state_from_toml(&path).expect("loads");
        // Vantom is 173 HP at Asc-0; the player-Creature is the 80-max-HP
        // Ironclad, so 173 uniquely identifies the loaded Vantom.
        assert!(inner.creatures.iter().any(|c| c.max_hp == 173));
    }

    #[test]
    fn scenario_builder_populates_card_tags_and_character() {
        // q6bix coverage-sweep root fix: the scenario builder must mirror
        // `card_from_spec` and set per-card `CardTags` plus the player
        // `character`. Without tags, tag-gated cards diverge from C# (FASTEN
        // only boosts block from `CardTag::Defend` sources → cs=10 rust=5).
        // Without `character`, character-card generators no-op (OrobicAcid
        // early-returns on `character: None` → hand cs=5 rust=2).
        let state = build_scenario_state_impl(
            0,
            "Ironclad",
            &["DEFEND_IRONCLAD".to_string()],
            70,
            80,
            &[],
            &[],
            "SLITHERING_STRANGLER_NORMAL",
            0,
            None,
            None,
        )
        .expect("scenario builds");
        assert_eq!(
            state.player.character,
            Some(sts2_core::CharacterId::Ironclad),
            "scenario builder must set the player character"
        );
        let spec = sts2_content::names::resolve_card_name("DEFEND_IRONCLAD", None)
            .expect("defend resolves");
        let defend = state
            .cards
            .iter()
            .find(|c| c.card_id == spec.id)
            .expect("defend present in arena");
        assert!(
            defend.has_tag(sts2_core::cards::CardTag::Defend),
            "scenario Defend must carry its canonical Defend tag (mirrors card_from_spec)"
        );
    }

    #[test]
    fn build_scenario_force_matches_scroll_starter_rotation() {
        // sts2-rs-u3wjj: forwarding C#'s observed initial move-states must
        // override the static SCROLLS_OF_BITING_WEAK starter slice (idx
        // 0,1,2 → CHOMP,CHEW,MORE_TEETH) so each Rust scroll opens on the
        // same move C# rolled. Here C# rolled the rotation
        // [MORE_TEETH, CHOMP, CHEW] (StarterMoveIdx 2 → +1 → +2).
        let move_states = vec![
            Some("MORE_TEETH".to_string()),
            Some("CHOMP".to_string()),
            Some("CHEW".to_string()),
        ];
        let state = build_scenario_state_impl(
            7,
            "Ironclad",
            &["STRIKE_IRONCLAD".to_string()],
            70,
            80,
            &[],
            &[],
            "SCROLLS_OF_BITING_WEAK",
            0,
            None,
            Some(&move_states),
        )
        .expect("scenario builds");
        let opener: Vec<&str> = state
            .creatures
            .iter()
            .filter(|c| c.species == Some(MonsterKind::ScrollOfBiting))
            .map(|c| {
                c.ai.as_ref()
                    .expect("scroll carries an AI")
                    .current_state_id()
            })
            .collect();
        assert_eq!(
            opener,
            vec!["MORE_TEETH", "CHOMP", "CHEW"],
            "each scroll must open on the move C# telegraphed"
        );

        // Control: without the override, the static encounter-table slice
        // (idx 0,1,2) gives the default CHOMP/CHEW/MORE_TEETH order.
        let default_state = build_scenario_state_impl(
            7,
            "Ironclad",
            &["STRIKE_IRONCLAD".to_string()],
            70,
            80,
            &[],
            &[],
            "SCROLLS_OF_BITING_WEAK",
            0,
            None,
            None,
        )
        .expect("scenario builds");
        let default_opener: Vec<&str> = default_state
            .creatures
            .iter()
            .filter(|c| c.species == Some(MonsterKind::ScrollOfBiting))
            .map(|c| c.ai.as_ref().expect("scroll AI").current_state_id())
            .collect();
        assert_eq!(default_opener, vec!["CHOMP", "CHEW", "MORE_TEETH"]);
    }

    #[test]
    fn build_seeded_state_is_deterministic() {
        // Same seed → equal states (RNG seeded identically, all other fields
        // come from `Default`).
        let a = build_seeded_state(42, 0);
        let b = build_seeded_state(42, 0);
        assert_eq!(a, b);
    }

    #[test]
    fn choice_kind_strings_cover_all_variants() {
        // Bead sts2-rs-ck2: every ChoiceKind must map to a unique
        // bridge string. If a new variant is added to the core enum
        // without updating the mapping, this is the canary.
        let names: Vec<&'static str> = [
            ChoiceKind::CardSelect,
            ChoiceKind::PutBack,
            ChoiceKind::ExhaustPile,
            ChoiceKind::OrbSlot,
            ChoiceKind::EnemyTarget,
            ChoiceKind::AddGeneratedToHand,
            ChoiceKind::AddGeneratedRetainedToHand,
            ChoiceKind::PaelsToothStash,
        ]
        .into_iter()
        .map(choice_kind_python_name)
        .collect();
        assert_eq!(
            names,
            vec![
                "card_select",
                "put_back",
                "exhaust_pile",
                "orb_slot",
                "enemy_target",
                "add_generated_to_hand",
                "add_generated_retained_to_hand",
                "paels_tooth_stash",
            ]
        );
        // Uniqueness check.
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "kind strings must be unique");
    }

    #[test]
    fn card_type_strings_match_python_bridge() {
        // Mirrors `_CARD_TYPE_NAMES` table at
        // `sts2-rl-agent@01c094d:sts2_env/parity/bridge_replay.py:43-50`.
        assert_eq!(card_type_python_name(CardType::Attack), "Attack");
        assert_eq!(card_type_python_name(CardType::Skill), "Skill");
        assert_eq!(card_type_python_name(CardType::Power), "Power");
        assert_eq!(card_type_python_name(CardType::Status), "Status");
        assert_eq!(card_type_python_name(CardType::Curse), "Curse");
    }

    #[test]
    fn build_seeded_state_distinct_seeds_distinct_rng() {
        // Different seeds → different RNG streams → states are not equal.
        let mut a = build_seeded_state(1, 0);
        let mut b = build_seeded_state(2, 0);
        // Pull one int from each; the streams must differ.
        let ra = a.rng.next_int(0, i32::MAX);
        let rb = b.rng.next_int(0, i32::MAX);
        assert_ne!(ra, rb);
    }

    #[test]
    fn scenario_builder_parses_upgrade_suffix() {
        // Sweep v2: a "+1" deck-id marker must build the card UPGRADED via
        // the canonical Card::upgrade() delta path (factory registered by
        // the relic-dispatch install that build_scenario_state_impl runs),
        // so cost/base_damage/base_block/keyword_override all reflect the
        // upgraded spec. STRIKE_IRONCLAD: base_damage 6 → 9.
        let state = build_scenario_state_impl(
            0,
            "Ironclad",
            &[
                "STRIKE_IRONCLAD+1".to_string(),
                "DEFEND_IRONCLAD".to_string(),
            ],
            70,
            80,
            &[],
            &[],
            "SLITHERING_STRANGLER_NORMAL",
            0,
            None,
            None,
        )
        .expect("scenario builds");
        let strike = &state.cards[0];
        assert!(strike.upgraded, "+1 marker must flip the upgraded flag");
        assert_eq!(strike.base_damage, 9, "upgrade delta must apply (6 → 9)");
        let defend = &state.cards[1];
        assert!(!defend.upgraded, "unmarked card stays base");
    }

    #[test]
    fn scenario_builder_rejects_bad_upgrade_suffix() {
        // Non-numeric (+x) AND out-of-range (+0, +2) suffixes must all err.
        // STS2 cards are single-level, so only "+1" upgrades; everything
        // else is a hard error rather than a silent clamp.
        for bad in [
            "STRIKE_IRONCLAD+x",
            "STRIKE_IRONCLAD+0",
            "STRIKE_IRONCLAD+2",
        ] {
            let err = build_scenario_state_impl(
                0,
                "Ironclad",
                &[bad.to_string()],
                70,
                80,
                &[],
                &[],
                "SLITHERING_STRANGLER_NORMAL",
                0,
                None,
                None,
            )
            .unwrap_err();
            assert!(err.contains("upgrade suffix"), "id {bad:?} got: {err}");
        }
    }
}
