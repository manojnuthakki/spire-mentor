//! Allocation profile of a real `Mcts::search()` — answers "what fraction of a
//! search can an arena actually remove?" (companion to `--bench bumpalo_arena`).
//!
//! A counting global allocator tallies every allocation during real searches,
//! bucketing the ~7176-byte `CombatState` shells (the only thing bumpalo can
//! remove) separately. Then a steady-state microbench measures the true cost
//! of one such shell malloc+free, so we can put an upper bound on bumpalo's
//! end-to-end win:  ceiling ≈ (shell allocs/search × shell alloc cost) / search time.
//!
//! Run: `cargo run -p sts2-bench --example alloc_profile --release`
#![allow(unsafe_code)] // local counting allocator for a measurement-only example

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use smallvec::SmallVec;
use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterId,
    MonsterKind, OrbSlots, PlayerCombatState, Side, StepResult, TurnEvents,
};
use sts2_mcts::{Mcts, MctsConfig};

static N_ALLOC: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static N_SHELL: AtomicU64 = AtomicU64::new(0); // size_of::<CombatState>() == 7176

const SHELL_LO: usize = 7000;
const SHELL_HI: usize = 7500;

struct Counting;

// SAFETY: forwards verbatim to the System allocator; the atomics only observe.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        N_ALLOC.fetch_add(1, Relaxed);
        BYTES.fetch_add(l.size() as u64, Relaxed);
        if (SHELL_LO..=SHELL_HI).contains(&l.size()) {
            N_SHELL.fetch_add(1, Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, l: Layout) {
        unsafe { System.dealloc(ptr, l) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn make_nibbit_combat(seed: u64) -> CombatState {
    let deck_size = ironclad_starter_deck().len();
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    for idx in 1..=deck_size {
        draw.push(CardUid::new(u32::try_from(idx).unwrap()).unwrap());
    }
    let nibbit_uid = CreatureUid::new(1).unwrap();
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
    assert_eq!(start_combat(&mut state), StepResult::Advanced);
    assert_eq!(start_player_turn(&mut state), StepResult::Advanced);
    state
}

fn main() {
    sts2_content::cards::dispatch::install_default_registry();
    let template = make_nibbit_combat(0);
    println!(
        "size_of::<CombatState>() = {} bytes",
        std::mem::size_of::<CombatState>()
    );

    let cfg = |seed: u64| MctsConfig {
        c_puct: 1.41,
        max_simulations: 1000,
        max_rollout_depth: 200,
        seed,
        ..MctsConfig::default()
    };

    // Warm up (registry, caches) — don't count it.
    {
        let mut m = Mcts::new(template.clone(), cfg(0));
        black_box(m.search());
    }

    const K: u64 = 20;
    N_ALLOC.store(0, Relaxed);
    BYTES.store(0, Relaxed);
    N_SHELL.store(0, Relaxed);
    let t0 = Instant::now();
    for s in 0..K {
        let mut m = Mcts::new(template.clone(), cfg(s + 1));
        black_box(m.search());
    }
    let elapsed = t0.elapsed();

    let n_alloc = N_ALLOC.load(Relaxed);
    let bytes = BYTES.load(Relaxed);
    let n_shell = N_SHELL.load(Relaxed);
    let per_search = elapsed / u32::try_from(K).unwrap();

    println!("\n--- per search (default config: 1000 sims, depth 200) ---");
    println!("time/search        : {per_search:?}");
    println!("allocations/search : {}", n_alloc / K);
    println!(
        "bytes/search       : {} ({:.1} MiB)",
        bytes / K,
        (bytes / K) as f64 / (1024.0 * 1024.0)
    );
    println!(
        "shell allocs/search: {}  (7176-byte CombatState boxes — the ONLY thing an arena removes)",
        n_shell / K
    );
    println!(
        "shells as % of all allocations: {:.2}%",
        100.0 * n_shell as f64 / n_alloc as f64
    );

    // Steady-state cost of ONE shell malloc+free (no accumulation — matches the
    // real per-node lifecycle, unlike the accumulate-1000 micro-bench).
    const M: u64 = 2_000_000;
    let t1 = Instant::now();
    for _ in 0..M {
        let b: Box<[u8; 7176]> = Box::new([0u8; 7176]);
        black_box(&b);
    }
    let shell_cost = t1.elapsed() / u32::try_from(M).unwrap();
    println!("\nsteady-state shell malloc+free+zero: {shell_cost:?} each");

    let removable = shell_cost * u32::try_from(n_shell / K).unwrap();
    println!("\n--- bumpalo ceiling ---");
    println!("removable/search (shell alloc/free): ~{removable:?}");
    println!(
        "as % of search time: ~{:.2}%   <-- the absolute most an arena can save",
        100.0 * removable.as_secs_f64() / per_search.as_secs_f64()
    );

    // --- fork-copy breakdown: how much of a clone is the RNG that chance mode
    // overwrites anyway? Clone is not byte-proportional, so measure it. ---
    const C: u64 = 1_000_000;
    let rng_template = template.run_rng.clone();
    let t2 = Instant::now();
    for _ in 0..C {
        black_box(rng_template.clone());
    }
    let rng_clone = t2.elapsed() / u32::try_from(C).unwrap();

    let t3 = Instant::now();
    for _ in 0..C {
        black_box(template.clone());
    }
    let state_clone = t3.elapsed() / u32::try_from(C).unwrap();

    println!("\n--- fork-copy breakdown ---");
    println!("CombatState::clone()      : {state_clone:?}");
    println!(
        "RunRngSet::clone() (4000 B): {rng_clone:?}  ({:.0}% of a CombatState clone)",
        100.0 * rng_clone.as_secs_f64() / state_clone.as_secs_f64()
    );
    let forks = u32::try_from(n_shell / K).unwrap();
    println!(
        "RunRngSet copy across {forks} forks/search: ~{:?}  (~{:.1}% of search) — chance mode \
         reseeds the 9 combat streams, so most of this is thrown away",
        rng_clone * forks,
        100.0 * (rng_clone * forks).as_secs_f64() / per_search.as_secs_f64()
    );
}
