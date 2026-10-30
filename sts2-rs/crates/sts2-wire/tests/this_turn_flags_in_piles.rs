//! This turn's Sly and Retain are shown on a card in every pile, and the Sly
//! keyword on a card outside combat (sts2-agent fork).
//!
//! In the game a single-turn Sly (`IsSlyThisTurn`, Hand Trick) and a single-turn
//! Retain (`ShouldRetainThisTurn`, Well-Laid Plans) stay on the card wherever it
//! goes until the turn's `EndOfTurnCleanup`, and Headless writes
//! `sly_this_turn` / `will_retain` for every card. A card with the Sly keyword
//! (Ricochet) is Sly in the deck and in a reward as well.

use sts2_core::cards::CardId;
use sts2_core::{CardUid, CombatState};

fn flags(v: &serde_json::Value) -> Vec<String> {
    v.get("flags")
        .and_then(|f| f.as_array())
        .map(|a| {
            a.iter()
                .map(|s| s.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn add_card(state: &mut CombatState, card_id: CardId) -> CardUid {
    let mut card =
        sts2_content::cards::spec_index::card_pod_from_id(card_id, false).expect("card pod");
    let uid = CardUid::new(u32::try_from(state.cards.len() + 1).expect("fits")).expect("nz");
    card.uid = uid;
    state.cards.push(card);
    uid
}

#[test]
fn a_single_turn_sly_or_retain_card_moved_out_of_hand_keeps_its_flag() {
    sts2_content::cards::dispatch::ensure_installed();
    let mut state = CombatState::default();
    let sly = add_card(&mut state, CardId::DefendIronclad);
    let retain = add_card(&mut state, CardId::StrikeIronclad);
    state.player.sly_this_turn.push(sly);
    state.player.retain_this_turn.push(retain);
    // Discarded (sly) and played (retain) this turn: out of hand.
    state.player.discard.push(sly);
    state.player.discard.push(retain);

    let card = |uid: CardUid| state.cards[uid.get() as usize - 1];
    let sly_value = sts2_wire::emit::combat::card_to_value(&state, &card(sly));
    assert!(
        flags(&sly_value).contains(&"sly_turn".to_string()),
        "sly_turn in the discard pile: {sly_value}"
    );
    let retain_value = sts2_wire::emit::combat::card_to_value(&state, &card(retain));
    assert!(
        flags(&retain_value).contains(&"retain_turn".to_string()),
        "retain_turn in the discard pile: {retain_value}"
    );
}

#[test]
fn a_sly_keyword_card_outside_combat_carries_sly() {
    sts2_content::cards::dispatch::ensure_installed();
    let ricochet =
        sts2_content::cards::spec_index::card_pod_from_id(CardId::Ricochet, false).expect("pod");
    let value = sts2_wire::emit::run_manager::card_value(&ricochet);
    assert!(
        flags(&value).contains(&"sly".to_string()),
        "Ricochet's card_value carries sly: {value}"
    );
    let strike = sts2_content::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
        .expect("pod");
    assert!(!flags(&sts2_wire::emit::run_manager::card_value(&strike)).contains(&"sly".to_string()));
}

/// The rebuild keeps a discarded card's `sly_turn`, so it round-trips.
#[test]
fn a_discarded_cards_sly_turn_round_trips() {
    let mut value: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/combat_action_ironclad.json"),
        )
        .expect("fixture"),
    )
    .expect("json");
    value["deck"]["draw_pile"][1]["flags"] = serde_json::json!(["sly_turn"]);
    let state = sts2_wire::state_from_json_str(&value.to_string()).expect("rebuilds");
    assert!(state.player.sly_this_turn.contains(&state.player.draw[1]));
    let back = sts2_wire::emit::combat_state_to_value(&state).expect("emits");
    assert_eq!(
        back["deck"]["draw_pile"][1]["flags"],
        serde_json::json!(["sly_turn"])
    );
}

/// The rebuild keeps a card's `retain_turn` in every pile, as the emit shows it
/// (the game's `ShouldRetainThisTurn` lasts the turn wherever the card is), and
/// the rebuilt state passes the invariants: a card played this turn and drawn
/// back keeps its Retain.
#[test]
fn a_cards_retain_turn_outside_hand_round_trips() {
    let mut value: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/combat_action_ironclad.json"),
        )
        .expect("fixture"),
    )
    .expect("json");
    value["deck"]["draw_pile"][1]["flags"] = serde_json::json!(["retain_turn"]);
    let state = sts2_wire::state_from_json_str(&value.to_string()).expect("rebuilds");
    let uid = state.player.draw[1];
    assert!(state.player.retain_this_turn.contains(&uid));
    sts2_core::invariants::check_combat_invariants(&state, "retain_turn in the draw pile")
        .expect("invariants hold");
    let back = sts2_wire::emit::combat_state_to_value(&state).expect("emits");
    assert_eq!(
        back["deck"]["draw_pile"][1]["flags"],
        serde_json::json!(["retain_turn"])
    );
}
