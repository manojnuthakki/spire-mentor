//! IntimidatingHelmet.BeforeCardPlayed observes the original ResourceInfo on
//! each actual play in CardModel.OnPlayWrapper, including manual replays.
use std::num::NonZeroU32;
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_core::cards::{CardId, VarKey};
use sts2_core::combat_loop::{auto_play_specific_card, play_card};
use sts2_core::{
    apply, legal_actions, Action, CardUid, CombatPhase, CombatState, MonsterKind, Power, PowerKind,
    PowerUid, Relic, RelicKind,
};

fn add(s: &mut CombatState, id: CardId) -> CardUid {
    let mut c = card_pod_from_id(id, false).unwrap();
    c.uid = CardUid::new(s.cards.len() as u32 + 1).unwrap();
    s.cards.push(c);
    c.uid
}

fn power(s: &mut CombatState, kind: PowerKind) {
    let id = PowerUid::new(s.powers.len() as u32 + 1).unwrap();
    s.powers.push(Power {
        id: id.0,
        kind,
        amount: 1,
        counter: 0,
        applier: None,
        skip_next_tick: false,
    });
    s.player.powers.push(id);
}

fn board(card: CardId, helmet: Option<bool>) -> CombatState {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.player.energy = 3;
    s.init_player_creature();
    let e = s.spawn_enemy(MonsterKind::Nibbit);
    let enemy = s.creatures.iter_mut().find(|c| c.id == e.0).unwrap();
    enemy.current_hp = 500;
    enemy.max_hp = 500;
    s.phase = CombatPhase::PlayerPlayPhase;
    let c = add(&mut s, card);
    s.player.hand.push(c);
    power(&mut s, PowerKind::EchoForm);
    if let Some(melted) = helmet {
        s.relics.push(Relic {
            id: NonZeroU32::new(1).unwrap(),
            kind: RelicKind::IntimidatingHelmet,
            counter: 0,
            is_melted: melted,
            is_wax: false,
        });
    }
    s
}

#[test]
fn echo_and_base_replays_grant_per_play_but_pay_energy_once() {
    for repeats in [0, 2] {
        let mut s = board(CardId::Bash, Some(false));
        s.cards[0].base_replay_count = repeats;
        let _ = play_card(&mut s, 0, Some(0));
        assert_eq!(s.player.block, 4 * (2 + repeats));
        assert_eq!(s.player.energy, 1);
        assert_eq!(
            s.combat_log.player_card_plays_this_combat,
            (2 + repeats) as u32
        );
    }
}

#[test]
fn replay_uses_original_modified_value_after_free_power_is_consumed() {
    for free in [false, true] {
        let mut s = board(CardId::Bash, Some(false));
        if free {
            power(&mut s, PowerKind::FreeAttack);
        } else {
            s.cards[0].cost = 1;
        }
        let _ = play_card(&mut s, 0, Some(0));
        assert_eq!(
            s.player.block, 0,
            "printed cost 2 is not the resource value"
        );
        assert_eq!(s.player.energy, if free { 3 } else { 2 });
        assert_eq!(s.combat_log.player_card_plays_this_combat, 2);
    }
}

#[test]
fn x_replays_use_raw_energy_value_without_chemical_x_bonus_or_second_payment() {
    for energy in [1, 3] {
        let mut s = board(CardId::Whirlwind, Some(false));
        s.player.energy = energy;
        s.relics.push(Relic {
            id: NonZeroU32::new(2).unwrap(),
            kind: RelicKind::ChemicalX,
            counter: 0,
            is_melted: false,
            is_wax: false,
        });
        let _ = play_card(&mut s, 0, None);
        assert_eq!(s.player.block, if energy >= 2 { 8 } else { 0 });
        assert_eq!(s.player.energy, 0);
    }
}

#[test]
fn parked_manual_body_and_clone_resume_grant_once_per_actual_play() {
    let mut s = board(CardId::Headbutt, Some(false));
    s.cards[0].cost = 2;
    for id in [CardId::StrikeIronclad, CardId::DefendIronclad] {
        let c = add(&mut s, id);
        s.player.discard.push(c);
    }
    let _ = play_card(&mut s, 0, Some(0));
    assert!(s.pending_choice.is_some());
    assert_eq!(s.player.block, 4, "initial BeforeCardPlayed already ran");
    let mut clone = s.clone();
    let pick = legal_actions(&s)
        .into_iter()
        .find(|a| matches!(a, Action::ResolveChoice { selection } if !selection.is_empty()))
        .unwrap();
    let _ = apply(&mut clone, pick.clone());
    assert_eq!(clone.player.block, 8);
    assert_eq!(s.player.block, 4);
    let _ = apply(&mut s, pick);
    assert_eq!(s, clone);
}

#[test]
fn helmet_block_precedes_each_body_slam_body() {
    let mut s = board(CardId::BodySlam, Some(false));
    s.cards[0].cost = 2;
    let _ = play_card(&mut s, 0, Some(0));
    let enemy = s.creatures.iter().find(|c| c.species.is_some()).unwrap();
    assert_eq!(enemy.current_hp, 500 - 4 - 8);
    assert_eq!(s.player.block, 8);
}

#[test]
fn absent_melted_nonqualifying_and_refused_plays_do_not_grant() {
    for helmet in [None, Some(true)] {
        let mut s = board(CardId::Bash, helmet);
        let _ = play_card(&mut s, 0, Some(0));
        assert_eq!(s.player.block, 0);
    }
    let mut s = board(CardId::StrikeIronclad, Some(false));
    let _ = play_card(&mut s, 0, Some(0));
    assert_eq!(s.player.block, 0);
    let mut s = board(CardId::Bash, Some(false));
    s.player.energy = 1;
    let before = s.clone();
    let _ = play_card(&mut s, 0, Some(0));
    assert_eq!(s, before);
}

#[test]
fn already_started_echo_series_keeps_helmet_callback_after_lethal_first_body() {
    let mut s = board(CardId::BodySlam, Some(false));
    s.cards[0].cost = 2;
    s.creatures
        .iter_mut()
        .find(|c| c.species.is_some())
        .unwrap()
        .current_hp = 1;
    let _ = play_card(&mut s, 0, Some(0));
    assert_eq!(s.player.block, 8);
    assert_eq!(s.player.energy, 1);
    assert_eq!(s.cards[0].combat_var(VarKey::IsDupe), 0);
}

#[test]
fn autoplay_uses_cost_value_without_paying_it_or_reusing_old_manual_spend() {
    for (card, cost, expected) in [
        (CardId::Bash, 2, 4),
        (CardId::Bash, 1, 0),
        (CardId::StrikeIronclad, 1, 0),
    ] {
        let mut s = board(card, Some(false));
        s.cards[0].cost = cost;
        s.cards[0].energy_spent = 77;
        let c = s.player.hand.remove(0);
        s.player.draw.push(c);
        assert!(auto_play_specific_card(&mut s, c, false));
        assert_eq!(s.player.block, expected);
        assert_eq!(s.player.energy, 3);
        assert_eq!(s.cards[0].energy_spent, i32::from(cost));
    }
}

#[test]
fn autoplay_x_value_ignores_chemical_x_and_does_not_spend_energy() {
    for energy in [1, 3] {
        let mut s = board(CardId::Whirlwind, Some(false));
        s.player.energy = energy;
        s.relics.push(Relic {
            id: NonZeroU32::new(2).unwrap(),
            kind: RelicKind::ChemicalX,
            counter: 0,
            is_melted: false,
            is_wax: false,
        });
        let c = s.player.hand.remove(0);
        s.player.draw.push(c);
        assert!(auto_play_specific_card(&mut s, c, false));
        assert_eq!(s.player.block, if energy >= 2 { 4 } else { 0 });
        assert_eq!(s.player.energy, energy);
    }
}

#[test]
fn autoplay_repeated_stomp_keeps_original_value_while_its_cost_falls() {
    let mut s = board(CardId::Stomp, Some(false));
    s.cards[0].base_replay_count = 2;
    let c = s.player.hand.remove(0);
    s.player.draw.push(c);
    assert!(auto_play_specific_card(&mut s, c, false));
    assert_eq!(s.player.block, 12);
    assert_eq!(s.cards[0].cost, 0);
    assert_eq!(s.cards[0].energy_spent, 3);
    assert_eq!(s.player.energy, 3);
}

#[test]
fn free_autoplay_parked_repeats_keep_zero_value_after_cost_cleanup() {
    let mut s = board(CardId::Headbutt, Some(false));
    s.cards[0].cost = 2;
    s.cards[0].base_replay_count = 1;
    power(&mut s, PowerKind::FreeAttack);
    for id in [CardId::StrikeIronclad, CardId::DefendIronclad] {
        let c = add(&mut s, id);
        s.player.discard.push(c);
    }
    let c = s.player.hand.remove(0);
    s.player.draw.push(c);
    assert!(auto_play_specific_card(&mut s, c, false));
    assert_eq!(s.cards[0].energy_spent, 0);
    assert_eq!(s.player.block, 0);
    assert!(s.pending_choice.is_some());
    let mut clone = s.clone();
    for state in [&mut s, &mut clone] {
        let pick = legal_actions(state)
            .into_iter()
            .find(|a| matches!(a, Action::ResolveChoice { selection } if !selection.is_empty()))
            .unwrap();
        let _ = apply(state, pick);
        assert_eq!(state.player.block, 0);
        assert_eq!(state.cards[0].energy_spent, 0);
        assert_eq!(state.cards[0].cost, 2);
    }
    assert_eq!(s, clone);
}

#[test]
fn history_course_dupe_uses_its_own_value_per_repeat_and_shaves_stomp() {
    let mut s = board(CardId::Bash, Some(false));
    s.round = 2;
    s.cards[0].base_replay_count = 1;
    s.cards[0].energy_spent = 77;
    s.relics.push(Relic {
        id: NonZeroU32::new(2).unwrap(),
        kind: RelicKind::HistoryCourse,
        counter: 0,
        is_melted: false,
        is_wax: false,
    });
    let original = s.player.hand[0];
    s.combat_log.prev_round_last_attack_skill_played = Some((CardId::Bash, false, Some(original)));
    let stomp = add(&mut s, CardId::Stomp);
    s.player.discard.push(stomp);
    let _ = sts2_core::relic_effects::dispatch_before_play_phase_start(&mut s);
    assert_eq!(s.player.block, 8);
    assert_eq!(s.cards[stomp.get() as usize - 1].cost, 1);
    assert_eq!(
        s.cards[0].energy_spent, 77,
        "the original source is not the dupe"
    );
    assert_eq!(s.player.energy, 3);
}

#[test]
fn qualifying_autoplay_selection_is_parked_after_block_and_resume_adds_only_replay() {
    let mut s = board(CardId::Headbutt, Some(false));
    s.cards[0].cost = 2;
    s.cards[0].base_replay_count = 1;
    for id in [CardId::StrikeIronclad, CardId::DefendIronclad] {
        let c = add(&mut s, id);
        s.player.discard.push(c);
    }
    let c = s.player.hand.remove(0);
    s.player.draw.push(c);
    assert!(auto_play_specific_card(&mut s, c, false));
    assert!(s.pending_choice.is_some());
    assert_eq!(s.player.block, 4);
    let mut clone = s.clone();
    for state in [&mut s, &mut clone] {
        let pick = legal_actions(state)
            .into_iter()
            .find(|a| matches!(a, Action::ResolveChoice { selection } if !selection.is_empty()))
            .unwrap();
        let _ = apply(state, pick);
        assert_eq!(state.player.block, 8);
        assert_eq!(state.player.energy, 3);
    }
    assert_eq!(s, clone);
}

#[test]
fn autoplay_absent_melted_and_ending_controls() {
    for helmet in [None, Some(true)] {
        let mut s = board(CardId::Bash, helmet);
        let c = s.player.hand.remove(0);
        s.player.draw.push(c);
        assert!(auto_play_specific_card(&mut s, c, false));
        assert_eq!(s.player.block, 0);
    }
    let mut s = board(CardId::Bash, Some(false));
    s.creatures
        .iter_mut()
        .find(|c| c.species.is_some())
        .unwrap()
        .current_hp = 0;
    let c = s.player.hand.remove(0);
    s.player.draw.push(c);
    let before = s.clone();
    assert!(!auto_play_specific_card(&mut s, c, false));
    assert_eq!(
        s, before,
        "ending refuses entry before any helmet callback or move"
    );
}
