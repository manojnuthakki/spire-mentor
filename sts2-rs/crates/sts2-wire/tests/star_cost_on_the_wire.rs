//! A card's star cost and Osty's powers are on the wire (sts2-agent fork).
//!
//! Headless writes every card's `star_cost` (C# `GetStarCostWithModifiers()`,
//! null for a card without one) and `star_cost_x` (`HasStarCostX`, Stardust),
//! in a reward, the deck, a shop and every combat pile; and Osty's powers
//! (Die For You). The engine sent neither.

use sts2_core::cards::CardId;
use sts2_core::{CardUid, CombatState};

fn pod(card_id: CardId) -> sts2_core::cards::Card {
    sts2_content::cards::spec_index::card_pod_from_id(card_id, false).expect("card pod")
}

fn add_card(state: &mut CombatState, card_id: CardId) -> CardUid {
    let mut card = pod(card_id);
    let uid = CardUid::new(u32::try_from(state.cards.len() + 1).expect("fits")).expect("nz");
    card.uid = uid;
    state.cards.push(card);
    uid
}

#[test]
fn a_cards_star_cost_is_on_its_spec() {
    sts2_content::cards::dispatch::ensure_installed();
    let guiding = sts2_wire::emit::run_manager::card_value(&pod(CardId::GuidingStar));
    assert_eq!(guiding["star_cost"], serde_json::json!(2), "{guiding}");
    assert!(guiding.get("star_cost_x").is_none(), "{guiding}");
    let cloak = sts2_wire::emit::run_manager::card_value(&pod(CardId::CloakOfStars));
    assert_eq!(cloak["star_cost"], serde_json::json!(1), "{cloak}");
    let strike = sts2_wire::emit::run_manager::card_value(&pod(CardId::StrikeIronclad));
    assert!(strike.get("star_cost").is_none(), "{strike}");
    assert!(strike.get("star_cost_x").is_none(), "{strike}");
    let stardust = sts2_wire::emit::run_manager::card_value(&pod(CardId::Stardust));
    assert_eq!(
        stardust["star_cost_x"],
        serde_json::json!(true),
        "{stardust}"
    );
    assert!(stardust.get("star_cost").is_none(), "{stardust}");
}

#[test]
fn a_combat_cards_star_cost_is_its_effective_cost() {
    sts2_content::cards::dispatch::ensure_installed();
    let mut state = CombatState::default();
    let dying = add_card(&mut state, CardId::DyingStar);
    let strike = add_card(&mut state, CardId::StrikeIronclad);
    state.player.hand.push(dying);
    state.player.hand.push(strike);
    let card = |s: &CombatState, uid: CardUid| s.cards[uid.get() as usize - 1];
    let v = sts2_wire::emit::combat::card_to_value(&state, &card(&state, dying));
    assert_eq!(v["star_cost"], serde_json::json!(3), "{v}");
    let v = sts2_wire::emit::combat::card_to_value(&state, &card(&state, strike));
    assert!(v.get("star_cost").is_none(), "{v}");
}

#[test]
fn ostys_powers_are_on_the_wire() {
    sts2_content::cards::dispatch::ensure_installed();
    {
        let (alive, hp) = (true, 6);
        let mut value: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/combat_action_ironclad.json"),
            )
            .expect("fixture"),
        )
        .expect("json");
        value["player"]["osty"] =
            serde_json::json!({"alive": alive, "hp": hp, "max_hp": 6, "block": 0});
        let state = sts2_wire::state_from_json_str(&value.to_string()).expect("rebuilds");
        let back = sts2_wire::emit::combat_state_to_value(&state).expect("emits");
        let powers = &back["player"]["osty"]["powers"];
        assert_eq!(
            powers[0]["name"],
            serde_json::json!("DIE_FOR_YOU"),
            "{back}"
        );
        assert_eq!(powers[0]["amount"], serde_json::json!(1), "{powers}");
    }
}
