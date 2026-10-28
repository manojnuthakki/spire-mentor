//! Phase-4 perf baseline bench (bead `sts2-rs-i9c`).
//!
//! Drives a full Cultist-vs-Ironclad combat to completion via
//! `apply(state, Action::EndTurn)` and reports combats/sec. Phase-4 plan
//! target is **≥30k combats/sec** single-thread on a typical developer
//! workstation (see
//! `docs/plans/2026-05-06-rust-porting-plan-v2.1-refined.md` line 109).
//!
//! ## Combat shape
//!
//! - Player: Ironclad starter (`hp=80`, `max_hp=80`, `max_energy=3`) with the
//!   10-card starter deck UIDs (5×Strike, 4×Defend, 1×Bash) seeded into
//!   `state.player.draw`. `state.cards` is left empty — at this pin the
//!   end-turn pipeline operates purely on `CardUid`s in piles
//!   (no `Action::PlayCard` until Phase 6, so the actual `Card` POD
//!   contents aren't dereferenced).
//! - Enemy: 1×Nibbit (hp=45, intent rolled by `start_combat` via the
//!   DSL-attached `MonsterAi`; the legacy STS1 stub enemies were
//!   removed in bead `sts2-rs-v1u5`).
//! - No relics / powers / potions on entry.
//!
//! ## What gets exercised
//!
//! Without `Action::PlayCard` (Phase 6), every "combat" is a sequence of
//! `Action::EndTurn` calls, each walking the full 6-step end-turn chain
//! (`end_player_turn_phase_one` → `phase_two` → `enemy_turn_start` →
//! `enemy_turn_exec` → `enemy_turn_end` → `start_player_turn`). Cultist
//! attacks land at 6 dmg/turn after turn 0; player dies in ~14 `EndTurns`.
//! The bench runs setup + the full `EndTurn` loop inside `b.iter` so the
//! reported per-iteration time is one full combat, not one turn.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use smallvec::SmallVec;

use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    apply, Action, CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory,
    MonsterId, MonsterKind, PlayerCombatState, Side, StepResult, TurnEvents,
};

/// Build a fully-initialized Cultist-vs-Ironclad combat ready for
/// `apply(Action::EndTurn)` to be called repeatedly until `is_over`.
///
/// Returns a state with `phase == PlayerPlayPhase` (i.e. `start_combat`
/// + `start_player_turn` already ran). The seed parameterizes the RNG
///   so callers can construct a tiny seed sweep if they want.
fn make_cultist_combat(seed: u64) -> CombatState {
    // Ironclad starter deck — 10 cards. We only need the UID side of
    // the arena for the EndTurn pipeline (which walks piles by UID,
    // never dereferencing the Card POD until Phase 6 lands
    // Action::PlayCard). `state.cards` therefore stays empty; the
    // starter deck is consulted only to pin the deck size at 10
    // (matches Python's `_get_starter_deck` count).
    let deck_size = ironclad_starter_deck().len();
    debug_assert_eq!(deck_size, 10);
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    for idx in 1..=deck_size {
        let raw = u32::try_from(idx).expect("deck size fits in u32");
        draw.push(CardUid::new(raw).expect("non-zero"));
    }

    let cultist_uid = CreatureUid::new(1).expect("non-zero");
    let cultist = Creature {
        id: cultist_uid.0,
        current_hp: 48,
        max_hp: 48,
        block: 0,
        powers: SmallVec::new(),
        species: Some(MonsterKind::Nibbit),
        monster_id: MonsterId("Nibbit"),
        // `start_combat` rolls the initial intent; `None` here is
        // overwritten on the first `start_combat` call.
        intent: None,
        intent_history: IntentHistory::default(),
        ai: None,
    };

    let player = PlayerCombatState {
        hand: SmallVec::new(),
        draw,
        discard: SmallVec::new(),
        exhaust: SmallVec::new(),
        play: SmallVec::new(),
        energy: 0,
        max_energy: 3,
        stars: 0,
        current_hp: 80,
        max_hp: 80,
        block: 0,
        powers: SmallVec::new(),
        character: None,
        potions: smallvec::SmallVec::new(),
        potion_slot_indices: smallvec::SmallVec::new(),
        potion_slots: 0,
        orbs: sts2_core::OrbSlots::default(),
        creature_uid: None,
        pet_uid: None,
        shivs_played_this_turn: 0,
        cards_played_this_turn: [0; 5],
        powered_hits_this_turn: 0,
        powered_hits_by_target_this_turn: smallvec::SmallVec::new(),
        powered_hits_at_card_play_start: 0,
        attack_command_suppressed_this_play: false,
        vigor_latch_card: None,
        generated_cards_this_combat: 0,
        skills_played_this_turn: 0,
        card_plays_started_this_turn: 0,
        cards_first_in_series_this_turn: 0,
        request_end_turn_after_play: false,
        retain_this_turn: smallvec::SmallVec::new(),
        sly_this_turn: smallvec::SmallVec::new(),
        sly_this_combat: smallvec::SmallVec::new(),
        pending_retain_count: 0,
        stars_gained_this_turn: 0,
        pinned_top: smallvec::SmallVec::new(),
        pinned_bottom: SmallVec::new(),
        deferred_draw_order: false,
    };

    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(cultist_uid);

    let mut state = CombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),

        run_rng: sts2_core::run_rng_set::RunRngSet::new((seed) as i32),
        creatures: vec![cultist],
        cards: Vec::new().into(),
        powers: Vec::new().into(),
        relics: Vec::new().into(),
        potions: Vec::new(),
        player,
        allies: SmallVec::new(),
        enemy_uids,
        pending_choice: None,
        resolving_card: None,
        pending_play: None,
        pending_turn_setup_resume: false,
        pending_draw_resume: None,
        pending_snecko_cost_randomize: false,
        pending_cluster_resume: None,
        pending_hand_draw_resume: None,
        pending_relic_hand_draw_resume: None,
        pending_wlp_retain_resume: false,
        pending_end_turn_resume: None,
        pending_autoplay_resume: None,
        autoplay_inline_resolve: false,
        pending_curse_choice: None,
        pending_potion_hook_resume: None,
        pending_enemy_exec_resume: None,
        combat_log: TurnEvents::default(),
        swipe_stolen: Vec::new(),
        escaped_creatures: 0,
        heist_recovered_gold: 0,
        room_extra_rewards: Vec::new(),
        deck_card_uids: Vec::new(),
        event_combat_timed_out: false,
        gold: 0,
        is_over: false,
        player_won: false,
        ascension: 0,
        master_deck: Vec::new(),
        last_added_card: None,
        is_elite: false,
        is_boss: false,
        act_index: 1,
        active_attack: None,
        energy_wasted: 0,
        cards_drawn: 0,
        pending_run_gold_loss: 0,
        last_drawn_card: None,
    };

    // Walk the setup → PlayerTurnStart → PlayerPlayPhase prologue once
    // outside of the EndTurn loop. `start_combat` rolls Cultist's
    // initial intent; `start_player_turn` clears block (round-1 path),
    // resets energy, draws 5, fires turn-start hooks.
    let r0 = start_combat(&mut state);
    debug_assert_eq!(r0, StepResult::Advanced);
    let r1 = start_player_turn(&mut state);
    debug_assert_eq!(r1, StepResult::Advanced);
    debug_assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);

    state
}

/// Run a single combat to completion via `apply(Action::EndTurn)` until
/// `is_over`. Returns the number of `EndTurn` actions that were fired —
/// useful for sanity-checking that `degenerate ≠ infinite` (a regression
/// in the end-turn pipeline could deadlock here).
#[inline]
fn run_combat(state: &mut CombatState) -> u32 {
    let mut turns = 0u32;
    while !state.is_over {
        let _ = apply(state, Action::EndTurn);
        turns += 1;
        // Defensive cap: a Cultist combat must terminate in well under
        // 100 EndTurns at hp=80, dmg=6 (~14 expected). If we hit the
        // cap, something in the end-turn pipeline regressed — fail the
        // bench loudly rather than spin forever.
        debug_assert!(
            turns < 1000,
            "cultist combat exceeded 1000 EndTurns — end-turn pipeline regression?"
        );
    }
    turns
}

fn cultist_full_combat(c: &mut Criterion) {
    // Sanity check: the deterministic seed-0 combat resolves with the
    // player dead (since cards do nothing yet — Phase 6 territory).
    {
        let mut state = make_cultist_combat(0);
        let turns = run_combat(&mut state);
        assert!(state.is_over, "combat should be is_over");
        assert!(!state.player_won, "Phase-4 perf bench expects CombatLost");
        assert!(
            (10..=30).contains(&turns),
            "expected ~14 turns to die at hp=80 vs Cultist Attack(6); got {turns}"
        );
    }

    let mut group = c.benchmark_group("cultist_full_combat");
    // One iteration = one full combat; throughput in combats/sec.
    group.throughput(Throughput::Elements(1));
    group.bench_function("seed0", |b| {
        b.iter(|| {
            let mut state = make_cultist_combat(black_box(0));
            black_box(run_combat(&mut state));
            black_box(state);
        });
    });
    group.finish();
}

/// Tighter sub-bench: just the `apply(EndTurn)` loop, excluding
/// `make_cultist_combat` setup. Lets us see the per-EndTurn cost in
/// isolation; combats/sec from the main bench dominates this number.
fn cultist_endturn_loop(c: &mut Criterion) {
    let mut group = c.benchmark_group("cultist_endturn_loop");
    group.throughput(Throughput::Elements(1));
    // Pre-build one combat outside the timed loop; clone it inside to
    // get a fresh state per iteration without paying setup cost.
    let template = make_cultist_combat(0);
    group.bench_function("seed0_loop_only", |b| {
        b.iter_batched(
            || template.clone(),
            |mut state| {
                black_box(run_combat(&mut state));
                black_box(state);
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, cultist_full_combat, cultist_endturn_loop);
criterion_main!(benches);
