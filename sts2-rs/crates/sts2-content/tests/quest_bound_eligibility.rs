//! Installed-DLL authority: Quest constructors, Tainted/Smoggy/Imbued,
//! SecretTechnique/Flechettes/EscapePlan/HandTrick, ChainsOfBindingPower.
//! Synthetic rule controls, not claims of natural acquisition or route reach.
use sts2_core::cards::{CardId, CardType, EnchantmentId};
use sts2_core::card_afflictions::{CardAffliction, CardAfflictionKind as Affliction};
use sts2_core::combat_loop::{auto_play_specific_card, end_player_turn_phase_one};
use sts2_core::hooks::{apply_effects, Effect, ListenerRef};
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::RunManager;
use sts2_core::{apply, Action, CardUid, CombatPhase, CombatState, MonsterKind, PowerKind, Side};

fn install() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_core::register_card_factory(sts2_content::cards::spec_index::card_pod_from_id);
}

fn state() -> CombatState {
    install();
    let mut s = CombatState { phase: CombatPhase::PlayerPlayPhase, ..Default::default() };
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.player.energy = 99;
    s.init_player_creature();
    let enemy = s.spawn_enemy(MonsterKind::Nibbit);
    let c = s.creatures.iter_mut().find(|c| c.id == enemy.0).unwrap();
    c.current_hp = 1000;
    c.max_hp = 1000;
    s
}

fn add(s: &mut CombatState, id: CardId) -> CardUid {
    let mut c = sts2_content::cards::spec_index::card_pod_from_id(id, false).unwrap();
    let uid = CardUid::new(u32::try_from(s.cards.len() + 1).unwrap()).unwrap();
    c.uid = uid;
    s.cards.push(c);
    uid
}

fn power(s: &mut CombatState, kind: PowerKind, amount: i32) {
    let player = s.player.creature_uid.unwrap();
    sts2_core::card_effects::apply_power_to_creature(s, player, kind, amount);
}

fn draw(s: &mut CombatState, uid: CardUid) {
    s.player.draw.push(uid);
    let _ = sts2_core::cards::draw_via_state(s, 1);
}

#[test]
fn step13_quest_imbued_selection_excludes_only_quests() {
    install();
    let mut rm = RunManager::new(0, CharacterId::Ironclad);
    rm.run_state.master_deck.clear();
    for id in [CardId::SpoilsMap, CardId::ByrdonisEgg, CardId::LanternKey, CardId::DefendIronclad] {
        rm.run_state.master_deck.push(sts2_content::cards::spec_index::card_pod_from_id(id, false).unwrap());
    }
    rm.run_state.pending_deck_enchants.push(sts2_core::runstate::PendingDeckEnchant {
        enchantment: EnchantmentId::Imbued, amount: 1,
        type_restriction: Some(CardType::Skill), secondary_type: None,
    });
    assert_eq!(rm.enchant_eligible_deck_indices(), vec![3]);
}

#[test]
fn step13_bound_legacy_draw_uses_power_amount() {
    let mut s = state();
    power(&mut s, PowerKind::ChainsOfBinding, 3);
    s.player.powers = s.player_power_uids();
    let uid = add(&mut s, CardId::StrikeIronclad);
    draw(&mut s, uid);
    assert_eq!(s.cards[0].affliction, Some(CardAffliction::new(Affliction::Bound, 3)));
}

#[test]
fn step13_quest_affliction_and_interactive_imbued_eligibility() {
    install();
    let mut rm = RunManager::new(0, CharacterId::Ironclad);
    rm.run_state.master_deck.clear();
    for id in [CardId::SpoilsMap, CardId::ByrdonisEgg, CardId::LanternKey] {
        let mut c = sts2_content::cards::spec_index::card_pod_from_id(id, false).unwrap();
        assert!(!c.is_x_cost(), "Quest is unplayable, not X cost");
        assert!(!c.set_affliction(CardAffliction::new(Affliction::Tainted, 3)));
        assert!(c.set_affliction(CardAffliction::new(Affliction::Bound, 3)), "Bound accepts Quest");
        rm.run_state.master_deck.push(c);
    }
    let defend = sts2_content::cards::spec_index::card_pod_from_id(CardId::DefendIronclad, false).unwrap();
    rm.run_state.master_deck.push(defend);
    rm.run_state.pending_deck_enchants.push(sts2_core::runstate::PendingDeckEnchant {
        enchantment: EnchantmentId::Imbued, amount: 1,
        type_restriction: Some(CardType::Skill), secondary_type: None,
    });
    assert_eq!(rm.enchant_eligible_deck_indices(), vec![3]);
    // Synthetic nonnegative-cost Quest still fails the base type gate.
    for c in &mut rm.run_state.master_deck[..3] { c.cost = 1; }
    assert_eq!(rm.enchant_eligible_deck_indices(), vec![3]);
}

#[test]
fn step13_quest_hand_draw_and_skill_selection_controls() {
    for id in [CardId::SpoilsMap, CardId::ByrdonisEgg, CardId::LanternKey, CardId::DefendIronclad] {
        let quest = id.is_quest();
        let mut s = state();
        let attack = add(&mut s, CardId::Flechettes);
        let held = add(&mut s, id);
        s.player.hand.extend([attack, held]);
        let before = s.creatures.iter().find(|c| c.species.is_some()).unwrap().current_hp;
        let _ = apply(&mut s, Action::PlayCard { hand_index: 0, target: Some(0) });
        let after = s.creatures.iter().find(|c| c.species.is_some()).unwrap().current_hp;
        assert_eq!(before - after, if quest { 0 } else { 5 });

        let mut s = state();
        let escape = add(&mut s, CardId::EscapePlan);
        let drawn = add(&mut s, id);
        s.player.hand.push(escape);
        s.player.draw.push(drawn);
        let _ = apply(&mut s, Action::PlayCard { hand_index: 0, target: None });
        assert_eq!(s.player.block, if quest { 0 } else { 3 });

        let mut s = state();
        let trick = add(&mut s, CardId::HandTrick);
        let held = add(&mut s, id);
        s.player.hand.extend([trick, held]);
        let _ = apply(&mut s, Action::PlayCard { hand_index: 0, target: None });
        assert_eq!(s.player.sly_this_turn.contains(&held), !quest);

        let mut s = state();
        let secret = add(&mut s, CardId::SecretTechnique);
        let candidate = add(&mut s, id);
        s.player.hand.push(secret);
        s.player.draw.push(candidate);
        let _ = apply(&mut s, Action::PlayCard { hand_index: 0, target: None });
        assert_eq!(s.player.hand.contains(&candidate), !quest);
    }
}

#[test]
fn step13_bound_amount_budget_competition_redraw_and_side() {
    for amount in [1, 3, 5] {
        let mut s = state();
        power(&mut s, PowerKind::ChainsOfBinding, amount);
        let occupied = add(&mut s, CardId::DefendIronclad);
        s.cards[0].affliction = Some(CardAffliction::new(Affliction::Hexed, 7));
        draw(&mut s, occupied);
        assert_eq!(s.combat_log.bound_cards_afflicted_by_chains_this_turn, 0);
        for i in 0..=amount {
            let uid = add(&mut s, if i == 0 { CardId::SpoilsMap } else { CardId::StrikeIronclad });
            draw(&mut s, uid);
            let mark = s.cards[uid.get() as usize - 1].affliction;
            assert_eq!(mark, (i < amount).then(|| CardAffliction::new(Affliction::Bound, amount)));
            if i == 0 {
                s.player.hand.retain(|u| *u != uid);
                draw(&mut s, uid);
                assert_eq!(s.combat_log.bound_cards_afflicted_by_chains_this_turn, 1, "redraw does not restack or spend budget");
            }
        }
        assert_eq!(s.cards[0].affliction, Some(CardAffliction::new(Affliction::Hexed, 7)));
    }
    let mut s = state();
    power(&mut s, PowerKind::ChainsOfBinding, 3);
    s.side = Side::Enemy;
    let uid = add(&mut s, CardId::DefendIronclad);
    draw(&mut s, uid);
    assert!(s.cards[0].affliction.is_none());
}

#[test]
fn step13_bound_autoplay_and_removed_power_lifetime() {
    let mut s = state();
    power(&mut s, PowerKind::ChainsOfBinding, 3);
    let first = add(&mut s, CardId::DefendIronclad);
    let second = add(&mut s, CardId::DefendIronclad);
    for c in &mut s.cards { c.affliction = Some(CardAffliction::new(Affliction::Bound, 3)); }
    s.player.draw.push(first);
    s.player.hand.push(second);
    assert!(auto_play_specific_card(&mut s, first, false));
    assert!(sts2_core::combat_loop::player_should_play_blocked(&s, &s.cards[1]));
    let uid = s.player_power_uids().into_iter().find(|u| s.powers.get_by_id(u.0).unwrap().kind == PowerKind::ChainsOfBinding).unwrap();
    apply_effects(&mut s, &[Effect::RemovePower { listener: ListenerRef::Power(uid) }]);
    assert!(s.cards[1].affliction.is_some(), "removal does not clear marks");
    power(&mut s, PowerKind::ChainsOfBinding, 3);
    assert!(!sts2_core::combat_loop::player_should_play_blocked(&s, &s.cards[1]), "replacement has a fresh latch");
}

#[test]
fn step13_bound_cleanup_uses_live_piles_and_runs_before_victory() {
    for lethal in [false, true] {
        let mut s = state();
        power(&mut s, PowerKind::ChainsOfBinding, 3);
        s.player.powers = s.player_power_uids();
        let live = add(&mut s, CardId::DefendIronclad);
        let removed = add(&mut s, CardId::DefendIronclad);
        s.player.hand.push(live);
        for c in &mut s.cards { c.affliction = Some(CardAffliction::new(Affliction::Bound, 3)); }
        s.combat_log.chains_bound_card_played_this_turn = true;
        if lethal {
            power(&mut s, PowerKind::TheBomb, 1);
            let uid = s.player_power_uids().into_iter().find(|u| s.powers.get_by_id(u.0).unwrap().kind == PowerKind::TheBomb).unwrap();
            s.powers.iter_mut().find(|p| p.id == uid.0).unwrap().counter = 9999;
        }
        let _ = end_player_turn_phase_one(&mut s);
        assert!(s.cards[live.get() as usize - 1].affliction.is_none());
        assert_eq!(s.cards[removed.get() as usize - 1].affliction, Some(CardAffliction::new(Affliction::Bound, 3)));
        assert!(!s.combat_log.chains_bound_card_played_this_turn);
        if lethal { assert!(s.is_over && s.player_won); }
    }
}
