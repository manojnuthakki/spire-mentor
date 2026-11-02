//! v0.107.1 CardEnergyCost.EndOfTurnCleanup, PlayerCombatState and
//! CombatManager.FlushPlayerHand/EndEnemyTurnInternal: expiry is at each
//! cleanup, before late hooks, rather than at the next player start.
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_core::cards::{CardId, VarKey};
use sts2_core::combat_loop::{end_player_turn_phase_two, enemy_turn_end, start_player_turn};
use sts2_core::{
    apply, legal_actions, Action, CardUid, CombatPhase, CombatState, MonsterKind, PowerKind,
};

fn add(s: &mut CombatState, id: CardId) -> CardUid {
    let mut c = card_pod_from_id(id, false).unwrap();
    c.uid = CardUid::new(s.cards.len() as u32 + 1).unwrap();
    s.cards.push(c);
    c.uid
}

fn board() -> CombatState {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.player.energy = 3;
    s.init_player_creature();
    let enemy = s.spawn_enemy(MonsterKind::Nibbit);
    let e = s.creatures.iter_mut().find(|c| c.id == enemy.0).unwrap();
    e.current_hp = 500;
    e.max_hp = 500;
    s.phase = CombatPhase::PlayerEndTurnPhaseOne;
    s
}

fn cost(s: &CombatState, uid: CardUid) -> i8 {
    s.cards[uid.get() as usize - 1].cost
}

#[test]
fn owned_clamped_shaves_expire_after_flush_in_every_pile_once() {
    let mut s = board();
    let mut cards = Vec::new();
    for pile in 0..5 {
        let uid = add(&mut s, CardId::Stomp);
        match pile {
            0 => s.player.hand.push(uid),
            1 => s.player.draw.push(uid),
            2 => s.player.discard.push(uid),
            3 => s.player.exhaust.push(uid),
            _ => s.player.play.push(uid),
        }
        cards.push(uid);
    }
    for _ in 0..5 {
        s.discount_stomp_on_attack_played();
    }
    assert!(cards.iter().all(|u| cost(&s, *u) == 0));
    let _ = end_player_turn_phase_two(&mut s);
    for uid in cards {
        assert_eq!(cost(&s, uid), 3);
        assert_eq!(
            s.cards[uid.get() as usize - 1].combat_var(VarKey::StompCostReductionThisTurn),
            0
        );
    }
    let before = s.cards.clone();
    s.phase = CombatPhase::EnemyTurnEnd;
    let _ = enemy_turn_end(&mut s);
    assert_eq!(s.cards, before, "a second cleanup must not restore twice");
}

#[test]
fn snecko_signed_deltas_and_pinpoint_expire_at_player_cleanup() {
    for delta in [-2, 2] {
        let mut s = board();
        let bash = add(&mut s, CardId::Bash);
        let c = &mut s.cards[bash.get() as usize - 1];
        c.cost = 2 + delta;
        c.set_combat_var(VarKey::SneckoCostDeltaThisTurn, i32::from(delta));
        c.set_combat_var(VarKey::CostAbsolutelySet, 1);
        s.player.discard.push(bash);
        let pin = add(&mut s, CardId::Pinpoint);
        s.player.draw.push(pin);
        s.discount_pinpoint_on_skill_played();
        let _ = end_player_turn_phase_two(&mut s);
        assert_eq!(cost(&s, bash), 2);
        assert_eq!(cost(&s, pin), 3);
        assert_eq!(
            s.cards[bash.get() as usize - 1].combat_var(VarKey::CostAbsolutelySet),
            0
        );
    }
}

#[test]
fn foreign_and_owned_banks_expire_without_consuming_until_played_rebank() {
    let mut s = board();
    let uid = add(&mut s, CardId::Stomp);
    let c = &mut s.cards[uid.get() as usize - 1];
    // Existing foreign reconstruction contract: stable 3, until-played
    // absolute 1. Owned shave happens AFTER reconstruction.
    c.cost = 1;
    c.set_combat_var(VarKey::WireCostStable, 3);
    c.set_combat_var(VarKey::WireTransientCostThisTurn, 0);
    c.set_combat_var(VarKey::WireCostExpiringOnPlay, 2);
    s.player.discard.push(uid);
    s.discount_stomp_on_attack_played();
    assert_eq!(cost(&s, uid), 0);
    let _ = end_player_turn_phase_two(&mut s);
    assert_eq!(cost(&s, uid), 1, "only the owned shave expires here");
    s.restore_wire_cost_expiring_on_play(uid);
    assert_eq!(
        cost(&s, uid),
        3,
        "existing until-played arithmetic is preserved"
    );
}

#[test]
fn foreign_turn_bank_and_until_played_bank_rebank_at_cleanup() {
    let mut s = board();
    let uid = add(&mut s, CardId::Bash);
    let c = &mut s.cards[uid.get() as usize - 1];
    c.cost = 0;
    c.set_combat_var(VarKey::WireCostStable, 2);
    c.set_combat_var(VarKey::WireTransientCostThisTurn, 1);
    c.set_combat_var(VarKey::WireCostExpiringOnPlay, 2);
    s.player.discard.push(uid);
    let _ = end_player_turn_phase_two(&mut s);
    assert_eq!(cost(&s, uid), 1);
    assert_eq!(
        s.cards[uid.get() as usize - 1].combat_var(VarKey::WireCostExpiringOnPlay),
        1
    );
    s.restore_wire_cost_expiring_on_play(uid);
    assert_eq!(cost(&s, uid), 2);
}

#[test]
fn wlp_choice_retains_discount_until_answer_then_flush_cleans_clone_independently() {
    let mut s = board();
    let uid = add(&mut s, CardId::Stomp);
    s.player.hand.push(uid);
    s.discount_stomp_on_attack_played();
    let p = s.player.creature_uid.unwrap();
    sts2_core::card_effects::apply_power_to_creature(&mut s, p, PowerKind::WellLaidPlans, 1);
    let _ = end_player_turn_phase_two(&mut s);
    assert!(s.pending_wlp_retain_resume);
    assert_eq!(cost(&s, uid), 2);
    let action = legal_actions(&s)
        .into_iter()
        .find(|a| {
            matches!(a,
        Action::ResolveChoice { selection } if !selection.is_empty())
        })
        .unwrap();
    let mut clone = s.clone();
    assert_eq!(cost(&clone, uid), 2);
    let _ = apply(&mut clone, action.clone());
    assert_eq!(cost(&clone, uid), 3);
    assert_eq!(cost(&s, uid), 2);
    assert!(s.pending_choice.is_some());
    let _ = apply(&mut s, action);
    assert_eq!(s, clone);
}

#[test]
fn real_late_i_am_invincible_skill_shaves_pinpoint_until_enemy_cleanup() {
    let mut s = board();
    let pin = add(&mut s, CardId::Pinpoint);
    s.player.discard.push(pin);
    s.discount_pinpoint_on_skill_played();
    let late = add(&mut s, CardId::IAmInvincible);
    s.player.draw.push(late);
    let _ = end_player_turn_phase_two(&mut s);
    assert_eq!(
        cost(&s, pin),
        2,
        "old shave expired, late real autoplay added one"
    );
    assert_eq!(
        s.cards[pin.get() as usize - 1].combat_var(VarKey::PinpointCostReductionThisTurn),
        1
    );
    s.phase = CombatPhase::EnemyTurnEnd;
    let _ = enemy_turn_end(&mut s);
    assert_eq!(cost(&s, pin), 3);
}

#[test]
fn bank_created_after_enemy_cleanup_survives_player_start_until_next_cleanup() {
    let mut s = board();
    let uid = add(&mut s, CardId::Stomp);
    s.player.discard.push(uid);
    s.phase = CombatPhase::EnemyTurnEnd;
    let _ = enemy_turn_end(&mut s);
    // Boundary control via the existing bank writer; no invented content hook.
    s.discount_stomp_on_attack_played();
    let _ = start_player_turn(&mut s);
    assert_eq!(cost(&s, uid), 2);
    s.phase = CombatPhase::PlayerEndTurnPhaseOne;
    let _ = end_player_turn_phase_two(&mut s);
    assert_eq!(cost(&s, uid), 3);
}

#[test]
fn no_modifier_and_until_played_only_controls_are_unchanged() {
    let mut s = board();
    let uid = add(&mut s, CardId::Bash);
    s.player.hand.push(uid);
    let late = add(&mut s, CardId::Stomp);
    s.player.discard.push(late);
    s.cards[late.get() as usize - 1].set_combat_var(VarKey::FreeUntilPlayed, 1);
    let _ = end_player_turn_phase_two(&mut s);
    assert_eq!(cost(&s, uid), 2);
    assert_eq!(cost(&s, late), 3);
    assert_eq!(
        s.cards[late.get() as usize - 1].combat_var(VarKey::FreeUntilPlayed),
        1
    );
}
