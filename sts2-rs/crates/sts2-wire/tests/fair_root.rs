//! The fair evaluation (`mcts_recommend(fair=True)`) under the honest mode
//! samples the ROOT action's hidden outcomes too (sts2-agent fork, review F3
//! of A4d): an action's draw is not the true next card, its rolls are not the
//! state's real dice.

use serde_json::{json, Value};
use sts2_core::{legal_actions, Action, CombatState};
use sts2_wire::emit::mcts::{fair_root_samples, recommend_value, RecommendParams, FAIR_ROOT_SAMPLES};
use sts2_wire::state_from_json_str;

fn fixture() -> Value {
    // Shrug It Off and a Defend in hand, Bludgeon and Wound to draw (in that
    // order: concrete, not a belief), an enemy attacking for 25 at 20 HP.
    serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fair_root_draw.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

fn params(sims: u32, seed: u64, hidden: &str) -> RecommendParams {
    RecommendParams::from_config(
        &json!({"sims": sims, "seed": seed, "fair": true, "threads": 1, "hidden_info": hidden}),
        "test",
    )
    .unwrap()
}

fn build(v: &Value) -> CombatState {
    state_from_json_str(&v.to_string()).expect("rebuilds")
}

fn q(v: &Value, sims: u32, hidden: &str) -> Vec<(String, f64, u64)> {
    let out = recommend_value(&build(v), &params(sims, 5, hidden), true);
    let mut rows: Vec<(String, f64, u64)> = out["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["action"].to_string(), a["q"].as_f64().unwrap(), a["visits"].as_u64().unwrap()))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn first_play(state: &CombatState) -> Action {
    legal_actions(state)
        .into_iter()
        .find(|a| matches!(a, Action::PlayCard { hand_index: 0, .. }))
        .unwrap()
}

fn drew_bludgeon(post: &CombatState) -> bool {
    post.player.hand.iter().any(|u| {
        format!("{:?}", post.cards.iter().find(|c| c.uid == *u).unwrap().card_id) == "Bludgeon"
    })
}

/// Shrug It Off draws one of two unseen cards. Its root samples draw either,
/// about half the time each, whichever order the state holds them in: before
/// the fix every sample took the true top card (Bludgeon in this order,
/// Wound when reversed), and its value moved by 0.043 (12 HP equivalents).
#[test]
fn a_root_draw_is_sampled_whatever_the_hidden_order() {
    for reverse in [false, true] {
        let mut v = fixture();
        if reverse {
            v["deck"]["draw_pile"].as_array_mut().unwrap().reverse();
        }
        let state = build(&v);
        let action = first_play(&state);
        let mut bludgeon = 0u32;
        let mut n = 0u32;
        for seed in 0..40 {
            let samples = fair_root_samples(&state, &action, &params(1600, seed, "beliefdraw"));
            assert_eq!(samples.len() as u32, FAIR_ROOT_SAMPLES);
            assert_eq!(samples.iter().map(|s| s.2).sum::<u32>(), 1600, "the budget split whole");
            bludgeon += samples.iter().filter(|s| drew_bludgeon(&s.0)).count() as u32;
            n += samples.len() as u32;
        }
        let f = f64::from(bludgeon) / f64::from(n);
        assert!((f - 0.5).abs() < 0.08, "reverse={reverse}: Bludgeon drawn in {f:.3} of the samples");
    }
}

fn boomerang() -> Value {
    boomerang_at(3)
}

fn boomerang_at(hp: u32) -> Value {
    // Sword Boomerang (3 hits at random enemies) against two enemies.
    let mut v = fixture();
    v["deck"]["hand"] = json!([
        {"id": "SWORD_BOOMERANG", "kind": "card", "upgraded": false, "stable_idx": 1, "type": "Attack"},
        {"id": "DEFEND_IRONCLAD", "kind": "card", "upgraded": false, "stable_idx": 2, "type": "Skill"}]);
    let mut enemy = v["encounter"][0].clone();
    enemy["max_hp"] = json!(40);
    let mut second = enemy.clone();
    second["uid"] = json!(3);
    second["max_hp"] = json!(40);
    v["encounter"] = json!([enemy, second]);
    v["encounter"][0]["hp"] = json!(hp);
    v["encounter"][1]["hp"] = json!(if hp > 3 { hp + 6 } else { hp }); // asymmetric: where the hits land matters
    if hp > 3 {
        v["player"]["hp"] = json!(60); // a fight to play, not a loss whatever happens
    }
    v
}

/// A root action's random targets don't use the state's real dice: the same
/// piles with the combat streams at other cursors score exactly the same.
/// Before the fix Sword Boomerang's hits (against enemies at 20 and 26 HP:
/// where they land changes what follows) fell where the true cursors put
/// them. (The run seed itself still reaches the search through the run-level
/// streams `belief_root` doesn't reseed, in the ordinary honest search too;
/// a caller hides it by leaving it off the wire, as the mirror does.)
#[test]
fn fair_values_dont_depend_on_the_states_dice() {
    let a = boomerang_at(20);
    let base = q(&a, 1600, "beliefdraw");
    for n in 1..=6 {  // several cursors: one of them rolls another hit pattern (8 patterns of 3 hits)
        let mut b = a.clone();
        b["rng"]["counters"] = json!({"shuffle": n, "monster_ai": n, "combat_targets": n, "niche": n});
        assert_eq!(base, q(&b, 1600, "beliefdraw"), "combat stream cursors at {n}");
    }
}

/// Sword Boomerang's three hits retarget among the living, so they always
/// kill both enemies (3 HP each): every root sample ends the combat. Each
/// ended sample weighs its share of the budget (the same per-sample path a
/// mix of ended and continuing samples takes), so the action's visits are
/// the whole budget and its value the win's. Before, an action that ended
/// the combat counted one visit, whatever the budget the other actions got.
#[test]
fn root_samples_that_end_the_combat_weigh_their_share() {
    let v = boomerang();
    let state = build(&v);
    let action = first_play(&state);
    let samples = fair_root_samples(&state, &action, &params(1600, 5, "beliefdraw"));
    assert!(samples.iter().all(|s| legal_actions(&s.0).is_empty() && s.0.player_won));
    let rows = q(&v, 1600, "beliefdraw");
    let (_, value, visits) = rows.iter().find(|r| r.0.contains("\"card_index\":0")).unwrap();
    assert_eq!(*visits, 1600, "every sample's share of the budget");
    assert!(*value > 0.5, "a win: {value}");
}

/// Outside the honest mode the fair evaluation is as it was: one application
/// on the state itself, the whole budget.
#[test]
fn other_modes_apply_the_action_once() {
    let state = build(&fixture());
    let action = first_play(&state);
    let samples = fair_root_samples(&state, &action, &params(1600, 5, "clairvoyant"));
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].2, 1600);
    assert!(drew_bludgeon(&samples[0].0), "the true top card");
}
