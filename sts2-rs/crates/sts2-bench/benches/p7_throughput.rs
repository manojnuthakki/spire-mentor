//! P7 throughput bench — bead `sts2-rs-d19`.
//!
//! Measures combats-per-second for an Ironclad-vs-Cultist Act-1 combat
//! **with the full P7 content stack installed**: relic / potion / orb
//! dispatch registries are populated before the timed loop. Sister
//! bench `combat.rs` reports the pre-P7 baseline (registries empty);
//! the delta between the two captures the dispatch cost of the
//! Phase-7 content layer.
//!
//! Bead acceptance: report the measured throughput as a baseline.
//! No hard gate at a specific combats/sec; if the measured number is
//! <30k combats/sec on a typical workstation, an open follow-up bead
//! tracks the perf regression.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use smallvec::SmallVec;

use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_content::potions::dispatch as potion_dispatch;
use sts2_content::relics::dispatch as relic_dispatch;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    apply, Action, CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory,
    MonsterId, MonsterKind, PlayerCombatState, Side, StepResult, TurnEvents,
};

fn make_state(seed: u64) -> CombatState {
    let deck_size = ironclad_starter_deck().len();
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
    let r0 = start_combat(&mut state);
    debug_assert_eq!(r0, StepResult::Advanced);
    let r1 = start_player_turn(&mut state);
    debug_assert_eq!(r1, StepResult::Advanced);
    state
}

#[inline]
fn run_combat(state: &mut CombatState) -> u32 {
    let mut turns = 0u32;
    while !state.is_over {
        let _ = apply(state, Action::EndTurn);
        turns += 1;
        debug_assert!(turns < 1000, "combat exceeded 1000 EndTurns");
    }
    turns
}

fn p7_full_combat(c: &mut Criterion) {
    // Install the full P7 content stack ONCE before the timed loop.
    relic_dispatch::install_default_registry();
    potion_dispatch::install_default_registry();

    let mut group = c.benchmark_group("p7_full_combat_with_content");
    group.throughput(Throughput::Elements(1));
    group.bench_function("seed0", |b| {
        b.iter(|| {
            let mut state = make_state(black_box(0));
            black_box(run_combat(&mut state));
            black_box(state);
        });
    });
    group.finish();
}

fn p7_endturn_loop_only(c: &mut Criterion) {
    relic_dispatch::install_default_registry();
    potion_dispatch::install_default_registry();

    let mut group = c.benchmark_group("p7_endturn_loop_only");
    group.throughput(Throughput::Elements(1));
    let template = make_state(0);
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

criterion_group!(benches, p7_full_combat, p7_endturn_loop_only);
criterion_main!(benches);
