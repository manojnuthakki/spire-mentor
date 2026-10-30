//! Engine invariant checks, shared between the combat libfuzzer target
//! (`fuzz/fuzz_targets/combat_step.rs`) and the full-run fuzzer
//! (`crate::full_run`).
//!
//! Bead `sts2-rs` (AutoSlay-inspired full-run fuzzer, B1): the 21-point
//! invariant suite was originally inlined in the `combat_step` fuzz
//! target as `panic!`/`assert!` checks. The full-run driver wants to run
//! the *same* checks after every step of a deep multi-act run, but it
//! lives in the main workspace (the `fuzz/` crate is an excluded
//! nightly-only sibling) and must **record** a failing seed rather than
//! abort the whole sweep. So the checks are lifted here as
//! `Result<(), String>`-returning functions:
//!
//! * [`check_combat_invariants`] — per-step structural checks (HP/block/
//!   energy bounds, pile conservation, hand cap, mirror consistency,
//!   termination consistency, power dedup/non-negativity, unplayable-card
//!   filtering, intent presence).
//! * [`check_post_step`] — before/after delta checks (`is_over`
//!   monotonicity, energy-spent-equals-cost, X-cost drains energy).
//! * [`check_turn_boundary`] — cross-turn checks fired on a player-turn
//!   boundary (counter resets, block reset, play-pile drain, echo-form
//!   decrement, no stale `pending_play`).
//!
//! The libfuzzer target keeps its `panic!` semantics by calling these and
//! `.unwrap()`-ing; the full-run sweep propagates the `Err` string into a
//! recorded failing-seed fixture. Keeping a single implementation here
//! means the two harnesses can never drift on what "valid engine state"
//! means.

use crate::card_effects::card_unplayable;
use crate::cards::CardId;
use crate::{legal_actions, Action, CardUid, CombatPhase, CombatState, PowerKind, RelicKind};

/// STS hand cap (10 cards).
pub const HAND_CAP: usize = 10;

/// PowerKinds whose `amount` field must never be negative. Strength and
/// Dexterity are deliberately omitted — they legitimately support
/// negative stacks (e.g. a Strength debuff below zero).
pub const NON_NEGATIVE_POWERS: &[PowerKind] = &[
    PowerKind::Vulnerable,
    PowerKind::Weak,
    PowerKind::Frail,
    PowerKind::Poison,
    PowerKind::Intangible,
];

/// Relics that legitimately retain (some) player block across turn
/// boundaries. If any is attached, the turn-boundary `block == 0` check
/// is skipped rather than allowlisting a per-relic amount.
pub const BLOCK_RETENTION_RELICS: &[RelicKind] = &[
    // SturdyClamp retains up to 10 block across the turn boundary
    // (`sts2-rs-d7w6` in relic_effects.rs). Add additional retention
    // relics here as they land.
    RelicKind::SturdyClamp,
];

#[must_use]
pub fn has_block_retention(state: &CombatState) -> bool {
    BLOCK_RETENTION_RELICS
        .iter()
        .any(|&kind| state.relics.owns(kind))
}

/// Reduce the assert-heavy port to mechanical `Err`-returns.
macro_rules! ensure {
    ($cond:expr, $($arg:tt)*) => {
        if !($cond) {
            return Err(format!($($arg)*));
        }
    };
}

/// Snapshot taken BEFORE each `apply()` call, used to validate delta
/// invariants (e.g. energy delta == card cost).
#[derive(Debug)]
pub struct PreStep {
    pub energy: i8,
    pub is_over: bool,
    /// If the action being applied is `PlayCard`, the
    /// `(id, cost, x_cost, weighted_drain)` tuple of the referenced card
    /// (looked up via `hand_index` BEFORE `apply` because the play removes
    /// the card from hand).
    ///
    /// `cost` is the **modifier-resolved** cost
    /// ([`crate::hooks::effective_card_cost`]) — the same number
    /// `combat_loop::play_card` subtracts — not the raw `card.cost`, which
    /// ignores Corruption / `Free*` / `CostAbsolutelySet` and would make the
    /// energy-delta check fire on correct engine behaviour.
    /// `weighted_drain` is the EXTRA energy the `Weighted` affliction drains
    /// on play (C# `Afflictions/Weighted.cs:8-11`, bead `sts2-rs-l65km`),
    /// which is likewise part of the legitimate spend.
    pub played: Option<(CardId, i8, bool, i8)>,
}

#[must_use]
pub fn snapshot_pre(state: &CombatState, action: &Action) -> PreStep {
    let played = if let Action::PlayCard { hand_index, .. } = action {
        state
            .player
            .hand
            .get(*hand_index as usize)
            .and_then(|uid| state.cards.iter().find(|c| c.uid == *uid))
            .map(|c| {
                let cost = i8::try_from(
                    crate::hooks::effective_card_cost(state, c.uid).clamp(0, i32::from(i8::MAX)),
                )
                .expect("clamp keeps value in i8 range");
                let weighted = match c.affliction {
                    Some(crate::card_afflictions::CardAffliction {
                        kind: crate::card_afflictions::CardAfflictionKind::Weighted,
                        stacks,
                        ..
                    }) => i8::try_from(stacks.clamp(0, i32::from(i8::MAX)))
                        .expect("clamp keeps value in i8 range"),
                    _ => 0,
                };
                (c.card_id, cost, c.is_x_cost(), weighted)
            })
    } else {
        None
    };
    PreStep {
        energy: state.player.energy,
        is_over: state.is_over,
        played,
    }
}

/// Snapshot taken at the START of each player turn. Used by
/// [`check_turn_boundary`] to validate cross-turn invariants on the NEXT
/// player-turn boundary.
#[derive(Clone, Debug)]
pub struct TurnAnchor {
    pub round: u16,
}

#[must_use]
pub fn snapshot_turn_anchor(state: &CombatState) -> TurnAnchor {
    TurnAnchor { round: state.round }
}

/// Per-step structural invariants (run after every `apply()`).
///
/// Returns `Err(msg)` describing the first violated invariant; `ctx` is a
/// short label (e.g. `"post-step"`) folded into the message.
///
/// # Errors
/// Returns `Err` if any structural invariant is violated.
pub fn check_combat_invariants(state: &CombatState, ctx: &str) -> Result<(), String> {
    // 1. Player HP / block / energy bounds.
    let php = state.player.current_hp;
    let pmax = state.player.max_hp;
    ensure!(
        php >= 0 && php <= pmax,
        "player HP out of range: hp={php} max={pmax} ({ctx})"
    );
    ensure!(state.player.block >= 0, "player block negative ({ctx})");
    ensure!(state.player.energy >= 0, "player energy negative ({ctx})");

    // 1a. `resolving_card` is a DISPATCH-SCOPED transient (GH #422):
    // `card_effects::dispatch_for_card` stamps it on the way in and restores
    // the previous value on the way out, so it must be `None` at every point
    // an observer can see the state. A leak here would mean some dispatch
    // path returned without restoring — and would silently pin later
    // "card currently being played" reads to a stale card.
    ensure!(
        state.resolving_card.is_none(),
        "resolving_card leaked out of a card dispatch: {:?} ({ctx})",
        state.resolving_card
    );

    // 1b. Creature HP / block bounds.
    for c in &state.creatures {
        ensure!(
            c.current_hp >= 0 && c.current_hp <= c.max_hp,
            "creature {} HP out of range: hp={} max={} ({ctx})",
            c.id,
            c.current_hp,
            c.max_hp
        );
        ensure!(c.block >= 0, "creature {} block negative ({ctx})", c.id);
    }

    // 1b'. Creature ids and card uids are unique. The dense id lookups
    // (`crate::creature_by_id`, `crate::card_by_uid`, J07) return the same
    // element as a first-match scan only under that invariant.
    for (i, c) in state.creatures.iter().enumerate() {
        ensure!(
            !state.creatures[..i].iter().any(|d| d.id == c.id),
            "duplicate creature id {} ({ctx})",
            c.id
        );
    }
    for (i, c) in state.cards.iter().enumerate() {
        ensure!(
            !state.cards[..i].iter().any(|d| d.uid == c.uid),
            "duplicate card uid {:?} ({ctx})",
            c.uid
        );
    }

    // 1c. PowerStore O(1) index ⇔ arena agreement (D1). The `uid -> slot`
    // side index must resolve every live power to its own slot (and so also
    // catches duplicate ids, which would alias the index). Cheap O(P) sweep,
    // only ever run inside this debug/fuzz invariant path.
    for (slot, p) in state.powers.iter().enumerate() {
        let resolved = state.powers.get_by_id(p.id).map(core::ptr::from_ref);
        ensure!(
            resolved == Some(core::ptr::from_ref(&state.powers[slot])),
            "PowerStore index/arena disagree for power id {} at slot {slot} ({ctx})",
            p.id
        );
    }

    // 3 + 4. Deck partition conservation + arena consistency.
    // BTreeMap (not HashMap) per the workspace determinism lint; CardUid
    // is a NonZeroU32 newtype with a total order, so key iteration is
    // deterministic.
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<CardUid, u32> = BTreeMap::new();
    let mut count = |uids: &[CardUid]| {
        for u in uids {
            *counts.entry(*u).or_insert(0) += 1;
        }
    };
    // The PILES partition the deck: every card is in exactly one of them.
    count(&state.player.hand);
    count(&state.player.draw);
    count(&state.player.discard);
    count(&state.player.exhaust);
    count(&state.player.play);
    for (uid, n) in &counts {
        ensure!(
            *n == 1,
            "card {uid:?} appears {n} times across piles ({ctx})"
        );
        ensure!(
            state.cards.iter().any(|c| c.uid == *uid),
            "card {uid:?} in pile but not in state.cards arena ({ctx})"
        );
    }

    // `pinned_top` / `pinned_bottom` / `retain_this_turn` are ANNOTATIONS on
    // cards that already live in a pile, not piles of their own — a pinned
    // card is in `draw` AND in `pinned_top` by construction. Counting them
    // into the partition tally above made any pinned state fail with "card
    // appears 2 times across piles". That was unreachable while pins only
    // ever existed inside a live combat; `sts2-py`'s wire round-trip made
    // reconstructed states carry them, so it became reachable.
    //
    // They still need the arena check, and they must reference a card that
    // is actually in the pile the annotation is about.
    let annotation = |uids: &[CardUid], pile: &[CardUid], what: &str| -> Result<(), String> {
        for u in uids {
            ensure!(
                state.cards.iter().any(|c| c.uid == *u),
                "card {u:?} in {what} but not in state.cards arena ({ctx})"
            );
            ensure!(
                pile.contains(u),
                "card {u:?} in {what} but not in the pile it annotates ({ctx})"
            );
        }
        Ok(())
    };
    annotation(&state.player.pinned_top, &state.player.draw, "pinned_top")?;
    annotation(
        &state.player.pinned_bottom,
        &state.player.draw,
        "pinned_bottom",
    )?;
    // `retain_this_turn` lasts the turn wherever the card goes (C#
    // `ShouldRetainThisTurn` until `EndOfTurnCleanup`; the live engine keeps a
    // played card's entry until the end-of-turn clear, and the retain flush
    // reads only the hand), so it annotates a card in any pile (sts2-agent fork).
    for u in &state.player.retain_this_turn {
        ensure!(
            state.cards.iter().any(|c| c.uid == *u),
            "card {u:?} in retain_this_turn but not in state.cards arena ({ctx})"
        );
        ensure!(
            [
                &state.player.hand[..],
                &state.player.draw[..],
                &state.player.discard[..],
                &state.player.exhaust[..],
                &state.player.play[..],
            ]
            .iter()
            .any(|pile| pile.contains(u)),
            "card {u:?} in retain_this_turn but in no pile ({ctx})"
        );
    }

    // 5. Hand cap.
    ensure!(
        state.player.hand.len() <= HAND_CAP,
        "hand size {} exceeds cap {HAND_CAP} ({ctx})",
        state.player.hand.len()
    );

    // 6. Enemy-uid <-> creature mirror.
    for uid in &state.enemy_uids {
        ensure!(
            state.creatures.iter().any(|c| c.id == uid.0),
            "enemy uid {uid:?} has no corresponding creature ({ctx})"
        );
    }

    // 7. Combat termination consistency.
    if state.is_over {
        let player_dead = php == 0;
        let all_enemies_dead = state
            .enemy_uids
            .iter()
            .filter_map(|u| state.creatures.iter().find(|c| c.id == u.0))
            .all(|c| c.current_hp == 0);
        ensure!(
            (state.player_won && all_enemies_dead) || player_dead,
            "is_over but inconsistent: player_won={} player_hp={php} all_enemies_dead={all_enemies_dead} ({ctx})",
            state.player_won
        );
    }

    // 8. Player creature mirror (d03 unification).
    if let Some(uid) = state.player.creature_uid {
        let mirror = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .ok_or_else(|| {
                format!("player creature_uid {uid:?} does not resolve to a creature ({ctx})")
            })?;
        ensure!(
            mirror.current_hp == php,
            "player/creature HP mirror desync: creature={} player={php} ({ctx})",
            mirror.current_hp
        );
        ensure!(
            mirror.max_hp == pmax,
            "player/creature max_hp mirror desync: creature={} player={pmax} ({ctx})",
            mirror.max_hp
        );
        ensure!(
            mirror.block == state.player.block,
            "player/creature block mirror desync: creature={} player={} ({ctx})",
            mirror.block,
            state.player.block
        );
    }

    // 9. No duplicate PowerKind per owner (player + each creature).
    let resolve = |uids: &[crate::PowerUid]| -> Vec<PowerKind> {
        uids.iter()
            .filter_map(|u| state.powers.get_by_id(u.0).map(|p| p.kind))
            .collect()
    };
    let player_kinds = resolve(&state.player.powers);
    for (i, k) in player_kinds.iter().enumerate() {
        for k2 in &player_kinds[i + 1..] {
            ensure!(k != k2, "duplicate PowerKind {k:?} on player ({ctx})");
        }
    }
    for c in &state.creatures {
        let kinds = resolve(&c.powers);
        for (i, k) in kinds.iter().enumerate() {
            for k2 in &kinds[i + 1..] {
                ensure!(
                    k != k2,
                    "duplicate PowerKind {k:?} on creature {} ({ctx})",
                    c.id
                );
            }
        }
    }

    // 10. Non-negative bound for NON_NEGATIVE_POWERS.
    let check_non_neg = |uids: &[crate::PowerUid], owner: &str| -> Result<(), String> {
        for u in uids {
            if let Some(p) = state.powers.get_by_id(u.0) {
                if NON_NEGATIVE_POWERS.contains(&p.kind) {
                    ensure!(
                        p.amount >= 0,
                        "{owner} has negative {kind:?}: amount={a} ({ctx})",
                        kind = p.kind,
                        a = p.amount
                    );
                }
            }
        }
        Ok(())
    };
    check_non_neg(&state.player.powers, "player")?;
    for c in &state.creatures {
        check_non_neg(&c.powers, &format!("creature {}", c.id))?;
    }

    // 11. Unplayable status cards must NOT appear in legal_actions as
    //     PlayCard targets.
    let legal = legal_actions(state);
    for a in &legal {
        if let Action::PlayCard { hand_index, .. } = a {
            if let Some(uid) = state.player.hand.get(*hand_index as usize) {
                if let Some(card) = state.cards.iter().find(|c| c.uid == *uid) {
                    ensure!(
                        !card_unplayable(card.card_id),
                        "unplayable card {:?} surfaced in legal_actions ({ctx})",
                        card.card_id
                    );
                }
            }
        }
    }

    // 12. Intent should not vanish after a creature has rolled at least
    //     one (narrowed to EnemyTurnExec + turn_index > 0 to exempt
    //     designed-inert spawns whose intent starts None).
    if matches!(state.phase, CombatPhase::EnemyTurnExec) {
        for uid in &state.enemy_uids {
            if let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) {
                if c.current_hp > 0 && c.intent_history.turn_index > 0 && c.intent.is_none() {
                    return Err(format!(
                        "alive enemy {} ({:?}, turn_index={}) lost its intent during EnemyTurnExec ({ctx})",
                        c.id, c.species, c.intent_history.turn_index
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Per-step delta invariants (before/after one `apply()`).
///
/// # Errors
/// Returns `Err` if `is_over` reverts, or the energy spent on a PlayCard
/// does not match the card's cost (or an X-cost card leaves energy).
pub fn check_post_step(pre: &PreStep, state: &CombatState, ctx: &str) -> Result<(), String> {
    // 13. is_over is monotonic.
    if pre.is_over {
        ensure!(state.is_over, "is_over reverted from true to false ({ctx})");
    }

    // 14 / 15. Energy spent matches played card's cost.
    if let Some((card_id, cost, x_cost, weighted)) = pre.played {
        if !x_cost
            && cost >= 0
            && state.pending_choice.is_none()
            && matches!(state.phase, CombatPhase::PlayerPlayPhase)
        {
            // The Weighted drain clamps at 0 rather than going negative
            // (`PlayerCmd.LoseEnergy`), so the floor is part of the contract.
            let expected = (pre.energy - cost - weighted).max(0);
            let actual = state.player.energy;
            // Cards whose own OnPlay gains energy (`PlayerCmd.GainEnergy`)
            // legitimately end above `expected`. The gain amount varies
            // (upgrades +1, ExpectAFight scales with attack count, and
            // NoEnergyGain can suppress it to exactly `expected`), so for
            // these we only keep the no-overspend half of the check.
            const ON_PLAY_ENERGY_GAIN_CARDS: &[CardId] = &[
                CardId::Bloodletting,
                CardId::Offering,
                CardId::ForgottenRitual,
                CardId::ExpectAFight,
            ];
            if ON_PLAY_ENERGY_GAIN_CARDS.contains(&card_id) {
                ensure!(
                    actual >= expected,
                    "energy overspend for energy-gain card {card_id:?} cost={cost} \
                     weighted={weighted}: before={before} after={actual} \
                     expected>={expected} ({ctx})",
                    before = pre.energy
                );
            } else {
                // `actual == pre.energy` means the play was rejected mid-resolve.
                ensure!(
                    actual == expected || actual == pre.energy,
                    "energy delta mismatch for {card_id:?} cost={cost} weighted={weighted}: \
                     before={before} after={actual} expected={expected} ({ctx})",
                    before = pre.energy
                );
            }
        }
        if x_cost
            && state.pending_choice.is_none()
            && matches!(state.phase, CombatPhase::PlayerPlayPhase)
        {
            ensure!(
                state.player.energy == 0,
                "X-cost card {card_id:?} left energy={} (should be 0) ({ctx})",
                state.player.energy
            );
        }
    }

    Ok(())
}

/// Cross-turn invariants, fired on a player-turn boundary.
///
/// # Errors
/// Returns `Err` if any per-turn counter, block, play-pile, active-attack,
/// or pending-play invariant is violated. (`_prev` is retained for the
/// snapshot-based API even though no surviving invariant compares against the
/// previous turn's anchor — EchoForm's old cross-turn budget decrement was
/// removed with bead `sts2-rs-8ett`.)
pub fn check_turn_boundary(
    _prev: &TurnAnchor,
    state: &CombatState,
    ctx: &str,
) -> Result<(), String> {
    // 16. Per-turn counters reset.
    ensure!(
        state.player.cards_played_this_turn == [0u32; 5],
        "cards_played_this_turn not reset at round {} ({ctx})",
        state.round
    );
    ensure!(
        state.player.shivs_played_this_turn == 0,
        "shivs_played_this_turn not reset at round {} ({ctx})",
        state.round
    );
    ensure!(
        state.player.powered_hits_this_turn == 0,
        "powered_hits_this_turn not reset at round {} ({ctx})",
        state.round
    );
    ensure!(
        state.player.skills_played_this_turn == 0,
        "skills_played_this_turn not reset at round {} ({ctx})",
        state.round
    );
    ensure!(
        state.player.stars_gained_this_turn == 0,
        "stars_gained_this_turn not reset at round {} ({ctx})",
        state.round
    );
    ensure!(
        state.player.pending_retain_count == 0,
        "pending_retain_count not reset at round {} ({ctx})",
        state.round
    );
    ensure!(
        !state.player.request_end_turn_after_play,
        "request_end_turn_after_play not reset at round {} ({ctx})",
        state.round
    );

    // 17. Player block resets unless a block-retention relic is present.
    if !has_block_retention(state) {
        ensure!(
            state.player.block == 0,
            "player block {} not reset at round {} (no retention relic) ({ctx})",
            state.player.block,
            state.round
        );
    }

    // 18. The `play` pile drained.
    ensure!(
        state.player.play.is_empty(),
        "player.play not drained at round {} ({} cards left) ({ctx})",
        state.round,
        state.player.play.len()
    );

    // 19. No active_attack between turns.
    ensure!(
        state.active_attack.is_none(),
        "active_attack still set at round {} ({ctx})",
        state.round
    );

    // 20. (removed) EchoForm no longer rides a per-round budget counter that
    // decrements across turns — `cards_first_in_series_this_turn` is a
    // count-up first-in-series tally reset to 0 each player turn, and the
    // echo decision reads the LIVE EchoForm amount per play (bead
    // `sts2-rs-8ett`). There is no cross-turn decrement invariant to assert.

    // 21. pending_play must not persist across a turn boundary.
    ensure!(
        state.pending_play.is_none(),
        "pending_play persisted across turn boundary at round {} ({ctx})",
        state.round
    );

    // 22. pending_autoplay_resume must not persist across a turn boundary
    // (parity bug `combat_select_ironclad`): an auto-play suspended on a
    // surfaced card-select must be resumed-and-finished before the turn ends.
    ensure!(
        state.pending_autoplay_resume.is_none(),
        "pending_autoplay_resume persisted across turn boundary at round {} ({ctx})",
        state.round
    );

    // 23. pending_enemy_exec_resume must not persist across a turn boundary: an
    // enemy turn suspended on a monster move's blocking card screen
    // (KnowledgeDemon CURSE_OF_KNOWLEDGE) must be resumed — and the rest of the
    // enemy turn run — before the next player turn begins.
    ensure!(
        state.pending_enemy_exec_resume.is_none(),
        "pending_enemy_exec_resume persisted across turn boundary at round {} ({ctx})",
        state.round
    );

    // 24. pending_end_turn_resume must not persist across a turn boundary: the
    // end-turn dispatch chain suspended on a listener auto-play's surfaced
    // card-select owes the rest of itself — the Stampede batch's remaining
    // iterations, the Howl replay, `BeforeTurnEnd`, the flush — and all of that
    // must run before the next player turn starts. Bead `sts2-rs-eirvo`: the
    // Stampede batch used to run straight through a surfaced select and the
    // chain only suspended after `BeforeTurnEnd`, so the marker was set (and
    // consumed) a full stage later than C# blocks.
    ensure!(
        state.pending_end_turn_resume.is_none(),
        "pending_end_turn_resume persisted across turn boundary at round {} ({ctx})",
        state.round
    );

    // 25. pending_cluster_resume must not persist across a turn boundary: the
    // `AfterPlayerTurnStart` cluster walk suspended mid-way owes the rest of
    // itself — the remaining snapshot powers, the Mayhem batch's un-played
    // collected cards, the turn-start tail — and the phase advance to
    // `PlayerPlayPhase` lives in that tail, so reaching a boundary with the
    // marker still set means something ran the tail out from under a suspended
    // walk. Bead `sts2-rs-2itwm`: Mayhem's phase-2 loop used to play straight
    // through a collected card's surfaced select, so the tail ran (and the turn
    // rolled over) with the select still open and the siblings still on Play.
    ensure!(
        state.pending_cluster_resume.is_none(),
        "pending_cluster_resume persisted across turn boundary at round {} ({ctx})",
        state.round
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CombatState, CreatureUid};

    /// A default combat state is structurally valid.
    #[test]
    fn default_state_passes() {
        let s = CombatState::default();
        assert!(check_combat_invariants(&s, "default").is_ok());
    }

    #[test]
    fn negative_player_block_fails() {
        let mut s = CombatState::default();
        s.player.block = -1;
        let err = check_combat_invariants(&s, "t").unwrap_err();
        assert!(err.contains("block negative"), "{err}");
    }

    #[test]
    fn negative_player_energy_fails() {
        let mut s = CombatState::default();
        s.player.energy = -1;
        let err = check_combat_invariants(&s, "t").unwrap_err();
        assert!(err.contains("energy negative"), "{err}");
    }

    #[test]
    fn player_hp_over_max_fails() {
        let mut s = CombatState::default();
        s.player.max_hp = 10;
        s.player.current_hp = 11;
        let err = check_combat_invariants(&s, "t").unwrap_err();
        assert!(err.contains("HP out of range"), "{err}");
    }

    /// A `creature_uid` that resolves to no creature is a mirror bug.
    #[test]
    fn dangling_player_creature_uid_fails() {
        let mut s = CombatState::default();
        s.player.creature_uid = Some(CreatureUid::new(99).unwrap());
        let err = check_combat_invariants(&s, "t").unwrap_err();
        assert!(err.contains("does not resolve"), "{err}");
    }

    #[test]
    fn is_over_revert_fails() {
        let s = CombatState::default();
        let pre = PreStep {
            energy: 0,
            is_over: true,
            played: None,
        };
        // state.is_over is false by default → reverting true→false is illegal.
        let err = check_post_step(&pre, &s, "t").unwrap_err();
        assert!(err.contains("is_over reverted"), "{err}");
    }

    /// A card whose own OnPlay gains energy (e.g. Bloodletting: cost 0,
    /// `GainEnergy(2)`) may legitimately end ABOVE `pre.energy - cost`.
    #[test]
    fn on_play_energy_gain_card_passes() {
        let mut s = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..Default::default()
        };
        s.player.energy = 5; // 3 - 0 + 2 gained
        let pre = PreStep {
            energy: 3,
            is_over: false,
            played: Some((CardId::Bloodletting, 0, false, 0)),
        };
        assert!(check_post_step(&pre, &s, "t").is_ok());
    }

    /// The no-overspend half of the check still holds for energy-gain cards.
    #[test]
    fn on_play_energy_gain_card_overspend_fails() {
        let mut s = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..Default::default()
        };
        s.player.energy = 0; // ExpectAFight cost 2 from 3 → floor is 1
        let pre = PreStep {
            energy: 3,
            is_over: false,
            played: Some((CardId::ExpectAFight, 2, false, 0)),
        };
        let err = check_post_step(&pre, &s, "t").unwrap_err();
        assert!(err.contains("energy overspend"), "{err}");
    }

    /// Non-energy-gain cards keep the strict equality check.
    #[test]
    fn normal_card_energy_surplus_still_fails() {
        let mut s = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..Default::default()
        };
        s.player.energy = 4; // Strike cost 1 from 3 should leave 2
        let pre = PreStep {
            energy: 3,
            is_over: false,
            played: Some((CardId::StrikeIronclad, 1, false, 0)),
        };
        let err = check_post_step(&pre, &s, "t").unwrap_err();
        assert!(err.contains("energy delta mismatch"), "{err}");
    }

    #[test]
    fn turn_boundary_counter_not_reset_fails() {
        let mut s = CombatState::default();
        s.player.cards_played_this_turn = [1, 0, 0, 0, 0];
        let anchor = snapshot_turn_anchor(&s);
        let err = check_turn_boundary(&anchor, &s, "t").unwrap_err();
        assert!(err.contains("cards_played_this_turn not reset"), "{err}");
    }

    /// A clean turn-start state passes the boundary checks.
    #[test]
    fn turn_boundary_clean_passes() {
        let s = CombatState::default();
        let anchor = snapshot_turn_anchor(&s);
        assert!(check_turn_boundary(&anchor, &s, "t").is_ok());
    }
}
