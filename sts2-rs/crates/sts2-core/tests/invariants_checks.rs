//! Coverage for [`sts2_core::invariants`] — the checker suite shared by the
//! `combat_step` libfuzzer target and the full-run sweep.
//!
//! Two directions, because a checker that never *fires* is untested in the
//! direction that matters:
//!
//!   * **Holds on real states** — a property test drives an Ironclad starter
//!     deck against a live Nibbit through arbitrary `legal_actions` picks and
//!     asserts every checker returns `Ok` after every step (plus the
//!     cross-turn checks on each player-turn boundary). This is the fuzz
//!     target's own drive loop reduced to a bounded, deterministic property.
//!   * **Fires on broken states** — hand-built states that each violate
//!     exactly one invariant, asserting the specific message comes back.
//!
//! The energy-delta contract in [`check_post_step`] is pinned against the C#:
//! `PlayerCombatState.LoseEnergy` (v0.107.1
//! `MegaCrit.Sts2.Core.Entities.Players/PlayerCombatState.cs:178-185`) clamps
//! to `[0, …]`, so an over-large drain floors at 0 rather than going negative —
//! which is why the checker's `expected` is `.max(0)`.

use hegel::generators as gs;
use hegel::TestCase;
use smallvec::SmallVec;
use std::num::NonZeroU32;

use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_content::monster_dispatch::{build_ai, MonsterSpawnArgs, SpawnPosition};
use sts2_core::card_afflictions::{CardAffliction, CardAfflictionKind};
use sts2_core::cards::{Card, CardId, CardType, TargetType};
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::invariants::{
    check_combat_invariants, check_post_step, check_turn_boundary, has_block_retention,
    snapshot_pre, snapshot_turn_anchor, PreStep, HAND_CAP,
};
use sts2_core::{
    apply, legal_actions, Action, CardUid, CombatPhase, CombatState, Creature, CreatureUid,
    DotNetRng, IntentHistory, MonsterId, MonsterKind, PlayerCombatState, Power, PowerKind,
    PowerUid, Relic, RelicKind, RunRngSet, StepResult,
};

// ── Fixtures ───────────────────────────────────────────────────────────

fn uid(raw: u32) -> CardUid {
    CardUid::new(raw).expect("non-zero")
}

fn cuid(raw: u32) -> CreatureUid {
    CreatureUid::new(raw).expect("non-zero")
}

fn puid(raw: u32) -> PowerUid {
    PowerUid::new(raw).expect("non-zero")
}

fn nz(raw: u32) -> NonZeroU32 {
    NonZeroU32::new(raw).expect("non-zero")
}

/// One arena card at `uid = raw`, cost `cost`. `state.cards` is indexed
/// positionally by `uid - 1` in `effective_card_cost`, so callers must keep
/// the arena ordered by uid.
fn make_card(raw: u32, card_id: CardId, cost: i8, card_type: CardType) -> Card {
    Card {
        uid: uid(raw),
        card_id,
        cost,
        card_type,
        target_type: TargetType::Enemy,
        ..Card::pod_defaults()
    }
}

/// A minimal but *structurally valid* state: one live enemy, a player with
/// real HP. Every negative test below starts here and breaks exactly one
/// thing, so a failure message can only come from the invariant under test.
fn valid_state() -> CombatState {
    let mut s = CombatState {
        phase: CombatPhase::PlayerPlayPhase,
        round: 1,
        player: PlayerCombatState {
            current_hp: 60,
            max_hp: 80,
            ..PlayerCombatState::default()
        },
        ..CombatState::default()
    };
    s.creatures.push(Creature {
        id: nz(1),
        current_hp: 40,
        max_hp: 45,
        block: 0,
        powers: SmallVec::new(),
        species: Some(MonsterKind::Nibbit),
        monster_id: MonsterId("Nibbit"),
        intent: None,
        intent_history: IntentHistory::default(),
        ai: None,
    });
    s.enemy_uids.push(cuid(1));
    s
}

/// Ironclad starter deck vs a live Nibbit, advanced to the first player
/// play phase. `seed` drives both the shuffle and the monster HP roll.
fn ironclad_vs_nibbit(seed: u64) -> CombatState {
    sts2_content::cards::dispatch::ensure_installed();

    let deck = ironclad_starter_deck();
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    let mut arena: Vec<Card> = Vec::with_capacity(deck.len());
    for (idx, spec) in deck.iter().enumerate() {
        let raw = u32::try_from(idx + 1).expect("deck fits u32");
        draw.push(uid(raw));
        arena.push(Card {
            uid: uid(raw),
            card_id: spec.id,
            cost: spec.cost,
            card_type: spec.card_type,
            target_type: spec.target_type,
            base_damage: spec.base_damage,
            base_block: spec.base_block,
            ..Card::pod_defaults()
        });
    }

    let player = PlayerCombatState {
        draw,
        energy: 0,
        max_energy: 3,
        current_hp: 80,
        max_hp: 80,
        ..PlayerCombatState::default()
    };

    let mut rng = DotNetRng::new(seed as i32);
    let hp = MonsterKind::Nibbit.starting_hp(&mut rng, 0);
    let enemy = Creature {
        id: nz(1),
        current_hp: hp,
        max_hp: hp,
        block: 0,
        powers: SmallVec::new(),
        species: Some(MonsterKind::Nibbit),
        monster_id: MonsterId(MonsterKind::Nibbit.python_name()),
        intent: None,
        intent_history: IntentHistory::default(),
        ai: Some(build_ai(
            MonsterKind::Nibbit,
            MonsterSpawnArgs::None,
            SpawnPosition {
                alone: true,
                front: true,
                back: true,
                index: 0,
            },
        )),
    };
    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(cuid(1));

    let mut state = CombatState {
        phase: CombatPhase::Setup,
        rng: DotNetRng::new(seed as i32),
        run_rng: RunRngSet::new(seed as i32),
        creatures: vec![enemy],
        cards: arena.into(),
        player,
        enemy_uids,
        ..CombatState::default()
    };
    assert_eq!(start_combat(&mut state), StepResult::Advanced);
    assert_eq!(start_player_turn(&mut state), StepResult::Advanced);
    state
}

// ── Property: the checkers hold across arbitrary legal play ────────────

/// Every checker must return `Ok` after every step of a real combat driven
/// through arbitrary `legal_actions` picks — the same contract the
/// `combat_step` fuzz target `.unwrap()`s. Also exercises the *passing*
/// direction of the checks that cannot be forced from a hand-built state
/// (unplayable-card filtering, intent presence, energy-delta on real plays).
#[hegel::test(test_cases = 40)]
fn checkers_hold_over_random_legal_play(tc: TestCase) {
    let seed = tc.draw(gs::integers::<u64>());
    let n_steps = tc.draw(gs::integers::<u32>().min_value(1).max_value(60)) as usize;
    let picks: Vec<u16> = (0..n_steps)
        .map(|_| tc.draw(gs::integers::<u16>()))
        .collect();

    let mut state = ironclad_vs_nibbit(seed);
    check_combat_invariants(&state, "post-setup").expect("fresh combat is valid");
    let mut anchor = snapshot_turn_anchor(&state);

    for (step, pick) in picks.iter().copied().enumerate() {
        if state.is_over {
            break;
        }
        let actions = legal_actions(&state);
        if actions.is_empty() {
            break;
        }
        let action = actions[pick as usize % actions.len()].clone();
        let pre = snapshot_pre(&state, &action);
        apply(&mut state, action.clone());

        check_post_step(&pre, &state, "post-step")
            .unwrap_or_else(|e| panic!("seed={seed} step={step} action={action:?}: {e}"));
        check_combat_invariants(&state, "post-step")
            .unwrap_or_else(|e| panic!("seed={seed} step={step} action={action:?}: {e}"));

        if state.round > anchor.round && matches!(state.phase, CombatPhase::PlayerPlayPhase) {
            check_turn_boundary(&anchor, &state, "turn-boundary")
                .unwrap_or_else(|e| panic!("seed={seed} step={step}: {e}"));
            anchor = snapshot_turn_anchor(&state);
        }
    }
}

/// Invariant 8 (`d03` player/creature unification) says the mirror row and
/// the legacy `PlayerCombatState` scalars always agree, and every
/// player-block mutation micro-syncs the row (bead `sts2-rs-73up8`:
/// `gain_block_player` → `sync_creature_block_from_player`, the damage path
/// likewise at `combat_loop.rs:4806`). Regression for the turn-start clear
/// (`combat_loop::continue_player_turn_setup`), which historically skipped
/// the sync and left surviving block behind on the Creature row.
#[test]
fn turn_start_block_clear_must_sync_the_player_mirror() {
    let mut state = ironclad_vs_nibbit(3);
    // Far more block than the Nibbit can chew through, so some survives the
    // enemy turn and reaches the next turn-start clear.
    sts2_core::card_effects::gain_block_player(&mut state, 200);
    assert!(state.player.block > 0);

    apply(&mut state, Action::EndTurn);
    assert_eq!(state.player.block, 0, "turn start clears player block");

    let mirror_uid = state
        .player
        .creature_uid
        .expect("start_combat mirrors player");
    let mirror = state
        .creatures
        .iter()
        .find(|c| c.id == mirror_uid.0)
        .expect("mirror row exists");
    assert_eq!(
        mirror.block, state.player.block,
        "mirror Creature block must follow the turn-start clear"
    );
    check_combat_invariants(&state, "post-turn-start").expect("mirror stays coherent");
}

// ── snapshot_pre ───────────────────────────────────────────────────────

#[test]
fn snapshot_pre_ignores_non_play_actions() {
    let s = valid_state();
    assert!(snapshot_pre(&s, &Action::EndTurn).played.is_none());
}

#[test]
fn snapshot_pre_records_resolved_cost_of_played_card() {
    let mut s = valid_state();
    s.cards
        .push(make_card(1, CardId::StrikeIronclad, 2, CardType::Attack));
    s.player.hand.push(uid(1));
    s.player.energy = 3;

    let pre = snapshot_pre(
        &s,
        &Action::PlayCard {
            hand_index: 0,
            target: None,
        },
    );
    assert_eq!(pre.energy, 3);
    assert!(!pre.is_over);
    assert_eq!(pre.played, Some((CardId::StrikeIronclad, 2, false, 0)));
}

/// An X-cost card carries `cost == -1`; the snapshot clamps that to 0 and
/// flags `x_cost` so `check_post_step` takes the "spends all energy" branch
/// instead of the fixed-delta one.
#[test]
fn snapshot_pre_clamps_x_cost_and_flags_it() {
    let mut s = valid_state();
    s.cards
        .push(make_card(1, CardId::Whirlwind, -1, CardType::Attack));
    s.player.hand.push(uid(1));

    let pre = snapshot_pre(
        &s,
        &Action::PlayCard {
            hand_index: 0,
            target: None,
        },
    );
    let (_, cost, x_cost, weighted) = pre.played.expect("play recorded");
    assert_eq!(cost, 0, "negative sentinel cost must clamp to 0");
    assert!(x_cost);
    assert_eq!(weighted, 0);
}

/// `Weighted` drains extra energy on play, so the snapshot has to carry the
/// stack count or the delta check fires on correct engine behaviour.
#[test]
fn snapshot_pre_carries_weighted_stacks() {
    let mut s = valid_state();
    let mut card = make_card(1, CardId::StrikeIronclad, 1, CardType::Attack);
    card.affliction = Some(CardAffliction {
        kind: CardAfflictionKind::Weighted,
        stacks: 2,
        applied_exhaust: false,
    });
    s.cards.push(card);
    s.player.hand.push(uid(1));

    let pre = snapshot_pre(
        &s,
        &Action::PlayCard {
            hand_index: 0,
            target: None,
        },
    );
    assert_eq!(pre.played, Some((CardId::StrikeIronclad, 1, false, 2)));
}

/// A non-Weighted affliction must not be read as an energy drain.
#[test]
fn snapshot_pre_ignores_other_afflictions() {
    let mut s = valid_state();
    let mut card = make_card(1, CardId::StrikeIronclad, 1, CardType::Attack);
    card.affliction = Some(CardAffliction {
        kind: CardAfflictionKind::Bound,
        stacks: 3,
        applied_exhaust: false,
    });
    s.cards.push(card);
    s.player.hand.push(uid(1));

    let pre = snapshot_pre(
        &s,
        &Action::PlayCard {
            hand_index: 0,
            target: None,
        },
    );
    assert_eq!(pre.played, Some((CardId::StrikeIronclad, 1, false, 0)));
}

#[test]
fn snapshot_pre_out_of_range_hand_index_records_nothing() {
    let s = valid_state();
    let pre = snapshot_pre(
        &s,
        &Action::PlayCard {
            hand_index: 7,
            target: None,
        },
    );
    assert!(pre.played.is_none());
}

// ── check_combat_invariants: creature bounds ───────────────────────────

#[test]
fn creature_hp_over_max_fails() {
    let mut s = valid_state();
    s.creatures[0].current_hp = s.creatures[0].max_hp + 1;
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("creature 1 HP out of range"), "{err}");
}

#[test]
fn creature_negative_hp_fails() {
    let mut s = valid_state();
    s.creatures[0].current_hp = -1;
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("creature 1 HP out of range"), "{err}");
}

#[test]
fn creature_negative_block_fails() {
    let mut s = valid_state();
    s.creatures[0].block = -3;
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("creature 1 block negative"), "{err}");
}

// ── check_combat_invariants: PowerStore index ⇔ arena ──────────────────

/// Two powers sharing one uid alias the `uid -> slot` side index: the id
/// resolves to the *last* slot, so the first slot no longer resolves to
/// itself. `PowerStore::push` debug-asserts against this, so the duplicate
/// has to come in through the `From<Vec<Power>>` rebuild path.
#[test]
fn duplicate_power_id_breaks_the_store_index() {
    let mut s = valid_state();
    s.powers = vec![
        Power::new(nz(1), PowerKind::Strength, 1),
        Power::new(nz(1), PowerKind::Dexterity, 2),
    ]
    .into();
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("PowerStore index/arena disagree"), "{err}");
}

#[test]
fn duplicate_creature_id_fails() {
    let mut s = valid_state();
    let twin = s.creatures[0].clone();
    s.creatures.push(twin);
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("duplicate creature id 1"), "{err}");
}

#[test]
fn duplicate_card_uid_fails() {
    let mut s = valid_state();
    s.cards
        .push(make_card(1, CardId::StrikeIronclad, 1, CardType::Attack));
    s.cards
        .push(make_card(1, CardId::DefendIronclad, 1, CardType::Skill));
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("duplicate card uid"), "{err}");
}

// ── check_combat_invariants: pile partition ────────────────────────────

#[test]
fn card_in_two_piles_fails() {
    let mut s = valid_state();
    s.cards
        .push(make_card(1, CardId::StrikeIronclad, 1, CardType::Attack));
    s.player.hand.push(uid(1));
    s.player.discard.push(uid(1));
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("appears 2 times across piles"), "{err}");
}

#[test]
fn card_in_pile_but_not_in_arena_fails() {
    let mut s = valid_state();
    s.player.exhaust.push(uid(9));
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("not in state.cards arena"), "{err}");
}

/// `pinned_top` / `pinned_bottom` / `retain_this_turn` annotate cards that
/// already live in a pile — they are not piles of their own, so a pinned
/// card counted once in `draw` must NOT trip the partition check.
#[test]
fn annotations_are_not_counted_as_piles() {
    let mut s = valid_state();
    s.cards
        .push(make_card(1, CardId::StrikeIronclad, 1, CardType::Attack));
    s.cards
        .push(make_card(2, CardId::DefendIronclad, 1, CardType::Skill));
    s.player.draw.push(uid(1));
    s.player.pinned_top.push(uid(1));
    s.player.pinned_bottom.push(uid(1));
    s.player.hand.push(uid(2));
    s.player.retain_this_turn.push(uid(2));
    check_combat_invariants(&s, "t").expect("annotations are not piles");
}

#[test]
fn pinned_top_must_be_in_the_draw_pile() {
    let mut s = valid_state();
    s.cards
        .push(make_card(1, CardId::StrikeIronclad, 1, CardType::Attack));
    s.player.hand.push(uid(1));
    s.player.pinned_top.push(uid(1));
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(
        err.contains("in pinned_top but not in the pile it annotates"),
        "{err}"
    );
}

#[test]
fn pinned_bottom_must_resolve_in_the_arena() {
    let mut s = valid_state();
    s.player.pinned_bottom.push(uid(42));
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(
        err.contains("in pinned_bottom but not in state.cards arena"),
        "{err}"
    );
}

/// A single-turn Retain lasts the turn wherever the card goes (C#
/// `ShouldRetainThisTurn` until `EndOfTurnCleanup`; a played card keeps its
/// entry), so it may annotate a card in any pile, but not one in none
/// (sts2-agent fork).
#[test]
fn retain_this_turn_must_be_in_a_pile() {
    let mut s = valid_state();
    s.cards
        .push(make_card(1, CardId::StrikeIronclad, 1, CardType::Attack));
    s.player.draw.push(uid(1));
    s.player.retain_this_turn.push(uid(1));
    check_combat_invariants(&s, "t").expect("a draw-pile card may keep this turn's Retain");
    s.player.draw.clear();
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("in retain_this_turn but in no pile"), "{err}");
}

// ── check_combat_invariants: hand cap / enemy mirror ───────────────────

#[test]
fn hand_over_cap_fails() {
    let mut s = valid_state();
    for i in 1..=(HAND_CAP as u32 + 1) {
        s.cards
            .push(make_card(i, CardId::StrikeIronclad, 1, CardType::Attack));
        s.player.hand.push(uid(i));
    }
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("exceeds cap"), "{err}");
}

#[test]
fn hand_exactly_at_cap_passes() {
    let mut s = valid_state();
    for i in 1..=(HAND_CAP as u32) {
        s.cards
            .push(make_card(i, CardId::StrikeIronclad, 1, CardType::Attack));
        s.player.hand.push(uid(i));
    }
    check_combat_invariants(&s, "t").expect("a full hand is legal");
}

#[test]
fn enemy_uid_without_creature_fails() {
    let mut s = valid_state();
    s.enemy_uids.push(cuid(7));
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("has no corresponding creature"), "{err}");
}

// ── check_combat_invariants: termination consistency ───────────────────

#[test]
fn is_over_with_live_enemy_and_live_player_fails() {
    let mut s = valid_state();
    s.is_over = true;
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("is_over but inconsistent"), "{err}");
}

#[test]
fn is_over_won_with_all_enemies_dead_passes() {
    let mut s = valid_state();
    s.creatures[0].current_hp = 0;
    s.is_over = true;
    s.player_won = true;
    check_combat_invariants(&s, "t").expect("won with every enemy dead is consistent");
}

/// A dead player ends combat regardless of the enemies' state.
#[test]
fn is_over_with_dead_player_passes() {
    let mut s = valid_state();
    s.player.current_hp = 0;
    s.is_over = true;
    check_combat_invariants(&s, "t").expect("dead player is a consistent termination");
}

/// `player_won` with a survivor is the inconsistency the check exists for.
#[test]
fn is_over_won_with_a_live_enemy_fails() {
    let mut s = valid_state();
    s.is_over = true;
    s.player_won = true;
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("all_enemies_dead=false"), "{err}");
}

// ── check_combat_invariants: player/creature mirror ────────────────────

/// Attach a player mirror Creature that agrees with the player fields.
fn with_player_mirror(s: &mut CombatState) {
    s.creatures.push(Creature {
        id: nz(2),
        current_hp: s.player.current_hp,
        max_hp: s.player.max_hp,
        block: s.player.block,
        powers: SmallVec::new(),
        species: None,
        monster_id: MonsterId("player"),
        intent: None,
        intent_history: IntentHistory::default(),
        ai: None,
    });
    s.player.creature_uid = Some(cuid(2));
}

#[test]
fn consistent_player_mirror_passes() {
    let mut s = valid_state();
    with_player_mirror(&mut s);
    check_combat_invariants(&s, "t").expect("agreeing mirror is valid");
}

#[test]
fn player_mirror_hp_desync_fails() {
    let mut s = valid_state();
    with_player_mirror(&mut s);
    s.creatures[1].current_hp -= 5;
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("player/creature HP mirror desync"), "{err}");
}

#[test]
fn player_mirror_max_hp_desync_fails() {
    let mut s = valid_state();
    with_player_mirror(&mut s);
    s.creatures[1].max_hp += 5;
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(
        err.contains("player/creature max_hp mirror desync"),
        "{err}"
    );
}

#[test]
fn player_mirror_block_desync_fails() {
    let mut s = valid_state();
    with_player_mirror(&mut s);
    s.player.block = 7;
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("player/creature block mirror desync"), "{err}");
}

// ── check_combat_invariants: power dedup + non-negativity ──────────────

#[test]
fn duplicate_power_kind_on_player_fails() {
    let mut s = valid_state();
    s.powers = vec![
        Power::new(nz(1), PowerKind::Strength, 2),
        Power::new(nz(2), PowerKind::Strength, 3),
    ]
    .into();
    s.player.powers.push(puid(1));
    s.player.powers.push(puid(2));
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(
        err.contains("duplicate PowerKind Strength on player"),
        "{err}"
    );
}

#[test]
fn duplicate_power_kind_on_creature_fails() {
    let mut s = valid_state();
    s.powers = vec![
        Power::new(nz(1), PowerKind::Vulnerable, 2),
        Power::new(nz(2), PowerKind::Vulnerable, 1),
    ]
    .into();
    s.creatures[0].powers.push(puid(1));
    s.creatures[0].powers.push(puid(2));
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(
        err.contains("duplicate PowerKind Vulnerable on creature 1"),
        "{err}"
    );
}

/// Two DIFFERENT kinds on one owner is the normal case.
#[test]
fn distinct_power_kinds_on_one_owner_pass() {
    let mut s = valid_state();
    s.powers = vec![
        Power::new(nz(1), PowerKind::Strength, 2),
        Power::new(nz(2), PowerKind::Dexterity, 1),
    ]
    .into();
    s.player.powers.push(puid(1));
    s.player.powers.push(puid(2));
    check_combat_invariants(&s, "t").expect("distinct kinds are fine");
}

#[test]
fn negative_vulnerable_on_player_fails() {
    let mut s = valid_state();
    s.powers = vec![Power::new(nz(1), PowerKind::Vulnerable, -1)].into();
    s.player.powers.push(puid(1));
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("player has negative Vulnerable"), "{err}");
}

#[test]
fn negative_poison_on_creature_fails() {
    let mut s = valid_state();
    s.powers = vec![Power::new(nz(1), PowerKind::Poison, -4)].into();
    s.creatures[0].powers.push(puid(1));
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(err.contains("creature 1 has negative Poison"), "{err}");
}

/// Strength / Dexterity are deliberately OUT of `NON_NEGATIVE_POWERS`:
/// a Strength debuff below zero is legal game state.
#[test]
fn negative_strength_is_allowed() {
    let mut s = valid_state();
    s.powers = vec![Power::new(nz(1), PowerKind::Strength, -3)].into();
    s.player.powers.push(puid(1));
    check_combat_invariants(&s, "t").expect("negative Strength is legal");
}

/// A dangling PowerUid resolves to nothing and is simply skipped — the
/// non-negativity sweep must not treat it as a violation.
#[test]
fn dangling_power_uid_is_skipped_by_the_non_negative_sweep() {
    let mut s = valid_state();
    s.player.powers.push(puid(9));
    check_combat_invariants(&s, "t").expect("unresolvable power uid is not a bound violation");
}

// ── check_combat_invariants: intent presence ───────────────────────────

#[test]
fn alive_enemy_losing_its_intent_mid_enemy_turn_fails() {
    let mut s = valid_state();
    s.phase = CombatPhase::EnemyTurnExec;
    s.creatures[0].intent = None;
    s.creatures[0].intent_history.turn_index = 1;
    let err = check_combat_invariants(&s, "t").unwrap_err();
    assert!(
        err.contains("lost its intent during EnemyTurnExec"),
        "{err}"
    );
}

/// A spawn that has never rolled (`turn_index == 0`) is exempt — designed
/// inert spawns start with `intent: None`.
#[test]
fn enemy_that_never_rolled_is_exempt_from_the_intent_check() {
    let mut s = valid_state();
    s.phase = CombatPhase::EnemyTurnExec;
    s.creatures[0].intent = None;
    s.creatures[0].intent_history.turn_index = 0;
    check_combat_invariants(&s, "t").expect("un-rolled spawn may have no intent");
}

#[test]
fn dead_enemy_is_exempt_from_the_intent_check() {
    let mut s = valid_state();
    s.phase = CombatPhase::EnemyTurnExec;
    s.creatures[0].current_hp = 0;
    s.creatures[0].intent = None;
    s.creatures[0].intent_history.turn_index = 3;
    check_combat_invariants(&s, "t").expect("dead enemies keep no intent");
}

/// The check is narrowed to `EnemyTurnExec`; an intent-less enemy outside
/// that phase is normal (intents are rolled at enemy-turn start).
#[test]
fn intent_check_only_applies_during_enemy_turn_exec() {
    let mut s = valid_state();
    s.phase = CombatPhase::PlayerPlayPhase;
    s.creatures[0].intent = None;
    s.creatures[0].intent_history.turn_index = 3;
    check_combat_invariants(&s, "t").expect("intent absence outside EnemyTurnExec is fine");
}

// ── check_post_step: energy deltas ─────────────────────────────────────

fn play_pre(energy: i8, cost: i8, x_cost: bool, weighted: i8) -> PreStep {
    PreStep {
        energy,
        is_over: false,
        played: Some((CardId::StrikeIronclad, cost, x_cost, weighted)),
    }
}

#[test]
fn energy_delta_matching_cost_passes() {
    let mut s = valid_state();
    s.player.energy = 2;
    check_post_step(&play_pre(3, 1, false, 0), &s, "t").expect("3 - 1 == 2");
}

#[test]
fn energy_delta_mismatch_fails() {
    let mut s = valid_state();
    s.player.energy = 0;
    let err = check_post_step(&play_pre(3, 1, false, 0), &s, "t").unwrap_err();
    assert!(err.contains("energy delta mismatch"), "{err}");
}

/// A play rejected mid-resolve leaves energy untouched — an accepted arm,
/// not a violation.
#[test]
fn unchanged_energy_reads_as_a_rejected_play() {
    let mut s = valid_state();
    s.player.energy = 3;
    check_post_step(&play_pre(3, 1, false, 0), &s, "t").expect("rejected play keeps energy");
}

/// Weighted's extra drain is part of the legitimate spend, and the total
/// floors at 0 (C# `PlayerCombatState.LoseEnergy` clamps to `[0, …]`).
#[test]
fn weighted_drain_is_expected_and_floors_at_zero() {
    let mut s = valid_state();
    s.player.energy = 0;
    check_post_step(&play_pre(1, 1, false, 3), &s, "t").expect("over-drain clamps at 0");

    // The drain is EXPECTED, so ignoring it is the violation: 3 - 1 - 1 == 1,
    // and an engine that only charged the printed cost would leave 2.
    s.player.energy = 2;
    let err = check_post_step(&play_pre(3, 1, false, 1), &s, "t").unwrap_err();
    assert!(err.contains("weighted=1"), "{err}");
    assert!(err.contains("expected=1"), "{err}");
}

#[test]
fn x_cost_card_must_drain_all_energy() {
    let mut s = valid_state();
    s.player.energy = 2;
    let err = check_post_step(&play_pre(3, 0, true, 0), &s, "t").unwrap_err();
    assert!(err.contains("should be 0"), "{err}");

    s.player.energy = 0;
    check_post_step(&play_pre(3, 0, true, 0), &s, "t").expect("X-cost spent everything");
}

/// Mid-resolve states (an open `pending_choice`, or any phase other than
/// `PlayerPlayPhase`) are skipped: the spend has not landed yet.
#[test]
fn energy_checks_are_skipped_mid_resolve() {
    let mut s = valid_state();
    s.player.energy = 0;
    s.phase = CombatPhase::EnemyTurnExec;
    check_post_step(&play_pre(3, 1, false, 0), &s, "t").expect("phase gate skips the check");

    let mut x = valid_state();
    x.player.energy = 5;
    x.phase = CombatPhase::EnemyTurnExec;
    check_post_step(&play_pre(5, 0, true, 0), &x, "t").expect("phase gate skips the X-cost check");
}

/// `is_over` staying true across a step is fine; only the revert is not.
#[test]
fn is_over_staying_true_passes() {
    let mut s = valid_state();
    s.player.current_hp = 0;
    s.is_over = true;
    let pre = PreStep {
        energy: 0,
        is_over: true,
        played: None,
    };
    check_post_step(&pre, &s, "t").expect("is_over may stay true");
}

// ── check_turn_boundary ────────────────────────────────────────────────

#[test]
fn per_turn_counters_must_all_reset() {
    let cases: Vec<(&str, fn(&mut CombatState))> = vec![
        ("shivs_played_this_turn", |s| {
            s.player.shivs_played_this_turn = 1;
        }),
        ("powered_hits_this_turn", |s| {
            s.player.powered_hits_this_turn = 2;
        }),
        ("skills_played_this_turn", |s| {
            s.player.skills_played_this_turn = 3;
        }),
        ("stars_gained_this_turn", |s| {
            s.player.stars_gained_this_turn = 4;
        }),
        ("pending_retain_count", |s| {
            s.player.pending_retain_count = 1;
        }),
        ("request_end_turn_after_play", |s| {
            s.player.request_end_turn_after_play = true;
        }),
    ];
    for (label, mutate) in cases {
        let mut s = valid_state();
        mutate(&mut s);
        let anchor = snapshot_turn_anchor(&s);
        let err = check_turn_boundary(&anchor, &s, "t").unwrap_err();
        assert!(
            err.contains(label),
            "expected {label} to be reported, got {err}"
        );
    }
}

#[test]
fn leftover_block_at_turn_boundary_fails() {
    let mut s = valid_state();
    s.player.block = 4;
    let anchor = snapshot_turn_anchor(&s);
    let err = check_turn_boundary(&anchor, &s, "t").unwrap_err();
    assert!(err.contains("player block 4 not reset"), "{err}");
}

/// SturdyClamp legitimately carries block across the boundary, so the block
/// check is skipped wholesale rather than allowlisting an amount.
#[test]
fn block_retention_relic_exempts_the_block_reset() {
    let mut s = valid_state();
    s.player.block = 10;
    s.relics.push(Relic::new(nz(1), RelicKind::SturdyClamp));
    assert!(has_block_retention(&s));
    let anchor = snapshot_turn_anchor(&s);
    check_turn_boundary(&anchor, &s, "t").expect("SturdyClamp retains block");
}

/// A melted relic is out of every hook dispatch, so it cannot be retaining
/// block either — the exemption must not apply.
#[test]
fn melted_retention_relic_does_not_exempt() {
    let mut s = valid_state();
    s.player.block = 10;
    let mut relic = Relic::new(nz(1), RelicKind::SturdyClamp);
    relic.is_melted = true;
    s.relics.push(relic);
    assert!(!has_block_retention(&s));
    let anchor = snapshot_turn_anchor(&s);
    let err = check_turn_boundary(&anchor, &s, "t").unwrap_err();
    assert!(err.contains("no retention relic"), "{err}");
}

/// An unrelated relic must not be mistaken for a retention relic.
#[test]
fn unrelated_relic_does_not_exempt_the_block_reset() {
    let mut s = valid_state();
    s.relics.push(Relic::new(nz(1), RelicKind::BurningBlood));
    assert!(!has_block_retention(&s));
    let anchor = snapshot_turn_anchor(&s);
    check_turn_boundary(&anchor, &s, "t").expect("zero block still passes");
}

#[test]
fn undrained_play_pile_at_turn_boundary_fails() {
    let mut s = valid_state();
    s.cards
        .push(make_card(1, CardId::StrikeIronclad, 1, CardType::Attack));
    s.player.play.push(uid(1));
    let anchor = snapshot_turn_anchor(&s);
    let err = check_turn_boundary(&anchor, &s, "t").unwrap_err();
    assert!(err.contains("play not drained"), "{err}");
    assert!(err.contains("1 cards left"), "{err}");
}
