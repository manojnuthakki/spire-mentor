//! Calling Bell adds Curse of the Bell to the deck, then offers its relics.
//!
//! The game (CallingBell.AfterObtained): CardPileCmd.AddCurseToDeck<CurseOfTheBell>,
//! then RewardsCmd.OfferCustom of a Common, an Uncommon and a Rare relic. The
//! engine offered the relics and left the curse out (calling-bell-curse).

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::cards::CardId;
use sts2_core::potions::CharacterId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{Phase, RunAction, RunManager, ScreenReward};
use sts2_core::runstate::{OfferedReward, RelicReward};

fn on_the_map() -> RunManager {
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
    rm
}

fn curses(rm: &RunManager) -> usize {
    rm.run_state
        .master_deck
        .iter()
        .filter(|c| c.card_id == CardId::CurseOfTheBell)
        .count()
}

#[test]
fn calling_bell_adds_curse_of_the_bell_then_offers_three_relics() {
    let mut rm = on_the_map();
    rm.run_state.interactive_rewards = true;
    let deck_before = rm.run_state.master_deck.len();
    rm.run_state.offer_custom_rewards(
        vec![OfferedReward::Relic(RelicReward::new(RelicId::CallingBell))],
        true,
    );
    // Any action opens the offered set (this one is refused on the map).
    rm.take_action(RunAction::ProceedRewards);
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert!(rm.run_state.relics.contains(&RelicId::CallingBell));
    assert_eq!(curses(&rm), 1, "Curse of the Bell is in the deck");
    assert_eq!(rm.run_state.master_deck.len(), deck_before + 1);
    // The bell's three relics are offered (its set opens over the claimed one).
    let relics = rm
        .reward_screen()
        .expect("the bell's set is open")
        .iter()
        .filter(|r| matches!(r, ScreenReward::Relic { .. }))
        .count();
    assert_eq!(relics, 3, "three relic rewards offered");
}
