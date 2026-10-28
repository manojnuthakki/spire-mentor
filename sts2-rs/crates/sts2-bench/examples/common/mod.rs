// Each example binary `#[path]`-includes this file as a private
// module and uses only a subset of its surface, so per-example
// `dead_code` warnings would fire on the unused helpers. Silence them
// at the module root — clippy `unused` lints don't add signal here
// because the "real" callsites live in sibling files cargo doesn't
// see during compilation of any one example.
#![allow(dead_code)]
//! Shared corpus + state-construction module for the bead `sts2-rs-yuyc`
//! MCTS A/B and oracle harnesses. Included into each example binary via
//! `#[path = "common/mod.rs"] mod common;` because cargo examples don't
//! get a real shared-library scope without a wrapping `[lib]` target.
//!
//! State construction follows the path documented in
//! `crates/sts2-cli/src/sim/scenario/starter.rs::build_state` BUT we
//! deliberately do not import the `sts2-cli` crate — per the bead
//! requirements, examples here only depend on `sts2-core` + `sts2-content`
//! + `sts2-mcts`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_content::cards::CardSpec;
use sts2_content::encounters::act1_overgrowth::ACT1_ENCOUNTERS;
use sts2_content::encounters::{EncounterDef, EncounterTier};
use sts2_content::monster_dispatch::{build_ai, SpawnPosition};
use sts2_core::cards::Card;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    apply, is_terminal, legal_actions, state_hash, Action, CardUid, CombatPhase, CombatState,
    Creature, CreatureUid, IntentHistory, MonsterId, OrbSlots, PlayerCombatState, Side, StepResult,
    TurnEvents,
};

/// Realize a harvested fixture to the state a search will see: load the
/// TOML, build the combat at the fixture's own `[player].ascension` (0
/// when the field is absent, which is every fixture harvested before the
/// hard pool), and drive the combat loop to the first `PlayerPlayPhase`.
/// `scenario_to_combat_state` alone returns a pre-`start_combat` state
/// whose only legal action is `EndTurn`.
///
/// Every position-book consumer (`combat_position_book`, `mcts_book_sweep`,
/// `book_position_probe`) realizes through this one function so they
/// cannot disagree about which state an entry names; the frozen
/// `state_hash` is the check that they did not. A book built from
/// ascension-0 fixtures hashes identically to before this helper existed.
///
/// # Errors
///
/// The loader's or the scenario builder's message, prefixed with the path.
pub fn realize_fixture(path: &std::path::Path, seed: u64) -> Result<CombatState, String> {
    let scenario = sts2_cli::sim::scenario::toml_file::load_scenario_file(path)
        .map_err(|e| format!("load {}: {e}", path.display()))?;
    let ascension = scenario
        .player
        .as_ref()
        .and_then(|p| p.ascension)
        .and_then(|a| u8::try_from(a).ok())
        .unwrap_or(0);
    let mut state = sts2_cli::scenario_to_combat_state_with_ascension(&scenario, seed, ascension)
        .map_err(|e| format!("realize {}: {e}", path.display()))?;
    let _ = start_combat(&mut state);
    let _ = start_player_turn(&mut state);
    Ok(state)
}

/// `[player].ascension` of a fixture, without realizing it. `0` when the
/// field is absent or the file does not parse.
pub fn fixture_ascension(path: &std::path::Path) -> u8 {
    sts2_cli::sim::scenario::toml_file::load_scenario_file(path)
        .ok()
        .and_then(|s| s.player.as_ref().and_then(|p| p.ascension))
        .and_then(|a| u8::try_from(a).ok())
        .unwrap_or(0)
}

/// The engine-side dominance proof — shared by `dominance_pairs` (which
/// enumerates a suite from it) and `combat_position_book` (which uses it
/// to mark the positions where its own 50 000-sim oracle is provably
/// wrong). One copy, because a second copy of a theorem drifts.
pub mod dominance;

// ─── Corpus types ───────────────────────────────────────────────────────────

/// Tier label as serialized in the corpus JSON. Mirrors
/// `EncounterTier`, kept separate so corpus files don't depend on the
/// non-`Serialize`-derived engine enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CorpusTier {
    /// "common" in bead language = STS2 `Weak ∪ Normal`. The corpus
    /// allocates 20 slots to this bucket, 20 to elite, 10 to boss.
    Common,
    Elite,
    Boss,
}

/// One entry in the persisted corpus. The deterministic seed +
/// `encounter_id` + `warmup_turns` are sufficient to reproduce the
/// post-warmup `CombatState` byte-for-byte at the same git SHA.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusEntry {
    /// Stable id (0..N) — used as the join key in A/B and oracle files.
    pub state_id: u32,
    /// `ACT1_ENCOUNTERS[i].id` — the STS2-native encounter identifier.
    pub encounter: String,
    /// "common" / "elite" / "boss" bucket. 20/20/10 split for N=50.
    pub tier: CorpusTier,
    /// Engine seed fed into `CombatState.rng`. Stable across runs.
    pub engine_seed: u64,
    /// Separate seed driving the deterministic warm-up policy. Distinct
    /// from `engine_seed` so the warm-up RNG state doesn't leak into
    /// the engine RNG.
    pub warmup_seed: u64,
    /// Number of warm-up turns played (1..=3) before the state is
    /// snapshotted into the corpus.
    pub warmup_turns: u32,
    /// `state_hash` of the post-warmup state. Diagnostic — diff tools
    /// can use it to detect a corpus regeneration.
    pub state_hash: u64,
}

/// Full corpus file schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Corpus {
    pub version: u32,
    pub git_sha: String,
    pub generator: String,
    pub entries: Vec<CorpusEntry>,
}

// ─── Public corpus generator ────────────────────────────────────────────────

/// Encounter-id selection (deterministic). 50 entries.
///
/// Bucketing:
/// - 20 common = round-robin over `Weak ∪ Normal` (4 + 12 = 16 unique)
/// - 20 elite  = round-robin over Elite (3 unique)
/// - 10 boss   = round-robin over Boss (3 unique)
///
/// Each slot gets a distinct `engine_seed` so HP rolls / draw shuffles
/// produce 50 unique seeded states even where the encounter id repeats.
#[must_use]
pub fn build_default_corpus() -> Vec<CorpusEntry> {
    let common: Vec<&EncounterDef> = ACT1_ENCOUNTERS
        .iter()
        .filter(|e| matches!(e.tier, EncounterTier::Weak | EncounterTier::Normal))
        .collect();
    let elite: Vec<&EncounterDef> = ACT1_ENCOUNTERS
        .iter()
        .filter(|e| e.tier == EncounterTier::Elite)
        .collect();
    let boss: Vec<&EncounterDef> = ACT1_ENCOUNTERS
        .iter()
        .filter(|e| e.tier == EncounterTier::Boss)
        .collect();

    let mut out = Vec::with_capacity(50);
    let mut state_id: u32 = 0;
    let mut engine_seed: u64 = 1;

    for i in 0..20 {
        let enc = common[i % common.len()];
        let warmup_turns = 1 + (u32::try_from(i).unwrap() % 3); // 1,2,3,1,2,3,...
        out.push(CorpusEntry {
            state_id,
            encounter: enc.id.to_string(),
            tier: CorpusTier::Common,
            engine_seed,
            warmup_seed: 0xC0FFEE_u64.wrapping_add(u64::from(state_id)),
            warmup_turns,
            state_hash: 0, // filled in after building the state
        });
        state_id += 1;
        engine_seed += 1;
    }
    for i in 0..20 {
        let enc = elite[i % elite.len()];
        let warmup_turns = 1 + (u32::try_from(i).unwrap() % 3);
        out.push(CorpusEntry {
            state_id,
            encounter: enc.id.to_string(),
            tier: CorpusTier::Elite,
            engine_seed,
            warmup_seed: 0xC0FFEE_u64.wrapping_add(u64::from(state_id)),
            warmup_turns,
            state_hash: 0,
        });
        state_id += 1;
        engine_seed += 1;
    }
    for i in 0..10 {
        let enc = boss[i % boss.len()];
        let warmup_turns = 1 + (u32::try_from(i).unwrap() % 3);
        out.push(CorpusEntry {
            state_id,
            encounter: enc.id.to_string(),
            tier: CorpusTier::Boss,
            engine_seed,
            warmup_seed: 0xC0FFEE_u64.wrapping_add(u64::from(state_id)),
            warmup_turns,
            state_hash: 0,
        });
        state_id += 1;
        engine_seed += 1;
    }
    out
}

// ─── State construction ─────────────────────────────────────────────────────

/// Build a `CombatState` for the named encounter using the supplied
/// engine seed. The Ironclad starter deck is the only player class
/// (bead constraint: "mid-Ironclad"). Mirrors
/// `sts2-cli::sim::scenario::starter::build_state` minus the
/// `Scenario` indirection.
///
/// # Panics
///
/// Panics if `encounter_id` is not in `ACT1_ENCOUNTERS`. Callers come
/// from `build_default_corpus()` which only picks valid ids.
#[must_use]
pub fn build_ironclad_state(encounter_id: &str, seed: u64) -> CombatState {
    build_ironclad_state_with_deck(encounter_id, seed, &ironclad_starter_deck())
}

/// Sibling to [`build_ironclad_state`] that takes the deck pile
/// explicitly. Bead `sts2-rs-yg5kz` (aze8.5) — lets the
/// `vantom_ceiling_probe` example swap between starter / sts2log-perfect
/// / random-full-pool deck sources without duplicating the player /
/// encounter / RNG plumbing. The starter helper above is preserved as
/// a zero-arg shorthand for existing callers.
#[must_use]
pub fn build_ironclad_state_with_deck(
    encounter_id: &str,
    seed: u64,
    deck_specs: &[CardSpec],
) -> CombatState {
    sts2_content::cards::dispatch::ensure_installed();

    let enc = ACT1_ENCOUNTERS
        .iter()
        .find(|e| e.id == encounter_id)
        .unwrap_or_else(|| panic!("unknown encounter id: {encounter_id}"));

    let mut cards_arena: Vec<Card> = Vec::with_capacity(deck_specs.len());
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    for (idx, spec) in deck_specs.iter().enumerate() {
        let raw = u32::try_from(idx + 1).expect("deck size fits in u32");
        let uid = CardUid::new(raw).expect("non-zero");
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
        draw.push(uid);
    }

    let mut rng = sts2_core::dotnet_rng::Rng::new((seed) as i32);
    let group_len = enc.monsters.len();
    let mut creatures: Vec<Creature> = Vec::with_capacity(group_len);
    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    for (idx, (kind, args)) in enc.monsters.iter().enumerate() {
        let raw = u32::try_from(idx + 1).expect("monster count fits u32");
        let uid = CreatureUid::new(raw).expect("non-zero");
        let hp = kind.starting_hp(&mut rng, 0);
        let position = SpawnPosition {
            alone: group_len == 1,
            front: idx == 0,
            back: idx + 1 == group_len && group_len > 1,
            index: u8::try_from(idx).unwrap_or(u8::MAX),
        };
        let ai = build_ai(*kind, *args, position);
        creatures.push(Creature {
            id: uid.0,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: SmallVec::new(),
            species: Some(*kind),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId(kind.python_name()),
            ai: Some(ai),
        });
        enemy_uids.push(uid);
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
        character: None,
        potions: SmallVec::new(),
        potion_slot_indices: SmallVec::new(),
        potion_slots: 0,
        orbs: OrbSlots::default(),
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
        retain_this_turn: SmallVec::new(),
        sly_this_turn: SmallVec::new(),
        sly_this_combat: SmallVec::new(),
        pending_retain_count: 0,
        stars_gained_this_turn: 0,
        pinned_top: smallvec::SmallVec::new(),
        pinned_bottom: SmallVec::new(),
        deferred_draw_order: false,
    };

    let is_elite = matches!(enc.tier, EncounterTier::Elite | EncounterTier::Boss);
    let is_boss = matches!(enc.tier, EncounterTier::Boss);

    let mut state = CombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng,
        run_rng: sts2_core::run_rng_set::RunRngSet::new(seed as i32),
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
        room_extra_rewards: Vec::new(),
        event_combat_timed_out: false,
        gold: 0,
        is_over: false,
        player_won: false,
        ascension: 0,
        master_deck: Vec::new(),
        last_added_card: None,
        is_elite,
        is_boss,
        act_index: 1,
        active_attack: None,
        energy_wasted: 0,
        cards_drawn: 0,
        pending_run_gold_loss: 0,
        last_drawn_card: None,
    };

    let r0 = start_combat(&mut state);
    debug_assert_eq!(r0, StepResult::Advanced);
    let r1 = start_player_turn(&mut state);
    debug_assert_eq!(r1, StepResult::Advanced);

    state
}

/// Deterministic warmup policy: play `turns` rounds of random legal
/// actions, biased to occasionally EndTurn so we don't burn the whole
/// hand on turn 1. Uses a SEPARATE RNG seeded from `warmup_seed` so
/// the engine RNG state stays a function of `engine_seed` alone — i.e.
/// changing `warmup_seed` doesn't perturb the post-warmup engine RNG
/// in confusing ways.
///
/// Stops early on terminal (a 1-turn warmup can kill a weak encounter).
/// On terminal, ROLLS BACK to the pre-action snapshot so the returned
/// state is always non-terminal and `Mcts::search` won't panic with
/// "search() called on terminal root". This means a "warmup_turns=3"
/// fixture against a 1-shot encounter may only advance 0-1 turns —
/// that's fine, the encounter id + the rolled-back state are still
/// reproducible across runs from the same seed.
pub fn warmup_random(state: &mut CombatState, warmup_seed: u64, turns: u32) {
    let mut policy_rng = sts2_core::dotnet_rng::Rng::new((warmup_seed) as i32);
    let mut turns_played: u32 = 0;
    // Safety cap so a misbehaving fixture can't infinite-loop the
    // generator. 512 actions across 3 turns is generous.
    for _step in 0..512 {
        if turns_played >= turns || is_terminal(state) {
            return;
        }
        let legal = legal_actions(state);
        if legal.is_empty() {
            return;
        }
        // ~25% chance to EndTurn early if it's legal, otherwise pick
        // a random card / choice. Biases toward sampling later turns
        // instead of grinding through the whole hand.
        let endturn_prob = policy_rng.next_int_exclusive(0, 4) == 0;
        let action = if legal.iter().any(|a| matches!(a, Action::EndTurn)) && endturn_prob {
            Action::EndTurn
        } else {
            let idx = policy_rng.next_int_exclusive(0, legal.len() as i32) as usize;
            legal[idx].clone()
        };
        let is_endturn = matches!(action, Action::EndTurn);
        // Snapshot the pre-action state. `CombatState: Clone` is cheap
        // (used heavily by MCTS rollouts) so this is fine in a warm-up
        // loop that runs at most 512 × 50 = 25 600 times in the
        // corpus build.
        let snapshot = state.clone();
        let _ = apply(state, action.clone());
        if is_terminal(state) {
            // Walking into terminal would leave `Mcts::search` with
            // an empty legal-action set — revert and stop.
            *state = snapshot;
            return;
        }
        if is_endturn {
            turns_played += 1;
        }
    }
}

/// Advance the position by a pseudo-random number of individual plies.
///
/// Without this every entry is the opening decision of a fresh turn at full
/// energy, because `scenario_to_combat_state` yields a combat-entry snapshot
/// and `start_player_turn` leaves it at ply 1. A book of turn-openings cannot
/// see any knob whose effect is confined to later plies — the v1 book scored
/// 168 gold actions of which **zero** were `EndTurn`, so the (since-deleted,
/// GH #445) `endturn_fpu_penalty` provably could not change a single outcome
/// on it. The blindness is structural and outlived that knob: v2 warms up
/// mid-turn specifically to expose end-turn decisions and still yields
/// **one** `EndTurn` gold answer out of 159, because the gold filter keeps
/// only positions both 50k-sim oracles resolve unanimously and a marginal
/// end-turn call is exactly what they will not agree on.
///
/// `common::warmup_random` does not solve this: it counts *end-turns*, so it
/// also lands on a turn boundary, just a later one.
///
/// The ply count is derived from the entry's own seed so the book is
/// reproducible, and terminal states are reverted rather than kept — a
/// terminal position has no decision to score.
pub fn warmup_plies(state: &mut CombatState, seed: u64, max_plies: u32) {
    if max_plies == 0 {
        return;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let mut rng = sts2_core::dotnet_rng::Rng::new((seed ^ 0x5EED_1F1E) as i32);
    let n = rng.next_int_exclusive(0, max_plies as i32 + 1);
    for _ in 0..n {
        if state.is_over || state.phase != CombatPhase::PlayerPlayPhase {
            return;
        }
        let legal = legal_actions(state);
        if legal.is_empty() {
            return;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let idx = rng.next_int_exclusive(0, legal.len() as i32) as usize;
        let snapshot = state.clone();
        let _ = sts2_core::apply(state, legal[idx].clone());
        if state.is_over {
            *state = snapshot;
            return;
        }
    }
}

/// Compose `build_ironclad_state` + `warmup_random` and stamp
/// `state_hash` back into the corpus entry.
pub fn realize_state(entry: &mut CorpusEntry) -> CombatState {
    let mut state = build_ironclad_state(&entry.encounter, entry.engine_seed);
    warmup_random(&mut state, entry.warmup_seed, entry.warmup_turns);
    entry.state_hash = state_hash(&state);
    state
}

// ─── A/B + oracle JSON schemas ──────────────────────────────────────────────

/// Per-budget result payload. The `wall_ms` field is informational and
/// MUST NOT participate in determinism checks — every other field is a
/// pure function of (state, sims, MctsConfig).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetResult {
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    pub visits: u32,
    pub pv_depth: u32,
    /// Principal variation rendered as `format!("{:?}", action)` strings.
    /// `Action` doesn't `derive(Serialize)` and we don't want to leak a
    /// schema dependency to engine internals — debug strings are stable
    /// enough for PV-agreement comparisons (we only check PV[0]).
    pub pv: Vec<String>,
    /// Top-level recommended action (most-visited child at the root).
    pub recommended: String,
    /// Wall-clock in milliseconds. Informational; excluded from
    /// determinism checks by convention. The A/B and oracle binaries
    /// zero this field on disk (so `diff -q` of two runs at the same
    /// git SHA is empty) and report timing via stderr instead. The
    /// schema preserves the field so future tools that DO want to
    /// pin timing samples can read it without a schema bump.
    #[serde(default)]
    pub wall_ms: u64,
}

/// Per-state row in the A/B / oracle JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerStateRow {
    pub state_id: u32,
    pub encounter: String,
    pub tier: CorpusTier,
    pub state_hash: u64,
    /// Budget → result. Use a `BTreeMap<String, _>` (keys are the
    /// stringified sim count, e.g. `"500"`) so JSON output is
    /// key-sorted regardless of insertion order.
    pub results: BTreeMap<String, BudgetResult>,
}

/// Top-level A/B / oracle JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbReport {
    pub version: u32,
    pub git_sha: String,
    pub engine_version: String,
    /// Sorted ascending by `state_id` so byte-for-byte diffs are stable.
    pub per_state: Vec<PerStateRow>,
}

// ─── Misc utilities ─────────────────────────────────────────────────────────

/// Render an `Action` as a short stable string (uses `Debug`). Used
/// for PV serialization + diff PV[0] comparison.
#[must_use]
pub fn action_to_string(action: &Action) -> String {
    format!("{action:?}")
}

/// Return the short git SHA at HEAD, or `"unknown"` if `git` is not
/// available. The A/B file names embed this — branch-switching mid-A/B
/// is the workflow.
#[must_use]
pub fn git_short_sha() -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}

/// Resolve the path of `<bench-crate>/data/<filename>`. The
/// resolution walks up from the example binary's `CARGO_MANIFEST_DIR`
/// so the path is stable regardless of CWD at invocation time.
#[must_use]
pub fn data_path(filename: &str) -> std::path::PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let mut p = std::path::PathBuf::from(manifest);
    p.push("data");
    p.push(filename);
    p
}

/// Pretty-print a `serde_json::Value` with 2-space indent. We go
/// through `serde_json::Value` (rather than direct
/// `to_writer_pretty(file, &T)`) because `BTreeMap` is the only
/// sorted-key container we need — both `Corpus` and `AbReport`
/// already wrap their unordered keys in `BTreeMap`. A `Value` round
/// trip is also a cheap way to enforce that nothing weird (e.g. a
/// `f32::NaN`) sneaks into the output.
pub fn write_json_pretty<P: AsRef<std::path::Path>, T: Serialize>(
    path: P,
    value: &T,
) -> std::io::Result<()> {
    let v = serde_json::to_value(value).expect("serde_json::to_value");
    let bytes = serde_json::to_vec_pretty(&v).expect("serde_json::to_vec_pretty");
    if let Some(parent) = path.as_ref().parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

// ─── SPRT (sts2-rs-fgw1x / s6umt) ────────────────────────────────────
//
// Shared between the standalone `sprt_analyze` post-hoc analyzer and
// the inline early-termination path in `mcts_ab_value_net`. See the
// `sprt_analyze` module docs for the math derivation; this module is
// the small canonical implementation of the binomial-Wald LLR.

/// SPRT bounds + error rates. Defaults match the s6umt bead spec
/// (`elo0=0, elo1=10, α=β=0.05`).
#[derive(Debug, Clone, Copy)]
pub struct SprtConfig {
    pub elo0: f64,
    pub elo1: f64,
    pub alpha: f64,
    pub beta: f64,
}

impl Default for SprtConfig {
    fn default() -> Self {
        Self {
            elo0: 0.0,
            elo1: 10.0,
            alpha: 0.05,
            beta: 0.05,
        }
    }
}

impl SprtConfig {
    /// Upper Wald bound (accept H1: Δelo ≥ elo1).
    #[must_use]
    pub fn upper_bound(&self) -> f64 {
        ((1.0 - self.beta) / self.alpha).ln()
    }
    /// Lower Wald bound (accept H0: Δelo ≤ elo0).
    #[must_use]
    pub fn lower_bound(&self) -> f64 {
        (self.beta / (1.0 - self.alpha)).ln()
    }
}

/// Trinomial paired outcome from the branch's perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SprtOutcome {
    BranchWin,
    BranchLoss,
    Draw,
}

/// Verdict returned by [`sprt_check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SprtVerdict {
    AcceptH1,
    AcceptH0,
    Continue,
}

/// Elo → expected score under the logistic Elo model.
#[must_use]
pub fn elo_to_score(elo: f64) -> f64 {
    1.0 / (1.0 + (-elo / 400.0 * std::f64::consts::LN_10).exp())
}

/// Wald binomial-Gaussian LLR after `(w, d, l)` paired outcomes.
/// Draws count as 0.5 (standard chess-engine convention).
#[must_use]
pub fn sprt_llr(w: u32, d: u32, l: u32, cfg: &SprtConfig) -> f64 {
    let n = f64::from(w + d + l);
    if n < 1.0 {
        return 0.0;
    }
    let score = (f64::from(w) + 0.5 * f64::from(d)) / n;
    let mean_sq = (f64::from(w) + 0.25 * f64::from(d)) / n;
    let var = (mean_sq - score * score).max(1e-9);
    let p0 = elo_to_score(cfg.elo0);
    let p1 = elo_to_score(cfg.elo1);
    let llr = ((score - p0).powi(2) - (score - p1).powi(2)) / (2.0 * var);
    llr * n
}

/// SPRT verdict after `(w, d, l)` paired outcomes. `min_samples`
/// guards against low-N variance instability (the empirical-variance
/// floor at 1e-9 produces huge spurious LLRs at very low N). 10 is
/// the standard fishtest setting.
#[must_use]
pub fn sprt_check(w: u32, d: u32, l: u32, cfg: &SprtConfig, min_samples: u32) -> SprtVerdict {
    if w + d + l < min_samples {
        return SprtVerdict::Continue;
    }
    let llr = sprt_llr(w, d, l, cfg);
    if llr >= cfg.upper_bound() {
        SprtVerdict::AcceptH1
    } else if llr <= cfg.lower_bound() {
        SprtVerdict::AcceptH0
    } else {
        SprtVerdict::Continue
    }
}

/// Classify a paired (baseline, branch) outcome from the branch's
/// perspective. With STS2's 1-seed-per-state harness, exactly one of
/// {wins, losses, ties} is non-zero per `BudgetResult`; if a future
/// multi-seed run produces ties, we route those to `Draw`.
#[must_use]
pub fn sprt_classify(base: &BudgetResult, branch: &BudgetResult) -> SprtOutcome {
    let base_label = if base.wins > base.losses && base.wins > base.ties {
        'W'
    } else if base.losses > base.wins && base.losses > base.ties {
        'L'
    } else {
        'D'
    };
    let branch_label = if branch.wins > branch.losses && branch.wins > branch.ties {
        'W'
    } else if branch.losses > branch.wins && branch.losses > branch.ties {
        'L'
    } else {
        'D'
    };
    match (base_label, branch_label) {
        ('L', 'W' | 'D') | ('D', 'W') => SprtOutcome::BranchWin,
        ('W', 'L' | 'D') | ('D', 'L') => SprtOutcome::BranchLoss,
        _ => SprtOutcome::Draw,
    }
}
