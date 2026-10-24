//! Phase-8 MCTS smoke bench (bead `sts2-rs-eqa`).
//!
//! Runs `Mcts::search()` with `max_simulations = 1000` against the same
//! Ironclad-vs-Nibbit vertical slice used by `combat.rs`, and reports
//! searches/sec (a.k.a. "combats/sec" in the bead's language — each
//! search drives ~1000 rollouts to terminal).
//!
//! ## Note on the bead's `≥100k combats/sec` target
//!
//! One MCTS search with `max_simulations=1000` runs ~1000 rollouts; each
//! rollout plays out to terminal (~14 turns × ~6 actions ≈ ~84 transitions).
//! At 1M transitions/sec engine throughput, a single search costs ~84ms,
//! so ~12 searches/sec is the order of magnitude to expect — not 100k.
//! The bead's headline number conflates "MCTS searches" with "state
//! transitions"; see `transitions_per_sec.rs` for the engine-throughput
//! figure. We report honest numbers here and let the coordinator decide
//! whether bead `sts2-rs-ung` (P8-4 profile-driven optimization) fires.

// Bead sts2-rs-ung (P8-4): opt-in mimalloc for the bench binary. The
// `sts2-bench` crate enables this by default; pass `--no-default-features`
// to A/B against the system allocator.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: sts2_core::alloc::MiMalloc = sts2_core::alloc::MiMalloc;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use smallvec::SmallVec;

use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterId,
    MonsterKind, OrbSlots, PlayerCombatState, Side, StepResult, TurnEvents,
};
use sts2_mcts::{Mcts, MctsConfig};

/// Build the Ironclad-vs-Nibbit vertical slice in `PlayerPlayPhase`. Mirrors
/// `combat.rs::make_nibbit_combat` — kept inline rather than factored out
/// because criterion benches compile as standalone binaries and `mod common`
/// sharing across `benches/*.rs` files requires extra harness ceremony that
/// costs more LOC than the duplication saves.
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
        potions: smallvec::SmallVec::new(),
        potion_slot_indices: smallvec::SmallVec::new(),
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
    enemy_uids.push(nibbit_uid);

    let mut state = CombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),

        run_rng: sts2_core::run_rng_set::RunRngSet::new((seed) as i32),
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

fn mcts_smoke_ironclad_vs_nibbit(c: &mut Criterion) {
    // Bead `sts2-rs-7hhd`: install the card-effect dispatch registry
    // before building the combat template. Without this, every
    // `Action::PlayCard` in the rollout silently no-ops and the MCTS
    // search converges on wins=0 for every action. The bug went
    // undetected because the original bench only `black_box()`-ed the
    // result; the pre-flight assertion below now catches it.
    sts2_content::cards::dispatch::install_default_registry();

    let template = make_nibbit_combat(0);
    let mut group = c.benchmark_group("mcts_smoke");
    // One iteration = one full `Mcts::search()` call (1000 sims).
    group.throughput(Throughput::Elements(1));

    // Bead `sts2-rs-7hhd`: one-time pre-flight smoke check. The
    // original bench `black_box()`-ed the result and silently passed
    // when MCTS was broken (e.g. card dispatch registry uninstalled →
    // every rollout reported wins=0/losses=N). Run one search up-front
    // and assert the search at least *produces* outcomes that sum to
    // the requested sim count — i.e. the rollout loop terminates and
    // the backprop bumps the per-action counters. A stronger
    // wins-or-ties assertion is not possible here because the existing
    // fixture (`cards: Vec::new()` + 48-HP Nibbit + only `EndTurn`
    // legal) is fundamentally degenerate: random play always loses.
    // The proper winnable-state assertion lives in
    // `crates/sts2-py/tests/mcts_recommend_wins.rs`.
    {
        let pre_cfg = MctsConfig {
            c_puct: 1.41,
            max_simulations: 1000,
            // sts2-rs-bhet3: depth=200 is intentional here. This is a
            // throughput / state-transitions criterion bench anchoring
            // the CLAUDE.md "searches/sec" perf number — we want to
            // measure the engine's max work, not leaf-eval quality.
            // Play-strength A/Bs use depth=20 (see mcts_ab*.rs).
            max_rollout_depth: 200,
            seed: 1,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(template.clone(), pre_cfg);
        let _ = mcts.search();
        let outcomes = mcts.root_outcomes();
        let total: u32 = outcomes.iter().map(|o| o.wins + o.losses + o.ties).sum();
        assert!(
            total >= pre_cfg.max_simulations,
            "MCTS smoke: expected backprop to bump >= {} counters; got \
             total={total} across {} outcomes",
            pre_cfg.max_simulations,
            outcomes.len()
        );
        // Sanity: at least the search produced *some* terminal in
        // either bucket — not pegged at 0 across all three counters.
        let total_wins: u32 = outcomes.iter().map(|o| o.wins).sum();
        let total_losses: u32 = outcomes.iter().map(|o| o.losses).sum();
        let total_ties: u32 = outcomes.iter().map(|o| o.ties).sum();
        assert!(
            total_wins + total_losses + total_ties > 0,
            "MCTS smoke: zero terminal outcomes across all buckets — \
             rollouts are not reaching terminal at all"
        );
    }

    let mut seed_counter: u64 = 0;
    group.bench_function("ironclad_vs_nibbit_1000sims", |b| {
        b.iter_batched(
            || {
                seed_counter = seed_counter.wrapping_add(1);
                (template.clone(), seed_counter)
            },
            |(state, seed)| {
                let config = MctsConfig {
                    c_puct: 1.41,
                    max_simulations: 1000,
                    // sts2-rs-bhet3: depth=200 intentional (throughput
                    // anchor, see pre-flight comment above).
                    max_rollout_depth: 200,
                    seed,
                    ..MctsConfig::default()
                };
                let mut mcts = Mcts::new(state, config);
                black_box(mcts.search());
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, mcts_smoke_ironclad_vs_nibbit);
criterion_main!(benches);
