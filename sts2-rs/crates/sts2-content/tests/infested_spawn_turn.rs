//! Infested admission follows CombatManager.AfterCreatureAdded's current side.
//! Native poison deaths occur before the enemy perform roster is walked: a
//! newborn has no rolled intent on that side and first performs next round.
use sts2_content::monsters::act1_overgrowth::infested_after_death_spawn;
use sts2_core::combat_loop::{enemy_turn_end, enemy_turn_exec, enemy_turn_start};
use sts2_core::monsters::Intent;
use sts2_core::{CombatPhase, CombatState, CreatureUid, MonsterKind, PowerKind, Side, StepResult};

fn board() -> (CombatState, CreatureUid) {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.init_player_creature();
    let phrog = s.spawn_enemy(MonsterKind::PhrogParasite);
    (s, phrog)
}

fn moves(s: &CombatState) -> Vec<&'static str> {
    s.enemy_uids
        .iter()
        .filter_map(|uid| {
            let c = s.creatures.iter().find(|c| c.id == uid.0)?;
            (c.species == Some(MonsterKind::Wriggler))
                .then(|| c.ai.as_ref().unwrap().current_state_id())
        })
        .collect()
}

fn enemy_round(s: &mut CombatState) {
    s.phase = CombatPhase::EnemyTurnStart;
    assert_eq!(enemy_turn_start(s), StepResult::Advanced);
    assert_eq!(enemy_turn_exec(s), StepResult::Advanced);
    assert_eq!(enemy_turn_end(s), StepResult::Advanced);
}

#[test]
fn infested_enemy_side_spawn_waits_for_its_first_actual_turn() {
    let (mut s, phrog) = board();
    s.creatures
        .iter_mut()
        .find(|c| c.id == phrog.0)
        .unwrap()
        .current_hp = 0;
    s.side = Side::Enemy;
    s.phase = CombatPhase::EnemyTurnExec;
    infested_after_death_spawn(&mut s, phrog);
    assert_eq!(moves(&s), vec!["SPAWNED_MOVE"; 4]);
    assert!(
        s.creatures
            .iter()
            .filter(|c| c.species == Some(MonsterKind::Wriggler))
            .all(|c| c.intent.is_none()),
        "enemy-side additions have not rolled a move"
    );
    // Cloning preserves unadmitted newborns; reading their AI changes nothing.
    let mut copy = s.clone();
    for state in [&mut s, &mut copy] {
        assert_eq!(enemy_turn_exec(state), StepResult::Advanced);
        assert_eq!(enemy_turn_end(state), StepResult::Advanced);
        assert_eq!(moves(state), vec!["SPAWNED_MOVE"; 4]);
        assert!(state
            .creatures
            .iter()
            .filter(|c| c.species == Some(MonsterKind::Wriggler))
            .all(|c| c.intent == Some(Intent::Stun)));
        assert_eq!(state.player.current_hp, 80);
        enemy_round(state);
        assert_eq!(
            moves(state),
            vec!["NASTY_BITE", "WRIGGLE", "NASTY_BITE", "WRIGGLE"]
        );
        assert_eq!(
            state.player.current_hp, 80,
            "the first actual action is the spawn stun"
        );
        enemy_round(state);
        assert_eq!(
            state.player.current_hp, 68,
            "two slot-specific bites happen only now"
        );
    }
}

#[test]
fn infested_poison_death_does_not_admit_newborns_into_that_enemy_walk() {
    let (mut s, phrog) = board();
    s.creatures
        .iter_mut()
        .find(|c| c.id == phrog.0)
        .unwrap()
        .current_hp = 1;
    sts2_core::card_effects::apply_power_to_creature(&mut s, phrog, PowerKind::Infested, 4);
    sts2_core::card_effects::apply_power_to_creature(&mut s, phrog, PowerKind::Poison, 1);
    enemy_round(&mut s);
    assert_eq!(moves(&s), vec!["SPAWNED_MOVE"; 4]);
    assert_eq!(s.player.current_hp, 80);
    enemy_round(&mut s);
    assert_eq!(
        moves(&s),
        vec!["NASTY_BITE", "WRIGGLE", "NASTY_BITE", "WRIGGLE"]
    );
    assert_eq!(s.player.current_hp, 80);
    enemy_round(&mut s);
    assert_eq!(s.player.current_hp, 68);
}

#[test]
fn infested_player_side_spawn_still_performs_on_the_next_enemy_walk() {
    let (mut s, phrog) = board();
    s.creatures
        .iter_mut()
        .find(|c| c.id == phrog.0)
        .unwrap()
        .current_hp = 0;
    s.side = Side::Player;
    s.phase = CombatPhase::PlayerPlayPhase;
    infested_after_death_spawn(&mut s, phrog);
    assert!(s
        .creatures
        .iter()
        .filter(|c| c.species == Some(MonsterKind::Wriggler))
        .all(|c| c.intent == Some(Intent::Stun)));
    enemy_round(&mut s);
    assert_eq!(
        moves(&s),
        vec!["NASTY_BITE", "WRIGGLE", "NASTY_BITE", "WRIGGLE"]
    );
    assert_eq!(s.player.current_hp, 80);
    enemy_round(&mut s);
    assert_eq!(s.player.current_hp, 68);
}
