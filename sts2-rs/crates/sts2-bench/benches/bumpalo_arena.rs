//! Library evaluation bench: **bumpalo arena allocation** for MCTS state
//! forking (not a bead — see `docs/perf/2026-06-22-bumpalo-arena-eval.md`).
//!
//! ## The hypothesis under test
//!
//! "MCTS forks a fresh state per node; an arena per rollout amortizes
//! alloc/free into one reset." Concretely, MCTS allocates one
//! `Box<CombatState>` per node (`MctsNode::realized`, used by TT / chance-node
//! search), all alive for the duration of a `search()` and freed at the end.
//! That is the textbook shape for an arena: many same-lifetime allocations,
//! one bulk teardown.
//!
//! ## What this bench actually measures
//!
//! The catch: `CombatState` is **not** a POD. It transitively owns
//! global-allocator heap buffers (a `dotnet_rng::Rng`, the `creatures` /
//! `cards` / `powers` / `relics` / `potions` `Vec`s, and a
//! `PlayerCombatState` with ~a dozen `SmallVec` piles). bumpalo can only
//! bump-allocate the **1368-byte struct shell** — the inner buffers are still
//! `Vec::clone()`d through the global allocator, and a bare `Bump::reset()`
//! would *leak* every one of them (bumpalo runs no destructors on reset). The
//! only correct usage for a heap-owning type is `bumpalo::boxed::Box`, which
//! runs `Drop` — but then each box must be dropped individually, so the inner
//! buffers are freed one-by-one exactly as in the `Box` baseline. The arena
//! amortizes *only* the per-node shell malloc/free into a pointer-bump + one
//! `reset()`.
//!
//! So this A/B isolates precisely the benefit bumpalo can deliver here:
//! replacing N shell mallocs + N shell frees with N pointer-bumps + 1 reset,
//! while both arms pay the identical (dominant) inner-buffer clone cost.
//!
//! `arena_per_search/box`     — current pattern: `Box<CombatState>` × N.
//! `arena_per_search/bumpalo` — `bumpalo::boxed::Box<CombatState>` × N + reset.
//!
//! Run: `cargo bench -p sts2-bench --bench bumpalo_arena`

use bumpalo::Bump;
use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use smallvec::SmallVec;

use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterId,
    MonsterKind, OrbSlots, PlayerCombatState, Side, StepResult, TurnEvents,
};

/// Number of `realized` states alive at once — order-of-magnitude of the node
/// count in a 1000-sim TT/chance-node search, the workload the arena targets.
const N: usize = 1000;

/// Ironclad-vs-Nibbit vertical slice in `PlayerPlayPhase`. Copied inline from
/// `mcts_smoke.rs::make_nibbit_combat` (criterion benches compile standalone;
/// sharing a `mod common` costs more ceremony than the duplication).
fn make_nibbit_combat(seed: u64) -> CombatState {
    let deck_size = ironclad_starter_deck().len();
    debug_assert_eq!(deck_size, 10);
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    for idx in 1..=deck_size {
        let raw = u32::try_from(idx).expect("deck size fits in u32");
        draw.push(CardUid::new(raw).expect("non-zero"));
    }

    let nibbit_uid = CreatureUid::new(1).expect("non-zero");
    let nibbit = Creature {
        id: nibbit_uid.0,
        current_hp: 48,
        max_hp: 48,
        block: 0,
        powers: SmallVec::new(),
        species: Some(MonsterKind::Nibbit),
        monster_id: MonsterId("Nibbit"),
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
        potions: SmallVec::new(),
        potion_slot_indices: SmallVec::new(),
        potion_slots: 0,
        orbs: OrbSlots::default(),
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
        retain_this_turn: SmallVec::new(),
        sly_this_turn: SmallVec::new(),
        sly_this_combat: SmallVec::new(),
        pending_retain_count: 0,
        stars_gained_this_turn: 0,
        pinned_top: SmallVec::new(),
        pinned_bottom: SmallVec::new(),
        deferred_draw_order: false,
    };

    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(nibbit_uid);

    let mut state = CombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new(seed as i32),
        run_rng: sts2_core::run_rng_set::RunRngSet::new(seed as i32),
        creatures: vec![nibbit],
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

    let r0 = start_combat(&mut state);
    debug_assert_eq!(r0, StepResult::Advanced);
    let r1 = start_player_turn(&mut state);
    debug_assert_eq!(r1, StepResult::Advanced);
    debug_assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
    state
}

fn bench_arena_per_search(c: &mut Criterion) {
    sts2_content::cards::dispatch::install_default_registry();
    let template = make_nibbit_combat(0);

    let mut group = c.benchmark_group("arena_per_search");
    // One iteration = allocate + tear down N realized states (one search worth).
    group.throughput(Throughput::Elements(N as u64));

    // Baseline: the pattern in MctsNode::realized today — Box<CombatState>.
    group.bench_function("box", |b| {
        b.iter_batched(
            || template.clone(),
            |state| {
                let mut realized: Vec<Box<CombatState>> = Vec::with_capacity(N);
                for _ in 0..N {
                    realized.push(Box::new(state.clone()));
                }
                black_box(&realized);
                // Drop frees N shells + N×(inner buffers) via the global allocator.
            },
            BatchSize::SmallInput,
        );
    });

    // Arena: bump-allocate the shells, free them in one reset. The boxes still
    // drop individually (that frees the inner Vec/SmallVec buffers — bumpalo
    // can't, since they live on the global heap).
    group.bench_function("bumpalo", |b| {
        let mut bump = Bump::new();
        b.iter_batched(
            || template.clone(),
            |state| {
                {
                    let mut realized: Vec<bumpalo::boxed::Box<'_, CombatState>> =
                        Vec::with_capacity(N);
                    for _ in 0..N {
                        realized.push(bumpalo::boxed::Box::new_in(state.clone(), &bump));
                    }
                    black_box(&realized);
                    // boxes drop here → inner buffers freed (global allocator).
                }
                // Shells freed in one shot; arena memory retained for reuse.
                bump.reset();
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

/// Reference point: cost of a single `CombatState::clone()` with no allocator
/// wrapper at all. Establishes how much of the `arena_per_search` numbers is
/// the unavoidable inner-buffer deep copy (which neither allocator can remove).
fn bench_clone_only(c: &mut Criterion) {
    let template = make_nibbit_combat(0);
    // Characterization: how much of a clone is memcpy vs malloc. A large
    // `size_of` with few heap fields ⇒ memcpy-bound ⇒ no allocator helps.
    eprintln!(
        "size_of::<CombatState>() = {} bytes; size_of::<RunRngSet>() = {} bytes",
        std::mem::size_of::<CombatState>(),
        std::mem::size_of::<sts2_core::run_rng_set::RunRngSet>(),
    );
    let mut group = c.benchmark_group("clone_only");
    group.bench_function("combatstate_clone", |b| {
        b.iter(|| black_box(template.clone()));
    });
    group.finish();
}

criterion_group!(benches, bench_arena_per_search, bench_clone_only);
criterion_main!(benches);
