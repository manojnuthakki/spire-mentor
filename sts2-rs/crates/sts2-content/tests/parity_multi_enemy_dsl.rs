//! P5-8 (sts2-rs-2nx): multi-enemy parity harness for the new `MonsterAi`
//! DSL ctors shipped by P5-3..P5-7 (ux2-3..7).
//!
//! Loads every `tests/parity/scripted/dsl-multi-*.json` fixture, builds a
//! matching Rust `CombatState` whose enemy's `monster_id` / HP / block /
//! powers mirror the recorded Python state, and asserts byte-equality
//! modulo `compare.exclude_fields=["intent","intent_damage","intent_hits"]`.
//!
//! The intent triple is excluded by design — Python's Mersenne-Twister and
//! Rust's Xoshiro256++ produce different rolls for the same seed (see
//! `crates/sts2-core/src/monsters.rs:22-29` and the inflight briefing §9).
//! Structural shape (`HP/max_hp/monster_id/powers/round/player`) is what
//! this harness locks; the bead exists to catch regressions in
//! `combat_state_to_bridge_state` shape during P6/P7 churn.
//!
//! **sts2-rs-gvn (P5-8 follow-up):** each fixture also walks ≥10 turns
//! via `apply(state, Action::EndTurn)`. The wider per-step
//! `compare.step_exclude_fields` list is required because Python and
//! Rust hit game-over at different rounds — `intent_damage` rolls
//! disagree across the two RNG families, so HP/block/powers diverge by
//! turn 1, and `/round` plus the player energy fields freeze at
//! different values once `is_over=true`. The fields that REMAIN locked
//! per step: top-level `type` ("`combat_action`") + key set,
//! `enemies/0/id`, `enemies/0/max_hp`, the three pile counts, `hand`
//! (empty), and `player/max_hp`. The spec-side surface was the
//! `auto_end_turns` knob of the since-deleted `record_fixture.py`
//! (recorder retired, fixtures frozen — see `PYTHON_SIM_PIN.md`).
//!
//! **Substitution note (bead ux2-8 acceptance):** the bead names
//! `Cultist` and `JawWorm` as the Act-1 representatives, but those are
//! Slay-the-Spire-classic stubs in `monsters/legacy_stubs.rs` with no
//! Python counterparts at the `01c094d` pin (Python's game roster is a
//! disjoint STS2-hybrid set). The harness substitutes `Flyconid` and
//! `CubexConstruct` — two Act-1 DSL ctors that exist in both
//! `sts2-content` and `sts2_env.monsters.act1` — so cross-language
//! parity is actually checkable. The bead's "5 monsters × 5 seeds = 25
//! fixtures" floor is preserved.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use smallvec::SmallVec;

use sts2_content::monsters::{
    act1_overgrowth::{create_cubex_construct_ai, create_flyconid_ai},
    act1_underdocks::create_lagavulin_matriarch_ai,
    act2_hive::create_knowledge_demon_ai,
    shared::create_the_adversary_mk_one_ai,
};
use sts2_core::monster_ai::MonsterAi;
use sts2_core::{
    apply, parity::combat_state_to_bridge_state, Action, CombatPhase, CombatState, Creature,
    CreatureUid, IntentHistory, MonsterId, MonsterKind, PlayerCombatState, Side, TurnEvents,
};

/// Resolve any `pending_choice` by picking option 0, mirroring the parity
/// harness's `_drain_rust_choices` / `_drain_csharp_choices` (both pick
/// index 0). Bead `sts2-rs-ybtmu` (retimed): KnowledgeDemon's
/// CURSE_OF_KNOWLEDGE raises a `MonsterCurseSelect` MID enemy turn (C# awaits
/// the card screen inside the move body), which suspends the rest of the
/// `end_turn` chain; the scripted walk must resolve it (as real play / the
/// sweep does) before snapshotting, otherwise the enemy turn never finishes,
/// the next `end_turn` is illegal and the walk wedges. Resolving to index 0
/// (Disintegration) reproduces the Python pin's
/// inline-curse observable — the chosen power lands in the already-excluded
/// `/player/powers`, so the per-step SHAPE is unchanged. A no-op for every
/// fixture whose monster never opens a choice.
fn drain_choices_index0(state: &mut CombatState) {
    for _ in 0..8 {
        if state.pending_choice.is_none() {
            return;
        }
        let mut sel: SmallVec<[u8; 4]> = SmallVec::new();
        sel.push(0);
        apply(state, Action::ResolveChoice { selection: sel });
    }
}

// ── Pretty diff (mirror of parity_harness.rs::pretty_diff_excluding) ─────

fn pretty_diff_excluding(expected: &Value, actual: &Value, exclude: &[String]) -> Option<String> {
    let mut out = String::new();
    diff_walk("", expected, actual, &mut out, exclude);
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn diff_walk(path: &str, expected: &Value, actual: &Value, out: &mut String, exclude: &[String]) {
    if exclude.iter().any(|p| p == path) {
        return;
    }
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            let mut keys: Vec<&String> = e.keys().chain(a.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let child = format!("{path}/{k}");
                match (e.get(k), a.get(k)) {
                    (Some(ev), Some(av)) => diff_walk(&child, ev, av, out, exclude),
                    (Some(ev), None) => {
                        if exclude.iter().any(|p| p == &child) {
                            continue;
                        }
                        out.push_str(&format!("  {child:<40} expected={ev}  actual=<missing>\n"));
                    }
                    (None, Some(av)) => {
                        if exclude.iter().any(|p| p == &child) {
                            continue;
                        }
                        out.push_str(&format!("  {child:<40} expected=<missing>  actual={av}\n"));
                    }
                    (None, None) => {}
                }
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            let max_len = e.len().max(a.len());
            for i in 0..max_len {
                let child = format!("{path}/{i}");
                match (e.get(i), a.get(i)) {
                    (Some(ev), Some(av)) => diff_walk(&child, ev, av, out, exclude),
                    (Some(ev), None) => {
                        if exclude.iter().any(|p| p == &child) {
                            continue;
                        }
                        out.push_str(&format!("  {child:<40} expected={ev}  actual=<missing>\n"));
                    }
                    (None, Some(av)) => {
                        if exclude.iter().any(|p| p == &child) {
                            continue;
                        }
                        out.push_str(&format!("  {child:<40} expected=<missing>  actual={av}\n"));
                    }
                    (None, None) => {}
                }
            }
        }
        _ => {
            if expected != actual {
                out.push_str(&format!(
                    "  {path:<40} expected={expected}  actual={actual}\n"
                ));
            }
        }
    }
}

// ── Fixture directory ────────────────────────────────────────────────────

fn scripted_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("tests").join("parity").join("scripted"))
        .expect("workspace root must be ancestor of CARGO_MANIFEST_DIR")
}

fn collect_dsl_multi_fixtures() -> Vec<PathBuf> {
    let dir = scripted_dir();
    if !dir.exists() {
        return Vec::new();
    }
    let mut out: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("json")
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|n| n.starts_with("dsl-multi-"))
        })
        .collect();
    out.sort();
    out
}

// ── Monster ctor registry ────────────────────────────────────────────────

/// Map a fixture's `metadata.encounter` to (`monster_id_str`, `MonsterAi`).
/// Mirrors the Python `enemies[i].builder` field used by the retired
/// recorder (`record_fixture.py`, since deleted — see `PYTHON_SIM_PIN.md`).
fn build_ai_for_encounter(encounter: &str) -> (&'static str, MonsterAi) {
    match encounter {
        "Flyconid" => ("FLYCONID", create_flyconid_ai()),
        "CubexConstruct" => ("CUBEX_CONSTRUCT", create_cubex_construct_ai()),
        "KnowledgeDemon" => ("KNOWLEDGE_DEMON", create_knowledge_demon_ai()),
        "TheAdversaryMkOne" => ("THE_ADVERSARY_MK_ONE", create_the_adversary_mk_one_ai()),
        "LagavulinMatriarch" => ("LAGAVULIN_MATRIARCH", create_lagavulin_matriarch_ai()),
        other => panic!("P5-8 harness: unknown encounter {other:?}"),
    }
}

fn build_state(fixture_initial: &Value, encounter: &str, seed: u64) -> CombatState {
    let enemy_json = &fixture_initial["enemies"][0];
    let monster_id_str = enemy_json["id"]
        .as_str()
        .expect("fixture enemy id must be a string");
    let max_hp = i32::try_from(enemy_json["max_hp"].as_i64().expect("fixture enemy max_hp"))
        .expect("max_hp fits i32");
    let hp =
        i32::try_from(enemy_json["hp"].as_i64().expect("fixture enemy hp")).expect("hp fits i32");
    let block = i32::try_from(enemy_json["block"].as_i64().unwrap_or(0)).expect("block fits i32");

    let (expected_id, ai) = build_ai_for_encounter(encounter);
    assert_eq!(
        monster_id_str, expected_id,
        "P5-8 harness: fixture monster_id {monster_id_str:?} \
         does not match registry entry {expected_id:?} for encounter {encounter:?}"
    );

    // First intent — used to populate Creature.intent so the Rust
    // bridge-state exporter emits the intent triple (which is then
    // excluded from comparison).
    let initial_intent = ai
        .current_move()
        .intents
        .first()
        .copied()
        .expect("initial MoveState must carry ≥1 intent");

    // Player shape mirrors the Python recorder's "no start_combat" snapshot:
    // energy = 0, round = 0 (Python `CombatState.__init__` defaults), hand empty.
    let player = PlayerCombatState {
        current_hp: 80,
        max_hp: 80,
        block: 0,
        energy: 0,
        max_energy: 3,
        hand: SmallVec::new(),
        draw: SmallVec::new(),
        discard: SmallVec::new(),
        exhaust: SmallVec::new(),
        play: SmallVec::new(),
        stars: 0,
        powers: SmallVec::new(),
        character: None,
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

    let enemy_uid = CreatureUid::new(1).expect("non-zero");
    // We use the raw `monster_id` string from the fixture (`MonsterId` is
    // a `&'static str` wrapper but the encounter registry above pins it
    // to a static).
    let monster_id = MonsterId(expected_id);
    let creature = Creature {
        id: enemy_uid.0,
        current_hp: hp,
        max_hp,
        block,
        powers: SmallVec::new(),
        // sts2-rs-vvz0: STS2-native placeholder species; the exporter
        // reads `monster_id` (not `species`) for the JSON `id` field,
        // so the `species` discriminant is irrelevant to the parity
        // diff.
        species: Some(MonsterKind::Mawler),
        monster_id,
        intent: Some(initial_intent),
        intent_history: IntentHistory::default(),
        ai: Some(ai),
    };

    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(enemy_uid);

    CombatState {
        belt_value: None,
        // sts2-rs-gvn: `PlayerPlayPhase` so subsequent `Action::EndTurn`
        // dispatches into `combat_loop::end_turn_dispatch` (the apply()
        // arm gates on phase != PlayerPlayPhase → no-op). Setting the
        // phase has no effect on `combat_state_to_bridge_state` (which
        // does not emit phase), so the initial-state shape match is
        // unaffected.
        phase: CombatPhase::PlayerPlayPhase,
        round: 0,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),

        run_rng: sts2_core::run_rng_set::RunRngSet::new((seed) as i32),
        creatures: vec![creature],
        cards: Vec::new().into(),
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
        is_elite: false,
        is_boss: false,
        act_index: 1,
        active_attack: None,
        energy_wasted: 0,
        cards_drawn: 0,
        pending_run_gold_loss: 0,
        last_drawn_card: None,
    }
}

fn load_fixture(path: &Path) -> Value {
    let raw = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {} as JSON: {e}", path.display()))
}

fn fixture_exclude(fixture: &Value) -> Vec<String> {
    fixture
        .get("compare")
        .and_then(|c| c.get("exclude_fields"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// sts2-rs-gvn: per-step exclude list applied during the 10-turn walk.
/// Falls back to the initial-state `exclude_fields` when absent.
///
/// The per-step list necessarily covers more ground than the initial
/// list: once `Action::EndTurn` fires, the Rust Xoshiro256++ rolls
/// `intent_damage` values that disagree with Python's MT, which
/// propagates through `apply_damage` into HP / block / powers / energy
/// / round (game-over arrives at different turns across languages,
/// freezing the energy and round counters at different values). The
/// remaining fields locked for shape: top-level `type` tag, top-level
/// keys, `enemies/0/id`, `enemies/0/max_hp`, the three pile counts,
/// `hand` (empty for these specs), and `player/max_hp`.
fn fixture_step_exclude(fixture: &Value) -> Vec<String> {
    match fixture
        .get("compare")
        .and_then(|c| c.get("step_exclude_fields"))
        .and_then(Value::as_array)
    {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        None => fixture_exclude(fixture),
    }
}

/// sts2-rs-gvn: bead acceptance — every fixture walks ≥ `MIN_TURNS` turns.
const MIN_TURNS: usize = 10;

/// Ctors whose initial move telegraphs NOTHING, because the C# `MoveState`
/// is constructed with no `params AbstractIntent[]`:
///
/// - `Byrdpip.cs:28`, `PaelsLegion.cs:35`, `BattleFriendV{1,2,3}.cs:28` —
///   `new MoveState("NOTHING_MOVE", _ => Task.CompletedTask)`.
///
/// Distinct from the dummies (`Architect`, `BigDummy`, `OneHpMonster`,
/// `TenHpMonster`), which pass `new HiddenIntent()` and therefore DO
/// telegraph one intent — the wire serializes `"Hidden"` for those, and
/// `parse_intent` maps it to `Intent::Unknown`. Collapsing the two shapes is
/// what the old Python port did, and it cost a divergence on every board a
/// silent creature stood on.
const SILENT_INITIAL_MOVE_CTORS: &[&str] = &[
    "byrdpip",
    "paels_legion",
    "battle_friend_v1",
    "battle_friend_v2",
    "battle_friend_v3",
];

#[test]
fn dsl_multi_enemy_fixtures_shape_lock() {
    let fixtures = collect_dsl_multi_fixtures();
    assert!(
        fixtures.len() >= 25,
        "P5-8 expected ≥25 dsl-multi-* fixtures under {}; found {}",
        scripted_dir().display(),
        fixtures.len(),
    );

    let mut by_encounter: BTreeMap<String, u32> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();

    for path in &fixtures {
        let fixture = load_fixture(path);
        let exclude = fixture_exclude(&fixture);
        let step_exclude = fixture_step_exclude(&fixture);
        assert!(
            !exclude.is_empty(),
            "fixture {} must have compare.exclude_fields per bead spec",
            path.display()
        );

        let encounter = fixture["metadata"]["encounter"]
            .as_str()
            .unwrap_or_else(|| panic!("fixture {} missing metadata.encounter", path.display()))
            .to_string();
        let seed = u64::try_from(
            fixture["metadata"]["seed"]
                .as_i64()
                .unwrap_or_else(|| panic!("fixture {} missing metadata.seed", path.display())),
        )
        .expect("seed fits u64");

        *by_encounter.entry(encounter.clone()).or_insert(0) += 1;

        let mut state = build_state(&fixture["initial_state"], &encounter, seed);
        let actual = combat_state_to_bridge_state(&state);
        let expected = &fixture["initial_state"];

        if let Some(diff) = pretty_diff_excluding(expected, &actual, &exclude) {
            failures.push(format!(
                "[{}] initial_state shape mismatch (exclude={exclude:?}):\n{diff}\n\nexpected: {}\n\nactual:   {}",
                path.display(),
                serde_json::to_string_pretty(expected).unwrap_or_default(),
                serde_json::to_string_pretty(&actual).unwrap_or_default(),
            ));
        }

        // sts2-rs-gvn: walk ≥10 turns, asserting per-step structural shape.
        //
        // The (retired) recorder appended 10 `end_turn` actions via the
        // `auto_end_turns: 10` spec knob of record_fixture.py. The Rust
        // analogue is `apply(state, Action::EndTurn)` which routes
        // through `combat_loop::end_turn_dispatch` → the 6-step chain
        // (end_player_turn_phase_{one,two} → enemy_turn_{start,exec,end}
        // → start_player_turn). DSL monsters drive the
        // `enemy_turn_exec` Step 4 via the wiring shipped by
        // sts2-rs-iod (`MonsterAi.roll_move_with_combat` +
        // `on_move_performed`).
        //
        // Per-step shape lock — Python and Rust hit game-over at
        // different rounds because `intent_damage` rolls disagree
        // across MT (Python) and Xoshiro256++ (Rust). Anything
        // downstream of a damage roll (player.hp/block/powers,
        // enemy.hp/block/intent/powers, /round once frozen by
        // is_over, player.energy/max_energy frozen post-game-over)
        // is in the `step_exclude_fields` set. The remaining locked
        // shape:
        //   * top-level `type` ("combat_action") and key set
        //   * `enemies/0/id` (monster_id) and `enemies/0/max_hp`
        //   * three pile counts (deck-empty fixtures → all 0)
        //   * `hand` (empty for these specs)
        //   * `player/max_hp` (80, never decreases)
        let steps = fixture["steps"]
            .as_array()
            .unwrap_or_else(|| panic!("fixture {} missing steps array", path.display()));
        assert!(
            steps.len() >= MIN_TURNS,
            "fixture {} has only {} steps; bead sts2-rs-gvn requires ≥{MIN_TURNS}",
            path.display(),
            steps.len(),
        );
        for (i, step) in steps.iter().enumerate() {
            apply(&mut state, Action::EndTurn);
            // Resolve any monster-raised choice (KnowledgeDemon curse) the
            // same way real play / the parity sweep does, so the walk
            // advances and the snapshot is taken post-resolution.
            drain_choices_index0(&mut state);
            let actual_step = combat_state_to_bridge_state(&state);
            let expected_step = &step["resulting_state"];
            if let Some(diff) = pretty_diff_excluding(expected_step, &actual_step, &step_exclude) {
                failures.push(format!(
                    "[{}] step[{i}] shape mismatch (step_exclude={step_exclude:?}):\n{diff}",
                    path.display(),
                ));
                // First step mismatch per fixture is enough signal; bail
                // out so the failure log stays readable.
                break;
            }
        }
    }

    // Coverage gate — bead spec requires ≥5 seeds × ≥5 monsters = ≥25.
    assert!(
        by_encounter.len() >= 5,
        "P5-8 coverage: expected ≥5 distinct encounters, got {} ({:?})",
        by_encounter.len(),
        by_encounter.keys().collect::<Vec<_>>(),
    );
    for (enc, n) in &by_encounter {
        assert!(
            *n >= 5,
            "P5-8 coverage: encounter {enc} has only {n} seeds (need ≥5)"
        );
    }

    assert!(
        failures.is_empty(),
        "P5-8 multi-enemy parity: {}/{} fixtures failed shape match\n\n{}",
        failures.len(),
        fixtures.len(),
        failures.join("\n\n----\n\n"),
    );

    eprintln!(
        "P5-8 multi-enemy parity: {} fixtures shape-locked across {} encounters",
        fixtures.len(),
        by_encounter.len()
    );
}

// ---------------------------------------------------------------------------
// P5-8 random-playout fuzzer extension — every DSL ctor
// ---------------------------------------------------------------------------
//
// The existing `random_playout_fuzz.rs` (P4-4 / sts2-rs-x02) exercises
// `apply(Action::EndTurn)` against the 5 `MonsterKind` variants. At this
// pin the new MonsterAi DSL ctors are not yet wired into
// `combat_loop::enemy_turn_exec` (that's a Phase-7/8 follow-up); so the
// "fuzz every monster ctor" requirement is met by driving each ctor's
// state machine directly through `roll_move(seed)` for many turns and
// asserting:
//   * every roll resolves to a valid `MoveState` (non-empty `intents`),
//   * no panic / infinite loop in `resolve_to_move_*` (safety counter
//     in `monster_ai.rs:248-251` would catch unboundedly recursive
//     state graphs),
//   * total turns across all ctors ≥ 1000 (bead acceptance criterion).
//
// `roll_move` is the workhorse that consumes RNG bits and walks
// `RandomBranch` / `ConditionalBranch` / `MoveState` graphs — it's the
// regression surface this fuzzer guards against P6/P7 churn.

use sts2_content::monsters::{
    act1_overgrowth::{self as ct_act1, InkletSlot, KinFollowerSlot},
    act1_underdocks::{self as ct_act4, PhantasmalGardenerSlot, ToadpoleSlot, TwoTailedRatSlot},
    act1_weak,
    act2_hive::{self as ct_act2, ExoskeletonSlot, InfestedPrismSlot},
    act3_glory as ct_act3,
    shared::{self as ct_shared, DenseVegetationWrigglerSlot},
};

fn registry() -> Vec<(&'static str, MonsterAi)> {
    vec![
        ("cubex_construct", ct_act1::create_cubex_construct_ai()),
        ("flyconid", ct_act1::create_flyconid_ai()),
        ("eye_with_teeth", ct_act1::create_eye_with_teeth_ai()),
        ("fogmog", ct_act1::create_fogmog_ai()),
        ("inklet_first", ct_act1::create_inklet_ai(InkletSlot::First)),
        (
            "inklet_second",
            ct_act1::create_inklet_ai(InkletSlot::Second),
        ),
        ("inklet_other", ct_act1::create_inklet_ai(InkletSlot::Other)),
        ("mawler", ct_act1::create_mawler_ai()),
        ("vine_shambler", ct_act1::create_vine_shambler_ai()),
        (
            "slithering_strangler",
            ct_act1::create_slithering_strangler_ai(),
        ),
        ("snapping_jaxfruit", ct_act1::create_snapping_jaxfruit_ai()),
        (
            "assassin_ruby_raider",
            ct_act1::create_assassin_ruby_raider_ai(),
        ),
        ("axe_ruby_raider", ct_act1::create_axe_ruby_raider_ai()),
        ("brute_ruby_raider", ct_act1::create_brute_ruby_raider_ai()),
        (
            "crossbow_ruby_raider",
            ct_act1::create_crossbow_ruby_raider_ai(),
        ),
        (
            "tracker_ruby_raider",
            ct_act1::create_tracker_ruby_raider_ai(),
        ),
        ("bygone_effigy", ct_act1::create_bygone_effigy_ai()),
        ("byrdonis", ct_act1::create_byrdonis_ai()),
        ("phrog_parasite", ct_act1::create_phrog_parasite_ai()),
        ("parafright", ct_act1::create_parafright_ai()),
        ("vantom", ct_act1::create_vantom_ai()),
        ("ceremonial_beast", ct_act1::create_ceremonial_beast_ai()),
        ("kin_priest", ct_act1::create_kin_priest_ai()),
        (
            "kin_follower_first",
            ct_act1::create_kin_follower_ai(KinFollowerSlot::First),
        ),
        (
            "kin_follower_second",
            ct_act1::create_kin_follower_ai(KinFollowerSlot::Second),
        ),
        (
            "kin_follower_third",
            ct_act1::create_kin_follower_ai(KinFollowerSlot::Third),
        ),
        ("shrinker_beetle", act1_weak::create_shrinker_beetle_ai()),
        (
            "fuzzy_wurm_crawler",
            act1_weak::create_fuzzy_wurm_crawler_ai(),
        ),
        (
            "nibbit_alone_front",
            act1_weak::create_nibbit_ai(true, true),
        ),
        (
            "nibbit_pack_front",
            act1_weak::create_nibbit_ai(false, true),
        ),
        ("leaf_slime_s", act1_weak::create_leaf_slime_s_ai()),
        ("twig_slime_s", act1_weak::create_twig_slime_s_ai()),
        ("leaf_slime_m", act1_weak::create_leaf_slime_m_ai()),
        ("twig_slime_m", act1_weak::create_twig_slime_m_ai()),
        ("thieving_hopper", ct_act2::create_thieving_hopper_ai()),
        ("tunneler", ct_act2::create_tunneler_ai()),
        ("bowlbug_egg", ct_act2::create_bowlbug_egg_ai()),
        ("bowlbug_nectar", ct_act2::create_bowlbug_nectar_ai()),
        ("bowlbug_rock", ct_act2::create_bowlbug_rock_ai()),
        ("bowlbug_silk", ct_act2::create_bowlbug_silk_ai()),
        (
            "exoskeleton_first",
            ct_act2::create_exoskeleton_ai(ExoskeletonSlot::First),
        ),
        ("chomper_scream", ct_act2::create_chomper_ai(true)),
        ("chomper_attack", ct_act2::create_chomper_ai(false)),
        ("hunter_killer", ct_act2::create_hunter_killer_ai()),
        ("wriggler", ct_act2::create_wriggler_ai()),
        ("louse_progenitor", ct_act2::create_louse_progenitor_ai()),
        ("myte", ct_act2::create_myte_ai(ct_act2::MyteSlot::First)),
        ("tough_egg", ct_act2::create_tough_egg_ai()),
        ("ovicopter", ct_act2::create_ovicopter_ai()),
        ("slumbering_beetle", ct_act2::create_slumbering_beetle_ai()),
        ("spiny_toad", ct_act2::create_spiny_toad_ai()),
        ("the_obscura", ct_act2::create_the_obscura_ai()),
        (
            "decimillipede_segment_0",
            ct_act2::create_decimillipede_segment_ai(0),
        ),
        (
            "decimillipede_segment_front_0",
            ct_act2::create_decimillipede_segment_front_ai(0),
        ),
        (
            "decimillipede_segment_middle_0",
            ct_act2::create_decimillipede_segment_middle_ai(0),
        ),
        (
            "decimillipede_segment_back_0",
            ct_act2::create_decimillipede_segment_back_ai(0),
        ),
        ("entomancer", ct_act2::create_entomancer_ai()),
        (
            "infested_prism_first",
            ct_act2::create_infested_prism_ai(InfestedPrismSlot::First),
        ),
        ("the_insatiable", ct_act2::create_the_insatiable_ai()),
        ("knowledge_demon", ct_act2::create_knowledge_demon_ai()),
        ("crusher", ct_act2::create_crusher_ai()),
        ("rocket", ct_act2::create_rocket_ai()),
        ("devoted_sculptor", ct_act3::create_devoted_sculptor_ai()),
        ("scroll_of_biting", ct_act3::create_scroll_of_biting_ai(0)),
        ("turret_operator", ct_act3::create_turret_operator_ai()),
        ("axebot_bootup", ct_act3::create_axebot_ai(true)),
        ("axebot_nobootup", ct_act3::create_axebot_ai(false)),
        ("zapbot", ct_act3::create_zapbot_ai()),
        ("stabbot", ct_act3::create_stabbot_ai()),
        ("guardbot", ct_act3::create_guardbot_ai()),
        ("noisebot", ct_act3::create_noisebot_ai()),
        ("fabricator", ct_act3::create_fabricator_ai()),
        ("frog_knight", ct_act3::create_frog_knight_ai()),
        ("globe_head", ct_act3::create_globe_head_ai()),
        ("owl_magistrate", ct_act3::create_owl_magistrate_ai()),
        ("slimed_berserker", ct_act3::create_slimed_berserker_ai()),
        ("the_lost", ct_act3::create_the_lost_ai()),
        ("the_forgotten", ct_act3::create_the_forgotten_ai()),
        ("flail_knight", ct_act3::create_flail_knight_ai()),
        (
            "act3_mysterious_knight",
            ct_act3::create_mysterious_knight_ai(),
        ),
        ("living_shield", ct_act3::create_living_shield_ai()),
        ("magi_knight", ct_act3::create_magi_knight_ai()),
        ("spectral_knight", ct_act3::create_spectral_knight_ai()),
        ("mecha_knight", ct_act3::create_mecha_knight_ai()),
        ("osty", ct_act3::create_osty_ai()),
        ("soul_nexus", ct_act3::create_soul_nexus_ai()),
        // Aeonglass replaced Doormaker in v0.107.1.
        ("aeonglass", ct_act3::create_aeonglass_ai()),
        ("queen", ct_act3::create_queen_ai()),
        ("test_subject", ct_act3::create_test_subject_ai()),
        ("corpse_slug_0", ct_act4::create_corpse_slug_ai(0)),
        ("seapunk", ct_act4::create_seapunk_ai()),
        ("sludge_spinner", ct_act4::create_sludge_spinner_ai()),
        (
            "toadpole_first",
            ct_act4::create_toadpole_ai(ToadpoleSlot::First),
        ),
        ("calcified_cultist", ct_act4::create_calcified_cultist_ai()),
        ("damp_cultist", ct_act4::create_damp_cultist_ai()),
        ("fossil_stalker", ct_act4::create_fossil_stalker_ai()),
        ("sneaky_gremlin", ct_act4::create_sneaky_gremlin_ai()),
        ("fat_gremlin", ct_act4::create_fat_gremlin_ai()),
        ("gremlin_merc", ct_act4::create_gremlin_merc_ai()),
        ("haunted_ship", ct_act4::create_haunted_ship_ai()),
        ("gas_bomb", ct_act4::create_gas_bomb_ai()),
        ("living_fog", ct_act4::create_living_fog_ai()),
        (
            "punch_construct_strong",
            ct_act4::create_punch_construct_ai(true),
        ),
        (
            "punch_construct_weak",
            ct_act4::create_punch_construct_ai(false),
        ),
        ("sewer_clam", ct_act4::create_sewer_clam_ai()),
        (
            "two_tailed_rat_first",
            ct_act4::create_two_tailed_rat_ai(TwoTailedRatSlot::First),
        ),
        (
            "phantasmal_gardener",
            ct_act4::create_phantasmal_gardener_ai(PhantasmalGardenerSlot::First),
        ),
        ("skulking_colony", ct_act4::create_skulking_colony_ai()),
        ("terror_eel", ct_act4::create_terror_eel_ai()),
        ("waterfall_giant", ct_act4::create_waterfall_giant_ai()),
        ("soul_fysh", ct_act4::create_soul_fysh_ai()),
        (
            "lagavulin_matriarch",
            ct_act4::create_lagavulin_matriarch_ai(),
        ),
        ("architect", ct_shared::create_architect_ai()),
        ("byrdpip", ct_shared::create_byrdpip_ai()),
        ("paels_legion", ct_shared::create_paels_legion_ai()),
        (
            "fake_merchant_monster",
            ct_shared::create_fake_merchant_monster_ai(),
        ),
        (
            "shared_mysterious_knight",
            ct_shared::create_mysterious_knight_ai(),
        ),
        (
            "dvw_first",
            ct_shared::create_dense_vegetation_wriggler_ai(DenseVegetationWrigglerSlot::First),
        ),
        (
            "torch_head_amalgam",
            ct_shared::create_torch_head_amalgam_ai(),
        ),
        (
            "the_adversary_mk_one",
            ct_shared::create_the_adversary_mk_one_ai(),
        ),
        (
            "the_adversary_mk_two",
            ct_shared::create_the_adversary_mk_two_ai(),
        ),
        (
            "the_adversary_mk_three",
            ct_shared::create_the_adversary_mk_three_ai(),
        ),
        ("battle_friend_v1", ct_shared::create_battle_friend_v1_ai()),
        ("battle_friend_v2", ct_shared::create_battle_friend_v2_ai()),
        ("battle_friend_v3", ct_shared::create_battle_friend_v3_ai()),
        ("big_dummy", ct_shared::create_big_dummy_ai()),
        ("one_hp_monster", ct_shared::create_one_hp_monster_ai()),
        ("ten_hp_monster", ct_shared::create_ten_hp_monster_ai()),
        (
            "single_attack_move_monster",
            ct_shared::create_single_attack_move_monster_ai(),
        ),
        (
            "multi_attack_move_monster",
            ct_shared::create_multi_attack_move_monster_ai(),
        ),
    ]
}

/// Random-playout fuzzer for every monster ctor. Drives each AI through
/// at least `TURNS_PER_CTOR` `roll_move` cycles with mutated seeds and
/// asserts no panic / no empty-intent state. Total turns far exceeds
/// the bead's ≥1000-turn floor.
#[test]
fn fuzz_every_dsl_ctor_no_panic() {
    const TURNS_PER_CTOR: u32 = 50;
    let ctors = registry();
    let n_ctors = ctors.len();
    assert!(
        n_ctors >= 100,
        "fuzz expects ≥100 ctors in registry; got {n_ctors}"
    );

    let mut total_turns: u64 = 0;
    // Some ctors (e.g. KnowledgeDemon, Lagavulin's sleep-check) use
    // ConditionalBranch nodes that require CombatState context. We use
    // a default state so the condition functions evaluate without
    // panicking — the bead's fuzz goal is "no panic / no infinite
    // loop" on 1000+ rolls, not semantic balance.
    let mut combat = CombatState::default();
    // Give a placeholder enemy so any `combat.creatures` index access
    // in a condition function doesn't OOB. Default state ships with
    // empty `creatures`; pre-seeding one creature is defensive armor
    // for future ctors whose condition body indexes by `combat_id`.
    combat.creatures.push(Creature {
        id: CreatureUid::new(1).expect("non-zero").0,
        current_hp: 100,
        max_hp: 100,
        block: 0,
        powers: SmallVec::new(),
        species: Some(MonsterKind::Mawler),
        monster_id: MonsterId("FUZZ_PLACEHOLDER"),
        intent: None,
        intent_history: IntentHistory::default(),
        ai: None,
    });

    for (name, mut ai) in ctors {
        // First call returns the initial move (no transition); the
        // bead's "exercise every ctor" metric counts each subsequent
        // roll as a turn.
        let initial = ai.current_move();
        // An empty intent list is only legal for the handful of creatures C#
        // builds that way — `new MoveState(id, body)` with no
        // `params AbstractIntent[]`. Everywhere else it means a port dropped
        // the telegraph metadata, which is what this assertion is here to
        // catch, so the exemption is an explicit allowlist rather than a
        // relaxed predicate.
        if SILENT_INITIAL_MOVE_CTORS.contains(&name) {
            assert!(
                initial.intents.is_empty(),
                "{name}: allowlisted as intent-less but telegraphs {:?}",
                initial.intents
            );
        } else {
            assert!(
                !initial.intents.is_empty(),
                "{name}: initial MoveState has empty intents"
            );
        }
        ai.on_move_performed();

        for turn in 0..TURNS_PER_CTOR {
            // Mutate seed per turn so RandomBranch nodes actually take
            // different paths across the playout — otherwise every
            // roll for a given ctor lands on the same branch.
            let seed = u64::from(turn)
                .wrapping_mul(0xa5a5_5a5a_a5a5_5a5a)
                .wrapping_add(0x1234_5678_9abc_def0);
            // `roll_move_with_combat` covers ConditionalBranch ctors;
            // pure RandomBranch/Move ctors fall through the same path.
            let mv = ai.roll_move_with_combat_seeded(seed, &combat);
            // Same if/else shape as the initial-move check above, and for the
            // same reason: `!is_empty() || allowlisted` asserts NOTHING for an
            // allowlisted ctor, so a port that dropped a telegraph inside one
            // would sail through. An allowlisted ctor is claimed to be
            // intent-less, so hold it to that.
            if SILENT_INITIAL_MOVE_CTORS.contains(&name) {
                assert!(
                    mv.intents.is_empty(),
                    "{name} turn={turn}: allowlisted as intent-less but telegraphs {:?}",
                    mv.intents
                );
            } else {
                assert!(
                    !mv.intents.is_empty(),
                    "{name} turn={turn}: rolled MoveState has empty intents"
                );
            }
            ai.on_move_performed();
            total_turns += 1;
        }
    }

    assert!(
        total_turns >= 1000,
        "fuzz total turns {total_turns} < 1000 (bead acceptance criterion)"
    );
    eprintln!(
        "P5-8 fuzz: {n_ctors} ctors × {TURNS_PER_CTOR} turns = {total_turns} total roll_move calls, no panics"
    );
}
