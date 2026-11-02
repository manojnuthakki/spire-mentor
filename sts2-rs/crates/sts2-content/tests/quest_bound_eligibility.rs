//! Installed-DLL authority: Quest constructors, Tainted/Smoggy/Imbued,
//! SecretTechnique/Flechettes/EscapePlan/HandTrick and typed cost predicates.
//! Synthetic rule controls, not claims of natural acquisition or route reach.
use sts2_core::card_afflictions::{CardAffliction, CardAfflictionKind as Affliction};
use sts2_core::cards::{CardId, CardType, EnchantmentId};
use sts2_core::hooks::ListenerRef;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::RunManager;
use sts2_core::{apply, Action, CardUid, CombatPhase, CombatState, MonsterKind, PowerKind};

fn install() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_core::register_card_factory(sts2_content::cards::spec_index::card_pod_from_id);
}

fn state() -> CombatState {
    install();
    let mut s = CombatState {
        phase: CombatPhase::PlayerPlayPhase,
        ..Default::default()
    };
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

#[test]
fn step13_quest_imbued_selection_excludes_only_quests() {
    install();
    let mut rm = RunManager::new(0, CharacterId::Ironclad);
    rm.run_state.master_deck.clear();
    for id in [
        CardId::SpoilsMap,
        CardId::ByrdonisEgg,
        CardId::LanternKey,
        CardId::DefendIronclad,
    ] {
        rm.run_state
            .master_deck
            .push(sts2_content::cards::spec_index::card_pod_from_id(id, false).unwrap());
    }
    rm.run_state
        .pending_deck_enchants
        .push(sts2_core::runstate::PendingDeckEnchant {
            enchantment: EnchantmentId::Imbued,
            amount: 1,
            type_restriction: Some(CardType::Skill),
            secondary_type: None,
        });
    assert_eq!(rm.enchant_eligible_deck_indices(), vec![3]);
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
        assert!(
            c.set_affliction(CardAffliction::new(Affliction::Bound, 3)),
            "Bound accepts Quest"
        );
        rm.run_state.master_deck.push(c);
    }
    let defend =
        sts2_content::cards::spec_index::card_pod_from_id(CardId::DefendIronclad, false).unwrap();
    rm.run_state.master_deck.push(defend);
    rm.run_state
        .pending_deck_enchants
        .push(sts2_core::runstate::PendingDeckEnchant {
            enchantment: EnchantmentId::Imbued,
            amount: 1,
            type_restriction: Some(CardType::Skill),
            secondary_type: None,
        });
    assert_eq!(rm.enchant_eligible_deck_indices(), vec![3]);
    // Synthetic nonnegative-cost Quest still fails the base type gate.
    for c in &mut rm.run_state.master_deck[..3] {
        c.cost = 1;
    }
    assert_eq!(rm.enchant_eligible_deck_indices(), vec![3]);
}

#[test]
fn step13_quest_hand_draw_and_skill_selection_controls() {
    for id in [
        CardId::SpoilsMap,
        CardId::ByrdonisEgg,
        CardId::LanternKey,
        CardId::DefendIronclad,
    ] {
        let quest = id.is_quest();
        let mut s = state();
        let attack = add(&mut s, CardId::Flechettes);
        let held = add(&mut s, id);
        s.player.hand.extend([attack, held]);
        let before = s
            .creatures
            .iter()
            .find(|c| c.species.is_some())
            .unwrap()
            .current_hp;
        let _ = apply(
            &mut s,
            Action::PlayCard {
                hand_index: 0,
                target: Some(0),
            },
        );
        let after = s
            .creatures
            .iter()
            .find(|c| c.species.is_some())
            .unwrap()
            .current_hp;
        assert_eq!(before - after, if quest { 0 } else { 5 });

        let mut s = state();
        let escape = add(&mut s, CardId::EscapePlan);
        let drawn = add(&mut s, id);
        s.player.hand.push(escape);
        s.player.draw.push(drawn);
        let _ = apply(
            &mut s,
            Action::PlayCard {
                hand_index: 0,
                target: None,
            },
        );
        assert_eq!(s.player.block, if quest { 0 } else { 3 });

        let mut s = state();
        let trick = add(&mut s, CardId::HandTrick);
        let held = add(&mut s, id);
        s.player.hand.extend([trick, held]);
        let _ = apply(
            &mut s,
            Action::PlayCard {
                hand_index: 0,
                target: None,
            },
        );
        assert_eq!(s.player.sly_this_turn.contains(&held), !quest);

        let mut s = state();
        let secret = add(&mut s, CardId::SecretTechnique);
        let candidate = add(&mut s, id);
        s.player.hand.push(secret);
        s.player.draw.push(candidate);
        let _ = apply(
            &mut s,
            Action::PlayCard {
                hand_index: 0,
                target: None,
            },
        );
        assert_eq!(s.player.hand.contains(&candidate), !quest);
    }
}

#[test]
fn step13_quest_cost_type_filters_preserve_real_skills() {
    for kind in [PowerKind::Corruption, PowerKind::FreeSkill] {
        let mut s = state();
        power(&mut s, kind, 1);
        let listener = ListenerRef::Power(s.player_power_uids()[0]);
        for id in [
            CardId::SpoilsMap,
            CardId::ByrdonisEgg,
            CardId::LanternKey,
            CardId::DefendIronclad,
        ] {
            let uid = add(&mut s, id);
            s.player.hand.push(uid);
            for known_uid in [None, Some(uid)] {
                let ctx = sts2_core::hooks::CardCostCtx {
                    card_id: Some(id),
                    card_uid: known_uid,
                    card_type: Some(CardType::Skill),
                    base_cost: 1,
                    ..Default::default()
                };
                assert_eq!(
                    sts2_core::hooks::modify_card_cost(&s, listener, &ctx).is_some(),
                    !id.is_quest()
                );
            }
        }
    }
}
