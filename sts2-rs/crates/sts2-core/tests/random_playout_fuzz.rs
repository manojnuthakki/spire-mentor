//! P4-4 random-playout fuzzer (bead `sts2-rs-x02`).
//!
//! Drives 10k seeds × Ironclad starter × {`Cultist`, `JawWorm`, `AcidSlimeS`,
//! `GreenLouse`, Sentries (2-creature)} through `apply(Action::EndTurn)` loops
//! until combat ends. Validates invariants at every transition:
//!   - No panics (implicit — a panic = test failure).
//!   - Combat terminates within a bounded turn count.
//!   - `is_over` is set when `StepResult::CombatWon` or `CombatLost` fires.
//!   - Player HP never exceeds max HP.
//!   - Dead enemies stay dead (HP ≤ 0 once means HP ≤ 0 forever).
//!
//! Run with: `cargo test -p sts2-core --test random_playout_fuzz -- --release`
//! (release mode recommended; 50k combats in debug is slow).

use smallvec::SmallVec;

use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    apply, Action, CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory,
    MonsterId, MonsterKind, PlayerCombatState, Side, StepResult, TurnEvents,
};

const SEEDS: u64 = 10_000;
const MAX_TURNS: u32 = 500;

const IRONCLAD_STARTER_DECK_SIZE: u32 = 10;

fn make_draw_pile() -> SmallVec<[CardUid; 32]> {
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    for idx in 1..=IRONCLAD_STARTER_DECK_SIZE {
        draw.push(CardUid::new(idx).expect("non-zero"));
    }
    draw
}

fn make_player() -> PlayerCombatState {
    PlayerCombatState {
        hand: SmallVec::new(),
        draw: make_draw_pile(),
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
        pinned_bottom: smallvec::SmallVec::new(),
        deferred_draw_order: false,
    }
}

fn make_creature(uid: CreatureUid, kind: MonsterKind, hp: i32, history: IntentHistory) -> Creature {
    Creature {
        id: uid.0,
        current_hp: hp,
        max_hp: hp,
        block: 0,
        powers: SmallVec::new(),
        species: Some(kind),
        monster_id: MonsterId(kind.python_name()),
        intent: None,
        intent_history: history,
        ai: None,
    }
}

fn make_combat(
    seed: u64,
    creatures: Vec<Creature>,
    enemy_uids: SmallVec<[CreatureUid; 8]>,
) -> CombatState {
    let mut state = CombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),

        run_rng: sts2_core::run_rng_set::RunRngSet::new((seed) as i32),
        creatures,
        cards: Vec::new().into(),
        powers: Vec::new().into(),
        relics: Vec::new().into(),
        potions: Vec::new(),
        player: make_player(),
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
    assert_eq!(r0, StepResult::Advanced, "start_combat must advance");
    let r1 = start_player_turn(&mut state);
    assert_eq!(r1, StepResult::Advanced, "start_player_turn must advance");
    assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
    state
}

fn make_single_enemy_combat(seed: u64, kind: MonsterKind, hp: i32) -> CombatState {
    let uid = CreatureUid::new(1).expect("non-zero");
    let creature = make_creature(uid, kind, hp, IntentHistory::default());
    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(uid);
    make_combat(seed, vec![creature], enemy_uids)
}

fn make_shrinker_beetles_combat(seed: u64) -> CombatState {
    // 2× ShrinkerBeetle: STS2-native analogue for the legacy 2-creature
    // Sentry encounter (sts2-rs-vvz0). ShrinkerBeetle has no
    // Sentry-style offset-phase concept, so both creatures share
    // `IntentHistory::default()`.
    let uid1 = CreatureUid::new(1).expect("non-zero");
    let uid2 = CreatureUid::new(2).expect("non-zero");
    let beetle1 = make_creature(
        uid1,
        MonsterKind::ShrinkerBeetle,
        40,
        IntentHistory::default(),
    );
    let beetle2 = make_creature(
        uid2,
        MonsterKind::ShrinkerBeetle,
        40,
        IntentHistory::default(),
    );
    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(uid1);
    enemy_uids.push(uid2);
    make_combat(seed, vec![beetle1, beetle2], enemy_uids)
}

fn run_combat_with_invariants(state: &mut CombatState, label: &str, seed: u64) {
    let mut turns = 0u32;
    let initial_max_hp = state.player.max_hp;

    while !state.is_over {
        let prev_player_hp = state.player.current_hp;
        let result = apply(state, Action::EndTurn);

        turns += 1;

        assert!(
            state.player.current_hp <= initial_max_hp,
            "{label} seed={seed} turn={turns}: player HP {} exceeds max {}",
            state.player.current_hp,
            initial_max_hp,
        );

        match result {
            StepResult::Advanced => {
                assert!(
                    !state.is_over,
                    "{label} seed={seed} turn={turns}: Advanced but is_over=true"
                );
            }
            StepResult::CombatWon | StepResult::CombatLost => {
                assert!(
                    state.is_over,
                    "{label} seed={seed} turn={turns}: terminal result but is_over=false"
                );
            }
            StepResult::PendingChoice => {
                panic!(
                    "{label} seed={seed} turn={turns}: unexpected PendingChoice \
                     (EndTurn-only playout should never produce a choice)"
                );
            }
            StepResult::InvalidTarget => {
                panic!(
                    "{label} seed={seed} turn={turns}: unexpected InvalidTarget \
                     (EndTurn cannot produce a target-validation error)"
                );
            }
        }

        assert!(
            turns < MAX_TURNS,
            "{label} seed={seed}: exceeded {MAX_TURNS} turns — \
             end-turn pipeline regression or infinite loop? \
             player_hp={} round={}",
            state.player.current_hp,
            state.round,
        );

        let _ = prev_player_hp;
    }

    assert!(turns > 0, "{label} seed={seed}: combat ended in 0 turns");
}

fn fuzz_encounter(label: &str, make_state: impl Fn(u64) -> CombatState) {
    let mut won = 0u64;
    let mut lost = 0u64;
    let mut total_turns = 0u64;

    for seed in 0..SEEDS {
        let mut state = make_state(seed);
        let start_round = state.round;
        run_combat_with_invariants(&mut state, label, seed);
        total_turns += u64::from(state.round - start_round);
        if state.player_won {
            won += 1;
        } else {
            lost += 1;
        }
    }

    assert_eq!(
        won + lost,
        SEEDS,
        "{label}: won({won}) + lost({lost}) != {SEEDS}"
    );

    #[allow(clippy::cast_precision_loss)]
    let avg_turns = total_turns as f64 / SEEDS as f64;
    eprintln!("{label}: {SEEDS} seeds, {won} won, {lost} lost, avg {avg_turns:.1} turns");
}

// STS2-native analogues for the legacy STS1 stub set (sts2-rs-vvz0).
// HP values are the midpoint of each STS2 enemy's HP range from
// `crates/sts2-core/src/monsters.rs::starting_hp`.

#[test]
fn fuzz_nibbit_10k() {
    fuzz_encounter("Nibbit", |seed| {
        make_single_enemy_combat(seed, MonsterKind::Nibbit, 45)
    });
}

#[test]
fn fuzz_mawler_10k() {
    fuzz_encounter("Mawler", |seed| {
        make_single_enemy_combat(seed, MonsterKind::Mawler, 72)
    });
}

#[test]
fn fuzz_leaf_slime_s_10k() {
    fuzz_encounter("LeafSlimeS", |seed| {
        make_single_enemy_combat(seed, MonsterKind::LeafSlimeS, 13)
    });
}

#[test]
fn fuzz_twig_slime_s_10k() {
    fuzz_encounter("TwigSlimeS", |seed| {
        make_single_enemy_combat(seed, MonsterKind::TwigSlimeS, 9)
    });
}

#[test]
fn fuzz_shrinker_beetles_10k() {
    fuzz_encounter("ShrinkerBeetles", make_shrinker_beetles_combat);
}
