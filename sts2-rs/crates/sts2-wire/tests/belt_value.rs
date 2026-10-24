//! A search's belt value (sts2-agent fork): `RecommendParams::belt_value`,
//! carried on the search's root copy, prices the potion belt in every
//! evaluation of the search, so a line pays for a drink wherever it drinks.
//! And every returned principal variation starts with the recommended action.

use std::sync::Arc;

use serde_json::{json, Value};
use sts2_core::belt_value::BeltValue;
use sts2_core::potions::PotionKind::{self, ExplosiveAmpoule as E, FirePotion as F};
use sts2_core::CombatState;
use sts2_wire::emit::mcts::{recommend_value, RecommendParams};
use sts2_wire::state_from_json_str;

/// The fair-root fixture with no cards to play or draw, the player at 80 HP,
/// a Fuzzy Wurm Crawler at 10 HP attacking for `attack`, and `potions`: only a
/// potion can win (Codex's F1 reproduction, at attack 4).
fn fight(attack: i64, potions: &[&str]) -> CombatState {
    // The content the Python module installs on import: without the potion
    // registry a drink does nothing.
    sts2_content::cards::dispatch::ensure_installed();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
    let mut v: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fair_root_draw.json"),
        )
        .unwrap(),
    )
    .unwrap();
    v["deck"]["hand"] = json!([]);
    v["deck"]["draw_pile"] = json!([]);
    v["player"]["hp"] = json!(80);
    let mut belt: Vec<Value> = potions.iter().map(|p| json!(p)).collect();
    belt.resize(3, Value::Null);
    v["player"]["potions"] = json!(belt);
    v["encounter"][0]["hp"] = json!(10);
    v["encounter"][0]["intents"] = json!([{"amount": attack, "kind": "Attack", "times": 1}]);
    state_from_json_str(&v.to_string()).expect("rebuilds")
}

/// Every sub-multiset of `base`, valued by `value`.
fn table(base: &[PotionKind], value: impl Fn(&[PotionKind]) -> f32) -> Arc<BeltValue> {
    let mut keys: Vec<Vec<PotionKind>> = vec![vec![]];
    for k in base {
        keys = keys.iter().flat_map(|s| [s.clone(), [s.clone(), vec![*k]].concat()]).collect();
    }
    let rows = keys.into_iter().map(|k| { let v = value(&k); (k, v) }).collect();
    Arc::new(BeltValue::new(base.to_vec(), rows, vec![], 8.0).unwrap())
}

#[allow(clippy::cast_precision_loss)]
fn additive(base: &[PotionKind], each: f32) -> Arc<BeltValue> {
    table(base, |k| k.len() as f32 * each)
}

fn params(sims: u32, seed: u64, threads: u32, belt: Option<Arc<BeltValue>>) -> RecommendParams {
    let mut p = RecommendParams::from_config(&json!({"sims": sims, "seed": seed, "threads": threads}), "test").unwrap();
    p.belt_value = belt;
    p
}

fn best(out: &Value) -> Value {
    out["actions"].as_array().unwrap().iter().find(|a| a["is_best"] == true).unwrap()["action"].clone()
}

/// The production seed schedule's first seeds (`splitmix64(agent_seed · 2^32 + n)`, agent seed 0).
fn seeds() -> [u64; 3] {
    let mix = |mut x: u64| {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    };
    [mix(0), mix(1), mix(2)]
}

#[test]
fn a_drink_costs_the_same_whenever_it_happens_so_the_search_doesnt_delay_it() {
    // Codex's F1: with the potion priced only at the root (bc7dc11's charge, the
    // leaf price at 0) the search ended the turn, took the 4 damage, and drank
    // later. Priced by the belt value in every evaluation, waiting only loses HP.
    let state = fight(4, &["FIRE_POTION"]);
    for seed in seeds() {
        let out = recommend_value(&state, &params(5000, seed, 1, Some(additive(&[F], 8.0))), true);
        assert_eq!(best(&out)["action"], "drink_potion", "seed {seed}: {}", best(&out));
    }
}

#[test]
fn a_fight_lost_unless_the_search_drinks_is_won_by_drinking() {
    // The enemy's attack kills the player; only the potion wins, however much it is worth kept.
    let state = fight(200, &["FIRE_POTION"]);
    for each in [8.0, 200.0] {
        let out = recommend_value(&state, &params(2000, seeds()[0], 1, Some(additive(&[F], each))), true);
        assert_eq!(best(&out)["action"], "drink_potion", "kept potion worth {each}");
    }
}

fn drunk(state: &CombatState, out: &Value) -> PotionKind {
    let slot = usize::try_from(best(out)["slot"].as_u64().expect("a drink")).unwrap();
    state.player.potions[slot]
}

#[test]
fn a_non_additive_table_changes_which_potion_is_drunk() {
    // Either potion wins at once. Keeping the Ampoule is worth 20, keeping the
    // Fire Potion 4, and both together no more than the Ampoule alone (not
    // additive): drink the Fire Potion. The mirror table: drink the Ampoule.
    let state = fight(4, &["FIRE_POTION", "EXPLOSIVE_AMPOULE"]);
    let keep_ampoule = table(&[F, E], |k| if k.contains(&E) { 20.0 } else if k.contains(&F) { 4.0 } else { 0.0 });
    let keep_fire = table(&[F, E], |k| if k.contains(&F) { 20.0 } else if k.contains(&E) { 4.0 } else { 0.0 });
    for threads in [1, 2] {
        let a = recommend_value(&state, &params(3000, seeds()[0], threads, Some(keep_ampoule.clone())), true);
        let b = recommend_value(&state, &params(3000, seeds()[0], threads, Some(keep_fire.clone())), true);
        assert_eq!((drunk(&state, &a), drunk(&state, &b)), (F, E), "threads {threads}");
    }
}

#[test]
fn interleaved_searches_with_different_tables_dont_interfere() {
    let state = fight(4, &["FIRE_POTION", "EXPLOSIVE_AMPOULE"]);
    let before = state.clone();
    let keep_ampoule = table(&[F, E], |k| if k.contains(&E) { 20.0 } else if k.contains(&F) { 4.0 } else { 0.0 });
    let keep_fire = table(&[F, E], |k| if k.contains(&F) { 20.0 } else if k.contains(&E) { 4.0 } else { 0.0 });
    let a1 = recommend_value(&state, &params(2000, 5, 1, Some(keep_ampoule.clone())), true);
    let b = recommend_value(&state, &params(2000, 5, 1, Some(keep_fire)), true);
    let plain = recommend_value(&state, &params(2000, 5, 1, None), true);
    let a2 = recommend_value(&state, &params(2000, 5, 1, Some(keep_ampoule)), true);
    assert_eq!(a1, a2);
    assert_ne!(a1, b);
    assert_eq!(plain, recommend_value(&state, &params(2000, 5, 1, None), true));
    assert_eq!(state, before);
    assert!(state.belt_value.is_none());
}

#[test]
fn no_belt_value_leaves_the_deployed_evaluation() {
    // potions · λ (8): the additive 8 table gives the same search.
    let state = fight(4, &["FIRE_POTION"]);
    let plain = recommend_value(&state, &params(1000, 9, 1, None), true);
    let table8 = recommend_value(&state, &params(1000, 9, 1, Some(additive(&[F], 8.0))), true);
    let q = |o: &Value| o["actions"].as_array().unwrap().iter().map(|a| (a["action"].to_string(), a["visits"].clone())).collect::<Vec<_>>();
    assert_eq!(q(&plain), q(&table8));
}

#[test]
fn a_principal_variation_starts_with_the_recommended_action() {
    // Codex's F4: a charge (or the Q ranking) can pick another child than the
    // tree's line; the PV must then be empty, on both output paths.
    let state = fight(4, &["FIRE_POTION"]);
    for threads in [1, 2] {
        for charge in [None, Some(1000.0), Some(-1000.0)] {
            let mut p = params(400, 3, threads, None);
            p.potion_charge_hp = charge.map(|c| [c; 8]);
            let out = recommend_value(&state, &p, true);
            let pv = out["pv"].as_array().unwrap();
            if let Some(first) = pv.first() {
                assert_eq!(*first, best(&out), "threads {threads}, charge {charge:?}");
            }
        }
    }
}

#[test]
fn a_search_with_a_belt_value_tells_states_with_different_belts_apart() {
    // The transposition key holds the belt only when the search prices it: the
    // deployed engine (no belt value) hashes as upstream does.
    let state = fight(4, &["FIRE_POTION", "EXPLOSIVE_AMPOULE"]);
    let drink = |s: &CombatState, slot| {
        let mut s = s.clone();
        sts2_core::apply(&mut s, sts2_core::Action::DrinkPotion { potion_slot: slot, target: None });
        s
    };
    let (a, b) = (drink(&state, 0), drink(&state, 1));
    assert_ne!(a.player.potions, b.player.potions);
    assert_eq!(sts2_core::search_hash(&a), sts2_core::search_hash(&b));
    let mut priced = state.clone();
    priced.belt_value = Some(additive(&[F, E], 8.0));
    let (a, b) = (drink(&priced, 0), drink(&priced, 1));
    assert_ne!(sts2_core::search_hash(&a), sts2_core::search_hash(&b));
    assert_ne!(sts2_core::state_hash(&a), sts2_core::state_hash(&b));
}

#[test]
fn two_drinks_that_leave_different_potions_stay_separate_children_with_a_belt_value() {
    // Before the transposition-table fix both drinks reached states that hashed
    // alike (the belt wasn't hashed), shared one child and reported the same
    // visits and value, whatever the table said.
    let state = fight(4, &["FIRE_POTION", "EXPLOSIVE_AMPOULE"]);
    let keep_ampoule = table(&[F, E], |k| if k.contains(&E) { 20.0 } else if k.contains(&F) { 4.0 } else { 0.0 });
    let out = recommend_value(&state, &params(3000, 1, 1, Some(keep_ampoule)), true);
    let drink = |slot: u64| {
        out["actions"].as_array().unwrap().iter()
            .find(|a| a["action"]["action"] == "drink_potion" && a["action"]["slot"] == slot).unwrap().clone()
    };
    let (fire, ampoule) = (drink(0), drink(1));
    assert!(fire["q"].as_f64().unwrap() > ampoule["q"].as_f64().unwrap() + 0.01, "{fire} {ampoule}");
    let visits = |a: &Value| a["visits"].as_u64().unwrap();
    let total: u64 = out["actions"].as_array().unwrap().iter().map(visits).sum();
    assert_eq!(total, 3000, "each simulation goes through one root child");
}

#[test]
fn a_search_without_a_belt_value_hashes_as_before() {
    // Pinned on the code before the fix (bc7dc11): the belt stays out of the key.
    let state = fight(4, &["FIRE_POTION", "EXPLOSIVE_AMPOULE"]);
    assert_eq!(sts2_core::search_hash(&state), PINNED_SEARCH_HASH);
    assert_eq!(sts2_core::state_hash(&state), PINNED_STATE_HASH);
}
const PINNED_SEARCH_HASH: u64 = 3_536_639_880_288_487_187;
const PINNED_STATE_HASH: u64 = 16_525_178_015_877_102_671;
