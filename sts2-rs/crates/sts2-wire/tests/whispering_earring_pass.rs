//! Whispering Earring's Late pass (F6 step42, review round 1): the paid play is
//! the whole `SpendResources` transaction, and a card that ends the turn stops
//! the pass and runs the end-turn pipeline.
use sts2_wire::scenario::build_scenario_state;

fn state(deck: &[&str], relics: &[&str], character: &str) -> sts2_core::CombatState {
    let deck: Vec<String> = deck.iter().map(|s| (*s).to_string()).collect();
    let relics: Vec<String> = relics.iter().map(|s| (*s).to_string()).collect();
    build_scenario_state(
        1,
        character,
        &deck,
        80,
        80,
        &relics,
        &[],
        "MAWLER_NORMAL",
        0,
        None,
        None,
    )
    .expect("scenario builds")
}

/// F2: five Void Forms with the Earring: the first ends the turn, so the rest
/// stay in the draw/hand and the enemy turn has run (round 2).
#[test]
fn void_form_ends_the_turn_inside_the_earring_pass() {
    let s = state(&["VOID_FORM"; 5], &["WHISPERING_EARRING"], "Regent");
    assert_eq!(
        s.round, 2,
        "the end-turn pipeline ran after the first Void Form"
    );
    assert_eq!(
        s.player_power_amount(sts2_core::PowerKind::VoidForm),
        2,
        "one Void Form was played, not five"
    );
}

/// F1: Stardust under the Earring spends the stars it captured as X.
#[test]
fn stardust_spends_its_stars_under_the_earring() {
    let s = state(
        &[
            "GATHER_LIGHT",
            "STARDUST",
            "STARDUST",
            "STARDUST",
            "STARDUST",
        ],
        &["WHISPERING_EARRING"],
        "Regent",
    );
    eprintln!(
        "stars={} hand={} discard={} energy={}",
        s.player.stars,
        s.player.hand.len(),
        s.player.discard.len(),
        s.player.energy
    );
    assert_eq!(
        s.player.stars, 0,
        "no Stardust left a star another could reuse"
    );
}
