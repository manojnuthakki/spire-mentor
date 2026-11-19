//! Sloth (Knowledge Demon's curse): `SlothPower.DisplayAmount` is the plays
//! started this turn (`SlothPower.cs:21`), the amount is the cap (3). The wire
//! emits it so (it showed the cap, 3, where the game shows 0, and sent back it
//! set the counter to the cap and refused every play), and reads it back as the
//! counter (`display` in, the engine's own emit is `display_amount`). sts2-agent fork, step 65.
use serde_json::{json, Value};
use sts2_core::combat_loop::player_should_play_blocked;
use sts2_core::CombatState;
use sts2_wire::emit::combat::combat_state_to_value;
use sts2_wire::state_from_json_str;

fn with_sloth(shown: i64) -> CombatState {
    let mut v: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/fair_root_draw.json"),
        )
        .unwrap(),
    )
    .unwrap();
    v["player"]["powers"] = json!([{"name": "SLOTH", "amount": 3, "display": shown}]);
    state_from_json_str(&v.to_string()).expect("rebuilds")
}

fn sloth(value: &Value) -> &Value {
    value["player"]["powers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "SLOTH")
        .expect("Sloth is up")
}

#[test]
fn emits_the_plays_started_as_the_display_and_the_cap_as_the_amount() {
    for shown in [0, 1, 2] {
        let state = with_sloth(shown);
        assert_eq!(i64::from(state.player.card_plays_started_this_turn), shown);
        let value = combat_state_to_value(&state).expect("emits");
        let p = sloth(&value);
        assert_eq!(p["amount"], 3);
        assert_eq!(p["display_amount"], shown, "plays started, not the cap");
    }
}

#[test]
fn the_shown_count_is_the_counter_that_gates_plays() {
    // The wire reads `display` as the plays started: 3 of the cap 3 refuse the next.
    for (shown, blocked) in [(0, false), (2, false), (3, true)] {
        let state = with_sloth(shown);
        let card = state.cards.iter().next().expect("a card").clone();
        assert_eq!(
            player_should_play_blocked(&state, &card),
            blocked,
            "shown {shown}"
        );
    }
}
