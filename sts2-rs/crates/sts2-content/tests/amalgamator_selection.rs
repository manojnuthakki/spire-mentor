//! Amalgamator's filtered, instance-sensitive removal and deferred add.
//! Authority: Amalgamator.CombineStrikes/CombineDefends/IsValid,
//! CardSelectCmd.FromDeckForRemoval/FromDeckGeneric, CardPileCmd.RemoveFromDeck/Add
//! (installed v0.107.1; extracts kept outside the repository).

use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_content::run_callbacks::{
    initialize_run_setup, new_ironclad_run_manager_from_string_seed,
};
use sts2_core::cards::{Card, CardId, CardTag, CardTags, EnchantmentId};
use sts2_core::events::EventId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};

fn card(id: CardId, upgraded: bool) -> Card {
    card_pod_from_id(id, upgraded).expect("card spec")
}

fn in_event(interactive: bool) -> RunManager {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
    let mut rm = new_ironclad_run_manager_from_string_seed("3");
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        let action = rm.get_available_actions()[0];
        rm.take_action(action);
    }
    assert_eq!(rm.phase(), Phase::MapChoice);
    rm.run_state.interactive_card_removal = interactive;
    rm.run_state
        .relics
        .extend([RelicId::LuckyFysh, RelicId::MoltenEgg, RelicId::ToxicEgg]);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::Amalgamator)),
        ActionResult::Advanced
    );
    rm
}

fn combine(rm: &mut RunManager, defend: bool) {
    assert_eq!(
        rm.take_action(RunAction::EventChoice {
            idx: usize::from(defend)
        }),
        ActionResult::Advanced
    );
}

fn branch(defend: bool) -> (CardId, CardId, CardId, CardTag) {
    if defend {
        (
            CardId::DefendIronclad,
            CardId::StrikeIronclad,
            CardId::UltimateDefend,
            CardTag::Defend,
        )
    } else {
        (
            CardId::StrikeIronclad,
            CardId::DefendIronclad,
            CardId::UltimateStrike,
            CardTag::Strike,
        )
    }
}

fn differing_selections(defend: bool) {
    let (basic, other, ultimate, tag) = branch(defend);
    for picks in [[2, 0], [1, 0]] {
        let mut rm = in_event(true);
        let mut eternal = card(basic, false);
        sts2_content::enchantments::apply_static_enchantment(
            &mut eternal,
            EnchantmentId::TezcatarasEmber,
            1,
        );
        let mut enchanted = card(basic, false);
        enchanted.set_enchantment(EnchantmentId::Spiral, 1);
        // A non-Basic with the requested tag and a wrong-tag Basic must
        // both stay out; the Eternal same-id copy precedes the valid ones.
        let mut nonbasic = card(CardId::PerfectedStrike, false);
        nonbasic.tags = CardTags::one(tag);
        rm.run_state.master_deck = vec![
            eternal,
            nonbasic,
            card(basic, false),
            card(other, false),
            card(basic, true),
            enchanted,
        ];
        let before = rm.run_state.master_deck.clone();
        let gold = rm.run_state.gold;
        let rng = format!("{:?}", rm.run_state.rng);
        combine(&mut rm, defend);
        assert_eq!(rm.run_state.pending_deck_removals, 2, "choice required");
        assert_eq!(rm.run_state.master_deck, before, "nothing removed or added");
        assert_eq!(rm.run_state.gold, gold, "Lucky Fysh has not fired");
        assert_eq!(rm.run_state.removal_offer_order(), vec![2, 4, 5]);
        assert_eq!(rm.get_available_actions().len(), 3);
        assert_eq!(
            rm.take_action(RunAction::RemoveDeckCard { idx: 3 }),
            ActionResult::InvalidForPhase
        );
        assert_eq!(
            rm.take_action(RunAction::RemoveDeckCard { idx: picks[0] }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.pending_deck_removals, 1);
        assert_eq!(rm.run_state.gold, gold, "no hook between removals");
        assert!(!rm
            .run_state
            .master_deck
            .iter()
            .any(|c| c.card_id == ultimate));
        assert_eq!(
            rm.take_action(RunAction::RemoveDeckCard { idx: picks[1] }),
            ActionResult::Advanced
        );
        let survivor = if picks[0] == 2 {
            &before[4]
        } else {
            &before[5]
        };
        assert_eq!(
            &rm.run_state.master_deck[..4],
            &[before[0], before[1], before[3], *survivor]
        );
        let added = rm.run_state.master_deck.last().unwrap();
        assert_eq!(added.card_id, ultimate);
        assert!(added.upgraded, "the matching egg modifies the deferred add");
        assert_eq!(rm.run_state.gold, gold + 15, "Lucky Fysh exactly once");
        assert_eq!(format!("{:?}", rm.run_state.rng), rng, "no RNG consumed");
        assert_eq!(rm.run_state.pending_deck_removals, 0);
        assert_eq!(rm.phase(), Phase::MapChoice);
        assert_eq!(
            rm.run_state.removal_offer_order().len(),
            4,
            "filter cleared"
        );
        assert_eq!(
            rm.take_action(RunAction::RemoveDeckCard { idx: 0 }),
            ActionResult::InvalidForPhase
        );
        assert_eq!(rm.run_state.gold, gold + 15, "no repeated continuation");
    }
}

#[test]
fn amalgamator_strikes_choose_distinguishable_instances_before_add_hooks() {
    differing_selections(false);
}

#[test]
fn amalgamator_defends_choose_distinguishable_instances_before_add_hooks() {
    differing_selections(true);
}

#[test]
fn amalgamator_zero_one_and_exactly_two_candidates_complete_without_prompt() {
    for defend in [false, true] {
        let (basic, other, ultimate, _) = branch(defend);
        for n in 0..=2 {
            let mut rm = in_event(true);
            rm.run_state.master_deck = vec![card(other, false), card(CardId::Greed, false)];
            rm.run_state
                .master_deck
                .extend((0..n).map(|_| card(basic, true)));
            let gold = rm.run_state.gold;
            let rng = format!("{:?}", rm.run_state.rng);
            combine(&mut rm, defend);
            assert_eq!(rm.run_state.pending_deck_removals, 0);
            assert_eq!(rm.run_state.master_deck.len(), 3);
            assert_eq!(rm.run_state.master_deck[0].card_id, other);
            assert_eq!(rm.run_state.master_deck[1].card_id, CardId::Greed);
            assert_eq!(rm.run_state.master_deck[2].card_id, ultimate);
            assert!(rm.run_state.master_deck[2].upgraded);
            assert_eq!(rm.run_state.gold, gold + 15);
            assert_eq!(format!("{:?}", rm.run_state.rng), rng);
            assert_eq!(rm.phase(), Phase::MapChoice);
            assert_eq!(rm.run_state.removal_offer_order().len(), 2);
        }
    }
}

#[test]
fn amalgamator_noninteractive_keeps_filtered_first_two_and_add_hooks() {
    for defend in [false, true] {
        let (basic, other, ultimate, _) = branch(defend);
        let mut rm = in_event(false);
        let mut enchanted = card(basic, false);
        enchanted.set_enchantment(EnchantmentId::Spiral, 1);
        rm.run_state.master_deck = vec![
            card(other, false),
            card(basic, true),
            card(basic, false),
            enchanted,
        ];
        let gold = rm.run_state.gold;
        combine(&mut rm, defend);
        assert_eq!(rm.run_state.pending_deck_removals, 0);
        assert_eq!(rm.run_state.master_deck.len(), 3);
        assert_eq!(rm.run_state.master_deck[0].card_id, other);
        assert_eq!(rm.run_state.master_deck[1], enchanted);
        assert_eq!(rm.run_state.master_deck[2].card_id, ultimate);
        assert!(rm.run_state.master_deck[2].upgraded);
        assert_eq!(rm.run_state.gold, gold + 15);
    }
}
