//! A captured wire dict goes from JSON text to a live `CombatState` with no
//! Python interpreter anywhere in the process.
//!
//! This is the property the crate exists for. Before the split, reaching
//! `CombatState` from a wire dict meant linking `libpython` and holding the
//! GIL, so nothing outside the `PyO3` extension could replay a capture. The
//! test crate here depends only on `sts2-wire` — if a `pyo3` symbol ever
//! creeps back into the parse path, this stops linking.

use sts2_wire::{parse_json, state_from_json_str, WireError};

fn fixture() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/combat_action_ironclad.json"),
    )
    .expect("fixture readable")
}

#[test]
fn a_captured_dict_parses_without_python() {
    let text = fixture();
    let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    let parsed = parse_json(&value).expect("fixture must parse");

    assert_eq!(parsed.wire_version, 1);
    assert_eq!(parsed.player.class, "ironclad");
    assert_eq!(parsed.player.hp, 54);
    assert_eq!(parsed.player.relics.len(), 2);
    assert_eq!(parsed.player.gold, Some(137));
    assert_eq!(parsed.deck.hand.len(), 2);
    assert_eq!(parsed.deck.draw_pile.len(), 3);
    assert_eq!(parsed.deck.pinned_top, vec![1]);
    assert_eq!(parsed.encounter.len(), 2);
    assert_eq!(parsed.ascension, 7);

    // A `u64` seed above `i64::MAX` — the case a naive `as_i64` read would
    // have silently rejected.
    assert_eq!(parsed.rng_seed, Some(18_446_744_073_709_551_557));

    // Key-sorted, per the `WireValue::entries` contract.
    // Sorted, though the fixture lists them shuffle / monster_ai /
    // combat_targets.
    let counter_names: Vec<&str> = parsed
        .rng_counters
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(counter_names, ["combat_targets", "monster_ai", "shuffle"]);
}

#[test]
fn a_captured_dict_rebuilds_a_combat_state() {
    let state = state_from_json_str(&fixture()).expect("fixture must rebuild");

    assert_eq!(state.player.current_hp, 54);
    assert_eq!(state.player.max_hp, 80);
    assert_eq!(state.player.block, 5);
    // Two enemies on the wire, one of them dead.
    assert_eq!(state.enemy_uids.len(), 2);
    // The piles came across, not just the scalars.
    assert_eq!(state.player.hand.len(), 2);
    assert_eq!(state.player.draw.len(), 3);
    assert_eq!(state.player.discard.len(), 1);
}

/// A wire `combat_vars` value lands behind the card's "may be nonzero" mask
/// (J02). `Card::combat_var` answers 0 from the mask alone when the key's bit
/// is clear, so a reconstruction path that wrote a value without going
/// through the mask-maintaining writers would read the fixture's
/// `free_this_turn: 1` back as 0, and the card would stop being free.
#[test]
fn wire_combat_vars_read_back_through_the_mask() {
    use sts2_core::cards::{CardId, VarKey};

    let state = state_from_json_str(&fixture()).expect("fixture must rebuild");
    let strike = state
        .player
        .hand
        .iter()
        .map(|uid| &state.cards[uid.get() as usize - 1])
        .find(|c| c.card_id == CardId::StrikeIronclad)
        .expect("the hand Strike came across");
    assert!(strike.combat_vars.maybe_nonzero(VarKey::FreeThisTurn));
    assert_eq!(strike.combat_var(VarKey::FreeThisTurn), 1);
    assert!(sts2_core::hooks::card_cost_locally_zeroed(&state, strike));
}

/// The synthetic `status:` / `curse:` ids resolve to their real cards rather
/// than degrading to the AscendersBane placeholder — the single largest
/// reconstruction gap the DLL path ever had, and worth pinning on the
/// interpreter-free route too.
#[test]
fn synthetic_status_and_curse_ids_resolve() {
    let value: serde_json::Value = serde_json::from_str(&fixture()).unwrap();
    let parsed = parse_json(&value).unwrap();

    let slimed = parsed
        .deck
        .draw_pile
        .iter()
        .find(|c| c.raw_id == "status:Slimed")
        .expect("slimed in draw pile");
    assert!(
        slimed.card_id.is_some(),
        "`status:Slimed` must resolve to a real CardId, not fall through to the placeholder"
    );

    let clumsy = parsed
        .deck
        .hand
        .iter()
        .find(|c| c.raw_id == "curse:Clumsy")
        .expect("clumsy in hand");
    assert!(clumsy.card_id.is_some(), "`curse:Clumsy` must resolve");
}

/// A card's own replays (`replay_count`, C# `BaseReplayCount`: Hidden Gem)
/// land on `Card::base_replay_count`; absent is 0, as before the field, and a
/// negative count is refused.
#[test]
fn a_cards_replay_count_comes_across() {
    let mut value: serde_json::Value = serde_json::from_str(&fixture()).unwrap();
    value["deck"]["hand"][0]["replay_count"] = serde_json::json!(2);
    let state = state_from_json_str(&value.to_string()).expect("rebuilds");
    let replays: Vec<i32> = state
        .player
        .hand
        .iter()
        .map(|uid| state.cards[uid.get() as usize - 1].base_replay_count)
        .collect();
    assert_eq!(replays, [2, 0]);

    value["deck"]["hand"][0]["replay_count"] = serde_json::json!(-1);
    let err = parse_json(&value).unwrap_err();
    assert!(matches!(err, WireError::Schema { .. }), "got {err:?}");
    assert_eq!(err.path(), "$.deck.hand[0].replay_count");
}

/// Keyword Sly granted for the combat (MasterPlanner) stays on the card in
/// every pile: a draw-pile card flagged `sly` comes back Sly, and is emitted
/// `sly` there; `sly_turn` (Hand Trick) stays hand-only.
#[test]
fn granted_sly_comes_across_in_every_pile() {
    let mut value: serde_json::Value = serde_json::from_str(&fixture()).unwrap();
    value["deck"]["draw_pile"][1]["flags"] = serde_json::json!(["sly"]); // a Defend: not intrinsically Sly
    let state = state_from_json_str(&value.to_string()).expect("rebuilds");
    let defend = state.player.draw[1];
    assert!(state.player.sly_this_combat.contains(&defend));
    let back = sts2_wire::emit::combat_state_to_value(&state).expect("emits");
    assert_eq!(back["deck"]["draw_pile"][1]["flags"], serde_json::json!(["sly"]));
    assert!(back["deck"]["draw_pile"][0].get("flags").is_none());
}

#[test]
fn failures_carry_their_path_and_class() {
    let mut value: serde_json::Value = serde_json::from_str(&fixture()).unwrap();

    // Unmodelled content → Unsupported.
    value["player"]["relics"][1]["id"] = serde_json::json!("DEFINITELY_NOT_A_RELIC");
    let err = parse_json(&value).unwrap_err();
    assert!(matches!(err, WireError::Unsupported { .. }), "got {err:?}");
    assert_eq!(err.path(), "$.player.relics[1]");
    assert!(err.to_string().contains("DEFINITELY_NOT_A_RELIC"));

    // Invariant violation → Schema.
    let mut value: serde_json::Value = serde_json::from_str(&fixture()).unwrap();
    value["player"]["hp"] = serde_json::json!(999);
    let err = parse_json(&value).unwrap_err();
    assert!(matches!(err, WireError::Schema { .. }), "got {err:?}");
    assert_eq!(err.path(), "$.player");

    // Missing required key → Schema, at the parent path.
    let mut value: serde_json::Value = serde_json::from_str(&fixture()).unwrap();
    value.as_object_mut().unwrap().remove("phase");
    let err = parse_json(&value).unwrap_err();
    assert_eq!(err.path(), "$");
    assert!(err.to_string().contains("missing required key `phase`"));
}

#[test]
fn malformed_json_text_fails_at_the_root() {
    let err = state_from_json_str("{not json").unwrap_err();
    assert_eq!(err.path(), "$");
    assert!(err.to_string().contains("invalid JSON"));
}

/// A small fight: a 100-hp Nibbit, `hand` (card ids) in hand, three Strikes to
/// draw, `relics`, 3 energy.
fn small_fight(hand: &[&str], relics: serde_json::Value) -> serde_json::Value {
    let card = |id: &str, i: usize| serde_json::json!({"id": id, "stable_idx": i, "kind": "card", "upgraded": false});
    serde_json::json!({
        "wire_version": 1, "phase": "PlayerPlayPhase", "round": 1, "side": "Player", "is_elite": false,
        "turn": 1, "combat_id": 0, "rng": {"seed": 1, "ascension": 0},
        "player": {"class": "ironclad", "hp": 80, "max_hp": 80, "block": 0, "energy": 3, "max_energy": 3,
                   "energy_regen_per_turn": 3, "gold": 0, "relics": relics, "powers": [], "potions": []},
        "deck": {"hand": hand.iter().enumerate().map(|(i, id)| card(id, i + 1)).collect::<Vec<_>>(),
                 "draw_pile": (0..3).map(|i| card("STRIKE_IRONCLAD", 10 + i)).collect::<Vec<_>>(),
                 "discard_pile": [], "exhaust_pile": []},
        "encounter": [{"kind": "Nibbit", "uid": 1, "hp": 100, "max_hp": 100, "block": 0, "powers": [],
                       "intents": [{"kind": "Attack", "amount": 6, "times": 1}], "intent_history": [],
                       "is_dead": false, "is_escaped": false}],
    })
}

fn play(state: &mut sts2_core::CombatState, hand_index: u8, target: Option<u8>) {
    let _ = sts2_core::apply(state, sts2_core::Action::PlayCard { hand_index, target });
}

/// C# `SoulsPower.OnEnchant` removes the card's Exhaust keyword: a Soul's
/// Power Molten Fist goes to the discard pile, a plain one is exhausted.
#[test]
fn a_souls_power_card_does_not_exhaust() {
    sts2_content::cards::dispatch::ensure_installed();
    for (enchanted, discarded, exhausted) in [(true, 1, 0), (false, 0, 1)] {
        let mut value = small_fight(&["MOLTEN_FIST"], serde_json::json!([]));
        if enchanted {
            value["deck"]["hand"][0]["enchantment"] = serde_json::json!("SOULS_POWER");
            value["deck"]["hand"][0]["enchantment_amount"] = serde_json::json!(1);
        }
        let mut state = state_from_json_str(&value.to_string()).expect("rebuilds");
        play(&mut state, 0, Some(0));
        assert_eq!(
            (state.player.discard.len(), state.player.exhaust.len()),
            (discarded, exhausted),
            "Soul's Power: {enchanted}"
        );
    }
}

/// C# `BrilliantScarf`: after 4 (non-auto) plays this turn, the 5th card in the
/// hand costs 0; the count (the relic's counter, its `DisplayAmount`) comes from
/// the wire and resets at the player's turn start.
#[test]
fn brilliant_scarf_makes_the_fifth_card_free() {
    sts2_content::cards::dispatch::ensure_installed();
    let scarf = |n: i32| serde_json::json!([{"id": "BRILLIANT_SCARF", "counter": n}]);
    let cost = |state: &sts2_core::CombatState, i: usize| {
        sts2_core::hooks::effective_card_cost(state, state.player.hand[i])
    };
    let state = state_from_json_str(&small_fight(&["BASH", "BASH"], scarf(3)).to_string()).unwrap();
    assert_eq!(cost(&state, 0), 2, "the 4th play pays");

    let mut state = state_from_json_str(&small_fight(&["BASH", "BASH"], scarf(4)).to_string()).unwrap();
    assert_eq!((cost(&state, 0), cost(&state, 1)), (0, 0), "the 5th is free, whichever it is");
    play(&mut state, 0, Some(0));
    assert_eq!(state.player.energy, 3, "the free Bash cost nothing");
    assert_eq!(state.relics[0].counter, 5);
    assert_eq!(cost(&state, 0), 2, "the 6th pays again");

    let _ = sts2_core::apply(&mut state, sts2_core::Action::EndTurn);
    assert_eq!(state.relics[0].counter, 0, "reset at the player's turn start");
}

/// `OvicopterNormal.Slots = [egg1..egg5, ovicopter]`: a rebuilt Ovicopter holds
/// slot 5 and the listed eggs the top egg slots, so its next lay puts the new
/// eggs before it, as the game's slot-sorted enemy list does.
#[test]
fn an_ovicopter_fight_rebuilds_with_its_egg_slots() {
    sts2_content::cards::dispatch::ensure_installed();
    let mut value = small_fight(&["STRIKE_IRONCLAD"], serde_json::json!([]));
    let enemy = |kind: &str, uid: i32, mv: &str| serde_json::json!({"kind": kind, "uid": uid, "hp": 20,
        "max_hp": 20, "block": 0, "powers": [], "intents": [], "intent_history": [], "move_state": mv,
        "is_dead": false, "is_escaped": false});
    value["encounter"] = serde_json::json!([enemy("ToughEgg", 1, "HATCH_MOVE"), enemy("ToughEgg", 2, "HATCH_MOVE"),
                                            enemy("Ovicopter", 3, "SMASH_MOVE")]);
    let state = state_from_json_str(&value.to_string()).expect("rebuilds");
    let slots: Vec<Option<u8>> = state.enemy_uids.iter().map(|uid| state.enemy_slot(*uid)).collect();
    assert_eq!(slots, [Some(3), Some(4), Some(5)]);
}
