//! Property test: the engine is deterministic w.r.t. the RNG seed +
//! input action sequence. Two independently-constructed combats with
//! the same seed, stepped through the same actions, must produce
//! byte-identical [`state_hash`] values at every step.
//!
//! This is the foundational property MCTS rollouts depend on. Any
//! source of non-determinism (HashMap iteration order, thread-local
//! RNG, system time) leaking into the engine would surface here.
//!
//! The fixture borrows the single-enemy-Ironclad shape from
//! `tests/random_playout_fuzz.rs`. Actions are restricted to
//! [`Action::EndTurn`] — it exercises the heavy paths (monster intent
//! rolls, turn-start/end hooks, draw/shuffle/discard) without needing
//! `legal_actions` filtering for legality (Action::EndTurn is always
//! legal during `PlayerPlayPhase`).

use hegel::generators as gs;
use hegel::TestCase;
use smallvec::SmallVec;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::hashing::state_hash;
use sts2_core::{
    apply, Action, CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory,
    MonsterId, MonsterKind, PlayerCombatState, Side, StepResult, TurnEvents,
};

const STARTER_DECK_SIZE: u32 = 10;

fn make_draw_pile() -> SmallVec<[CardUid; 32]> {
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    for idx in 1..=STARTER_DECK_SIZE {
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
        retain_this_turn: SmallVec::new(),
        sly_this_turn: SmallVec::new(),
        sly_this_combat: SmallVec::new(),
        pending_retain_count: 0,
        stars_gained_this_turn: 0,
        pinned_top: SmallVec::new(),
        pinned_bottom: SmallVec::new(),
        deferred_draw_order: false,
    }
}

fn make_creature(uid: CreatureUid, kind: MonsterKind, hp: i32) -> Creature {
    Creature {
        id: uid.0,
        current_hp: hp,
        max_hp: hp,
        block: 0,
        powers: SmallVec::new(),
        species: Some(kind),
        monster_id: MonsterId(kind.python_name()),
        intent: None,
        intent_history: IntentHistory::default(),
        ai: None,
    }
}

fn make_combat(seed: u64, kind: MonsterKind, hp: i32) -> CombatState {
    let uid = CreatureUid::new(1).expect("non-zero");
    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(uid);
    let mut state = CombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),

        run_rng: sts2_core::run_rng_set::RunRngSet::new((seed) as i32),
        creatures: vec![make_creature(uid, kind, hp)],
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
    assert_eq!(start_combat(&mut state), StepResult::Advanced);
    assert_eq!(start_player_turn(&mut state), StepResult::Advanced);
    state
}

const MONSTERS: [MonsterKind; 3] = [
    MonsterKind::Nibbit,
    MonsterKind::Mawler,
    MonsterKind::ShrinkerBeetle,
];

#[hegel::test(test_cases = 200)]
fn same_seed_same_actions_same_hash(tc: TestCase) {
    let seed = tc.draw(gs::integers::<u64>());
    let monster_idx = tc.draw(gs::integers::<u32>().min_value(0).max_value(2)) as usize;
    let kind = MONSTERS[monster_idx];
    let hp = tc.draw(gs::integers::<i32>().min_value(20).max_value(80));
    let n_turns = tc.draw(gs::integers::<u32>().min_value(0).max_value(40));

    let mut state_a = make_combat(seed, kind, hp);
    let mut state_b = make_combat(seed, kind, hp);

    // Initial-state determinism is the first checkpoint: two
    // independently-constructed states with the same seed must hash
    // identically. (Catches any nondeterminism leaking into
    // `start_combat` / `start_player_turn` themselves.)
    assert_eq!(
        state_hash(&state_a),
        state_hash(&state_b),
        "initial-state hashes diverged at seed={seed} kind={kind:?}"
    );

    for turn in 0..n_turns {
        if state_a.is_over || state_b.is_over {
            // Both must end on the same step.
            assert_eq!(
                state_a.is_over, state_b.is_over,
                "is_over diverged at turn {turn}"
            );
            assert_eq!(
                state_a.player_won, state_b.player_won,
                "player_won diverged at turn {turn}"
            );
            break;
        }
        let r_a = apply(&mut state_a, Action::EndTurn);
        let r_b = apply(&mut state_b, Action::EndTurn);
        let h_a = state_hash(&state_a);
        let h_b = state_hash(&state_b);
        if r_a != r_b || h_a != h_b {
            tc.note(&format!(
                "DIVERGED at turn={turn} seed={seed} kind={kind:?} hp={hp}: \
                 r_a={r_a:?} r_b={r_b:?} h_a={h_a:#x} h_b={h_b:#x}"
            ));
        }
        assert_eq!(r_a, r_b, "StepResult diverged at turn {turn}");
        assert_eq!(h_a, h_b, "state_hash diverged at turn {turn}");
    }
}

#[hegel::test(test_cases = 50)]
fn different_seed_usually_differs(tc: TestCase) {
    // Sanity: with overwhelming probability, two different seeds on
    // the same fixture produce different starting hashes. This is the
    // contrapositive sanity check — without it, the determinism test
    // above would also pass if `state_hash` were a constant. We allow
    // for the rare seed collision but require it within 100 turns of
    // EndTurns either the hashes diverge OR the combat terminates.
    let seed_a = tc.draw(gs::integers::<u64>());
    let seed_b = tc.draw(gs::integers::<u64>());
    if seed_a == seed_b {
        return;
    }
    let mut state_a = make_combat(seed_a, MonsterKind::Nibbit, 48);
    let mut state_b = make_combat(seed_b, MonsterKind::Nibbit, 48);

    // The shuffled draw orders should already differ for almost every
    // seed pair; we don't require initial-hash divergence (collisions
    // are possible) but require divergence OR termination within 100
    // turns.
    let mut diverged = state_hash(&state_a) != state_hash(&state_b);
    for _ in 0..100 {
        if state_a.is_over && state_b.is_over {
            break;
        }
        if !state_a.is_over {
            let _ = apply(&mut state_a, Action::EndTurn);
        }
        if !state_b.is_over {
            let _ = apply(&mut state_b, Action::EndTurn);
        }
        if state_hash(&state_a) != state_hash(&state_b) {
            diverged = true;
            break;
        }
    }
    assert!(
        diverged || (state_a.is_over && state_b.is_over),
        "two distinct seeds produced byte-identical trajectories — \
         state_hash may be too coarse, or RNG isn't actually consumed"
    );
}
