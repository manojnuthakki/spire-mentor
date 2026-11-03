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
    let mut clone = s.clone();
    for state in [&mut s, &mut clone, &mut restored] {
        state.side = Side::Enemy;
        state.phase = CombatPhase::EnemyTurnExec;
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
