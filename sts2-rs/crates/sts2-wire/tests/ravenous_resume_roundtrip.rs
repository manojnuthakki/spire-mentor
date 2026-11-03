//! The engine's own wire must retain an existing Slug forced follow-up.
//! This does not claim exact reconstruction from a native Stun observation
//! whose interrupted move is absent, or change the agent-facing schema.
use sts2_content::monsters::act1_underdocks::create_corpse_slug_ai;
use sts2_core::combat_loop::{enemy_turn_end, enemy_turn_exec};
use sts2_core::{CombatPhase, CombatState, MonsterKind, Side, StepResult};

fn roundtrip(original: &'static str, performed_stun: bool) {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 5000;
    s.player.max_hp = 5000;
    s.init_player_creature();
    let uid = s.spawn_enemy(MonsterKind::CorpseSlug);
    let mut ai = create_corpse_slug_ai(0);
    ai.force_current_state(original);
    ai.on_move_performed();
    ai.force_current_state("STUNNED");
    ai.force_next_state(original);
    if performed_stun {
        ai.on_move_performed();
    }
    let intent = ai.current_move().intents.first().copied();
    s.set_creature_ai(uid, ai);
    s.creatures
        .iter_mut()
        .find(|c| c.id == uid.0)
        .unwrap()
        .intent = intent;
    let before = sts2_wire::emit::combat_state_to_value(&s).expect("emits");
    assert_eq!(before, sts2_wire::emit::combat_state_to_value(&s).unwrap());
    let mut restored = sts2_wire::state_from_json_str(&before.to_string()).expect("roundtrip");
    assert_eq!(
        before["encounter"][0]["ravenous_resume"],
        sts2_wire::emit::combat_state_to_value(&restored).unwrap()["encounter"][0]
            ["ravenous_resume"]
    );
    let mut clone = s.clone();
    for state in [&mut s, &mut clone, &mut restored] {
        state.side = Side::Enemy;
        state.phase = if performed_stun {
            CombatPhase::EnemyTurnEnd
        } else {
            CombatPhase::EnemyTurnExec
        };
        if !performed_stun {
            assert_eq!(enemy_turn_exec(state), StepResult::Advanced);
        }
        assert_eq!(enemy_turn_end(state), StepResult::Advanced);
        let current = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .unwrap()
            .ai
            .as_ref()
            .unwrap()
            .current_state_id();
        assert_eq!(current, original);
        assert_eq!(state.player.current_hp, 5000);
    }
}

#[test]
fn unperformed_glomp_resume_survives_engine_wire_and_clone() {
    roundtrip("GLOMP", false);
}

#[test]
fn performed_goop_resume_and_completion_survive_engine_wire_and_clone() {
    roundtrip("GOOP", true);
}

fn stunned_wire() -> serde_json::Value {
    let mut s = CombatState::default();
    s.init_player_creature();
    let uid = s.spawn_enemy(MonsterKind::CorpseSlug);
    let mut ai = create_corpse_slug_ai(1);
    ai.force_current_state("STUNNED");
    ai.force_next_state("GLOMP");
    s.set_creature_ai(uid, ai);
    s.creatures
        .iter_mut()
        .find(|c| c.id == uid.0)
        .unwrap()
        .intent = Some(sts2_core::Intent::Stun);
    sts2_wire::emit::combat_state_to_value(&s).unwrap()
}

#[test]
fn undisclosed_native_stun_import_does_not_fabricate_an_exact_resume() {
    let mut wire = stunned_wire();
    wire["encounter"][0]
        .as_object_mut()
        .unwrap()
        .remove("ravenous_resume");
    // Existing native imports remain approximate, including strict goldens.
    // Missing information must not become an advertised exact engine queue.
    let state = sts2_wire::state_from_json(&wire).unwrap();
    assert_eq!(
        state
            .creatures
            .iter()
            .find_map(|c| c.ai.as_ref())
            .unwrap()
            .forced_next_state(),
        None
    );
    assert!(
        sts2_wire::emit::combat_state_to_value(&state).unwrap()["encounter"][0]
            .get("ravenous_resume")
            .is_none()
    );
}

#[test]
fn stale_or_invalid_private_resume_is_rejected() {
    let good = stunned_wire();
    for (key, value) in [
        ("resume", serde_json::json!("STUNNED")),
        ("resume", serde_json::Value::Null),
        ("performed_first", serde_json::json!(false)),
        ("performed_current", serde_json::json!(1)),
    ] {
        let mut wire = good.clone();
        wire["encounter"][0]["ravenous_resume"][key] = value;
        assert!(sts2_wire::state_from_json(&wire).is_err(), "{key}");
    }
    let mut wire = good;
    wire["encounter"][0]["move_state"] = serde_json::json!("GLOMP");
    assert!(sts2_wire::state_from_json(&wire).is_err());
}
