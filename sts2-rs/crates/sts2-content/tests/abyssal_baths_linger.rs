//! Abyssal Baths: Immerse opens the Linger / Exit Baths page.
//!
//! The game's AbyssalBaths.Immerse gains 2 max HP, takes the Damage var (3),
//! raises it by 1 and opens a page with Linger (the same again, the damage
//! rising each time) and Exit Baths (finish). The engine finished the event on
//! Immerse (SS0EV5XA8B: back on the map where the game shows the page).

use sts2_core::events::EventId;
use sts2_core::runstate::RunState;

use sts2_content::events::dispatch::{present, resolve_by_index};

fn keys(state: &RunState) -> Vec<&'static str> {
    present(state)
        .expect("an event is active")
        .iter()
        .map(|o| o.loc_key)
        .collect()
}

#[test]
fn immerse_opens_linger_and_exit_and_the_damage_rises() {
    let mut state = RunState::new();
    state.max_hp = 80;
    state.current_hp = 60;
    state.current_event = Some(EventId::AbyssalBaths);
    assert_eq!(
        keys(&state),
        [
            "ABYSSAL_BATHS.pages.INITIAL.options.IMMERSE",
            "ABYSSAL_BATHS.pages.INITIAL.options.ABSTAIN"
        ]
    );
    let outcome = resolve_by_index(&mut state, 0).expect("immerse");
    assert!(!outcome.finished);
    assert_eq!((state.max_hp, state.current_hp), (82, 59));
    assert_eq!(
        keys(&state),
        [
            "ABYSSAL_BATHS.pages.ALL.options.LINGER",
            "ABYSSAL_BATHS.pages.ALL.options.EXIT_BATHS"
        ]
    );
    let outcome = resolve_by_index(&mut state, 0).expect("linger");
    assert!(!outcome.finished);
    assert_eq!((state.max_hp, state.current_hp), (84, 57), "linger takes 4");
    resolve_by_index(&mut state, 0).expect("linger again");
    assert_eq!((state.max_hp, state.current_hp), (86, 54), "then 5");
    let outcome = resolve_by_index(&mut state, 1).expect("exit");
    assert!(outcome.finished);
}
