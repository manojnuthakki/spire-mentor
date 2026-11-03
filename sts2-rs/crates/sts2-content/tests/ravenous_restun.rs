//! Native Ravenous refuses replacing an unperformed stun, but each ally death
//! still grants Strength. Slug resumes its original deterministic move once.
use sts2_content::monsters::act1_underdocks::create_corpse_slug_ai;
use sts2_core::combat_loop::{enemy_turn_end, enemy_turn_exec};
use sts2_core::hooks::{dispatch_after_death, AfterDeathCtx};
use sts2_core::{CombatPhase, CombatState, CreatureUid, MonsterKind, PowerKind, Side, StepResult};

fn board(starter: u8, side: Side) -> (CombatState, CreatureUid, [CreatureUid; 3]) {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 5000;
    s.player.max_hp = 5000;
    s.init_player_creature();
    s.side = side;
    let slug = s.spawn_enemy(MonsterKind::CorpseSlug);
    let ai = create_corpse_slug_ai(starter);
    let intent = ai.current_move().intents.first().copied();
    s.set_creature_ai(slug, ai);
    s.creatures
        .iter_mut()
        .find(|c| c.id == slug.0)
        .unwrap()
        .intent = intent;
    sts2_core::card_effects::apply_power_to_creature(&mut s, slug, PowerKind::Ravenous, 4);
    let allies = std::array::from_fn(|_| s.spawn_enemy(MonsterKind::Tunneler));
    (s, slug, allies)
}

fn current(s: &CombatState, uid: CreatureUid) -> &str {
    s.creatures
        .iter()
        .find(|c| c.id == uid.0)
        .unwrap()
        .ai
        .as_ref()
        .unwrap()
        .current_state_id()
}

fn strength(s: &CombatState, uid: CreatureUid) -> i32 {
    s.creatures
        .iter()
        .find(|c| c.id == uid.0)
        .unwrap()
        .powers
        .iter()
        .filter_map(|p| s.powers.get_by_id(p.0))
        .filter(|p| p.kind == PowerKind::Strength)
        .map(|p| p.amount)
        .sum()
}

fn death(s: &mut CombatState, uid: CreatureUid, side: Side) {
    s.creatures
        .iter_mut()
        .find(|c| c.id == uid.0)
        .unwrap()
        .current_hp = 0;
    dispatch_after_death(
        s,
        &AfterDeathCtx {
            dead: uid,
            dead_side: side,
        },
    );
}

fn turn(s: &mut CombatState) {
    s.side = Side::Enemy;
    s.phase = CombatPhase::EnemyTurnExec;
    assert_eq!(enemy_turn_exec(s), StepResult::Advanced);
    assert_eq!(enemy_turn_end(s), StepResult::Advanced);
}

fn repeated_deaths(starter: u8, original: &str, next: &str) {
    for side in [Side::Player, Side::Enemy] {
        let (mut s, slug, allies) = board(starter, side);
        for dead in allies {
            death(&mut s, dead, Side::Enemy);
            assert_eq!(current(&s, slug), "STUNNED");
        }
        assert_eq!(strength(&s, slug), 12);
        let hp = s.player.current_hp;
        let mut clone = s.clone();
        for state in [&mut s, &mut clone] {
            turn(state);
            assert_eq!(state.player.current_hp, hp);
            assert_eq!(current(state, slug), original);
            turn(state);
            assert_eq!(current(state, slug), next);
            assert_eq!(strength(state, slug), 12);
        }
        assert_eq!(s.player.current_hp, clone.player.current_hp);
    }
}

#[test]
fn whip_slap_resumes_once_after_three_ally_deaths() {
    repeated_deaths(0, "WHIP_SLAP", "GLOMP");
}

#[test]
fn glomp_resumes_once_after_three_ally_deaths() {
    repeated_deaths(1, "GLOMP", "GOOP");
}

#[test]
fn goop_resumes_once_after_three_ally_deaths() {
    repeated_deaths(2, "GOOP", "WHIP_SLAP");
}

#[test]
fn performed_stun_can_be_replaced_before_roll_without_losing_original_resume() {
    let (mut s, slug, allies) = board(1, Side::Enemy);
    death(&mut s, allies[0], Side::Enemy);
    s.phase = CombatPhase::EnemyTurnExec;
    assert_eq!(enemy_turn_exec(&mut s), StepResult::Advanced);
    assert_eq!(current(&s, slug), "STUNNED");
    // A different ally dies after this slot actually performed, before rolling.
    death(&mut s, allies[1], Side::Enemy);
    assert_eq!(strength(&s, slug), 8);
    assert_eq!(enemy_turn_end(&mut s), StepResult::Advanced);
    assert_eq!(current(&s, slug), "STUNNED");
    let hp = s.player.current_hp;
    turn(&mut s);
    assert_eq!(s.player.current_hp, hp);
    assert_eq!(current(&s, slug), "GLOMP");
}

#[test]
fn dead_owner_self_death_and_opposite_side_do_not_trigger_ravenous() {
    for case in 0..3 {
        let (mut s, slug, allies) = board(2, Side::Player);
        match case {
            0 => {
                s.creatures
                    .iter_mut()
                    .find(|c| c.id == slug.0)
                    .unwrap()
                    .current_hp = 0;
                death(&mut s, allies[0], Side::Enemy);
            }
            1 => death(&mut s, slug, Side::Enemy),
            _ => {
                let player = s.player.creature_uid.unwrap();
                death(&mut s, player, Side::Player);
            }
        }
        assert_eq!(strength(&s, slug), 0);
        assert_eq!(current(&s, slug), "GOOP");
    }
}
