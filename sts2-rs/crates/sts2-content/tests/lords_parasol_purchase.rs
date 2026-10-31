//! Lord's Parasol buys everything on entering a merchant room (sts2-agent
//! fork, lords-parasol-purchase). C# `LordsParasol.AfterRoomEntered` runs
//! `PurchaseEverything`: every stocked character card, colorless card, relic
//! and potion through `OnTryPurchaseWrapper(inventory, ignoreCost: true)`
//! (no gold lost; a potion with no open slot fails and stays stocked), then
//! the card removal with the cost ignored and not cancelable. Values: the
//! Headless scenarios `lords_parasol_buys_everything` and
//! `lords_parasol_full_belt` (Ironclad, seed "3", forced room entry).

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_from_string_seed,
};
use sts2_core::events::EventId;
use sts2_core::potions::{CharacterId, PotionKind};
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};
use sts2_core::shop::is_shop_entry_available;
use sts2_core::RelicId;

fn seed_3_with_the_parasol(potions: &[PotionKind]) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_from_string_seed("3", CharacterId::Ironclad)
        .with_neow_start();
    initialize_run_setup(&mut rm);
    rm.run_state.interactive_rewards = true;
    rm.run_state.interactive_card_removal = true;
    rm.run_state.interactive_potions = true;
    rm.run_state.relics = vec![RelicId::BurningBlood, RelicId::LordsParasol];
    rm.run_state.potions = potions.to_vec();
    rm
}

fn deck_names(rm: &RunManager) -> Vec<String> {
    rm.run_state
        .master_deck
        .iter()
        .map(|c| format!("{:?}", c.card_id))
        .collect()
}

fn nothing_stocked(rm: &RunManager) -> bool {
    let inv = rm.shop_inventory().expect("inventory");
    inv.cards
        .iter()
        .chain(inv.colorless_cards.iter())
        .all(|e| !is_shop_entry_available(e.price))
        && inv.relics.iter().all(|e| !is_shop_entry_available(e.price))
        && inv
            .potions
            .iter()
            .all(|e| !is_shop_entry_available(e.price))
}

#[test]
fn entering_a_shop_buys_everything_then_opens_the_removal() {
    let mut rm = seed_3_with_the_parasol(&[]);
    let deck_before = rm.run_state.master_deck.len();
    assert_eq!(rm.run_state.gold, 99);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Shop);
    // The 7 cards, the 3 relics and the 3 potions are bought for nothing.
    assert_eq!(
        (
            rm.run_state.gold,
            rm.run_state.master_deck.len() - deck_before,
            rm.run_state.relics.clone(),
            rm.run_state.potions.clone(),
        ),
        (
            99,
            7,
            vec![
                RelicId::BurningBlood,
                RelicId::LordsParasol,
                RelicId::UnsettlingLamp,
                RelicId::MealTicket,
                RelicId::WingCharm,
            ],
            vec![
                PotionKind::FlexPotion,
                PotionKind::SpeedPotion,
                PotionKind::PowerPotion,
            ],
        )
    );
    // In the game's order (the scenario's Setup Strike, Breakthrough, Flame
    // Barrier, Shrug It Off, Feel No Pain, Flash of Steel, Secret Weapon).
    assert_eq!(
        deck_names(&rm)[deck_before..],
        [
            "SetupStrikeCard",
            "Breakthrough",
            "FlameBarrierCard",
            "ShrugItOff",
            "FeelNoPainCard",
            "FlashOfSteel",
            "SecretWeapon",
        ]
    );
    assert!(nothing_stocked(&rm));
    // Then the removal, not cancelable: only deck picks are offered.
    assert_eq!(rm.run_state.pending_deck_removals, 1);
    let actions = rm.get_available_actions();
    assert!(actions
        .iter()
        .all(|a| matches!(a, RunAction::RemoveDeckCard { .. })));
    let order = rm.run_state.removal_offer_order();
    let bash = order
        .iter()
        .position(|&d| format!("{:?}", rm.run_state.master_deck[d].card_id) == "Bash")
        .expect("Bash is removable");
    assert_eq!(
        rm.take_action(RunAction::RemoveDeckCard { idx: bash }),
        ActionResult::Advanced
    );
    assert!(!deck_names(&rm).iter().any(|d| d == "Bash"));
    assert_eq!(rm.phase(), Phase::Shop);
    assert_eq!(rm.run_state.gold, 99);
    let inv = rm.shop_inventory().expect("inventory");
    assert!(inv.removal_used);
    assert_eq!(rm.run_state.card_shop_removals_used, 1);
    // Nothing left to buy (the belt's discards aside).
    assert!(rm
        .get_available_actions()
        .iter()
        .all(|a| matches!(a, RunAction::LeaveShop | RunAction::DiscardPotion { .. })));
}

#[test]
fn a_full_belt_leaves_the_potions_stocked() {
    let mut rm = seed_3_with_the_parasol(&[PotionKind::FirePotion; 3]);
    // Without the belt's actions there is no pause before each potion
    // (lords-parasol-pause: tests/lords_parasol_pause.rs).
    rm.run_state.interactive_potions = false;
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.potions, vec![PotionKind::FirePotion; 3]);
    let inv = rm.shop_inventory().expect("inventory");
    assert!(inv.potions.iter().all(|e| is_shop_entry_available(e.price)));
    assert!(inv.relics.iter().all(|e| !is_shop_entry_available(e.price)));
    assert_eq!(rm.run_state.relics.len(), 5);
    assert_eq!(rm.run_state.pending_deck_removals, 1);
}

#[test]
fn the_fake_merchant_is_not_a_merchant_room() {
    let mut rm = seed_3_with_the_parasol(&[]);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::FakeMerchant)),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.relics.len(), 2);
    assert_eq!(rm.run_state.pending_deck_removals, 0);
    let inv = rm.shop_inventory().expect("inventory");
    assert!(inv.relics.iter().all(|e| is_shop_entry_available(e.price)));
}

/// A bought relic that offers rewards (Cauldron's potions) holds the
/// purchases until its set is done (C# `OnTryPurchaseWrapper` awaits the
/// obtain); then the potions and the removal follow. Seed "shop14" stocks
/// Cauldron as its last relic.
#[test]
fn a_relics_rewards_hold_the_purchases_until_they_are_done() {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_from_string_seed("shop14", CharacterId::Ironclad)
        .with_neow_start();
    initialize_run_setup(&mut rm);
    rm.run_state.interactive_rewards = true;
    rm.run_state.interactive_potions = true;
    rm.run_state.interactive_card_removal = true;
    rm.run_state.relics.push(RelicId::LordsParasol);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::CardReward);
    assert_eq!(rm.run_state.relics.last(), Some(&RelicId::Cauldron));
    let inv = rm.shop_inventory().expect("inventory");
    assert!(inv.potions.iter().all(|e| is_shop_entry_available(e.price)));
    assert_eq!(rm.run_state.pending_deck_removals, 0);
    assert_eq!(
        rm.take_action(RunAction::ProceedRewards),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Shop);
    assert_eq!(rm.run_state.potions.len(), 3);
    assert!(nothing_stocked(&rm));
    assert_eq!(rm.run_state.pending_deck_removals, 1);
}
