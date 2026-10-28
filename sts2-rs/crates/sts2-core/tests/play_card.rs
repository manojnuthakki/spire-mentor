//! Behavioral tests for `Action::PlayCard` (bead sts2-rs-y97 — vertical-slice
//! P6 expansion). These exercise the Strike + Defend mechanical correctness
//! independently of the Python sim: a Rust-side fresh combat state is built,
//! cards are played, and post-state invariants are asserted directly.
//!
//! Parity fixtures (Python-vs-Rust byte-equality for `play_card` actions) are
//! a separate follow-up bead — they require wiring the bridge-JSON hand
//! exporter (`crates/sts2-core/src/parity.rs::combat_state_to_bridge_state`
//! currently emits `"hand": []` unconditionally). These behavioral tests
//! catch regressions in the card-play machinery without that prerequisite.

use smallvec::SmallVec;

use sts2_core::card_effects;
use sts2_core::cards::{Card, CardId, CardTags, CardType, TargetType};
use sts2_core::{
    apply, Action, CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory,
    MonsterId, MonsterKind, PlayerCombatState, Side, StepResult, TurnEvents,
};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// Build a fresh `PlayerPlayPhase` state with a hand containing exactly the
/// cards specified. Player at 50/50 HP, energy=3, no powers. A single
/// Cultist at 48/48 HP with no AI (parity-irrelevant). Round=1.
///
/// Each `(card_id, base_damage, base_block, cost, target_type)` tuple
/// becomes one card pushed to the cards arena and into the player's hand.
fn build_handful(cards: &[(CardId, i32, i32, i8, TargetType)]) -> CombatState {
    // Bead sts2-rs-ov4 (P6-1): the `PlayCard` dispatch registry must be
    // populated before any handler can fire. `ensure_installed` is
    // idempotent; calling it from every test fixture is the simplest way
    // to guarantee setup without a one-shot test harness.
    card_effects::ensure_installed();

    let mut cards_arena: Vec<Card> = Vec::with_capacity(cards.len());
    let mut hand: SmallVec<[CardUid; 16]> = SmallVec::new();
    for (idx, &(card_id, base_damage, base_block, cost, target_type)) in cards.iter().enumerate() {
        let raw = u32::try_from(idx + 1).expect("hand size fits u32");
        let uid = CardUid::new(raw).expect("non-zero");
        let card_type = match target_type {
            TargetType::Enemy => CardType::Attack,
            _ => CardType::Skill,
        };
        cards_arena.push(Card {
            uid,
            card_id,
            cost,
            card_type,
            target_type,
            upgraded: false,
            base_damage,
            base_block,
            combat_vars: sts2_core::cards::CombatVars::new(),
            enchantments: sts2_core::cards::CardEnchantments::new(),
            tags: CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        hand.push(uid);
    }

    let player = PlayerCombatState {
        current_hp: 50,
        max_hp: 50,
        block: 0,
        energy: 3,
        max_energy: 3,
        hand,
        draw: SmallVec::new(),
        discard: SmallVec::new(),
        exhaust: SmallVec::new(),
        play: SmallVec::new(),
        stars: 0,
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
    };

    let cultist_uid = CreatureUid::new(1).expect("non-zero");
    let cultist = Creature {
        id: cultist_uid.0,
        current_hp: 48,
        max_hp: 48,
        block: 0,
        powers: SmallVec::new(),
        species: Some(MonsterKind::Nibbit),
        intent: None,
        intent_history: IntentHistory::default(),
        monster_id: MonsterId("Nibbit"),
        ai: None,
    };

    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(cultist_uid);

    CombatState {
        belt_value: None,
        phase: CombatPhase::PlayerPlayPhase,
        round: 1,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new(0_i32),

        run_rng: sts2_core::run_rng_set::RunRngSet::new(0_i32),
        creatures: vec![cultist],
        cards: cards_arena.into(),
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
    }
}

// ---------------------------------------------------------------------------
// Strike
// ---------------------------------------------------------------------------

#[test]
fn play_strike_drops_cultist_hp_by_6() {
    let mut state = build_handful(&[(CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy)]);

    let result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );

    assert_eq!(result, StepResult::Advanced);
    assert_eq!(state.creatures[0].current_hp, 42, "Cultist HP 48 - 6 = 42");
    assert_eq!(state.player.energy, 2, "energy 3 - 1 = 2");
    assert_eq!(state.player.hand.len(), 0, "Strike consumed from hand");
    assert_eq!(state.player.discard.len(), 1, "Strike moved to discard");
    assert_eq!(state.player.play.len(), 0, "play pile cleaned up");
}

#[test]
fn snecko_rolled_cost_reverts_to_base_when_card_is_played() {
    // Bead `sts2-rs-mnjxg`: SneckoOil sets a card's cost via C#
    // `EnergyCost.SetThisTurnOrUntilPlayed` — an `EndOfTurn | WhenPlayed`
    // modifier that clears the MOMENT the card is played (not only at turn
    // start). Model a SneckoOil'd Strike (base cost 1, rolled to 0, delta -1
    // banked) and confirm that playing it reverts the cost to base and clears
    // the banked delta — so a replay this turn (e.g. BottledPotential redraw,
    // coverage seed 5880) pays the BASE cost, not the stale roll.
    use sts2_core::cards::VarKey;
    let mut state = build_handful(&[(CardId::StrikeIronclad, 6, 0, 0, TargetType::Enemy)]);
    // Strike's base cost is 1; the fixture set the (rolled) cost to 0. Bank the
    // Snecko delta the way `drink_snecko_oil` does (roll 0 - base 1 = -1).
    state.cards[0].cost = 0;
    state.cards[0].set_combat_var(VarKey::SneckoCostDeltaThisTurn, -1);

    let result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );
    assert_eq!(result, StepResult::Advanced);
    // Played for the rolled cost (0): energy unchanged from 3.
    assert_eq!(state.player.energy, 3, "rolled cost 0 spent no energy");
    // The played card reverted to its base cost (1) and the delta slot cleared.
    assert_eq!(
        state.cards[0].cost, 1,
        "Snecko cost reverts to base when the card is played (until-played semantics)"
    );
    assert_eq!(
        state.cards[0].combat_var(VarKey::SneckoCostDeltaThisTurn),
        0,
        "banked Snecko delta cleared on play"
    );
}

#[test]
fn play_strike_with_no_target_is_invalid_target() {
    // Bead sts2-rs-vaq: a missing target on an Enemy card surfaces as
    // `InvalidTarget`, not a silent `Advanced` no-op.
    let mut state = build_handful(&[(CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy)]);

    let result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: None,
        },
    );

    assert_eq!(result, StepResult::InvalidTarget);
    assert_eq!(state.creatures[0].current_hp, 48, "no damage");
    assert_eq!(state.player.energy, 3, "no energy spent");
    assert_eq!(state.player.hand.len(), 1, "card stays in hand");
}

#[test]
fn play_strike_out_of_range_target_is_invalid_target() {
    // Bead sts2-rs-vaq: a target slot past the alive-enemy count returns
    // `InvalidTarget` and leaves energy + hand untouched. Construct a
    // single-Cultist state, then aim Strike at slot 1.
    let mut state = build_handful(&[(CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy)]);
    let energy_before = state.player.energy;
    let hand_before = state.player.hand.len();
    let hp_before = state.creatures[0].current_hp;

    let result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(1),
        },
    );

    assert_eq!(result, StepResult::InvalidTarget);
    assert_eq!(
        state.player.energy, energy_before,
        "no energy spent on invalid target"
    );
    assert_eq!(
        state.player.hand.len(),
        hand_before,
        "card stays in hand on invalid target"
    );
    assert_eq!(
        state.creatures[0].current_hp, hp_before,
        "no damage applied"
    );
}

#[test]
fn play_strike_without_enough_energy_is_noop() {
    let mut state = build_handful(&[(CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy)]);
    state.player.energy = 0;

    let result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );

    assert_eq!(result, StepResult::Advanced);
    assert_eq!(state.creatures[0].current_hp, 48, "no damage");
    assert_eq!(state.player.energy, 0);
    assert_eq!(state.player.hand.len(), 1, "card stays in hand");
}

#[test]
fn play_strike_out_of_bounds_hand_index_is_noop() {
    let mut state = build_handful(&[(CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy)]);

    let result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 5,
            target: Some(0),
        },
    );

    assert_eq!(result, StepResult::Advanced);
    assert_eq!(state.creatures[0].current_hp, 48);
    assert_eq!(state.player.hand.len(), 1);
}

// ---------------------------------------------------------------------------
// Defend
// ---------------------------------------------------------------------------

#[test]
fn play_defend_grants_5_block() {
    let mut state = build_handful(&[(CardId::DefendIronclad, 0, 5, 1, TargetType::Self_)]);

    let result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: None,
        },
    );

    assert_eq!(result, StepResult::Advanced);
    assert_eq!(state.player.block, 5);
    assert_eq!(state.player.energy, 2);
    assert_eq!(state.player.hand.len(), 0);
    assert_eq!(state.player.discard.len(), 1);
}

#[test]
fn play_defend_block_stacks_additively() {
    let mut state = build_handful(&[
        (CardId::DefendIronclad, 0, 5, 1, TargetType::Self_),
        (CardId::DefendIronclad, 0, 5, 1, TargetType::Self_),
    ]);

    apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: None,
        },
    );
    assert_eq!(state.player.block, 5);

    apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: None,
        },
    );
    assert_eq!(state.player.block, 10, "two defends stack additively");
}

// ---------------------------------------------------------------------------
// Combat-end semantics
// ---------------------------------------------------------------------------

#[test]
fn strike_killing_last_enemy_returns_combat_won() {
    let mut state = build_handful(&[(CardId::StrikeIronclad, 50, 0, 1, TargetType::Enemy)]);
    // Cultist starts at 48 — one Strike(50) overkills.

    let result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );

    assert_eq!(result, StepResult::CombatWon);
    assert!(state.is_over);
    assert!(state.player_won);
    assert_eq!(state.creatures[0].current_hp, 0);
}

// ---------------------------------------------------------------------------
// Vertical-slice integration: a winning combat against Cultist
// ---------------------------------------------------------------------------
//
// Walks a hand-rolled action sequence that demonstrates Ironclad can actually
// kill a Cultist using only Strike + Defend, threaded through Cultist's
// Dark-Strike enemy turns. Asserts `player_won == true` at the end.
//
// Cultist takes 48 HP. Strike deals 6 per cast. Eight Strikes = 48 dmg = dead.
// Player starts at 50, Cultist deals 6/turn. Player needs to block enough to
// survive 4-5 enemy turns. Defend gives 5 block → enough to fully absorb
// a 6-dmg attack with 1 leak.
//
// Strategy: 3 Strikes + 0 Defends per turn (kills in 3 turns assuming 3
// Strikes per turn, but starter deck has 5 Strikes + 4 Defends + 1 Bash,
// and we draw 5 cards per turn). We don't model deck draws here — we
// hand-construct a state where the hand is curated to demonstrate a win.

fn build_winning_state() -> CombatState {
    // Curated hand: 3 Strikes + 2 Defends (5 cards = BASE_DRAW)
    build_handful(&[
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::DefendIronclad, 0, 5, 1, TargetType::Self_),
        (CardId::DefendIronclad, 0, 5, 1, TargetType::Self_),
    ])
}

#[test]
fn ironclad_can_kill_cultist_via_strike_spam() {
    // Pure damage rollout: 3 Strikes per "turn" against a stationary Cultist.
    // We ignore enemy turns (no apply(EndTurn)) — this test only proves the
    // damage chain works end-to-end. 3 Strikes × 6 dmg = 18 per "turn".
    // 48 / 18 = 2.67 → need 3 turns of 3 Strikes each = 9 Strikes total.
    //
    // The curated hand has only 3 Strikes + 2 Defends. To play 9 Strikes we'd
    // need to redraw — which requires a full deck pile and start_player_turn.
    // For an *isolated* "Strike enough times to kill Cultist" proof we
    // hand-stack 8 Strikes (48 / 6 = 8).
    let mut state = build_handful(&[
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
    ]);
    // Boost energy so we can play all 8 in one "phase" — proves the damage
    // chain, not the energy/turn loop (which is bf8/qct territory).
    state.player.energy = 8;

    for _ in 0..8 {
        let result = apply(
            &mut state,
            Action::PlayCard {
                hand_index: 0,
                target: Some(0),
            },
        );
        if matches!(result, StepResult::CombatWon) {
            break;
        }
    }

    assert!(state.is_over, "combat should end after 8 Strikes");
    assert!(state.player_won, "player should win — Cultist HP = 0");
    assert_eq!(state.creatures[0].current_hp, 0);
}

// Sanity reference: the winning-state helper above is exposed for future
// integration tests that walk player+enemy turns. Currently used only to
// document the curated 5-card starter-style hand.
#[test]
fn winning_state_builder_returns_valid_state() {
    let state = build_winning_state();
    assert_eq!(state.player.hand.len(), 5);
    assert_eq!(state.cards.len(), 5);
    assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
}

// Note: end-to-end "freshly drawn hand from build_encounter_state" smoke is
// exercised by the sts2-py determinism tests (which call build_encounter_state
// → run_playout). Keeping it out of sts2-core's test harness avoids a reverse
// dependency from sts2-core onto sts2-content.

// Moved to sts2-py's test suite: see crates/sts2-py/src/lib.rs::tests
// `build_encounter_state_populates_cards_arena`. That test covers the
// full "freshly built combat → hand populated → play Strike → damage"
// integration without forcing sts2-core to take a reverse dependency
// on sts2-content.

// ---------------------------------------------------------------------------
// Bash (sts2-rs-06w)
// ---------------------------------------------------------------------------
//
// Bash is the third Ironclad starter card: Attack 8, cost 2, applies
// Vulnerable(2) to the target on hit. These tests cover the load-bearing
// invariants of the per-card power-application site introduced in this bead.
//
// Parity fixtures (Python-vs-Rust byte-equality for play_card actions) are
// deferred per the sibling-bead note in tests/play_card.rs:6-10 — the
// recorder lacks a `play_card` action type and the Rust bridge JSON
// hardcodes `"hand": []` at parity.rs:216. These behavioral tests guard
// the Bash-specific damage + Vulnerable application until the bridge hand
// exporter + recorder play_card support land.

use sts2_core::PowerKind;

#[test]
fn play_bash_deals_8_damage_and_applies_vulnerable_2() {
    // Bash on Cultist at 48 HP: post-state Cultist HP = 40, Cultist has
    // Vulnerable(2), player energy = 1 (starting energy 3 - Bash cost 2).
    //
    // The Vulnerable is applied AFTER the damage (matching Python's
    // BashAttack.use order), so the 8 damage is *not* multiplied by 1.5
    // on this play — the multiplier only kicks in on subsequent attacks
    // while Vulnerable persists.
    let mut state = build_handful(&[(CardId::Bash, 8, 0, 2, TargetType::Enemy)]);

    let result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );

    assert_eq!(result, StepResult::Advanced);
    assert_eq!(state.creatures[0].current_hp, 40, "Cultist 48 - 8 = 40");
    assert_eq!(state.player.energy, 1, "Bash costs 2 from 3");
    assert_eq!(state.player.hand.len(), 0, "Bash consumed from hand");
    assert_eq!(state.player.discard.len(), 1, "Bash moved to discard");

    // Vulnerable(2) attached to the Cultist.
    assert_eq!(state.creatures[0].powers.len(), 1);
    let vuln_uid = state.creatures[0].powers[0];
    let vuln = state
        .powers
        .iter()
        .find(|p| p.id == vuln_uid.0)
        .expect("Vulnerable power instance should exist in state.powers");
    assert_eq!(vuln.kind, PowerKind::Vulnerable);
    assert_eq!(vuln.amount, 2);
}

#[test]
fn play_bash_then_strike_uses_vulnerable_multiplier() {
    // Bash (8 dmg, +Vuln 2) → Strike (6 raw × 1.5 = 9 dmg). Cultist
    // 48 → 40 → 31. Locks the canonical composition: Bash applies the
    // debuff after damage; the *next* attack picks up the 3/2 multiplier
    // through the VTHIN-2 damage pipeline.
    let mut state = build_handful(&[
        (CardId::Bash, 8, 0, 2, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
    ]);

    // Play Bash.
    let r1 = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );
    assert_eq!(r1, StepResult::Advanced);
    assert_eq!(state.creatures[0].current_hp, 40);

    // Play Strike — first hand index since Bash moved to discard.
    let r2 = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );
    assert_eq!(r2, StepResult::Advanced);
    // 6 raw × 3/2 (Vulnerable) → floor(9.0) = 9; 40 - 9 = 31.
    assert_eq!(
        state.creatures[0].current_hp, 31,
        "Strike under Vulnerable(2) should deal 9 dmg: 40 - 9 = 31"
    );
}

#[test]
fn play_bash_on_dead_target_no_vulnerable_application() {
    // Bash with absurd damage overkills the Cultist. After the damage step
    // the creature is dead (HP=0); apply_vulnerable_to_enemy must NOT
    // attach a Vulnerable instance (Python apply_power early-returns on
    // dead creatures). The play still resolves to CombatWon.
    let mut state = build_handful(&[(CardId::Bash, 100, 0, 2, TargetType::Enemy)]);

    let result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );

    assert_eq!(result, StepResult::CombatWon);
    assert_eq!(state.creatures[0].current_hp, 0);
    assert!(state.creatures[0].powers.is_empty());
    assert!(state.powers.is_empty());
}

#[test]
fn play_bash_stacks_vulnerable_amount_when_already_present() {
    // Two Bashes back-to-back: the second Bash should ADD to the existing
    // Vulnerable amount (duration stacking) rather than push a second
    // Power instance. After Bash×2 the Cultist carries a single
    // Vulnerable(4) instance.
    let mut state = build_handful(&[
        (CardId::Bash, 8, 0, 2, TargetType::Enemy),
        (CardId::Bash, 8, 0, 2, TargetType::Enemy),
    ]);
    state.player.energy = 4; // 2 × Bash @ 2 each.

    apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );
    apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );

    // Second Bash benefits from Vulnerable(2) applied by the first:
    // 8 raw × 1.5 = 12 dmg; 48 - 8 - 12 = 28.
    assert_eq!(state.creatures[0].current_hp, 28);

    // Exactly one Vulnerable instance, amount = 4 (2 + 2 stacked).
    assert_eq!(state.creatures[0].powers.len(), 1);
    let vuln_uid = state.creatures[0].powers[0];
    let vuln = state
        .powers
        .iter()
        .find(|p| p.id == vuln_uid.0)
        .expect("Vulnerable should still be the only power on the cultist");
    assert_eq!(vuln.kind, PowerKind::Vulnerable);
    assert_eq!(vuln.amount, 4);
}

// ---------------------------------------------------------------------------
// Winning-combat trajectory (sts2-rs-06w)
// ---------------------------------------------------------------------------
//
// Hand-driven proof that the player can win a Cultist combat using the
// starter-card set. The bead spec asks for one demonstrably-winning
// trajectory (not optimal). We curate a hand with 1 Bash + 7 Strikes and
// boost player energy so the play loop completes in a single PlayPhase
// without needing draw / EndTurn / enemy AI plumbing — those paths are
// already exercised in combat_loop.rs's per-phase unit tests; the unique
// contribution of this test is asserting that the *whole* card-play
// machinery composes into a win against a real enemy POD.
//
// Damage budget: Bash (8) + Strike×7 with Vulnerable active = 8 + 7×9 = 71.
// Cultist has 48 HP, so the player wins after Bash + ⌈40/9⌉ = 5 Strikes.

#[test]
fn ironclad_wins_cultist_combat_with_bash_and_strikes() {
    let mut state = build_handful(&[
        (CardId::Bash, 8, 0, 2, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
    ]);
    state.player.energy = 9; // Bash(2) + 7 × Strike(1) = 9.

    // Bash first → applies Vulnerable(2) for the subsequent Strikes.
    let bash_result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );
    assert_eq!(bash_result, StepResult::Advanced);
    assert_eq!(state.creatures[0].current_hp, 40);

    // Strikes until the Cultist dies. Each Strike under Vulnerable deals
    // floor(6 * 1.5) = 9 damage. 40 / 9 = ⌈4.44⌉ = 5 Strikes to kill.
    let mut won = false;
    for _ in 0..7 {
        let r = apply(
            &mut state,
            Action::PlayCard {
                hand_index: 0,
                target: Some(0),
            },
        );
        if matches!(r, StepResult::CombatWon) {
            won = true;
            break;
        }
    }

    assert!(won, "expected CombatWon during Strike spam after Bash");
    assert!(state.is_over);
    assert!(state.player_won, "player_won flag set at combat end");
    assert_eq!(state.creatures[0].current_hp, 0);
}

// ---------------------------------------------------------------------------
// Stomp dynamic cost (bead `sts2-rs-stomp`)
// ---------------------------------------------------------------------------

/// Stomp costs 1 less energy for each Attack the owner plays this turn
/// (C# `Stomp.BeforeCardPlayed` → `EnergyCost.AddThisTurn(-1)`). Playing a
/// single Strike must drop the in-hand Stomp from 3 to 2.
#[test]
fn stomp_costs_one_less_after_one_attack() {
    let mut state = build_handful(&[
        (CardId::StrikeIronclad, 6, 0, 1, TargetType::Enemy),
        (CardId::Stomp, 12, 0, 3, TargetType::AllEnemies),
    ]);
    let stomp = state
        .cards
        .iter()
        .position(|c| c.card_id == CardId::Stomp)
        .expect("Stomp in arena");
    assert_eq!(state.cards[stomp].cost, 3, "Stomp base cost is 3");

    apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        },
    );

    assert_eq!(
        state.cards[stomp].cost, 2,
        "Stomp costs 1 less after playing one Attack"
    );
}

// ---------------------------------------------------------------------------
// X-cost spec regression (bead `sts2-rs-y2znm`)
// ---------------------------------------------------------------------------
//
// Whirlwind / Cascade / Tempest / MultiCast / HeavenlyDrill were ported with
// `cost: 0`, so `Card::is_x_cost()` returned false and the engine never
// stamped `energy_spent` — their handlers silently looped 0 times in real
// combat. The fix sets `cost: -1` in the three *_specs_generated.rs files.
//
// This test exercises Whirlwind through the REAL `apply()` path (not
// `dispatch()`, which bypasses the X-cost drain) to lock in the fix:
// playing Whirlwind with energy=3 must stamp `energy_spent=3` on the card
// and reduce the enemy's HP by `3 ticks × 5 base_damage = 15`.

/// Play Whirlwind (cost -1, AllEnemies, 5 base_damage) with energy=3 via the
/// real engine path.  Asserts:
///   (a) `card.energy_spent == 3` after play (engine stamped it),
///   (b) enemy HP dropped by 15 (3 ticks × 5 damage = effect actually fired).
///
/// Prior to the fix (cost=0), `is_x_cost()` returned false → `energy_spent`
/// stayed 0 → the handler looped 0 times → no damage.
#[test]
fn y2znm_whirlwind_xcost_energy_spent_stamped_and_damage_fires() {
    // Install the full sts2-content card registry so Whirlwind's handler
    // is reachable from `apply()`.
    sts2_content::cards::dispatch::ensure_installed();

    // Build a state with a single Whirlwind (AllEnemies, cost=-1) in hand.
    // build_handful maps TargetType::AllEnemies → CardType::Skill; override
    // to Attack to match C# (Whirlwind is an Attack card).
    let mut state = build_handful(&[(CardId::Whirlwind, 5, 0, -1, TargetType::AllEnemies)]);
    let whirlwind_idx = state
        .cards
        .iter()
        .position(|c| c.card_id == CardId::Whirlwind)
        .expect("Whirlwind in arena");
    state.cards[whirlwind_idx].card_type = CardType::Attack;

    state.player.energy = 3;
    let enemy_hp_before = state.creatures[0].current_hp;

    let result = apply(
        &mut state,
        Action::PlayCard {
            hand_index: 0,
            target: None, // AllEnemies cards dispatch with target=None
        },
    );

    assert_eq!(result, StepResult::Advanced, "Whirlwind play must advance");
    // (a) energy_spent stamped to 3 (pre-play energy).
    assert_eq!(
        state.cards[whirlwind_idx].energy_spent, 3,
        "engine must stamp energy_spent=3 (X-cost gate requires cost=-1)",
    );
    // (b) effect fired: 3 ticks × 5 base_damage = 15 HP reduction.
    assert_eq!(
        enemy_hp_before - state.creatures[0].current_hp,
        15,
        "Whirlwind X=3 must deal 3×5=15 damage — looped 0× before the fix",
    );
    // Player energy drained to 0.
    assert_eq!(state.player.energy, 0, "X-cost drains all energy");
}

/// The discount clamps at 0 (no negative cost) and is fully undone at the
/// owner's next turn start (C# `AddThisTurn` reset). Exercises the
/// `discount` / `restore` helpers directly across the 0-floor.
#[test]
fn stomp_discount_clamps_at_zero_then_restores_at_turn_start() {
    let mut state = build_handful(&[(CardId::Stomp, 12, 0, 3, TargetType::AllEnemies)]);
    let stomp = state
        .cards
        .iter()
        .position(|c| c.card_id == CardId::Stomp)
        .expect("Stomp in arena");

    for _ in 0..5 {
        state.discount_stomp_on_attack_played();
    }
    assert_eq!(
        state.cards[stomp].cost, 0,
        "cost floors at 0 after more attacks than the base cost"
    );

    state.restore_stomp_costs_for_turn_start();
    assert_eq!(
        state.cards[stomp].cost, 3,
        "cost restored to base 3 at the owner's next turn start"
    );
}
