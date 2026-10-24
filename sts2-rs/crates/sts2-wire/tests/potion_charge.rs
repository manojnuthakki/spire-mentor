//! The potion carry-over charge through `recommend_value` (sts2-agent fork):
//! `RecommendParams::potion_charge_hp` lowers each drink's reported Q by its
//! slot's charge, so the ranking `mcts_recommend` returns (and its `is_best`)
//! prices a drink against carrying the potion on. Charges far outside any
//! tie band bracket the mechanism, so the tests can't flake on the search.

use serde_json::{json, Value};
use sts2_core::{victory_hp_equiv_to_q, CombatState};
use sts2_wire::emit::mcts::{recommend_value, RecommendParams};
use sts2_wire::state_from_json_str;

/// The fair-root fixture (a Fuzzy Wurm Crawler at 30 HP) holding a Fire Potion.
fn with_potion() -> CombatState {
    let mut v: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fair_root_draw.json"),
        )
        .unwrap(),
    )
    .unwrap();
    v["player"]["potions"] = json!(["FIRE_POTION", null, null]);
    state_from_json_str(&v.to_string()).expect("rebuilds")
}

fn params(charge: Option<f32>) -> RecommendParams {
    let mut p = RecommendParams::from_config(&json!({"sims": 400, "seed": 3, "threads": 1}), "test").unwrap();
    p.potion_charge_hp = charge.map(|c| [c; 8]);
    p
}

fn drink(out: &Value) -> &Value {
    out["actions"].as_array().unwrap().iter().find(|a| a["action"]["action"] == "drink_potion").expect("a drink")
}

fn best(out: &Value) -> String {
    let a = out["actions"].as_array().unwrap().iter().find(|a| a["is_best"] == true).unwrap();
    a["action"]["action"].as_str().unwrap().to_string()
}

#[test]
fn no_charge_leaves_the_output_unchanged() {
    let state = with_potion();
    let a = recommend_value(&state, &params(None), true);
    let b = recommend_value(&state, &RecommendParams { potion_charge_hp: None, ..params(None) }, true);
    assert_eq!(a, b);
    assert!(a.get("potion_charge_hp").is_none());
}

#[test]
fn a_charge_lowers_the_drinks_q_by_exactly_the_charge_and_changes_the_pick() {
    let state = with_potion();
    let raw = recommend_value(&state, &params(None), true);
    // A whole-win charge: no drink can be best. A whole-win bonus: the drink is best.
    let charged = recommend_value(&state, &params(Some(1000.0)), true);
    let bonus = recommend_value(&state, &params(Some(-1000.0)), true);
    assert_ne!(best(&charged), "drink_potion");
    assert_eq!(best(&bonus), "drink_potion");
    let q = |o: &Value| drink(o)["q"].as_f64().unwrap();
    let c = f64::from(victory_hp_equiv_to_q(1000.0));
    assert!((q(&raw) - c - q(&charged)).abs() < 1e-4, "{} {} {}", q(&raw), c, q(&charged));
    assert_eq!(drink(&raw)["visits"], drink(&charged)["visits"]); // the search itself is the same
    assert_eq!(charged["potion_charge_hp"], json!(vec![1000.0_f32; 8]));
    // Every other action's numbers are untouched.
    let others = |o: &Value| {
        o["actions"].as_array().unwrap().iter().filter(|a| a["action"]["action"] != "drink_potion")
            .map(|a| (a["action"].to_string(), a["q"].to_string(), a["visits"].to_string()))
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(others(&raw), others(&charged));
}

#[test]
fn a_slot_past_the_belt_is_not_charged() {
    let state = with_potion();
    let mut p = params(None);
    p.potion_charge_hp = Some([0.0, 1000.0, 1000.0, 1000.0, 1000.0, 1000.0, 1000.0, 1000.0]);
    let out = recommend_value(&state, &p, true);
    let raw = recommend_value(&state, &params(None), true);
    assert_eq!(drink(&out)["q"], drink(&raw)["q"]);
}
