//! A locked event option carries the game's `_LOCKED` text key.
//!
//! In the game an `EventOption` is locked exactly when it has no handler
//! (`IsLocked => OnChosen == null`), and each event builds its locked option
//! under its own text key: Self-Help Book's `READ_ENTIRE_BOOK_LOCKED` when the
//! deck has no Power to enchant, Luminous Choir's `OFFER_TRIBUTE_LOCKED` when
//! the player can't pay the tribute.

use sts2_core::cards::CardId;
use sts2_core::events::EventId;
use sts2_core::runstate::RunState;

use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_content::events::dispatch::present;

fn state_in(event: EventId, deck: &[CardId], gold: i32) -> RunState {
    let mut state = RunState::new();
    state.master_deck = deck
        .iter()
        .map(|id| card_pod_from_id(*id, false).expect("card pod"))
        .collect();
    state.gold = gold;
    state.current_event = Some(event);
    state
}

fn key_and_enabled(state: &RunState) -> Vec<(&'static str, bool)> {
    present(state)
        .expect("an event is active")
        .iter()
        .map(|o| (o.loc_key, o.enabled))
        .collect()
}

#[test]
fn self_help_book_locks_a_type_with_no_card_under_its_locked_key() {
    let state = state_in(
        EventId::SelfHelpBook,
        &[CardId::StrikeIronclad, CardId::DefendIronclad, CardId::Bash],
        99,
    );
    assert_eq!(
        key_and_enabled(&state),
        vec![
            ("SELF_HELP_BOOK.pages.INITIAL.options.READ_THE_BACK", true),
            ("SELF_HELP_BOOK.pages.INITIAL.options.READ_PASSAGE", true),
            (
                "SELF_HELP_BOOK.pages.INITIAL.options.READ_ENTIRE_BOOK_LOCKED",
                false
            ),
        ]
    );
}

#[test]
fn luminous_choir_locks_the_tribute_without_the_gold_under_its_locked_key() {
    let state = state_in(EventId::LuminousChoir, &[CardId::StrikeIronclad], 0);
    let options = key_and_enabled(&state);
    assert_eq!(
        options[1],
        (
            "LUMINOUS_CHOIR.pages.INITIAL.options.OFFER_TRIBUTE_LOCKED",
            false
        )
    );
    let state = state_in(EventId::LuminousChoir, &[CardId::StrikeIronclad], 999);
    assert_eq!(
        key_and_enabled(&state)[1],
        ("LUMINOUS_CHOIR.pages.INITIAL.options.OFFER_TRIBUTE", true)
    );
}

#[test]
fn crystal_sphere_never_locks_uncover_future() {
    let state = state_in(EventId::CrystalSphere, &[CardId::StrikeIronclad], 0);
    assert_eq!(
        key_and_enabled(&state)[0],
        ("CRYSTAL_SPHERE.pages.INITIAL.options.UNCOVER_FUTURE", true)
    );
}
