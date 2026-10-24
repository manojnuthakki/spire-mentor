//! Root-parallel MCTS speedup bench.
//!
//! Benches [`sts2_mcts::par_search`] at a fixed *total* simulation budget
//! split across 1/2/4 rayon workers, on a power-rich Ironclad-vs-Nibbit
//! slice (player + enemy loaded with passive powers so the per-rollout
//! engine work is realistic, not the degenerate power-sparse starter).
//!
//! Root parallelization runs `num_workers` independent single-threaded
//! trees on clones of the root, each with `total/num_workers` sims, and
//! merges the per-root-child counters — so the *total* search work is
//! constant across worker counts and the wall-time drop is the parallel
//! speedup. Measured on a 48-core host (2026-06-08): ~1.7x @2, ~3.0x @4,
//! ~5.6x @8 vs single-threaded at the same 2000-sim budget.
//!
//! Run on a quiet host; the absolute searches/sec is load-sensitive, but
//! the worker-to-worker *ratio* (the speedup) is what this bench tracks.

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: sts2_core::alloc::MiMalloc = sts2_core::alloc::MiMalloc;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use smallvec::SmallVec;

use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_core::card_effects::apply_power_to_creature;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterId,
    MonsterKind, OrbSlots, PlayerCombatState, PowerKind, Side, StepResult, TurnEvents,
};
use sts2_mcts::{par_search, MctsConfig};

/// Power-rich Ironclad-vs-Nibbit slice in `PlayerPlayPhase`. Inlined (not
/// shared) for the same reason as the other benches — criterion files
/// compile as standalone binaries.
fn make_power_rich_combat(seed: u64) -> CombatState {
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
        current_hp: 400,
        max_hp: 400,
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

    let player_uid = state
        .player
        .creature_uid
        .expect("player creature uid set after start_combat");
    for (kind, amt) in [
        (PowerKind::Strength, 4),
        (PowerKind::Dexterity, 4),
        (PowerKind::Vigor, 3),
        (PowerKind::Artifact, 2),
        (PowerKind::Thorns, 3),
        (PowerKind::Rupture, 2),
        (PowerKind::Vulnerable, 3),
        (PowerKind::Weak, 3),
        (PowerKind::Frail, 3),
    ] {
        apply_power_to_creature(&mut state, player_uid, kind, amt);
    }
    for (kind, amt) in [
        (PowerKind::Strength, 3),
        (PowerKind::Artifact, 2),
        (PowerKind::Metallicize, 3),
        (PowerKind::Thorns, 2),
        (PowerKind::Ritual, 2),
        (PowerKind::Vulnerable, 3),
        (PowerKind::Weak, 3),
    ] {
        apply_power_to_creature(&mut state, nibbit_uid, kind, amt);
    }

    state
}

fn mcts_parallel_speedup(c: &mut Criterion) {
    sts2_content::cards::dispatch::install_default_registry();

    let template = make_power_rich_combat(0);
    const TOTAL_SIMS: u32 = 2000;

    let mut group = c.benchmark_group("mcts_parallel");
    group.throughput(Throughput::Elements(1)); // one search per iter
    group.sample_size(20);

    let mut seed: u64 = 0;
    for workers in [1u32, 2, 4] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{workers}w_2000sims")),
            &workers,
            |b, &workers| {
                b.iter_batched(
                    || {
                        seed = seed.wrapping_add(1);
                        (template.clone(), seed)
                    },
                    |(state, s)| {
                        let cfg = MctsConfig {
                            c_puct: 1.41,
                            max_simulations: TOTAL_SIMS,
                            max_rollout_depth: 200,
                            seed: s,
                            ..MctsConfig::default()
                        };
                        black_box(par_search(state, cfg, workers));
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

criterion_group!(benches, mcts_parallel_speedup);
criterion_main!(benches);
