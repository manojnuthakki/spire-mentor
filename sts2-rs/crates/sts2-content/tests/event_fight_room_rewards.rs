//! An event's fight that does not resume the event (C#
//! `EventModel.EnterCombatWithoutExitingEvent(..., shouldResumeAfterCombat:
//! false)`) is a `CombatRoom` of its encounter's room type: once won,
//! `CombatRoom.OfferRoomEndRewards` → `RewardsSet.WithRewardsFromRoom` gives
//! the room's own rewards (a Monster room's gold, potion roll and
//! `CardReward(ForRoom(Monster))`) plus the event's extra rewards
//! (`CombatRoom.AddExtraReward`), sorted by `RewardsSetIndex`, as the room's
//! set (not a custom one). Only an encounter with `ShouldGiveRewards == false`
//! (Battleworn Dummy's) offers an empty set. The gold is the encounter's own
//! (`FakeMerchantEventEncounter`: 300). Values: the Headless scenarios
//! `foul_potion_fake_merchant_fight` (Ironclad, seed "3") and
//! `lantern_key_to_war_historian_repy`. sts2-agent fork,
//! `event-fight-room-rewards`.

use sts2_content::run_callbacks::{
    default_relic_grab_bag, new_run_manager_for_character_from_string_seed,
};
use sts2_core::cards::CardId;
use sts2_core::events::EventId;
use sts2_core::potions::{CharacterId, PotionKind};
use sts2_core::run_manager::{
    ActionResult, ForcedRoom, Phase, RunAction, RunManager, ScreenReward,
};
use sts2_core::RelicId;

fn seed_3_at(event: EventId, gold: i32, potions: &[PotionKind]) -> RunManager {
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
    rm.run_state.potions = potions.to_vec();
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(event)),
        ActionResult::Advanced
    );
    rm
}

/// Play the fight out (HP pinned high), each turn's first playable card.
fn win_the_fight(rm: &mut RunManager) {
    rm.debug_set_player(Some(9999), Some(9999), None);
    for _ in 0..5000 {
        if rm.phase() != Phase::Combat {
            return;
        }
        let actions = rm.get_available_actions();
        let a = actions
            .iter()
            .copied()
            .find(|a| matches!(a, RunAction::PlayCard { .. }))
            .unwrap_or(actions[0]);
        rm.take_action(a);
    }
    panic!("the fight never ended");
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Gold(i32),
    Potion,
    Relic(RelicId),
    Special(CardId),
    Card(usize),
    Remove,
}

fn screen(rm: &RunManager) -> Vec<Kind> {
    rm.reward_screen()
        .expect("the rewards screen")
        .iter()
        .map(|r| match r {
            ScreenReward::Gold { amount } => Kind::Gold(*amount),
            ScreenReward::Potion { .. } => Kind::Potion,
            ScreenReward::Relic { relic } => Kind::Relic(relic.relic_id),
            ScreenReward::SpecialCard { card } => Kind::Special(card.card_id),
            ScreenReward::Card { options, .. } => Kind::Card(options.len()),
            ScreenReward::RemoveCard => Kind::Remove,
        })
        .collect()
}

#[test]
fn foul_potion_fake_merchant_fight_offers_the_rooms_rewards_and_its_extras() {
    let mut rm = seed_3_at(
        EventId::FakeMerchant,
        99,
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
    assert_eq!(
        rm.take_action(RunAction::DrinkPotion {
            potion_slot: 0,
            target: None
        }),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Combat);
    win_the_fight(&mut rm);
    assert_eq!(rm.phase(), Phase::CardReward);
    let set = rm.reward_set().expect("a rewards set");
    assert!(
        !set.custom,
        "the room's set (source room), not a custom one"
    );
    // The encounter's 300 gold (no potion dropped on this seed), the rug and
    // the five unsold fake relics, then the room's card reward.
    assert_eq!(
        screen(&rm),
        vec![
            Kind::Gold(300),
            Kind::Relic(RelicId::FakeMerchantsRug),
            Kind::Relic(RelicId::FakeVenerableTeaSet),
            Kind::Relic(RelicId::FakeSneckoEye),
            Kind::Relic(RelicId::FakeBloodVial),
            Kind::Relic(RelicId::FakeHappyFlower),
            Kind::Relic(RelicId::FakeAnchor),
            Kind::Card(3),
        ]
    );
    assert!(rm.run_state.current_event.is_none(), "the event is over");
    // Claim the rug, proceed: the map, the event not resumed.
    assert_eq!(
        rm.take_action(RunAction::ClaimReward { idx: 1 }),
        ActionResult::Advanced
    );
    assert!(rm.run_state.relics.contains(&RelicId::FakeMerchantsRug));
    assert_eq!(
        rm.take_action(RunAction::ProceedRewards),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::MapChoice);
    assert_eq!(rm.run_state.gold, 45);
}

#[test]
fn lantern_key_fight_offers_the_rooms_rewards_and_the_key() {
    let mut rm = seed_3_at(EventId::TheLanternKey, 99, &[]);
    assert_eq!(rm.phase(), Phase::Event);
    // KEEP_THE_KEY, then FIGHT.
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 1 }),
        ActionResult::Advanced
    );
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Combat);
    let before = rm.run_state.rng.rewards.counter();
    win_the_fight(&mut rm);
    assert_eq!(rm.phase(), Phase::CardReward);
    let set = rm.reward_set().expect("a rewards set");
    assert!(
        !set.custom,
        "the room's set (source room), not a custom one"
    );
    let listed = screen(&rm);
    // A Monster room's gold (10-20 at A0), maybe a potion, the key (a special
    // card), then the room's three-card reward.
    let Some(Kind::Gold(gold)) = listed.first() else {
        panic!("the room's gold first: {listed:?}");
    };
    assert!((10..=20).contains(gold), "Monster gold: {gold}");
    let rest: Vec<&Kind> = listed[1..]
        .iter()
        .filter(|k| !matches!(k, Kind::Potion))
        .collect();
    assert_eq!(
        rest,
        vec![&Kind::Special(CardId::LanternKey), &Kind::Card(3)]
    );
    // Potion odds, gold, (potion), three cards' reads: the room's rolls.
    assert!(rm.run_state.rng.rewards.counter() > before + 3);
    assert!(rm.run_state.current_event.is_none(), "the event is over");
    let key = listed
        .iter()
        .position(|k| matches!(k, Kind::Special(_)))
        .expect("the key");
    assert_eq!(
        rm.take_action(RunAction::ClaimReward { idx: key }),
        ActionResult::Advanced
    );
    assert!(rm
        .run_state
        .master_deck
        .iter()
        .any(|c| c.card_id == CardId::LanternKey));
    assert_eq!(
        rm.take_action(RunAction::ProceedRewards),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::MapChoice);
}
