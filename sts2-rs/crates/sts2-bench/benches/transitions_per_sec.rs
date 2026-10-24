//! Phase-8 state-transition throughput bench (bead `sts2-rs-eqa`).
//!
//! Pure `apply(state, Action::EndTurn)` throughput — no MCTS overhead.
//! Isolates the cost of one engine transition so we can map MCTS
//! search cost back to per-action time.
//!
//! Acceptance target from the bead: **≥1M transitions/sec**. Plausible —
//! `combat.rs` already drives ~14 `EndTurns` per combat and reports combats/sec
//! on the same hardware; transitions/sec should be ~14× that number.

// Bead sts2-rs-ung (P8-4): opt-in mimalloc for the bench binary.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: sts2_core::alloc::MiMalloc = sts2_core::alloc::MiMalloc;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use smallvec::SmallVec;

use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    apply, Action, CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory,
    MonsterId, MonsterKind, PlayerCombatState, Side, StepResult, TurnEvents,
};

fn make_cultist_combat(seed: u64) -> CombatState {
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

/// Power-RICH variant of [`make_cultist_combat`]: same Ironclad-starter vs
/// Nibbit shell, but the player + enemy are loaded with a long set of passive
/// powers so every hook dispatcher (`modify_damage`, `modify_hp_lost`,
/// `modify_block`, the `iter_power_listeners` walk, …) resolves a realistic
/// listener set per transition. This is the regime the D1 O(1) power-resolution
/// index targets — the default fixture is power-sparse (1–2 powers), where the
/// O(N·P) scans the index removes are invisible.
///
/// The player is given NO block/heal powers (it plays no cards here and dies
/// only to enemy attacks), and the enemy HP is raised so the fight stays
/// power-rich for its full length instead of ending in a couple of turns.
fn make_power_rich_combat(seed: u64) -> CombatState {
    use sts2_core::card_effects::apply_power_to_creature;
    use sts2_core::PowerKind;

    let mut state = make_cultist_combat(seed);

    // Keep the enemy alive long enough to sample many power-rich transitions.
    if let Some(enemy) = state.creatures.first_mut() {
        enemy.current_hp = 400;
        enemy.max_hp = 400;
    }

    let player_uid = state
        .player
        .creature_uid
        .expect("player creature uid set after start_combat");
    // Passive, non-block-generating powers → sustained dispatch walk without
    // making the player un-killable.
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

    let enemy_uid = state.enemy_uids[0];
    for (kind, amt) in [
        (PowerKind::Strength, 3),
        (PowerKind::Artifact, 2),
        (PowerKind::Metallicize, 3),
        (PowerKind::Thorns, 2),
        (PowerKind::Ritual, 2),
        (PowerKind::Vulnerable, 3),
        (PowerKind::Weak, 3),
    ] {
        apply_power_to_creature(&mut state, enemy_uid, kind, amt);
    }

    state
}

/// Apply `EndTurn` repeatedly until combat is over, counting transitions.
#[inline]
fn drive_until_terminal(state: &mut CombatState) -> u32 {
    let mut n = 0u32;
    while !state.is_over {
        let _ = apply(state, Action::EndTurn);
        n += 1;
        debug_assert!(n < 1000, "end-turn pipeline regression?");
    }
    n
}

fn transitions_per_sec_endturn(c: &mut Criterion) {
    let template = make_cultist_combat(0);
    // Calibrate N once: one full Cultist combat is ~14 EndTurns at hp=80
    // vs Attack(6). Use that as the per-iter transition count so criterion's
    // throughput math reports a clean transitions/sec.
    let n_per_iter: u64 = {
        let mut s = template.clone();
        let count = drive_until_terminal(&mut s);
        u64::from(count)
    };

    let mut group = c.benchmark_group("transitions_per_sec");
    group.throughput(Throughput::Elements(n_per_iter));
    group.bench_function("endturn_full_combat", |b| {
        b.iter_batched(
            || template.clone(),
            |mut state| {
                black_box(drive_until_terminal(&mut state));
                black_box(state);
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn transitions_per_sec_power_rich(c: &mut Criterion) {
    let template = make_power_rich_combat(0);
    let n_per_iter: u64 = {
        let mut s = template.clone();
        let count = drive_until_terminal(&mut s);
        u64::from(count)
    };

    let mut group = c.benchmark_group("transitions_per_sec");
    group.throughput(Throughput::Elements(n_per_iter));
    group.bench_function("endturn_power_rich", |b| {
        b.iter_batched(
            || template.clone(),
            |mut state| {
                black_box(drive_until_terminal(&mut state));
                black_box(state);
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    transitions_per_sec_endturn,
    transitions_per_sec_power_rich
);
criterion_main!(benches);
