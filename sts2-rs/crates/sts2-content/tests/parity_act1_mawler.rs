//! sts2-rs-t557 — Phase 5 follow-up: the 1 representative Act-1 parity
//! fixture called out by plan §Verification step 6.
//!
//! The Act-1 wiring umbrella (`sts2-rs-7x18` / Phase 5) landed the
//! integration smoke test (`act1_smoke.rs`: 29 monsters × 22 encounters,
//! random play, no panics). The smoke test guards against panics /
//! invalid actions but does NOT diff against the Python sim. This file
//! adds the one representative cross-language parity fixture that
//! plan-doc §Verification step 6 explicitly required.
//!
//! ## Choice of encounter — Mawler
//!
//! Mawler is a single-monster Act-1 fight with deterministic HP (72,
//! no rng-roll) and a `RAND(RIP_AND_TEAR | ROAR | CLAW)` selector that
//! exercises both `RandomBranchState` and `MoveRepeatType` enforcement
//! (`monsters/act1.py:223-250` ↔ `monsters/act1.rs:269-…`). Because the
//! random branch consumes RNG bits, the realised intent diverges
//! across Python MT and Rust Xoshiro256++ — so the structural shape
//! lock here is identical in spirit to the existing
//! `dsl-multi-cubex-construct` fixture family (sts2-rs-gvn): top-level
//! `type` tag + keys, `enemies/0/id`, `enemies/0/max_hp`, the three
//! pile counts, `hand`, and `player/max_hp` stay byte-locked across
//! the 10-turn walk.
//!
//! Mirrors `parity_multi_enemy_dsl.rs::dsl_multi_enemy_fixtures_shape_lock`
//! in shape but loads exactly the one `act1-mawler-seed0.json` fixture
//! (the bead explicitly scopes to 1 fixture; 5-seed coverage for
//! Mawler is a downstream follow-up).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use smallvec::SmallVec;

use sts2_content::monsters::act1_overgrowth::create_mawler_ai;
use sts2_core::{
    apply, parity::combat_state_to_bridge_state, Action, CombatPhase, CombatState, Creature,
    CreatureUid, IntentHistory, MonsterId, MonsterKind, PlayerCombatState, Side, TurnEvents,
};

// Pretty-diff helper — identical to the one in parity_multi_enemy_dsl.rs.
// Kept inlined (not exported) to avoid coupling the two harnesses.
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

fn fixture_path() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(|root| {
            root.join("tests")
                .join("parity")
                .join("scripted")
                .join("act1-mawler-seed0.json")
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

fn build_state(fixture_initial: &Value, seed: u64) -> CombatState {
    let enemy_json = &fixture_initial["enemies"][0];
    let max_hp = i32::try_from(enemy_json["max_hp"].as_i64().expect("fixture enemy max_hp"))
        .expect("max_hp fits i32");
    let hp =
        i32::try_from(enemy_json["hp"].as_i64().expect("fixture enemy hp")).expect("hp fits i32");
    let block = i32::try_from(enemy_json["block"].as_i64().unwrap_or(0)).expect("block fits i32");

    let ai = create_mawler_ai();
    let initial_intent = ai
        .current_move()
        .intents
        .first()
        .copied()
        .expect("Mawler's initial MoveState must carry ≥1 intent");

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
        // sts2-rs-vvz0: STS2-native `Mawler` species; the bridge
        // exporter reads `monster_id` ("MAWLER" below) for the JSON
        // `id` field, so `species` is independent of the exported id.
        species: Some(MonsterKind::Mawler),
        monster_id: MonsterId("MAWLER"),
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

const MIN_TURNS: usize = 10;

#[test]
fn act1_mawler_seed0_shape_lock() {
    let path = fixture_path();
    let fixture = load_fixture(&path);

    let exclude = fixture_exclude(&fixture);
    let step_exclude = fixture_step_exclude(&fixture);
    assert!(
        !exclude.is_empty(),
        "fixture {} must declare compare.exclude_fields",
        path.display()
    );

    let encounter = fixture["metadata"]["encounter"]
        .as_str()
        .expect("fixture must declare metadata.encounter");
    assert_eq!(encounter, "Mawler");

    let seed = u64::try_from(
        fixture["metadata"]["seed"]
            .as_i64()
            .expect("fixture must declare metadata.seed"),
    )
    .expect("seed fits u64");

    let mut state = build_state(&fixture["initial_state"], seed);

    // Initial-state shape lock.
    let actual = combat_state_to_bridge_state(&state);
    let expected = &fixture["initial_state"];
    if let Some(diff) = pretty_diff_excluding(expected, &actual, &exclude) {
        panic!(
            "[{}] initial_state shape mismatch (exclude={exclude:?}):\n{diff}\n\n\
             expected: {}\n\nactual:   {}",
            path.display(),
            serde_json::to_string_pretty(expected).unwrap_or_default(),
            serde_json::to_string_pretty(&actual).unwrap_or_default(),
        );
    }

    // 10-turn walk — per-step structural shape.
    let steps = fixture["steps"]
        .as_array()
        .expect("fixture must carry a steps array");
    assert!(
        steps.len() >= MIN_TURNS,
        "fixture {} has only {} steps; sts2-rs-t557 mirrors sts2-rs-gvn's ≥{MIN_TURNS}",
        path.display(),
        steps.len(),
    );
    for (i, step) in steps.iter().enumerate() {
        apply(&mut state, Action::EndTurn);
        let actual_step = combat_state_to_bridge_state(&state);
        let expected_step = &step["resulting_state"];
        if let Some(diff) = pretty_diff_excluding(expected_step, &actual_step, &step_exclude) {
            panic!(
                "[{}] step[{i}] shape mismatch (step_exclude={step_exclude:?}):\n{diff}",
                path.display(),
            );
        }
    }
}
