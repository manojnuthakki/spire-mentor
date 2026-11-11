//! An egg relic bought in the shop upgrades the cards already on the shelf (sts2-agent fork, egg-shop-upgrades).
//! C# `MerchantInventory.UpdateEntries` runs on every entry's `PurchaseCompleted` and each card entry's
//! `UpdateEntry` re-runs `Hook.ModifyMerchantCardCreationResults`. No reroll, no RNG.

use sts2_content::run_callbacks::{
    default_relic_grab_bag, new_run_manager_for_character_from_string_seed,
};
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};
use sts2_core::RelicId;

fn shop() -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm =
        new_run_manager_for_character_from_string_seed("TD9HVE4Q6H", CharacterId::Ironclad)
            .with_neow_start();
    let (by_rarity, fallback, boss) = default_relic_grab_bag();
    rm.run_state
        .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
    rm.run_state.interactive_rewards = true;
    rm.run_state.gold = 2000;
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Shop);
    rm
}

fn offers(rm: &RunManager, ty: sts2_core::cards::CardType) -> Vec<(String, bool, i32, bool)> {
    let inv = rm.shop_inventory().unwrap();
    inv.cards
        .iter()
        .chain(&inv.colorless_cards)
        .map(|e| {
            let c = e.card.as_ref().unwrap();
            (
                format!("{:?}", c.card_id),
                c.upgraded,
                e.price,
                c.card_type == ty && c.is_upgradable(),
            )
        })
        .collect()
}

fn buy_egg_then_compare(egg: RelicId, expect_type: sts2_core::cards::CardType) {
    let mut rm = shop();
    // put the egg on sale in relic slot 0 (the rolled relics are random)
    rm.debug_shop_inventory_mut().unwrap().relics[0].relic_id = Some(egg);
    let before = offers(&rm, expect_type);
    let (shops, rewards) = (
        rm.run_state.rng.shops.counter(),
        rm.run_state.rng.rewards.counter(),
    );
    assert_eq!(
        rm.take_action(RunAction::ShopBuyRelic { idx: 0 }),
        ActionResult::Advanced
    );
    assert!(rm.run_state.relics.contains(&egg));
    let after = offers(&rm, expect_type);
    assert_eq!(before.len(), after.len());
    let mut changed = 0;
    for (b, a) in before.iter().zip(&after) {
        assert_eq!(b.0, a.0, "the same cards stay on the shelf");
        assert_eq!(b.2, a.2, "prices are not rolled again");
        assert_eq!(a.1, b.1 || b.3, "{} upgraded after the {egg:?}", a.0);
        changed += usize::from(a.1 && !b.1);
    }
    assert!(changed > 0, "{egg:?}: some offer was upgraded");
    assert_eq!(
        (shops, rewards),
        (
            rm.run_state.rng.shops.counter(),
            rm.run_state.rng.rewards.counter()
        ),
        "the refresh draws no RNG"
    );
}

#[test]
fn buying_frozen_egg_upgrades_the_powers_on_the_shelf() {
    buy_egg_then_compare(RelicId::FrozenEgg, sts2_core::cards::CardType::Power);
}

#[test]
fn buying_molten_egg_upgrades_the_attacks_on_the_shelf() {
    buy_egg_then_compare(RelicId::MoltenEgg, sts2_core::cards::CardType::Attack);
}
