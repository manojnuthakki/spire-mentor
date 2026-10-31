//! A locked event option carries the game's `_LOCKED` text key.
//!
//! In the game an `EventOption` is locked exactly when it has no handler
//! (`IsLocked => OnChosen == null`), and each event builds its locked option
//! under its own text key: Self-Help Book's `READ_ENTIRE_BOOK_LOCKED` when the
//! deck has no Power to enchant, Luminous Choir's `OFFER_TRIBUTE_LOCKED` when
//! the player can't pay the tribute. Also Ranwid the Elder, Symbiote, Stone of
//! All Time, Grave of the Forgotten, Wood Carvings, Zen Weaver and Endless
//! Conveyor (item `event-locks-rest`).

use sts2_core::cards::CardId;
use sts2_core::events::EventId;
use sts2_core::potions::PotionKind;
use sts2_core::runstate::RunState;
use sts2_core::RelicId;

use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_content::events::dispatch::present;

fn state_in(event: EventId, deck: &[CardId], gold: i32) -> RunState {
    sts2_content::cards::dispatch::ensure_installed();
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

#[test]
fn ranwid_locks_the_potion_and_the_relic_with_nothing_to_give() {
    // Burning Blood is a Starter relic, not tradable.
    let mut state = state_in(EventId::RanwidTheElder, &[CardId::StrikeIronclad], 200);
    state.relics = vec![RelicId::BurningBlood];
    state.potions.clear();
    assert_eq!(
        key_and_enabled(&state),
        vec![
            (
                "RANWID_THE_ELDER.pages.INITIAL.options.POTION_LOCKED",
                false
            ),
            ("RANWID_THE_ELDER.pages.INITIAL.options.GOLD", true),
            ("RANWID_THE_ELDER.pages.INITIAL.options.RELIC_LOCKED", false),
        ]
    );
    state.relics.push(RelicId::Anchor);
    state.potions.push(PotionKind::BlockPotion);
    assert_eq!(
        key_and_enabled(&state),
        vec![
            ("RANWID_THE_ELDER.pages.INITIAL.options.POTION", true),
            ("RANWID_THE_ELDER.pages.INITIAL.options.GOLD", true),
            ("RANWID_THE_ELDER.pages.INITIAL.options.RELIC", true),
        ]
    );
}

#[test]
fn symbiote_locks_approach_without_an_attack_corrupted_can_take() {
    // Corrupted.CanEnchantCardType is Attack only: Defends can't take it.
    let state = state_in(EventId::Symbiote, &[CardId::DefendIronclad], 99);
    assert_eq!(
        key_and_enabled(&state)[0],
        ("SYMBIOTE.pages.INITIAL.options.APPROACH_LOCKED", false)
    );
    let state = state_in(
        EventId::Symbiote,
        &[CardId::DefendIronclad, CardId::StrikeIronclad],
        99,
    );
    assert_eq!(
        key_and_enabled(&state)[0],
        ("SYMBIOTE.pages.INITIAL.options.APPROACH", true)
    );
}

#[test]
fn stone_of_all_time_locks_lift_without_a_potion_and_push_without_an_attack() {
    let mut state = state_in(EventId::StoneOfAllTime, &[CardId::DefendIronclad], 99);
    state.potions.clear();
    assert_eq!(
        key_and_enabled(&state),
        vec![
            ("STONE_OF_ALL_TIME.pages.INITIAL.options.LIFT_LOCKED", false),
            ("STONE_OF_ALL_TIME.pages.INITIAL.options.PUSH_LOCKED", false),
        ]
    );
    let mut state = state_in(EventId::StoneOfAllTime, &[CardId::StrikeIronclad], 99);
    state.potions.push(PotionKind::BlockPotion);
    assert_eq!(
        key_and_enabled(&state),
        vec![
            ("STONE_OF_ALL_TIME.pages.INITIAL.options.LIFT", true),
            ("STONE_OF_ALL_TIME.pages.INITIAL.options.PUSH", true),
        ]
    );
}

#[test]
fn grave_locks_confront_without_an_exhaust_card_souls_power_can_take() {
    // SoulsPower.CanEnchant also needs the card's own Exhaust keyword.
    let state = state_in(
        EventId::GraveOfTheForgotten,
        &[CardId::StrikeIronclad, CardId::DefendIronclad],
        99,
    );
    assert_eq!(
        key_and_enabled(&state),
        vec![
            (
                "GRAVE_OF_THE_FORGOTTEN.pages.INITIAL.options.CONFRONT_LOCKED",
                false
            ),
            ("GRAVE_OF_THE_FORGOTTEN.pages.INITIAL.options.ACCEPT", true),
        ]
    );
    let state = state_in(
        EventId::GraveOfTheForgotten,
        &[CardId::StrikeIronclad, CardId::Offering],
        99,
    );
    assert_eq!(
        key_and_enabled(&state)[0],
        (
            "GRAVE_OF_THE_FORGOTTEN.pages.INITIAL.options.CONFRONT",
            true
        )
    );
}

#[test]
fn wood_carvings_locks_snake_without_a_card_slither_can_take() {
    // Slither.CanEnchant refuses X-cost cards; the base rule refuses curses.
    let state = state_in(
        EventId::WoodCarvings,
        &[CardId::Whirlwind, CardId::AscendersBane],
        99,
    );
    assert_eq!(
        key_and_enabled(&state),
        vec![
            ("WOOD_CARVINGS.pages.INITIAL.options.BIRD", true),
            ("WOOD_CARVINGS.pages.INITIAL.options.SNAKE_LOCKED", false),
            ("WOOD_CARVINGS.pages.INITIAL.options.TORUS", true),
        ]
    );
    let state = state_in(
        EventId::WoodCarvings,
        &[CardId::Whirlwind, CardId::StrikeIronclad],
        99,
    );
    assert_eq!(
        key_and_enabled(&state)[1],
        ("WOOD_CARVINGS.pages.INITIAL.options.SNAKE", true)
    );
}

#[test]
fn zen_weaver_locks_what_the_gold_cannot_pay_under_one_locked_key() {
    let state = state_in(EventId::ZenWeaver, &[CardId::StrikeIronclad], 124);
    assert_eq!(
        key_and_enabled(&state),
        vec![
            (
                "ZEN_WEAVER.pages.INITIAL.options.BREATHING_TECHNIQUES",
                true
            ),
            ("ZEN_WEAVER.pages.INITIAL.options.LOCKED", false),
            ("ZEN_WEAVER.pages.INITIAL.options.LOCKED", false),
        ]
    );
    let state = state_in(EventId::ZenWeaver, &[CardId::StrikeIronclad], 249);
    assert_eq!(
        key_and_enabled(&state)[1..],
        [
            ("ZEN_WEAVER.pages.INITIAL.options.EMOTIONAL_AWARENESS", true),
            ("ZEN_WEAVER.pages.INITIAL.options.LOCKED", false),
        ]
    );
    let state = state_in(EventId::ZenWeaver, &[CardId::StrikeIronclad], 250);
    assert_eq!(
        key_and_enabled(&state)[2],
        (
            "ZEN_WEAVER.pages.INITIAL.options.ARACHNID_ACUPUNCTURE",
            true
        )
    );
}

#[test]
fn endless_conveyor_locks_the_grab_below_the_dish_cost() {
    let state = state_in(EventId::EndlessConveyor, &[CardId::StrikeIronclad], 39);
    assert_eq!(
        key_and_enabled(&state)[0],
        ("ENDLESS_CONVEYOR.pages.ALL.options.LOCKED", false)
    );
    let state = state_in(EventId::EndlessConveyor, &[CardId::StrikeIronclad], 40);
    let (key, enabled) = key_and_enabled(&state)[0];
    assert!(enabled);
    assert_ne!(key, "ENDLESS_CONVEYOR.pages.ALL.options.LOCKED");
}
