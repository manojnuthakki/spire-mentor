//! Full-path regression test for `SkittishPower` (PhantasmalGardener).
//!
//! C# `../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.Models.Powers/SkittishPower.cs:56-69`
//! (`AfterAttack`): ONCE PER TURN, when a card-sourced (`ModelSource is
//! CardModel`) `ValueProp.Move` attack lands UNBLOCKED damage on the owner, the
//! owner gains `Amount` block (`ValueProp.Unpowered`). `:71-82`
//! (`AfterSideTurnEnd`) clears `hasGainedBlockThisTurn` on the non-owner side's
//! turn end — for a monster owner, the player's turn end.
//!
//! `PhantasmalGardener.cs:92` spawns the gardener with `SkittishPower(6)`. The
//! engine recorded that power (`sts2-core::monsters::on_spawn_self_buffs`) but
//! had NO listener body, so a live divergence corpus showed the targeted
//! gardener at world block 6 / mirror block 0 in all 6 records — search
//! over-valued every attack into a gardener by 6.

use smallvec::SmallVec;
use std::num::NonZeroU32;

use sts2_content::cards::dispatch as content_dispatch;
use sts2_core::cards::{Card, CardId, CardType, TargetType};
use sts2_core::hooks::{dispatch_after_turn_end, TurnSideCtx};
use sts2_core::{
    CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, Power,
    PowerKind, PowerUid, Side,
};

const SKITTISH_PID: u32 = 50;

fn push_card(state: &mut CombatState, id: CardId, dmg: i32) -> sts2_core::CardUid {
    let raw = u32::try_from(state.cards.len() + 1).expect("u32");
    let uid = sts2_core::CardUid::new(raw).expect("nz");
    state.cards.push(Card {
        uid,
        card_id: id,
        cost: 0,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy,
        upgraded: false,
        base_damage: dmg,
        base_block: 0,
        ..Card::pod_defaults()
    });
    uid
}

/// A gardener with `Skittish(6)`, 200 HP, and whatever starting block the
/// caller asks for.
fn gardener_state(enemy_block: i32) -> CombatState {
    content_dispatch::ensure_installed();
    // The latch hook installs when a PhantasmalGardener AI is built; this test
    // mounts the power directly, so install it explicitly (same shape as the
    // CurlUp regression test).
    sts2_content::monsters::act1_underdocks::install_skittish_handlers();

    let mut state = CombatState::default();
    let eid = NonZeroU32::new(1).expect("nz");
    let pid = NonZeroU32::new(SKITTISH_PID).expect("nz");
    state.powers.push(Power {
        id: pid,
        kind: PowerKind::Skittish,
        amount: 6,
        counter: 0,
        applier: None,
        skip_next_tick: false,
    });
    let mut epowers: SmallVec<[PowerUid; 8]> = SmallVec::new();
    epowers.push(PowerUid(pid));
    state.creatures.push(Creature {
        id: eid,
        current_hp: 200,
        max_hp: 200,
        block: enemy_block,
        powers: epowers,
        species: Some(MonsterKind::PhantasmalGardener),
        intent: None,
        intent_history: IntentHistory::default(),
        monster_id: MonsterId("PhantasmalGardener"),
        ai: None,
    });
    state.enemy_uids.push(CreatureUid(eid));

    state.player.current_hp = 80;
    state.player.max_hp = 80;
    state.player.energy = 99;
    state.player.max_energy = 99;
    let _ = state.init_player_creature();
    state.phase = CombatPhase::PlayerPlayPhase;
    state.side = Side::Player;
    state
}

fn skittish_counter(state: &CombatState) -> i32 {
    state
        .powers
        .iter()
        .find(|p| p.kind == PowerKind::Skittish)
        .map_or(-1, |p| p.counter)
}

/// Play the card at `hand_idx` into enemy 0.
fn play(state: &mut CombatState, hand_idx: u8) {
    let step = sts2_core::combat_loop::play_card(state, hand_idx, Some(0));
    assert_ne!(
        step,
        sts2_core::StepResult::PendingChoice,
        "fixture cards must resolve without an interactive choice"
    );
}

/// The core contract: a card attack that lands unblocked damage grants the
/// gardener `Amount` block, ONCE per turn, and the block lands only AFTER the
/// whole card resolves (C# `AfterAttack` fires once per `AttackCommand`, so a
/// multi-hit card's own later hits are never absorbed by the fresh block).
#[test]
fn skittish_grants_block_once_per_turn_after_the_card_resolves() {
    let mut state = gardener_state(0);

    // RIP_AND_TEAR hits the SAME target twice for `base_damage` each. If the
    // block were granted on the first hit (the shape the CurlUp port originally
    // had), the second hit would be absorbed and HP would drop by only 7.
    let rip = push_card(&mut state, CardId::RipAndTear, 7);
    state.player.hand.push(rip);
    play(&mut state, 0);

    assert_eq!(
        state.creatures[0].current_hp,
        200 - 14,
        "both hits of the 2-hit card land on HP — the block arrives after the card"
    );
    assert_eq!(
        state.creatures[0].block, 6,
        "Skittish grants SkittishAmount=6 block (PhantasmalGardener.cs:92)"
    );
    assert_eq!(
        skittish_counter(&state),
        1,
        "hasGainedBlockThisTurn is set (SkittishPower.cs:63)"
    );

    // Second unblocked attack THIS turn: `!HasGainedBlockThisTurn` fails, so no
    // second grant. 20 damage vs 6 block → 14 to HP, block spent to 0.
    let big = push_card(&mut state, CardId::StrikeIronclad, 20);
    state.player.hand.push(big);
    play(&mut state, 0);
    assert_eq!(
        state.creatures[0].block, 0,
        "once per turn — the second unblocked hit grants nothing"
    );
    assert_eq!(state.creatures[0].current_hp, 200 - 14 - 14);

    // Player turn end resets the flag (C# `AfterSideTurnEnd`, side != owner's).
    dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(skittish_counter(&state), 0, "the per-turn flag is cleared");

    // Next turn it re-arms — Skittish is NOT consumed like CurlUp.
    let again = push_card(&mut state, CardId::StrikeIronclad, 20);
    state.player.hand.push(again);
    play(&mut state, 0);
    assert_eq!(
        state.creatures[0].block, 6,
        "Skittish re-arms every turn (the power is never removed)"
    );
}

/// `damageResult.UnblockedDamage != 0` (SkittishPower.cs:61): a FULLY BLOCKED
/// card attack does not arm Skittish at all. This is the gate that differs from
/// `CurlUpPower`, whose C# body ignores the `DamageResult` entirely.
#[test]
fn skittish_ignores_a_fully_blocked_attack() {
    let mut state = gardener_state(30);

    let strike = push_card(&mut state, CardId::StrikeIronclad, 6);
    state.player.hand.push(strike);
    play(&mut state, 0);

    assert_eq!(
        state.creatures[0].current_hp, 200,
        "the hit was fully absorbed"
    );
    assert_eq!(
        state.creatures[0].block, 24,
        "block only went down by the 6 it absorbed — no Skittish grant on top"
    );
    assert_eq!(
        skittish_counter(&state),
        0,
        "hasGainedBlockThisTurn stays false for a fully-blocked hit"
    );
}

/// A card whose attack is followed by a selection in the same play (Headbutt:
/// `DamageCmd.Attack(...).Execute`, then the discard pick) pauses with the
/// attack command complete, so C#'s `AfterAttack` has already granted the
/// block at the pick (A6d4: V7W8Q5R79A at A5, step 76, Headless 6 where the
/// engine showed 0 until the pick resolved).
#[test]
fn skittish_block_shows_at_a_mid_play_selection_after_the_attack() {
    let mut state = gardener_state(0);
    let headbutt = push_card(&mut state, CardId::Headbutt, 9);
    let strike = push_card(&mut state, CardId::StrikeIronclad, 6);
    let defend = push_card(&mut state, CardId::DefendIronclad, 0);
    state.player.discard.push(strike);
    state.player.discard.push(defend);
    state.player.hand.push(headbutt);
    let step = sts2_core::combat_loop::play_card(&mut state, 0, Some(0));
    assert_eq!(
        step,
        sts2_core::StepResult::PendingChoice,
        "Headbutt asks for a discard card"
    );
    assert_eq!(state.creatures[0].current_hp, 200 - 9);
    assert_eq!(
        state.creatures[0].block, 6,
        "the attack command finished before the pick: Skittish's block is there"
    );
    assert_eq!(skittish_counter(&state), 1);
    let _ = sts2_core::apply(
        &mut state,
        sts2_core::Action::ResolveChoice {
            selection: smallvec::smallvec![0],
        },
    );
    assert_eq!(
        state.creatures[0].block, 6,
        "granted once, not again at the play's end"
    );
}
