//! Standalone MCTS profiling harness (perf-investigation, not a bead).
//!
//! Builds the same Ironclad-vs-Nibbit slice as `benches/mcts_smoke.rs`
//! and runs N full `Mcts::search()` calls, reporting wall-clock
//! searches/sec. Designed to be run under callgrind for a function-level
//! breakdown of the hot path:
//!
//!   cargo build --release --example mcts_profile
//!   valgrind --tool=callgrind --callgrind-out-file=cg.out \
//!     target/release/examples/mcts_profile <searches> <sims>
//!
//! Plain timing run (no profiler):
//!   target/release/examples/mcts_profile 200 1000

use smallvec::SmallVec;
use std::time::Instant;

use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterId,
    MonsterKind, OrbSlots, PlayerCombatState, Side, StepResult, TurnEvents,
};
use sts2_mcts::{Mcts, MctsConfig};

fn make_nibbit_combat(seed: u64) -> CombatState {
    let deck_size = ironclad_starter_deck().len();
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
    assert_eq!(r0, StepResult::Advanced);
    let r1 = start_player_turn(&mut state);
    assert_eq!(r1, StepResult::Advanced);
    assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
    state
}

fn main() {
    sts2_content::cards::dispatch::install_default_registry();

    let mut args = std::env::args().skip(1);
    let n_searches: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(200);
    let sims: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1000);

    let mut template = make_nibbit_combat(0);

    // Optional `rich` env flag: load the player + enemy with several
    // passive powers so the hook dispatchers (`modify_damage`,
    // `modify_hp_lost`, `modify_block`, ...) walk a realistically-long
    // listener set — the regime where the single-pass listener folds
    // actually pay off (the default starter-deck fixture is power-sparse,
    // so those folds are ~neutral on it). Toggle with `MCTS_PROFILE_RICH=1`.
    if std::env::var("MCTS_PROFILE_RICH").as_deref() == Ok("1") {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::PowerKind;
        let player_uid = template
            .player
            .creature_uid
            .expect("player creature uid set after start_combat");
        for (kind, amt) in [
            (PowerKind::Strength, 3),
            (PowerKind::Dexterity, 3),
            (PowerKind::Vigor, 2),
            (PowerKind::Vulnerable, 2),
            (PowerKind::Weak, 2),
            (PowerKind::Frail, 2),
        ] {
            apply_power_to_creature(&mut template, player_uid, kind, amt);
        }
        let enemy_uid = template.enemy_uids[0];
        for (kind, amt) in [
            (PowerKind::Strength, 3),
            (PowerKind::Vulnerable, 2),
            (PowerKind::Weak, 2),
        ] {
            apply_power_to_creature(&mut template, enemy_uid, kind, amt);
        }
    }

    // Warmup (also primes the allocator / caches).
    {
        let cfg = MctsConfig {
            c_puct: 1.41,
            max_simulations: sims,
            max_rollout_depth: 200,
            seed: 1,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(template.clone(), cfg);
        let _ = mcts.search();
    }

    // Optional 3rd arg: number of root-parallel workers. 0/1 (default)
    // uses the single-threaded `Mcts::search`; >1 splits the sim budget
    // across `par_search` rayon workers (same total sims, lower wall time
    // — i.e. more tree covered per wall-second at a fixed budget).
    let workers: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);

    let start = Instant::now();
    let mut sink = 0u64;
    for i in 0..n_searches {
        let cfg = MctsConfig {
            c_puct: 1.41,
            max_simulations: sims,
            max_rollout_depth: 200,
            seed: u64::from(i) + 1,
            ..MctsConfig::default()
        };
        if workers <= 1 {
            let mut mcts = Mcts::new(template.clone(), cfg);
            let action = mcts.search();
            sink = sink.wrapping_add(format!("{action:?}").len() as u64);
        } else {
            let result = sts2_mcts::par_search(template.clone(), cfg, workers);
            sink = sink.wrapping_add(format!("{:?}", result.best_action).len() as u64);
        }
    }
    let elapsed = start.elapsed();

    let per_search = elapsed.as_secs_f64() / f64::from(n_searches);
    eprintln!(
        "searches={n_searches} sims={sims} workers={workers} total={:.3}s per_search={:.3}ms searches/sec={:.1} (sink={sink})",
        elapsed.as_secs_f64(),
        per_search * 1000.0,
        1.0 / per_search,
    );
}
