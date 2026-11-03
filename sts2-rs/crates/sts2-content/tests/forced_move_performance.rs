//! Fresh DLL `MoveState` admission belongs to the move that actually starts,
//! independently of a replacement forced while its body is running.
use sts2_content::monsters::{act1_overgrowth, act1_underdocks, act2_hive};
use sts2_core::combat_loop::{enemy_turn_end, enemy_turn_exec};
use sts2_core::monster_ai::MonsterAi;
use sts2_core::{CombatPhase, CombatState, CreatureUid, MonsterKind, PowerKind, Side, StepResult};

fn board(
    kind: MonsterKind,
    mut ai: MonsterAi,
    current: Option<&'static str>,
    hp: i32,
) -> (CombatState, CreatureUid) {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 5000;
    s.player.max_hp = 5000;
    s.init_player_creature();
    s.side = Side::Enemy;
    s.phase = CombatPhase::EnemyTurnExec;
    if let Some(id) = current {
        ai.force_current_state(id);
    }
    let intent = ai.current_move().intents.first().copied();
    let uid = s.spawn_enemy(kind);
    s.set_creature_ai(uid, ai);
    let c = s.creatures.iter_mut().find(|c| c.id == uid.0).unwrap();
    c.current_hp = hp;
    c.intent = intent;
    (s, uid)
}

fn power(s: &mut CombatState, uid: CreatureUid, kind: PowerKind, n: i32) {
    sts2_core::card_effects::apply_power_to_creature(s, uid, kind, n);
}

fn amount(s: &CombatState, uid: CreatureUid, kind: PowerKind) -> i32 {
    let c = s.creatures.iter().find(|c| c.id == uid.0).unwrap();
    c.powers
        .iter()
        .filter_map(|p| s.powers.get_by_id(p.0))
        .filter(|p| p.kind == kind)
        .map(|p| p.amount)
        .sum()
}

fn ai(s: &CombatState, uid: CreatureUid) -> &MonsterAi {
    s.creatures
        .iter()
        .find(|c| c.id == uid.0)
        .unwrap()
        .ai
        .as_ref()
        .unwrap()
}

fn turn(s: &mut CombatState) {
    s.phase = CombatPhase::EnemyTurnExec;
    assert_eq!(enemy_turn_exec(s), StepResult::Advanced);
    assert_eq!(enemy_turn_end(s), StepResult::Advanced);
}

#[test]
fn plow_own_move_reflection_preserves_one_stun_then_beast_cry() {
    let (mut s, beast) = board(
        MonsterKind::CeremonialBeast,
        act1_overgrowth::create_ceremonial_beast_ai(),
        Some("PLOW"),
        153,
    );
    power(&mut s, beast, PowerKind::Plow, 150);
    power(&mut s, beast, PowerKind::Strength, 8);
    let player = s.player.creature_uid.unwrap();
    power(&mut s, player, PowerKind::FlameBarrier, 4);
    turn(&mut s);
    assert_eq!(ai(&s, beast).current_state_id(), "STUN");
    assert_eq!(ai(&s, beast).move_history(), &["PLOW"]);
    assert_eq!(amount(&s, beast, PowerKind::Plow), 0);
    assert_eq!(amount(&s, beast, PowerKind::Strength), 2);
    let hp = s.player.current_hp;
    let mut cloned = s.clone();
    for state in [&mut s, &mut cloned] {
        turn(state);
        assert_eq!(state.player.current_hp, hp);
        assert_eq!(ai(state, beast).current_state_id(), "BEAST_CRY");
        assert_eq!(ai(state, beast).move_history(), &["PLOW", "STUN"]);
        turn(state);
        assert_ne!(ai(state, beast).current_state_id(), "STUN");
    }
}

#[test]
fn tunneler_own_move_thorns_preserves_dizzy_then_bite() {
    let (mut s, tunneler) = board(
        MonsterKind::Tunneler,
        act2_hive::create_tunneler_ai(),
        Some("BELOW_MOVE_1"),
        59,
    );
    power(&mut s, tunneler, PowerKind::Burrowed, 1);
    s.creatures
        .iter_mut()
        .find(|c| c.id == tunneler.0)
        .unwrap()
        .block = 2;
    let player = s.player.creature_uid.unwrap();
    power(&mut s, player, PowerKind::Thorns, 3);
    turn(&mut s);
    assert_eq!(ai(&s, tunneler).current_state_id(), "DIZZY_MOVE");
    assert_eq!(ai(&s, tunneler).move_history(), &["BELOW_MOVE_1"]);
    assert_eq!(amount(&s, tunneler, PowerKind::Burrowed), 0);
    assert_eq!(
        s.creatures
            .iter()
            .find(|c| c.id == tunneler.0)
            .unwrap()
            .current_hp,
        58
    );
    let hp = s.player.current_hp;
    turn(&mut s);
    assert_eq!(ai(&s, tunneler).current_state_id(), "BITE_MOVE");
    assert_eq!(s.player.current_hp, hp);
    turn(&mut s);
    assert!(s.player.current_hp < hp);
}

#[test]
fn giant_own_move_lethal_reflection_waits_for_steam_capture() {
    let (mut s, giant) = board(
        MonsterKind::WaterfallGiant,
        act1_underdocks::create_waterfall_giant_ai(),
        Some("PRESSURE_UP"),
        2,
    );
    power(&mut s, giant, PowerKind::SteamEruption, 42);
    power(&mut s, giant, PowerKind::Vulnerable, 5);
    let player = s.player.creature_uid.unwrap();
    power(&mut s, player, PowerKind::Thorns, 3);
    turn(&mut s);
    assert_eq!(ai(&s, giant).current_state_id(), "ABOUT_TO_BLOW");
    assert_eq!(ai(&s, giant).move_history(), &["PRESSURE_UP"]);
    assert_eq!(amount(&s, giant, PowerKind::Vulnerable), 0);
    assert_eq!(amount(&s, giant, PowerKind::SteamEruption), 45);
    let hp = s.player.current_hp;
    turn(&mut s);
    assert_eq!(s.player.current_hp, hp);
    assert_eq!(ai(&s, giant).current_state_id(), "EXPLODE");
    assert_eq!(amount(&s, giant, PowerKind::SteamEruption), 0);
    s.phase = CombatPhase::EnemyTurnExec;
    assert_eq!(enemy_turn_exec(&mut s), StepResult::Advanced);
    assert_eq!(enemy_turn_end(&mut s), StepResult::CombatWon);
    assert_eq!(s.player.current_hp, hp - 45);
}

#[test]
fn repeated_external_static_stun_still_performs_once_before_follow_up() {
    let (mut s, tunneler) = board(
        MonsterKind::Tunneler,
        act2_hive::create_tunneler_ai(),
        Some("BELOW_MOVE_1"),
        59,
    );
    for _ in 0..2 {
        sts2_core::hooks::apply_effects(
            &mut s,
            &[sts2_core::hooks::Effect::ForceMonsterMove {
                target: tunneler,
                state_id: "DIZZY_MOVE",
            }],
        );
    }
    let hp = s.player.current_hp;
    turn(&mut s);
    assert_eq!(s.player.current_hp, hp);
    assert_eq!(ai(&s, tunneler).current_state_id(), "BITE_MOVE");
    assert_eq!(ai(&s, tunneler).move_history(), &["DIZZY_MOVE"]);
}

#[test]
fn blocked_body_is_admitted_once_and_completes_only_after_choice() {
    let (mut s, demon) = board(
        MonsterKind::KnowledgeDemon,
        act2_hive::create_knowledge_demon_ai(),
        None,
        200,
    );
    assert!(!ai(&s, demon).performed_first_move());
    let original = ai(&s, demon).current_state_id();
    assert_eq!(enemy_turn_exec(&mut s), StepResult::PendingChoice);
    assert!(
        !ai(&s, demon).performed_first_move(),
        "the body is still awaiting its choice"
    );
    assert_eq!(ai(&s, demon).move_history(), &[original]);
    let mut copy = s.clone();
    for state in [&mut s, &mut copy] {
        let _ = sts2_core::apply(
            state,
            sts2_core::Action::ResolveChoice {
                selection: smallvec::smallvec![0],
            },
        );
        assert!(ai(state, demon).performed_first_move());
        assert_eq!(ai(state, demon).move_history(), &[original]);
        assert!(state.pending_enemy_exec_resume.is_none());
    }
}

#[test]
fn actor_killed_during_its_body_completes_but_dead_roster_slot_never_starts() {
    let (mut s, tunneler) = board(
        MonsterKind::Tunneler,
        act2_hive::create_tunneler_ai(),
        None,
        1,
    );
    let survivor = s.spawn_enemy(MonsterKind::Nibbit);
    let player = s.player.creature_uid.unwrap();
    power(&mut s, player, PowerKind::Thorns, 3);
    assert_eq!(enemy_turn_exec(&mut s), StepResult::Advanced);
    assert!(ai(&s, tunneler).performed_first_move());
    assert_eq!(ai(&s, tunneler).move_history(), &["BITE_MOVE"]);
    assert!(
        s.creatures
            .iter()
            .find(|c| c.id == survivor.0)
            .unwrap()
            .current_hp
            > 0
    );
    s.phase = CombatPhase::EnemyTurnExec;
    assert_eq!(enemy_turn_exec(&mut s), StepResult::Advanced);
    assert_eq!(
        ai(&s, tunneler).move_history(),
        &["BITE_MOVE"],
        "a corpse is not admitted again"
    );
}
