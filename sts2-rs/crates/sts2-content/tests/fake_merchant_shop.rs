//! The Fake Merchant event (C# `FakeMerchant`, sts2-agent fork, S18): no
//! options; `BeforeEventStarted` stocks a `MerchantInventory` with 6 of the 9
//! fake relics (shuffled on the event's own stream), each priced by its
//! `MerchantRelicEntry` (`MerchantCost` 50 × `Shops.NextFloat(0.85, 1.15)`),
//! sold by the `NFakeMerchant` node. Prices skip the merchant price hooks and
//! nothing restocks (both need a `MerchantRoom`); there are no cards, potions
//! or card removal. A Foul Potion thrown at the merchant
//! (`FoulPotion.OnUse` → `FakeMerchant.FoulPotionThrown`) enters
//! `FakeMerchantEventEncounter` with the rug and every unsold relic as extra
//! rewards. Values: the Headless scenarios `fake_merchant_shop` and
//! `foul_potion_fake_merchant_fight` (Ironclad, seed "3", forced room entry).

use sts2_content::run_callbacks::{
    default_relic_grab_bag, new_run_manager_for_character_from_string_seed,
};
use sts2_core::events::{EventEncounter, EventId};
use sts2_core::monsters::MonsterKind;
use sts2_core::potions::{CharacterId, PotionKind};
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};
use sts2_core::runstate::Reward;
use sts2_core::RelicId;

fn seed_3_at_the_fake_merchant(
    gold: i32,
    relics: &[RelicId],
    potions: &[PotionKind],
) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_from_string_seed("3", CharacterId::Ironclad)
        .with_neow_start();
    let (by_rarity, fallback, boss) = default_relic_grab_bag();
    rm.run_state
        .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
    rm.run_state.interactive_rewards = true;
    rm.run_state.interactive_potions = true;
    rm.run_state.gold = gold;
    rm.run_state.relics.extend_from_slice(relics);
    rm.run_state.potions = potions.to_vec();
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::FakeMerchant)),
        ActionResult::Advanced
    );
    rm
}

fn stock(rm: &RunManager) -> Vec<(Option<RelicId>, i32)> {
    rm.shop_inventory()
        .expect("the fake merchant's inventory")
        .relics
        .iter()
        .map(|e| (e.relic_id, e.price))
        .collect()
}

#[test]
fn fake_merchant_is_a_relics_only_shop_at_its_entries_prices() {
    let mut rm = seed_3_at_the_fake_merchant(
        120,
        &[RelicId::MembershipCard],
        &[PotionKind::FoulPotion, PotionKind::BloodPotion],
    );
    assert_eq!(rm.phase(), Phase::Shop);
    assert_eq!(rm.run_state.current_event, Some(EventId::FakeMerchant));
    assert_eq!(
        stock(&rm),
        vec![
            (Some(RelicId::FakeLeesWaffle), 54),
            (Some(RelicId::FakeVenerableTeaSet), 54),
            (Some(RelicId::FakeSneckoEye), 49),
            (Some(RelicId::FakeBloodVial), 46),
            (Some(RelicId::FakeHappyFlower), 55),
            (Some(RelicId::FakeAnchor), 51),
        ]
    );
    let inv = rm.shop_inventory().expect("inventory");
    assert!(inv.cards.is_empty() && inv.colorless_cards.is_empty() && inv.potions.is_empty());
    assert!(inv.fake_merchant);
    let actions = rm.get_available_actions();
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, RunAction::ShopRemoveCard { .. })),
        "no card removal: {actions:?}"
    );
    // The Foul Potion can be thrown at the merchant.
    assert!(actions.contains(&RunAction::DrinkPotion {
        potion_slot: 0,
        target: None
    }));

    // Membership Card doesn't halve the price outside a MerchantRoom.
    assert_eq!(
        rm.take_action(RunAction::ShopBuyRelic { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.gold, 66);
    assert_eq!(rm.run_state.relics.last(), Some(&RelicId::FakeLeesWaffle));
    assert_eq!(stock(&rm)[0].0, Some(RelicId::FakeLeesWaffle));
    assert!(!sts2_core::shop::is_shop_entry_available(stock(&rm)[0].1));
    assert_eq!(
        rm.take_action(RunAction::ShopBuyRelic { idx: 1 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.gold, 12);
    assert!(!rm
        .get_available_actions()
        .iter()
        .any(|a| matches!(a, RunAction::ShopBuyRelic { .. })));

    // Proceed leaves the event for the map.
    assert_eq!(rm.take_action(RunAction::LeaveShop), ActionResult::Advanced);
    assert_eq!(rm.phase(), Phase::MapChoice);
    assert_eq!(rm.run_state.current_event, None);
    assert!(rm.shop_inventory().is_none());
}

#[test]
fn courier_restocks_nothing_at_the_fake_merchant() {
    let mut rm = seed_3_at_the_fake_merchant(120, &[RelicId::TheCourier], &[]);
    // The Courier's discount is a merchant price hook too: the entry's price.
    let before = rm.run_state.rng.shops.counter();
    assert_eq!(
        rm.take_action(RunAction::ShopBuyRelic { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.gold, 66);
    assert_eq!(rm.run_state.rng.shops.counter(), before, "no restock roll");
    assert!(!sts2_core::shop::is_shop_entry_available(stock(&rm)[0].1));
}

#[test]
fn foul_potion_thrown_at_the_fake_merchant_starts_its_fight() {
    let mut rm = seed_3_at_the_fake_merchant(
        99,
        &[],
        &[
            PotionKind::FoulPotion,
            PotionKind::FoulPotion,
            PotionKind::FoulPotion,
        ],
    );
    assert_eq!(
        rm.take_action(RunAction::ShopBuyRelic { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.gold, 45);
    assert_eq!(
        rm.take_action(RunAction::DrinkPotion {
            potion_slot: 0,
            target: None
        }),
        ActionResult::Advanced
    );
    // No 100 gold (that is a MerchantRoom's); the fight against The Merchant???.
    assert_eq!(rm.run_state.gold, 45);
    assert_eq!(rm.phase(), Phase::Combat);
    assert!(rm.shop_inventory().is_none());
    let combat = rm.combat.as_ref().expect("the fight");
    assert_eq!(combat.player.potions.len(), 2);
    let enemy = combat
        .creatures
        .iter()
        .find(|c| c.species.is_some())
        .expect("an enemy");
    assert_eq!(enemy.species, Some(MonsterKind::FakeMerchantMonster));
    assert_eq!(enemy.max_hp, 165);
    let req = rm.run_state.event_combat.as_ref().expect("an event fight");
    assert_eq!(req.encounter, EventEncounter::FakeMerchant);
    assert!(!req.resume);
    let extras: Vec<RelicId> = req
        .victory_rewards
        .iter()
        .map(|r| match r {
            Reward::Relic(rr) => rr.relic_id,
            other => panic!("non-relic extra reward: {other:?}"),
        })
        .collect();
    // The rug, then every unsold relic in the inventory's order.
    assert_eq!(
        extras,
        vec![
            RelicId::FakeMerchantsRug,
            RelicId::FakeVenerableTeaSet,
            RelicId::FakeSneckoEye,
            RelicId::FakeBloodVial,
            RelicId::FakeHappyFlower,
            RelicId::FakeAnchor,
        ]
    );
}
