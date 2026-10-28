//! sts2-rs-rggx — Acts 2+3 parity fixtures per encounter.
//!
//! Walks the eight `tests/parity/scripted/act{2,3}-*-seed0.json` shape-lock
//! fixtures (one per tier per act — weak / normal / elite / boss) recorded
//! by the since-deleted `scripts/record_fixture.py` against the Python sim
//! at pin `01c094d` (recorder retired, fixtures frozen — see
//! `PYTHON_SIM_PIN.md`). The harness mirrors `parity_multi_enemy_dsl.rs` /
//! `parity_act1_mawler.rs` in shape: builds a single-enemy Rust
//! `CombatState` whose `monster_id` / HP / block / powers track the
//! recorded initial state, then drives `apply(state, Action::EndTurn)`
//! for the recorded `auto_end_turns` count.
//!
//! Per-step shape lock — Python's Mersenne-Twister and Rust's
//! Xoshiro256++ produce different intent rolls for the same seed (see
//! `crates/sts2-core/src/monsters.rs:22-29`), so damage-derived fields
//! (HP / block / powers / energy) and post-game-over freeze fields
//! (/round) are in `compare.step_exclude_fields`. The fields that
//! REMAIN locked across the 10-turn walk: top-level `type` tag + key
//! set, `enemies/0/id`, `enemies/0/max_hp`, the three pile counts,
//! `hand` (empty for these specs), and `player/max_hp`.
//!
//! Parent bead: `sts2-rs-6ynt.10` (Acts 2+3 MonsterKind + dispatch +
//! encounter tables). The encounter pick mirrors the per-tier roster
//! published in `crates/sts2-content/src/encounters/{act2,act3}.rs`:
//!
//! | Act | Weak             | Normal       | Elite         | Boss         |
//! |-----|------------------|--------------|---------------|--------------|
//! |  2  | `ThievingHopper` | `TheObscura` | `Entomancer`  | `TheInsatiable` |
//! |  3  | `DevotedSculptor`| `GlobeHead`  | `MechaKnight` | `Queen`      |
//!
//! All eight are single-enemy encounters (no `MonsterSpawnArgs` payload)
//! so the recorder's `builder: "module:function"` slot can resolve
//! without kwargs. Multi-enemy variants (Exoskeletons / Chompers /
//! Bowlbugs / Decimillipede / Knights) are intentionally deferred to
//! a follow-up bead — they need the `enemies[i].builder_kwargs` path
//! the recorder doesn't yet support.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use smallvec::SmallVec;

use sts2_content::monsters::{
    act2_hive::{
        create_entomancer_ai, create_the_insatiable_ai, create_the_obscura_ai,
        create_thieving_hopper_ai,
    },
    act3_glory::{
        create_devoted_sculptor_ai, create_globe_head_ai, create_mecha_knight_ai, create_queen_ai,
    },
};
use sts2_core::card_effects::apply_power_to_creature;
use sts2_core::monster_ai::MonsterAi;
use sts2_core::{
    apply, parity::combat_state_to_bridge_state, Action, CombatPhase, CombatState, Creature,
    CreatureUid, IntentHistory, MonsterId, MonsterKind, PlayerCombatState, PowerKind, Side,
    TurnEvents,
};

// ── Pretty diff (mirror of parity_multi_enemy_dsl.rs) ───────────────────

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

/// Encounters wired by this harness (sts2-rs-69u4 partial). Other
/// `act2-` / `act3-` fixtures (e.g. the rggx Tunneler setup-only
/// fixture, or follow-up multi-creature / power-on-spawn fixtures
/// not yet supported) are skipped by name prefix.
const WIRED_ENCOUNTER_PREFIXES: &[&str] = &[
    "act2-entomancer-",
    "act2-the-insatiable-",
    "act2-the-obscura-",
    "act2-thieving-hopper-",
    "act3-devoted-sculptor-",
    "act3-globe-head-",
    "act3-mecha-knight-",
    "act3-queen-",
];

fn collect_act23_fixtures() -> Vec<PathBuf> {
    let dir = scripted_dir();
    if !dir.exists() {
        return Vec::new();
    }
    let mut out: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                return false;
            }
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                return false;
            };
            WIRED_ENCOUNTER_PREFIXES
                .iter()
                .any(|pfx| name.starts_with(pfx))
        })
        .collect();
    out.sort();
    out
}

// ── Encounter → (monster_id_str, AI) registry ───────────────────────────

fn build_ai_for_encounter(encounter: &str) -> (&'static str, MonsterAi) {
    // sts2-rs-69u4 partial: 4 single-creature, no-start-of-combat-power
    // encounters. sts2-rs-lwkz (this bead) adds ThievingHopper, which
    // applies THIEVERY x15 on spawn — wired below via `apply_start_of_combat_powers`.
    // The multi-creature spawn-on-start encounters (Entomancer summons
    // insect tokens; TheInsatiable spawns a minion stack; Queen pairs
    // with TorchHeadAmalgam per C# `Queen.cs`) are deferred to a
    // follow-up bead since they need a multi-Creature initial-state path.
    // (The "Queen spawns RoyalGuard" claim was a Python-pin fabrication
    // removed by `sts2-rs-2u9e2`; C# Queen has no summon.)
    match encounter {
        "TheObscura" => ("THE_OBSCURA", create_the_obscura_ai()),
        "DevotedSculptor" => ("DEVOTED_SCULPTOR", create_devoted_sculptor_ai()),
        "GlobeHead" => ("GLOBE_HEAD", create_globe_head_ai()),
        "MechaKnight" => ("MECHA_KNIGHT", create_mecha_knight_ai()),
        "ThievingHopper" => ("THIEVING_HOPPER", create_thieving_hopper_ai()),
        // sts2-rs-mmqj: Entomancer carries PERSONAL_HIVE x1 on spawn.
        // TheInsatiable and Queen spawn minions during step execution
        // (not at initial state, so build_state stays single-creature).
        "Entomancer" => ("ENTOMANCER", create_entomancer_ai()),
        "TheInsatiable" => ("THE_INSATIABLE", create_the_insatiable_ai()),
        "Queen" => ("QUEEN", create_queen_ai()),
        other => panic!("sts2-rs-69u4 harness: unknown / deferred encounter {other:?}"),
    }
}

/// Apply per-encounter start-of-combat power stacks that the Python
/// pin attaches in the monster's on-spawn hook. Mirrors the
/// `apply_power` calls in `sts2_env/monsters/{act2,act4}.py`.
/// Bead `sts2-rs-lwkz`.
fn apply_start_of_combat_powers(state: &mut CombatState, enemy_uid: CreatureUid, encounter: &str) {
    if encounter == "ThievingHopper" {
        // Port-pin: sts2-rl-agent@01c094d:sts2_env/monsters/act2.py:L60
        apply_power_to_creature(state, enemy_uid, PowerKind::Thievery, 15);
    }
    if encounter == "Entomancer" {
        // Port-pin: sts2-rl-agent@01c094d:sts2_env/monsters/act2.py
        // (Entomancer ctor seeds PERSONAL_HIVE x1 at spawn).
        apply_power_to_creature(state, enemy_uid, PowerKind::PersonalHive, 1);
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
        "rggx harness: fixture monster_id {monster_id_str:?} \
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
    let monster_id = MonsterId(expected_id);
    let creature = Creature {
        id: enemy_uid.0,
        current_hp: hp,
        max_hp,
        block,
        powers: SmallVec::new(),
        // STS2-native placeholder species; the bridge-state exporter
        // reads `monster_id` (not `species`) for the JSON `id` field,
        // so the discriminant is irrelevant to the parity diff (same
        // workaround as `parity_multi_enemy_dsl.rs`).
        species: Some(MonsterKind::Mawler),
        monster_id,
        intent: Some(initial_intent),
        intent_history: IntentHistory::default(),
        ai: Some(ai),
    };

    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(enemy_uid);

    let mut state = CombatState {
        belt_value: None,
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
    };
    apply_start_of_combat_powers(&mut state, enemy_uid, encounter);
    state
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

/// Bead acceptance: each fixture walks ≥ `MIN_TURNS` turns (same as the
/// dsl-multi family).
const MIN_TURNS: usize = 10;

/// Bead acceptance: ≥ 8 fixtures. sts2-rs-69u4 (4) + sts2-rs-lwkz (1
/// ThievingHopper) + sts2-rs-mmqj (3 — Entomancer with PERSONAL_HIVE on
/// spawn, TheInsatiable, Queen — minion spawn happens during step
/// execution so build_state stays single-creature, with /enemies/1/*
/// dynamic fields step-excluded per spec).
const MIN_FIXTURES: usize = 8;

#[test]
fn act23_per_tier_fixtures_shape_lock() {
    let fixtures = collect_act23_fixtures();
    assert!(
        fixtures.len() >= MIN_FIXTURES,
        "sts2-rs-rggx expected ≥{MIN_FIXTURES} act2-/act3- fixtures under {}; found {}",
        scripted_dir().display(),
        fixtures.len(),
    );

    // Coverage gate — 4 tiers × 2 acts = 8 distinct encounters.
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

        let steps = fixture["steps"]
            .as_array()
            .unwrap_or_else(|| panic!("fixture {} missing steps array", path.display()));
        assert!(
            steps.len() >= MIN_TURNS,
            "fixture {} has only {} steps; bead sts2-rs-rggx requires ≥{MIN_TURNS}",
            path.display(),
            steps.len(),
        );
        for (i, step) in steps.iter().enumerate() {
            apply(&mut state, Action::EndTurn);
            let actual_step = combat_state_to_bridge_state(&state);
            let expected_step = &step["resulting_state"];
            if let Some(diff) = pretty_diff_excluding(expected_step, &actual_step, &step_exclude) {
                failures.push(format!(
                    "[{}] step[{i}] shape mismatch (step_exclude={step_exclude:?}):\n{diff}",
                    path.display(),
                ));
                // First step mismatch per fixture is enough signal.
                break;
            }
        }
    }

    // Coverage gate — 4 tiers × 2 acts = 8 distinct encounters.
    assert!(
        by_encounter.len() >= MIN_FIXTURES,
        "sts2-rs-rggx coverage: expected ≥{MIN_FIXTURES} distinct encounters, got {} ({:?})",
        by_encounter.len(),
        by_encounter.keys().collect::<Vec<_>>(),
    );

    assert!(
        failures.is_empty(),
        "sts2-rs-rggx — {} fixture shape mismatch(es):\n\n{}",
        failures.len(),
        failures.join("\n\n"),
    );
}
