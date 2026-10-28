//! sts2-rs-r0w5n — jdlfq.2.replacement: act1 Bygone Effigy parity
//! fixture. Replaces sts2-rs-fgzcz (STS1 "jaw-worm" — does not
//! exist in STS2; see fgzcz close-reason + the
//! `feedback_sts1_stubs_not_sts2` auto-memory).
//!
//! Sibling to `parity_act1_byrdonis.rs` (sts2-rs-may54, simple-AI
//! tier representative) and `parity_act1_mawler.rs` (sts2-rs-t557,
//! multi-attack tier representative).
//!
//! ## Choice of encounter — Bygone Effigy
//!
//! Bygone Effigy is a real STS2 act1 elite:
//!   - Linear state chain INITIAL_SLEEP → WAKE → SLASHES (loop).
//!   - Non-trivial intent mix: Sleep / Buff(Strength,10) / Attack(15).
//!   - Deterministic HP (127 floor, 132 asc) — no rng range.
//!   - Deck setup proven via
//!     tests/parity/blunders/floor008_bygone_effigy_endturn.toml.
//!
//! The original jdlfq.2 slot was branching defend/strike; Bygone
//! Effigy is technically stateful-sequential, but it's the cleanest
//! STS2 act1 elite that hadn't been covered yet by Mawler/Byrdonis.
//! Tier-label drift is tracked in sts2-rs-arheo (jdlfq.rescope
//! umbrella).
//!
//! ## Scope at this landing
//!
//! Setup-only — `steps: []`. The driver verifies the initial
//! bridge-state matches the fixture (with the standard intent-triple
//! exclusion). The full step-walk needs a Python-side recording
//! round-trip and lands separately as an r0w5n follow-up.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use smallvec::SmallVec;

use sts2_content::monsters::act1_overgrowth::create_bygone_effigy_ai;
use sts2_core::{
    apply, parity::combat_state_to_bridge_state, Action, CombatPhase, CombatState, Creature,
    CreatureUid, IntentHistory, MonsterId, MonsterKind, PlayerCombatState, Side, TurnEvents,
};

fn fixture_path() -> PathBuf {
    fixture_path_named("act1-bygone-effigy-setup-only-seed0.json")
}

/// r0w5n.1 step-walk fixture (bead `sts2-rs-ouojn`). Separate file
/// from the setup-only sentinel so both shape locks coexist.
fn fixture_path_step_walk() -> PathBuf {
    fixture_path_named("act1-bygone-effigy-seed0.json")
}

fn fixture_path_named(filename: &str) -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(|root| {
            root.join("tests")
                .join("parity")
                .join("scripted")
                .join(filename)
        })
        .expect("workspace root must be ancestor of CARGO_MANIFEST_DIR")
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

fn build_state(fixture_initial: &Value, seed: u64) -> CombatState {
    let enemy_json = &fixture_initial["enemies"][0];
    let max_hp = i32::try_from(enemy_json["max_hp"].as_i64().expect("fixture enemy max_hp"))
        .expect("max_hp fits i32");
    let hp =
        i32::try_from(enemy_json["hp"].as_i64().expect("fixture enemy hp")).expect("hp fits i32");
    let block = i32::try_from(enemy_json["block"].as_i64().unwrap_or(0)).expect("block fits i32");

    let ai = create_bygone_effigy_ai();
    let initial_intent = ai
        .current_move()
        .intents
        .first()
        .copied()
        .expect("Bygone Effigy's initial MoveState must carry >=1 intent");

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
        potions: SmallVec::new(),
        potion_slot_indices: SmallVec::new(),
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
        retain_this_turn: SmallVec::new(),
        sly_this_turn: SmallVec::new(),
        sly_this_combat: SmallVec::new(),
        pending_retain_count: 0,
        stars_gained_this_turn: 0,
        pinned_top: smallvec::SmallVec::new(),
        pinned_bottom: SmallVec::new(),
        deferred_draw_order: false,
    };

    let enemy_uid = CreatureUid::new(1).expect("non-zero");
    let creature = Creature {
        id: enemy_uid.0,
        current_hp: hp,
        max_hp,
        block,
        powers: SmallVec::new(),
        species: Some(MonsterKind::BygoneEffigy),
        monster_id: MonsterId("BYGONE_EFFIGY"),
        intent: Some(initial_intent),
        intent_history: IntentHistory::default(),
        ai: Some(ai),
    };

    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(enemy_uid);

    CombatState {
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
        deck_card_uids: Vec::new(),
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

/// `sts2-rs-r0w5n`: structural-shape-lock of the Bygone Effigy
/// act1 initial-state bridge export against the hand-rolled
/// `act1-bygone-effigy-setup-only-seed0.json` fixture.
#[test]
fn act1_bygone_effigy_setup_only_shape_lock() {
    let path = fixture_path();
    let fixture = load_fixture(&path);

    let exclude = fixture_exclude(&fixture);
    assert!(
        !exclude.is_empty(),
        "fixture {} must declare compare.exclude_fields",
        path.display()
    );

    let encounter = fixture["metadata"]["encounter"]
        .as_str()
        .expect("fixture must declare metadata.encounter");
    assert_eq!(encounter, "BygoneEffigy");

    let seed = u64::try_from(
        fixture["metadata"]["seed"]
            .as_i64()
            .expect("fixture must declare metadata.seed"),
    )
    .expect("seed fits u64");

    let state = build_state(&fixture["initial_state"], seed);
    let bridge = combat_state_to_bridge_state(&state);
    let bridge_json = serde_json::to_value(&bridge).expect("bridge state serializes");
    let expected_initial = &fixture["initial_state"];

    let fixture_keys: Vec<&str> = expected_initial
        .as_object()
        .expect("initial_state must be JSON object")
        .keys()
        .map(String::as_str)
        .collect();
    for key in &fixture_keys {
        assert!(
            bridge_json.get(key).is_some(),
            "bridge state missing top-level key `{key}` (fixture: {})",
            path.display()
        );
    }

    for (key, expected_value) in expected_initial
        .as_object()
        .expect("initial_state must be JSON object")
    {
        if exclude.iter().any(|p| p.starts_with(&format!("/{key}"))) {
            continue;
        }
        let actual = bridge_json
            .get(key)
            .unwrap_or_else(|| panic!("bridge missing key {key}"));
        compare_excluding(&format!("/{key}"), expected_value, actual, &exclude, &path);
    }
}

fn compare_excluding(path: &str, expected: &Value, actual: &Value, exclude: &[String], src: &Path) {
    if exclude.iter().any(|p| p == path) {
        return;
    }
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (k, ev) in e {
                let child = format!("{path}/{k}");
                if exclude.iter().any(|p| p == &child) {
                    continue;
                }
                let av = a.get(k).unwrap_or_else(|| {
                    panic!(
                        "{}: bridge missing key {child} (expected={ev})",
                        src.display()
                    )
                });
                compare_excluding(&child, ev, av, exclude, src);
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            assert_eq!(
                e.len(),
                a.len(),
                "{}: array length mismatch at {path} (expected={}, actual={})",
                src.display(),
                e.len(),
                a.len()
            );
            for (i, (ev, av)) in e.iter().zip(a.iter()).enumerate() {
                let child = format!("{path}/{i}");
                compare_excluding(&child, ev, av, exclude, src);
            }
        }
        _ => assert_eq!(
            expected,
            actual,
            "{}: value mismatch at {path} (expected={expected}, actual={actual})",
            src.display()
        ),
    }
}

/// Bead `sts2-rs-ouojn` (r0w5n.1): 10-turn end_turn shape-lock walk
/// over the Bygone Effigy INITIAL_SLEEP → WAKE → SLASHES state
/// chain. Mirrors the parafright (sts2-rs-r9n4w) step-walk pattern;
/// Python parity recording is the future delivery (same infra
/// blocker as ohxoq.1-4 / may54.1 / r9n4w).
#[test]
fn act1_bygone_effigy_seed0_step_walk_shape_lock() {
    let path = fixture_path_step_walk();
    let fixture = load_fixture(&path);

    let exclude = fixture_exclude(&fixture);
    let step_exclude = fixture_step_exclude(&fixture);
    assert!(
        !exclude.is_empty(),
        "fixture {} must declare compare.exclude_fields",
        path.display()
    );
    assert!(
        !step_exclude.is_empty(),
        "fixture {} must declare compare.step_exclude_fields",
        path.display()
    );

    let encounter = fixture["metadata"]["encounter"]
        .as_str()
        .expect("fixture must declare metadata.encounter");
    assert_eq!(encounter, "BygoneEffigy");

    let seed = u64::try_from(
        fixture["metadata"]["seed"]
            .as_i64()
            .expect("fixture must declare metadata.seed"),
    )
    .expect("seed fits u64");

    let mut state = build_state(&fixture["initial_state"], seed);

    // Initial-state shape lock (same exclusions as setup-only sentinel).
    let actual = combat_state_to_bridge_state(&state);
    let actual_json = serde_json::to_value(&actual).expect("bridge serializes");
    let expected_initial = &fixture["initial_state"];
    if let Some(diff) = pretty_diff_excluding(expected_initial, &actual_json, &exclude) {
        panic!(
            "[{}] initial_state shape mismatch (exclude={exclude:?}):\n{diff}",
            path.display(),
        );
    }

    // 10-turn end_turn walk.
    let steps = fixture["steps"]
        .as_array()
        .expect("fixture must carry a steps array");
    const MIN_TURNS: usize = 10;
    assert!(
        steps.len() >= MIN_TURNS,
        "fixture {} has only {} steps; ouojn mirrors r9n4w/t557's >={MIN_TURNS}",
        path.display(),
        steps.len(),
    );
    for (i, step) in steps.iter().enumerate() {
        let action_kind = step["action"]["action"]
            .as_str()
            .expect("step action must declare a kind");
        assert_eq!(
            action_kind,
            "end_turn",
            "step[{i}] in {} unexpected action kind",
            path.display()
        );
        apply(&mut state, Action::EndTurn);
        let actual_step = combat_state_to_bridge_state(&state);
        let actual_step_json = serde_json::to_value(&actual_step).expect("bridge serializes");
        let expected_step = &step["resulting_state"];
        if let Some(diff) = pretty_diff_excluding(expected_step, &actual_step_json, &step_exclude) {
            panic!(
                "[{}] step[{i}] shape mismatch (step_exclude={step_exclude:?}):\n{diff}",
                path.display(),
            );
        }
    }
}

fn fixture_step_exclude(fixture: &Value) -> Vec<String> {
    fixture
        .get("compare")
        .and_then(|c| c.get("step_exclude_fields"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Pretty-diff helper — mirrors the parafright (sts2-rs-r9n4w) +
/// mawler (sts2-rs-t557) inline copies. Kept private to this
/// translation unit (each parity driver carries its own copy by
/// convention).
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
