//! A relic obtained on a rewards screen modifies the card reward already rolled.
//!
//! The game's CardReward subscribes to its player's RelicObtained and applies
//! the new relic's TryModifyCardRewardOptions(Late) to the cards it rolled
//! (CardReward.OnRelicObtained), until the reward is opened or skipped: Frozen
//! Egg claimed first shows the set's Power upgraded (EK9RFEKVWV's Stampede).
//! The engine applied the egg only when a reward was rolled.

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::cards::CardId;
use sts2_core::potions::CharacterId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{Phase, RunAction, RunManager};
use sts2_core::runstate::{OfferedReward, RelicReward};

fn screen() -> RunManager {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_with_ascension(3, CharacterId::Ironclad, 0);
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        let a = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    rm.run_state.interactive_rewards = true;
    rm.run_state.offer_custom_rewards(
        vec![
            OfferedReward::Relic(RelicReward::new(RelicId::FrozenEgg)),
            OfferedReward::Card {
                options: Some(vec![CardId::Inflame, CardId::Anger]),
                upgraded: vec![false, false],
            },
        ],
        true,
    );
    // Any action opens the offered set (this one is refused on the map).
    rm.take_action(RunAction::ProceedRewards);
    assert!(rm.reward_screen().is_some(), "the set is open");
    rm
}

#[test]
fn frozen_egg_claimed_first_upgrades_the_sets_power() {
    let mut rm = screen();
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert!(rm.run_state.relics.contains(&RelicId::FrozenEgg));
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert_eq!(rm.card_reward_options(), [CardId::Inflame, CardId::Anger]);
    assert_eq!(rm.card_reward_upgraded_flags(), [true, false]);
}

#[test]
fn a_card_reward_opened_before_the_egg_is_left_as_rolled() {
    let mut rm = screen();
    // Open the card reward and skip it (C# unsubscribes at the end of OnSelect).
    rm.take_action(RunAction::ClaimReward { idx: 1 });
    rm.take_action(RunAction::PickCardReward { idx: None });
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert!(rm.run_state.relics.contains(&RelicId::FrozenEgg));
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert_eq!(rm.card_reward_upgraded_flags(), [false, false]);
}
