//! Mad Science's type and rider are on the wire (sts2-agent fork).
//!
//! Tinker Time builds a Mad Science whose type (C# `MadScience.Type =>
//! TinkerTimeType`) and rider (`TinkerTimeRider`, a `[SavedProperty]`) the
//! player picks. Headless writes the card's `type` and `tinker_time_rider`
//! (the game's `RiderEffect` name) wherever the card is shown: the deck and
//! every combat pile. The engine sent the spec's Attack in combat and no
//! rider anywhere.

use sts2_content::cards::colorless::{set_mad_science_variant, TinkerTimeRider};
use sts2_core::cards::{CardId, CardType};
use sts2_core::{CardUid, CombatState};

fn wisdom_skill() -> sts2_core::cards::Card {
    sts2_content::cards::dispatch::ensure_installed();
    let mut card =
        sts2_content::cards::spec_index::card_pod_from_id(CardId::MadScience, false).expect("pod");
    set_mad_science_variant(&mut card, CardType::Skill, TinkerTimeRider::Wisdom);
    card
}

#[test]
fn a_deck_mad_science_shows_its_type_and_rider() {
    let v = sts2_wire::emit::run_manager::card_value(&wisdom_skill());
    assert_eq!(v["type"], serde_json::json!("Skill"), "{v}");
    assert_eq!(v["tinker_time_rider"], serde_json::json!("Wisdom"), "{v}");
}

#[test]
fn a_combat_mad_science_shows_its_type_and_rider() {
    let mut state = CombatState::default();
    let mut card = wisdom_skill();
    let uid = CardUid::new(1).expect("nz");
    card.uid = uid;
    state.cards.push(card);
    state.player.hand.push(uid);
    let v = sts2_wire::emit::combat::card_to_value(&state, &state.cards[0]);
    assert_eq!(v["type"], serde_json::json!("Skill"), "{v}");
    assert_eq!(v["tinker_time_rider"], serde_json::json!("Wisdom"), "{v}");
}

#[test]
fn other_cards_carry_no_rider() {
    sts2_content::cards::dispatch::ensure_installed();
    let strike = sts2_content::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
        .expect("pod");
    let v = sts2_wire::emit::run_manager::card_value(&strike);
    assert!(v.get("tinker_time_rider").is_none(), "{v}");
}
