//! F0 root blindfold regression gates (bead `sts2-rs-vqckz`, plan
//! `docs/plans/2026-07-12-clairvoyance-removal-plan.md` §3.1/§3.4).
//!
//! `HiddenInfoMode::RootScramble` must make the search's chosen action a
//! pure function of the search seed + player-observable information —
//! never of the incoming (unobservable) draw-pile order or RNG cursors.
//! These tests exercise that invariance directly (as opposed to the
//! statistical `clairvoyance_probe` bench, which measures the leak across
//! a corpus).

mod common;

use common::build_vertical_slice;
use sts2_core::blindfold::{belief_root, blindfold_root};
use sts2_core::cards::{pin_bottom, pin_top};
use sts2_mcts::{HiddenInfoMode, Mcts, MctsConfig};

fn root_scramble_config(seed: u64, sims: u32) -> MctsConfig {
    MctsConfig {
        max_simulations: sims,
        seed,
        hidden_info: HiddenInfoMode::RootScramble,
        ..MctsConfig::default()
    }
}

fn belief_draw_config(seed: u64, sims: u32) -> MctsConfig {
    MctsConfig {
        max_simulations: sims,
        seed,
        hidden_info: HiddenInfoMode::BeliefDraw,
        ..MctsConfig::default()
    }
}

/// Two root states that differ ONLY in (a) the unknown portion of the
/// draw-pile order and (b) how far the RNG stream cursors have advanced
/// must produce the SAME chosen action under `RootScramble` at a fixed
/// search seed — the blindfold must canonicalize the pile before
/// reshuffling it, or the two different incoming permutations would land
/// on two different fake orders.
#[test]
fn root_scramble_invariant_to_pile_order_and_rng_cursor() {
    let base = build_vertical_slice(7);
    assert!(
        base.player.draw.len() >= 2,
        "fixture needs a multi-card unknown draw pile to make this test meaningful"
    );

    let state_a = base.clone();

    let mut state_b = base.clone();
    // (a) permute the unknown draw-pile order.
    state_b.player.draw.reverse();
    assert_ne!(
        state_a.player.draw, state_b.player.draw,
        "reversal must actually change the pile order for this test to be meaningful"
    );
    // (b) advance RNG cursors — legacy stream + a couple of named combat
    // streams — away from state_a's cursor positions.
    let _ = state_b.rng.next_int(0, 1_000_000);
    let _ = state_b.run_rng.shuffle.next_int(0, 1_000_000);
    let _ = state_b.run_rng.monster_ai.next_int(0, 1_000_000);

    let cfg = root_scramble_config(0xC0FF_EE00, 200);
    let action_a = Mcts::new(state_a, cfg).search();
    let action_b = Mcts::new(state_b, cfg).search();
    assert_eq!(
        format!("{action_a:?}"),
        format!("{action_b:?}"),
        "RootScramble must pick the same action regardless of the incoming \
         hidden pile order / RNG cursor position"
    );
}

/// Determinism: the same state + config searched twice (fresh `Mcts`
/// instances) must pick the same action — `RootScramble`'s blindfold seed
/// is a pure function of `config.seed`, so repeating the search must be
/// exactly repeatable (a property downstream measurement campaigns rely
/// on for reproducibility).
#[test]
fn root_scramble_is_deterministic_for_fixed_seed() {
    let state = build_vertical_slice(11);
    let cfg = root_scramble_config(0xBADC_0FFE, 200);

    let action_1 = Mcts::new(state.clone(), cfg).search();
    let action_2 = Mcts::new(state, cfg).search();
    assert_eq!(
        format!("{action_1:?}"),
        format!("{action_2:?}"),
        "same state + config must reproduce the same chosen action"
    );
}

/// `Mcts::reroot` installs the caller's state verbatim (no blindfold), so
/// tree reuse under an honest mode would silently restore full
/// clairvoyance from move 2 onward. It must hard-assert instead of
/// re-leaking — honest modes require a fresh `Mcts` per decision.
#[test]
#[should_panic(expected = "reroot is unsupported under RootScramble")]
fn reroot_panics_under_root_scramble() {
    let state = build_vertical_slice(3);
    let cfg = root_scramble_config(0xDEAD_BEEF, 50);
    let mut mcts = Mcts::new(state.clone(), cfg);
    let action = mcts.search();
    // The panic must fire regardless of whether the edge exists — reroot
    // asserts before any tree inspection.
    let _ = mcts.reroot(&action, state);
}

/// `blindfold_root` must preserve pinned-position knowledge exactly like
/// `cards::shuffle` does: pinned cards keep their declared slot, and the
/// unpinned middle stays a permutation of the same card multiset (no
/// cards created, destroyed, or moved between piles).
#[test]
fn blindfold_root_preserves_pins() {
    let mut state = build_vertical_slice(13);
    assert!(
        state.player.draw.len() >= 4,
        "fixture needs at least 4 draw-pile cards to pin top+bottom and still have a middle"
    );

    let top_uid = state.player.draw[0];
    let bottom_uid = state.player.draw[state.player.draw.len() - 1];
    pin_top(&mut state.player, top_uid);
    pin_bottom(&mut state.player, bottom_uid);

    let mut original_middle: Vec<_> = state
        .player
        .draw
        .iter()
        .copied()
        .filter(|uid| *uid != top_uid && *uid != bottom_uid)
        .collect();
    original_middle.sort_by_key(|uid| uid.0);

    blindfold_root(&mut state, 0x5EED_0042);

    assert_eq!(
        state.player.pinned_top.as_slice(),
        &[top_uid],
        "pinned_top must survive the blindfold unchanged"
    );
    assert_eq!(
        state.player.pinned_bottom.as_slice(),
        &[bottom_uid],
        "pinned_bottom must survive the blindfold unchanged"
    );
    assert_eq!(
        state.player.draw.first().copied(),
        Some(top_uid),
        "pinned top card must stay at the top of the draw pile"
    );
    assert_eq!(
        state.player.draw.last().copied(),
        Some(bottom_uid),
        "pinned bottom card must stay at the bottom of the draw pile"
    );

    let mut middle_after: Vec<_> = state
        .player
        .draw
        .iter()
        .copied()
        .filter(|uid| *uid != top_uid && *uid != bottom_uid)
        .collect();
    middle_after.sort_by_key(|uid| uid.0);
    assert_eq!(
        middle_after, original_middle,
        "the unpinned middle must remain a permutation of the same card multiset"
    );
}

// ── F2 `HiddenInfoMode::BeliefDraw` (bead `sts2-rs-09x5n`, lightspeed port) ──

/// Two root states that differ ONLY in (a) the unknown portion of the
/// draw-pile order and (b) how far the RNG stream cursors have advanced
/// must produce the SAME chosen action under `BeliefDraw` at a fixed
/// search seed — mirrors `root_scramble_invariant_to_pile_order_and_rng_cursor`
/// for the F2 mode. `belief_root` canonicalizes (sorts, no shuffle) before
/// touching anything else, so the two incoming permutations collapse to the
/// identical belief-state pile before the search ever reads it.
#[test]
fn belief_draw_invariant_to_pile_order_and_rng_cursor() {
    let base = build_vertical_slice(7);
    assert!(
        base.player.draw.len() >= 2,
        "fixture needs a multi-card unknown draw pile to make this test meaningful"
    );

    let state_a = base.clone();

    let mut state_b = base.clone();
    state_b.player.draw.reverse();
    assert_ne!(
        state_a.player.draw, state_b.player.draw,
        "reversal must actually change the pile order for this test to be meaningful"
    );
    let _ = state_b.rng.next_int(0, 1_000_000);
    let _ = state_b.run_rng.shuffle.next_int(0, 1_000_000);
    let _ = state_b.run_rng.monster_ai.next_int(0, 1_000_000);

    let cfg = belief_draw_config(0xC0FF_EE00, 200);
    let action_a = Mcts::new(state_a, cfg).search();
    let action_b = Mcts::new(state_b, cfg).search();
    assert_eq!(
        format!("{action_a:?}"),
        format!("{action_b:?}"),
        "BeliefDraw must pick the same action regardless of the incoming hidden \
         pile order / RNG cursor position"
    );
}

/// Determinism: the same state + config searched twice (fresh `Mcts`
/// instances) must pick the same action under `BeliefDraw`.
#[test]
fn belief_draw_is_deterministic_for_fixed_seed() {
    let state = build_vertical_slice(11);
    let cfg = belief_draw_config(0xBADC_0FFE, 200);

    let action_1 = Mcts::new(state.clone(), cfg).search();
    let action_2 = Mcts::new(state, cfg).search();
    assert_eq!(
        format!("{action_1:?}"),
        format!("{action_2:?}"),
        "same state + config must reproduce the same chosen action"
    );
}

/// `Mcts::reroot` must hard-assert under `BeliefDraw` exactly like it does
/// under `RootScramble`/`DrawChance` — reroot installs the caller's state
/// verbatim (no belief-root conversion), which would silently drop
/// `deferred_draw_order` and re-leak hidden information from move 2 onward.
#[test]
#[should_panic(expected = "reroot is unsupported under BeliefDraw")]
fn reroot_panics_under_belief_draw() {
    let state = build_vertical_slice(3);
    let cfg = belief_draw_config(0xDEAD_BEEF, 50);
    let mut mcts = Mcts::new(state.clone(), cfg);
    let action = mcts.search();
    let _ = mcts.reroot(&action, state);
}

/// `belief_root` must preserve pinned-position knowledge exactly like
/// `blindfold_root` does: pinned cards keep their declared slot, and the
/// unpinned middle stays a permutation of the same card multiset — plus
/// (unique to F2) the middle ends up SORTED and
/// [`sts2_core::PlayerCombatState::deferred_draw_order`] is set.
#[test]
fn belief_root_preserves_pins_sorts_middle_and_sets_flag() {
    let mut state = build_vertical_slice(13);
    assert!(
        state.player.draw.len() >= 4,
        "fixture needs at least 4 draw-pile cards to pin top+bottom and still have a middle"
    );
    assert!(
        !state.player.deferred_draw_order,
        "a freshly-built fixture must default the flag to false"
    );

    let top_uid = state.player.draw[0];
    let bottom_uid = state.player.draw[state.player.draw.len() - 1];
    pin_top(&mut state.player, top_uid);
    pin_bottom(&mut state.player, bottom_uid);

    let mut expected_middle: Vec<_> = state
        .player
        .draw
        .iter()
        .copied()
        .filter(|uid| *uid != top_uid && *uid != bottom_uid)
        .collect();
    expected_middle.sort_by_key(|uid| uid.0);

    belief_root(&mut state, 0x5EED_0043);

    assert!(
        state.player.deferred_draw_order,
        "belief_root must set the flag so cards::draw defers randomness"
    );
    assert_eq!(state.player.pinned_top.as_slice(), &[top_uid]);
    assert_eq!(state.player.pinned_bottom.as_slice(), &[bottom_uid]);
    assert_eq!(state.player.draw.first().copied(), Some(top_uid));
    assert_eq!(state.player.draw.last().copied(), Some(bottom_uid));

    let middle_after: Vec<_> = state
        .player
        .draw
        .iter()
        .copied()
        .filter(|uid| *uid != top_uid && *uid != bottom_uid)
        .collect();
    assert_eq!(
        middle_after, expected_middle,
        "the unpinned middle must be the SAME multiset, in SORTED (canonical) order"
    );
}

/// F7 `havoc-belief`, second half: a belief root's search must not depend on
/// the state's RUN-level randomness (the run seed and the cursors of the
/// run-draw streams: rewards, shops, transformations, unknown-map-point,
/// treasure-room), only on the search seed. The combat engine reads none of
/// those streams, so the root Q and the chosen action are identical across
/// states that differ only there (the old note's "about 0.01-0.03 in Q" does
/// not reproduce at this pin: the run-level reseed is moot, not a defect).
#[test]
fn belief_draw_root_value_ignores_the_run_seed_and_run_stream_cursors() {
    let base = build_vertical_slice(7);
    let cfg = belief_draw_config(0xC0FF_EE01, 400);

    let mut reference = Mcts::new(base.clone(), cfg);
    let ref_action = reference.search();
    let ref_q = reference.root_q();

    for run_seed in [1, 12345, -987_654] {
        let mut state = base.clone();
        state.run_rng = sts2_core::run_rng_set::RunRngSet::new(run_seed);
        // Same combat-stream positions as `base` are irrelevant (belief_root
        // abandons them); advance every run-level stream too.
        for _ in 0..5 {
            let _ = state.run_rng.rewards.next_int(0, 1_000_000);
            let _ = state.run_rng.shops.next_int(0, 1_000_000);
            let _ = state.run_rng.transformations.next_int(0, 1_000_000);
            let _ = state.run_rng.unknown_map_point.next_int(0, 1_000_000);
            let _ = state.run_rng.treasure_room.next_int(0, 1_000_000);
        }
        let mut m = Mcts::new(state, cfg);
        let action = m.search();
        assert_eq!(format!("{action:?}"), format!("{ref_action:?}"));
        assert_eq!(
            m.root_q().map(f32::to_bits),
            ref_q.map(f32::to_bits),
            "run seed {run_seed}: the root Q must not move with run-level state"
        );
    }
}
