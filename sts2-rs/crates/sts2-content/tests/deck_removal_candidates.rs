//! A deck removal counts the removable cards, as the game does
//! (deck-removal-candidate-count).
//!
//! CardSelectCmd.FromDeckForRemoval offers the deck's IsRemovable cards
//! (`!Keywords.Contains(Eternal)`); FromDeckGeneric then returns nothing when
//! there is none, and, without RequireManualConfirmation (a fixed count:
//! `new CardSelectorPrefs(prompt, n)` has MinSelect == MaxSelect), returns
//! every candidate without a choice when there are no more than MinSelect.
//! PreservedFog.AfterObtained then adds Folly (AddCurseToDeck) once.

use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::cards::CardId;
use sts2_core::potions::CharacterId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{Phase, RunAction, RunManager};
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
    rm.run_state.interactive_rewards = true;
    rm.run_state.interactive_card_removal = true;
    rm.run_state.relics.push(RelicId::LuckyFysh);
    rm.run_state.relics.push(RelicId::DarkstonePeriapt);
    rm
}

fn set_deck(rm: &mut RunManager, ids: &[CardId]) {
    rm.run_state.master_deck = ids
        .iter()
        .map(|&id| card_pod_from_id(id, false).expect("a card spec"))
        .collect();
}

fn deck(rm: &RunManager) -> Vec<CardId> {
    rm.run_state.master_deck.iter().map(|c| c.card_id).collect()
}

/// Claim `relic` from an offered reward set, then leave the rewards screen.
fn obtain_then_leave(rm: &mut RunManager, relic: RelicId) {
    rm.run_state
        .offer_custom_rewards(vec![OfferedReward::Relic(RelicReward::new(relic))], true);
    rm.take_action(RunAction::ProceedRewards);
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert!(rm.run_state.relics.contains(&relic));
}

fn reaches_the_map(rm: &mut RunManager) {
    for _ in 0..5 {
        if rm.phase() == Phase::MapChoice {
            return;
        }
        let actions = rm.get_available_actions();
        assert!(!actions.is_empty(), "stalled in {:?}", rm.phase());
        let a = if actions.contains(&RunAction::ProceedRewards) {
            RunAction::ProceedRewards
        } else {
            actions[0]
        };
        rm.take_action(a);
    }
    assert_eq!(rm.phase(), Phase::MapChoice);
}

/// One removable card of the three asked: it goes without a choice, then Folly
/// once, with Lucky Fysh's gold and Darkstone Periapt's max HP.
#[test]
fn preserved_fog_with_one_removable_card_removes_it_and_adds_folly_once() {
    let mut rm = on_the_map();
    set_deck(
        &mut rm,
        &[
            CardId::CurseOfTheBell,
            CardId::Greed,
            CardId::StrikeIronclad,
        ],
    );
    let (gold, max_hp) = (rm.run_state.gold, rm.run_state.max_hp);
    obtain_then_leave(&mut rm, RelicId::PreservedFog);
    assert_eq!(rm.run_state.pending_deck_removals, 0, "no selection opens");
    assert_eq!(
        deck(&rm),
        vec![CardId::CurseOfTheBell, CardId::Greed, CardId::Folly]
    );
    assert_eq!(rm.run_state.gold - gold, 15, "Lucky Fysh once");
    assert_eq!(rm.run_state.max_hp - max_hp, 6, "Darkstone Periapt once");
    reaches_the_map(&mut rm);
}

/// No removable card: the removal returns nothing at once, then Folly.
#[test]
fn preserved_fog_with_only_eternal_curses_adds_folly_once() {
    let mut rm = on_the_map();
    set_deck(&mut rm, &[CardId::CurseOfTheBell, CardId::Greed]);
    let (gold, max_hp) = (rm.run_state.gold, rm.run_state.max_hp);
    obtain_then_leave(&mut rm, RelicId::PreservedFog);
    assert_eq!(rm.run_state.pending_deck_removals, 0);
    assert_eq!(
        deck(&rm),
        vec![CardId::CurseOfTheBell, CardId::Greed, CardId::Folly]
    );
    assert_eq!(rm.run_state.gold - gold, 15);
    assert_eq!(rm.run_state.max_hp - max_hp, 6);
    reaches_the_map(&mut rm);
}

/// Exactly as many removable cards as asked: all go without a choice
/// (`list.Count <= prefs.MinSelect`).
#[test]
fn preserved_fog_with_exactly_three_removable_cards_takes_them_without_a_choice() {
    let mut rm = on_the_map();
    set_deck(
        &mut rm,
        &[
            CardId::StrikeIronclad,
            CardId::Greed,
            CardId::DefendIronclad,
            CardId::Bash,
        ],
    );
    obtain_then_leave(&mut rm, RelicId::PreservedFog);
    assert_eq!(rm.run_state.pending_deck_removals, 0);
    assert_eq!(deck(&rm), vec![CardId::Greed, CardId::Folly]);
    reaches_the_map(&mut rm);
}

/// More removable cards than asked: the choice opens over the removable ones
/// only, and Folly follows the last pick.
#[test]
fn preserved_fog_with_four_removable_cards_offers_them_only() {
    let mut rm = on_the_map();
    set_deck(
        &mut rm,
        &[
            CardId::StrikeIronclad,
            CardId::Greed,
            CardId::DefendIronclad,
            CardId::Bash,
            CardId::StrikeIronclad,
        ],
    );
    obtain_then_leave(&mut rm, RelicId::PreservedFog);
    assert_eq!(rm.run_state.pending_deck_removals, 3);
    assert_eq!(rm.get_available_actions().len(), 4, "Greed is not offered");
    for _ in 0..3 {
        rm.take_action(RunAction::RemoveDeckCard { idx: 0 });
    }
    assert_eq!(
        deck(&rm),
        vec![CardId::Greed, CardId::StrikeIronclad, CardId::Folly]
    );
    reaches_the_map(&mut rm);
}

/// Precise Scissors (one card) with only Eternal curses: nothing to remove.
#[test]
fn precise_scissors_with_only_eternal_curses_removes_nothing() {
    let mut rm = on_the_map();
    set_deck(&mut rm, &[CardId::AscendersBane, CardId::Greed]);
    obtain_then_leave(&mut rm, RelicId::PreciseScissors);
    assert_eq!(rm.run_state.pending_deck_removals, 0);
    assert_eq!(deck(&rm), vec![CardId::AscendersBane, CardId::Greed]);
    reaches_the_map(&mut rm);
}

/// Precarious Shears (two cards, then 16 damage) with one removable card: it
/// goes, then the damage, at once.
#[test]
fn precarious_shears_with_one_removable_card_removes_it_then_damages() {
    let mut rm = on_the_map();
    set_deck(&mut rm, &[CardId::Greed, CardId::DefendIronclad]);
    let hp = rm.run_state.current_hp;
    obtain_then_leave(&mut rm, RelicId::PrecariousShears);
    assert_eq!(rm.run_state.pending_deck_removals, 0);
    assert_eq!(deck(&rm), vec![CardId::Greed]);
    assert_eq!(hp - rm.run_state.current_hp, 16);
    reaches_the_map(&mut rm);
}
