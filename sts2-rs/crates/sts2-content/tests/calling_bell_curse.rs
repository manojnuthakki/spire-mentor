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

/// The curse goes through the game's deck add (CardPileCmd.Add to the Deck):
/// held relics' deck-add effects run before the bell's relics are offered.
/// Lucky Fysh: +15 gold a card added; Darkstone Periapt: +6 max HP a curse.
#[test]
fn calling_bell_curse_runs_the_deck_add_hooks_before_its_relics() {
    let mut rm = on_the_map();
    rm.run_state.interactive_rewards = true;
    rm.run_state.relics.push(RelicId::LuckyFysh);
    rm.run_state.relics.push(RelicId::DarkstonePeriapt);
    let (gold, max_hp) = (rm.run_state.gold, rm.run_state.max_hp);
    rm.run_state.offer_custom_rewards(
        vec![OfferedReward::Relic(RelicReward::new(RelicId::CallingBell))],
        true,
    );
    rm.take_action(RunAction::ProceedRewards);
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert_eq!(curses(&rm), 1);
    // The bell's set is open and nothing in it is claimed yet.
    assert!(rm.reward_screen().is_some(), "the bell's set is open");
    assert_eq!(rm.run_state.gold - gold, 15, "Lucky Fysh's gold");
    assert_eq!(
        rm.run_state.max_hp - max_hp,
        6,
        "Darkstone Periapt's max HP"
    );
}

/// Another relic adding its curse the same way: Cursed Pearl's Greed
/// (AddCurseToDeck<Greed>, then 333 gold) with Lucky Fysh held.
#[test]
fn cursed_pearl_greed_runs_the_deck_add_hooks() {
    let mut rm = on_the_map();
    rm.run_state.interactive_rewards = true;
    rm.run_state.relics.push(RelicId::LuckyFysh);
    let gold = rm.run_state.gold;
    rm.run_state.offer_custom_rewards(
        vec![OfferedReward::Relic(RelicReward::new(RelicId::CursedPearl))],
        true,
    );
    rm.take_action(RunAction::ProceedRewards);
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert!(rm.run_state.relics.contains(&RelicId::CursedPearl));
    assert!(rm
        .run_state
        .master_deck
        .iter()
        .any(|c| c.card_id == CardId::Greed));
    assert_eq!(
        rm.run_state.gold - gold,
        15 + 333,
        "Lucky Fysh's 15 and the pearl's 333"
    );
}
