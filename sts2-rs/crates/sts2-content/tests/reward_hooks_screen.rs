//! The rewards relics add to a won combat room (C# `Hook.ModifyRewards` in
//! `RewardsSet.GenerateWithoutOffering`, sts2-agent fork, `reward-hooks`):
//! Prayer Wheel's `CardReward(ForRoom(Monster))` in a Monster room, White
//! Star's `CardReward(ForRoom(Boss))` and Black Star's `RelicReward(player)`
//! in an Elite room. The set populates the room's own rewards and its extras
//! first, then the hooks' (`Populate` over the added rewards in list order),
//! so the room's card reward and an elite's relic roll as they would without
//! the hooks, and the hooks' rewards roll after them on the `Rewards` stream.
//! The scenario `rewards_screen_after_combat` (Prayer Wheel) is the Headless
//! side.

use sts2_content::run_callbacks::{
    default_relic_grab_bag, new_run_manager_for_character_from_string_seed,
};
use sts2_core::cards::CardId;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{
    ActionResult, ForcedRoom, Phase, RunAction, RunManager, ScreenReward,
};
use sts2_core::RelicId;

fn fight(encounter: &str, relics: &[RelicId]) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_from_string_seed("0", CharacterId::Ironclad)
        .with_neow_start();
    let (by_rarity, fallback, boss) = default_relic_grab_bag();
    rm.run_state
        .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
    rm.run_state.interactive_rewards = true;
    rm.run_state.relics.extend_from_slice(relics);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat(encounter)),
        ActionResult::Advanced
    );
    rm.debug_set_player(Some(9999), Some(9999), None);
    for _ in 0..5000 {
        if rm.phase() != Phase::Combat {
            assert_eq!(rm.phase(), Phase::CardReward);
            return rm;
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

fn cards(rm: &RunManager) -> Vec<Vec<CardId>> {
    rm.reward_screen()
        .expect("the rewards screen")
        .iter()
        .filter_map(|r| match r {
            ScreenReward::Card { options, .. } => Some(options.clone()),
            _ => None,
        })
        .collect()
}

fn relics(rm: &RunManager) -> Vec<RelicId> {
    rm.reward_screen()
        .expect("the rewards screen")
        .iter()
        .filter_map(|r| match r {
            ScreenReward::Relic { relic } => Some(relic.relic_id),
            _ => None,
        })
        .collect()
}

#[test]
fn prayer_wheel_adds_a_second_monster_card_reward_rolled_after_the_rooms() {
    let plain = fight("NIBBITS_WEAK", &[]);
    let wheel = fight("NIBBITS_WEAK", &[RelicId::PrayerWheel]);
    let plain_cards = cards(&plain);
    let wheel_cards = cards(&wheel);
    assert_eq!(plain_cards.len(), 1);
    assert_eq!(
        wheel_cards.len(),
        2,
        "the room's card reward and Prayer Wheel's"
    );
    // The room's card reward rolls before the hook's.
    assert_eq!(wheel_cards[0], plain_cards[0]);
    assert_eq!(wheel_cards[1].len(), 3);
    assert!(
        wheel.run_state.rng.rewards.counter() > plain.run_state.rng.rewards.counter(),
        "the hook's card reward reads the Rewards stream"
    );
    // The rest of the screen is the room's.
    let gold = |rm: &RunManager| -> Vec<i32> {
        rm.reward_screen()
            .unwrap()
            .iter()
            .filter_map(|r| match r {
                ScreenReward::Gold { amount } => Some(*amount),
                _ => None,
            })
            .collect()
    };
    assert_eq!(gold(&wheel), gold(&plain));
}

#[test]
fn white_star_and_black_star_add_their_rewards_to_an_elite_after_the_rooms() {
    let plain = fight("BYGONE_EFFIGY_ELITE", &[]);
    let stars = fight(
        "BYGONE_EFFIGY_ELITE",
        &[RelicId::WhiteStar, RelicId::BlackStar],
    );
    let plain_cards = cards(&plain);
    let star_cards = cards(&stars);
    assert_eq!(plain_cards.len(), 1);
    assert_eq!(
        star_cards.len(),
        2,
        "the room's card reward and White Star's"
    );
    assert_eq!(star_cards[0], plain_cards[0], "the room's card rolls first");
    assert_eq!(star_cards[1].len(), 3);
    let plain_relics = relics(&plain);
    let star_relics = relics(&stars);
    assert_eq!(plain_relics.len(), 1, "the elite's relic");
    assert_eq!(star_relics.len(), 2, "the elite's and Black Star's");
    assert_eq!(
        star_relics[0], plain_relics[0],
        "the elite's relic rolls first"
    );
    assert_ne!(star_relics[1], star_relics[0]);
}

#[test]
fn white_star_adds_nothing_in_a_monster_room_and_prayer_wheel_nothing_in_an_elite() {
    let monster = fight("NIBBITS_WEAK", &[RelicId::WhiteStar, RelicId::BlackStar]);
    assert_eq!(cards(&monster).len(), 1);
    assert!(relics(&monster).is_empty());
    let elite = fight("BYGONE_EFFIGY_ELITE", &[RelicId::PrayerWheel]);
    assert_eq!(cards(&elite).len(), 1);
    assert_eq!(relics(&elite).len(), 1);
}
