//! A merchant entry is affordable at the price the shop shows and charges
//! (sts2-agent fork, shop-discount-buy). C# `MerchantEntry.EnoughGold` is
//! `Cost <= Player.Gold`, and `Cost` folds `Hook.ModifyMerchantPrice`
//! (The Courier, Membership Card) over the rolled price while the current room
//! is a `MerchantRoom`. The Fake Merchant's shop is an event room's, so its
//! entries are affordable at their own price.

use sts2_content::run_callbacks::{
    default_relic_grab_bag, new_run_manager_for_character_from_string_seed,
};
use sts2_core::events::EventId;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};
use sts2_core::shop::{is_shop_entry_available, merchant_price_with_modifiers};
use sts2_core::RelicId;

fn seed_3_in(room: ForcedRoom<'_>, relics: &[RelicId]) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_from_string_seed("3", CharacterId::Ironclad)
        .with_neow_start();
    let (by_rarity, fallback, boss) = default_relic_grab_bag();
    rm.run_state
        .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
    rm.run_state.interactive_rewards = true;
    rm.run_state.relics.extend_from_slice(relics);
    rm.run_state.gold = 0;
    assert_eq!(rm.force_enter_room(room), ActionResult::Advanced);
    assert_eq!(rm.phase(), Phase::Shop);
    rm
}

/// Every stocked entry's (action, rolled price), in the shop's order.
fn entries(rm: &RunManager) -> Vec<(RunAction, i32)> {
    let inv = rm.shop_inventory().expect("a shop inventory");
    let mut out = Vec::new();
    for (idx, e) in inv
        .cards
        .iter()
        .chain(inv.colorless_cards.iter())
        .enumerate()
    {
        if is_shop_entry_available(e.price) && e.card.is_some() {
            out.push((RunAction::ShopBuyCard { idx }, e.price));
        }
    }
    for (idx, e) in inv.relics.iter().enumerate() {
        if is_shop_entry_available(e.price) && e.relic_id.is_some() {
            out.push((RunAction::ShopBuyRelic { idx }, e.price));
        }
    }
    for (idx, e) in inv.potions.iter().enumerate() {
        if is_shop_entry_available(e.price) && e.potion_kind.is_some() {
            out.push((RunAction::ShopBuyPotion { idx }, e.price));
        }
    }
    out
}

#[test]
fn discounted_entries_are_buyable_with_the_gold_their_shown_price_needs() {
    let relics = [RelicId::MembershipCard, RelicId::TheCourier];
    let mut rm = seed_3_in(ForcedRoom::Shop, &relics);
    let stocked = entries(&rm);
    assert!(!stocked.is_empty());
    let mut offered = Vec::new();
    for (action, base) in &stocked {
        let cost = merchant_price_with_modifiers(&relics, *base);
        assert!(cost < *base);
        rm.run_state.gold = cost;
        offered.push((*base, cost, rm.get_available_actions().contains(action)));
        rm.run_state.gold = cost - 1;
        assert!(!rm.get_available_actions().contains(action));
    }
    assert!(
        offered.iter().all(|(_, _, ok)| *ok),
        "(base, shown price, offered at that gold): {offered:?}"
    );

    // Card removal is a merchant entry too.
    let base = rm.shop_inventory().expect("inventory").removal_cost;
    let cost = merchant_price_with_modifiers(&relics, base);
    rm.run_state.gold = cost;
    let removes = rm
        .get_available_actions()
        .iter()
        .filter(|a| matches!(a, RunAction::ShopRemoveCard { .. }))
        .count();
    assert_eq!(
        removes,
        rm.run_state.master_deck.len(),
        "removal at {cost} (base {base})"
    );

    // The purchase charges the shown price.
    let (action, base) = stocked[0];
    let cost = merchant_price_with_modifiers(&relics, base);
    rm.run_state.gold = cost;
    rm.take_action(action);
    assert_eq!(rm.run_state.gold, 0);
}

#[test]
fn fake_merchant_relics_are_affordable_at_their_own_price() {
    let relics = [RelicId::MembershipCard];
    let mut rm = seed_3_in(ForcedRoom::Event(EventId::FakeMerchant), &relics);
    let stocked = entries(&rm);
    assert_eq!(stocked.len(), 6);
    for (action, base) in stocked {
        rm.run_state.gold = base - 1;
        assert!(!rm.get_available_actions().contains(&action));
        rm.run_state.gold = base;
        assert!(rm.get_available_actions().contains(&action));
    }
}
