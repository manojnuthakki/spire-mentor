//! Spiraling Whirlpool's Observe the Spiral enchants a Basic Strike or Defend.
//!
//! The game's SpiralingWhirlpool.ObserveTheSpiral opens
//! FromDeckForEnchantment(Spiral) over `Spiral.CanEnchant`: an enchantable
//! Basic card tagged Strike or Defend. The engine queued a card reward instead
//! (MZ96UQVRSB, VU45NWWFP8: a card reward on the Sim, the enchant pick on
//! Headless), and its event rule took any enchantable card.

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::cards::{CardId, EnchantmentId};
use sts2_core::events::EventId;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{ForcedRoom, Phase, RunAction};

#[test]
fn observe_the_spiral_opens_the_spiral_pick_over_basic_strikes_and_defends() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_with_ascension(3, CharacterId::Ironclad, 0);
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        let a = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    for id in [CardId::PerfectedStrike, CardId::Inflame] {
        let card = sts2_content::cards::spec_index::card_pod_from_id(id, false).expect("pod");
        rm.run_state.master_deck.push(card);
    }
    // The Sim turns interactive deck picks on (sim.ENGINE_FLAGS).
    rm.run_state.interactive_card_transform = true;
    rm.force_enter_room(ForcedRoom::Event(EventId::SpiralingWhirlpool));
    rm.take_action(RunAction::EventChoice { idx: 0 });
    assert!(rm.run_state.pending_rewards.is_empty(), "no card reward");
    assert_eq!(
        rm.run_state
            .pending_deck_enchants
            .first()
            .map(|e| e.enchantment),
        Some(EnchantmentId::Spiral)
    );
    let eligible: Vec<CardId> = rm
        .enchant_eligible_deck_indices()
        .into_iter()
        .map(|i| rm.run_state.master_deck[i].card_id)
        .collect();
    assert_eq!(eligible.len(), 9, "{eligible:?}");
    assert!(eligible
        .iter()
        .all(|c| matches!(c, CardId::StrikeIronclad | CardId::DefendIronclad)));
}
