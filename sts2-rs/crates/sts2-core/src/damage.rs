//! Direct `apply_damage` primitive.
//!
//! Mirrors `sts2-rl-agent@01c094d:sts2_env/core/damage.py::apply_damage` end
//! to end. Pipeline order, locked by the Python source-of-truth:
//!
//!   1. `modify_damage` (additive → multiplicative → cap → single floor)
//!      — plumbed by bead sts2-rs-58w; this fn calls it.
//!   2. `before_damage_received` event hook (Thorns, `FlameBarrier`).
//!   3. Block absorption — `min(damage, block)` is consumed by `target.block`,
//!      the residual is the unblocked damage.
//!   4. `modify_hp_lost` chain (additive → multiplicative → cap → single
//!      floor) — plumbed by bead sts2-rs-nj8 (this bead). Phase-2 has zero
//!      active listeners so the chain is identity for VTHIN-2; Phase-7
//!      Buffer / `HardenedShell` / `TungstenRod` arms slot in without
//!      breaking byte-equality.
//!   5. HP loss on the target.
//!   6. `after_damage_received` + `after_damage_given` event hooks.
//!
//! Bead sts2-rs-5if (VTHIN-2) shipped this primitive with the
//! `modify_damage` pipeline + block + HP write but NO HP-loss chain; bead
//! sts2-rs-nj8 inserts step 4. With no Phase-2 listener exercising
//! `modify_hp_lost`, VTHIN-2's byte-equality with Python is preserved.
//!
//! Bead sts2-rs-v7d (P2-2c) wired the `dealer: CreatureRef`, `target: CreatureRef`,
//! and `props: ValueProps` triple through `DamageCtx` + `HpLostCtx` so the
//! per-listener Strength / Vulnerable / Weak / Frail / Dexterity / Intangible
//! arms can fire owner-target + `is_powered` guards. Multi-creature scenarios
//! (Strength on enemy A only buffs enemy A's attacks, etc.) now route
//! correctly.

use crate::cards::CardId;
use crate::hooks::{self, CreatureRef, DamageCtx, HpLostCtx, ValueProps};
use crate::relic_effects::channels::{relic_channel_live, PlannedChannel};
use crate::{CombatState, CreatureUid};

// Hot-path presence gates (perf). `PowerStore::present_kinds()` is a
// documented superset of every live power's kind, so a presence-check miss
// provably means no live power of that kind exists and the guarded walk can
// be skipped bit-identically (see the `PowerKindSet` docs in lib.rs).
// Single-kind gates use `PowerKindSet::contains` (one word test); only the
// genuinely multi-kind gate below needs a mask + `intersects`.
const ADAPTABLE_OR_REATTACH_MASK: crate::PowerKindSet =
    crate::PowerKindSet::from_kinds(&[crate::PowerKind::Adaptable, crate::PowerKind::Reattach]);

/// Remove every `Constrict` power whose recorded `applier` is `dead`.
///
/// Mirrors C# `MegaCrit.Sts2.Core.Models.Powers/ConstrictPower.cs:24-31`
/// `AfterDeath`: `if (!wasRemovalPrevented && creature == base.Applier)
/// PowerCmd.Remove(this)`. When the snake that applied Constrict dies, its
/// Constrict drops off the target — otherwise the player keeps bleeding
/// from a corpse. Unlinks the central [`crate::Power`] row from
/// `state.powers`, the legacy `state.player.powers` list, and every
/// creature's mirror power list (bead `sts2-rs-q6bix`).
pub(crate) fn remove_constrict_from_dead_applier(state: &mut CombatState, dead: CreatureUid) {
    let ids: smallvec::SmallVec<[std::num::NonZeroU32; 2]> = state
        .powers
        .iter()
        .filter(|p| p.kind == crate::PowerKind::Constrict && p.applier == Some(dead))
        .map(|p| p.id)
        .collect();
    for pid in ids {
        state.powers.retain(|p| p.id != pid);
        state.player.powers.retain(|u| u.0 != pid);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != pid);
        }
    }
}

/// `HexPower.AfterDeath` (`HexPower.cs:37-43`): when the creature that
/// APPLIED Hex (SpectralKnight) dies, the power removes itself (and with it
/// its Hexed afflictions and their dynamic Ethereal). Identical unlink shape to Constrict /
/// Shrink. Powers applied without an applier stamp are left alone. Sweep
/// seed 4170 (bead `sts2-rs-l7k3d` follow-up).
pub(crate) fn remove_hex_from_dead_applier(state: &mut CombatState, dead: CreatureUid) {
    let ids: smallvec::SmallVec<[std::num::NonZeroU32; 2]> = state
        .powers
        .iter()
        .filter(|p| p.kind == crate::PowerKind::Hex && p.applier == Some(dead))
        .map(|p| p.id)
        .collect();
    for pid in ids {
        crate::monster_power_effects::hex_before_power_unlinked(state, crate::PowerUid(pid));
        state.powers.retain(|p| p.id != pid);
        state.player.powers.retain(|u| u.0 != pid);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != pid);
        }
    }
}

/// C# `DampenPower.AfterDeath` (`DampenPower.cs:49-62`): when the creature that
/// cast this Dampen dies (and no casters remain), the power removes itself —
/// `PowerCmd.Remove(this)` fires `AfterRemoved`, which RE-UPGRADES every card
/// Dampen had downgraded. MagiKnight is the sole Dampen source and
/// KNIGHTS_ELITE spawns exactly one, so the single `applier` stamped at apply
/// time is the only caster. Mirrors [`remove_hex_from_dead_applier`] but adds
/// the card re-upgrade. Routed through the death-record chokepoint so it fires
/// on every kill path — including a player card play (sweep seed 615: FIGHT_ME
/// killed the MagiKnight, C# dropped DAMPEN, Rust kept it; deaths from card
/// plays never reach the enemy-turn `dispatch_after_death_listeners`).
pub(crate) fn remove_dampen_from_dead_applier(state: &mut CombatState, dead: CreatureUid) {
    let ids: smallvec::SmallVec<[std::num::NonZeroU32; 2]> = state
        .powers
        .iter()
        .filter(|p| p.kind == crate::PowerKind::Dampen && p.applier == Some(dead))
        .map(|p| p.id)
        .collect();
    if ids.is_empty() {
        return;
    }
    // Re-upgrade the cards Dampen downgraded BEFORE dropping the instance
    // (C# `AfterRemoved`). The handler scans the DampenDowngradedMarker, so it
    // is correct to run while the power instance still exists.
    if let Some(player_uid) = state.player.creature_uid {
        let _ = crate::monster_power_effects::dispatch_after_removed_for_power(
            state,
            player_uid,
            crate::PowerKind::Dampen,
        );
    }
    for pid in ids {
        state.powers.retain(|p| p.id != pid);
        state.player.powers.retain(|u| u.0 != pid);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != pid);
        }
    }
}

/// `ShrinkPower.AfterDeath` (`ShrinkPower.cs:71-73`): `if (!wasRemovalPrevented
/// && creature == base.Applier) PowerCmd.Remove(this)`. Identical shape to
/// Constrict — when the ShrinkerBeetle that applied Shrink dies, its Shrink
/// drops off the player; otherwise the player stays shrunk by a corpse. Unlinks
/// the [`crate::Power`] row from `state.powers`, the legacy
/// `state.player.powers` list, and every creature's mirror power list
/// (bead `sts2-rs-creca`).
pub(crate) fn remove_shrink_from_dead_applier(state: &mut CombatState, dead: CreatureUid) {
    let ids: smallvec::SmallVec<[std::num::NonZeroU32; 2]> = state
        .powers
        .iter()
        .filter(|p| p.kind == crate::PowerKind::Shrink && p.applier == Some(dead))
        .map(|p| p.id)
        .collect();
    for pid in ids {
        state.powers.retain(|p| p.id != pid);
        state.player.powers.retain(|u| u.0 != pid);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != pid);
        }
    }
}

/// `IllusionPower.ShouldPowerBeRemovedOnDeath(power) => power.Type == Debuff`
/// (`IllusionPower.cs:54-57`). When a creature carrying `IllusionPower` dies,
/// C# `RemoveAllPowersAfterDeath` strips every Debuff-typed power off it (the
/// illusion dissolves its debuffs) — and this runs BEFORE `AfterDeath` arms
/// the revive, so the revived illusion comes back debuff-free. Rust otherwise
/// retains monster-corpse powers under the c8why NN-slot policy; this is a
/// targeted carve-out gated on `Illusion` presence. FOGMOG repro: EyeWithTeeth
/// (Illusion+Minion) takes Vulnerable from THUNDERCLAP, is killed mid-player-
/// turn by Panache, and C# strips the Vulnerable → powers.len 2 vs Rust's 3.
/// Bead `sts2-rs-9fygi`.
pub(crate) fn remove_debuffs_from_dead_illusion(state: &mut CombatState, dead: CreatureUid) {
    // Perf gate: no live Illusion power ⇒ `has_illusion` is provably false.
    if !state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Illusion)
    {
        return;
    }
    let Some(creature) = state.creatures.iter().find(|c| c.id == dead.0) else {
        return;
    };
    let has_illusion = creature.powers.iter().any(|puid| {
        state
            .powers
            .get_by_id(puid.0)
            .is_some_and(|p| p.kind == crate::PowerKind::Illusion)
    });
    if !has_illusion {
        return;
    }
    // Collect this creature's Debuff-typed power ids, then unlink them from all
    // three power surfaces (central rows + every creature mirror), mirroring
    // `remove_shrink_from_dead_applier`'s multi-surface unlink.
    let creature_power_uids: smallvec::SmallVec<[std::num::NonZeroU32; 8]> =
        creature.powers.iter().map(|u| u.0).collect();
    // C# `IllusionPower.ShouldPowerBeRemovedOnDeath` (v0.107.1) is
    // `if (power.Type == PowerType.Debuff) return !(power is ITemporaryPower);`
    // — it strips static-Debuff powers but KEEPS temporary-power debuffs
    // (`ManglePower`/`TemporaryStrengthPower` & friends, all `ITemporaryPower`),
    // mirroring Rend's debuff filter (`cards/colorless/rend.rs`). Without the
    // exclusion a MANGLE applied to an illusion was wrongly dropped on the
    // illusion's death (seed 2232, OMNISLICE splash kill).
    let debuff_ids: smallvec::SmallVec<[std::num::NonZeroU32; 4]> = state
        .powers
        .iter()
        .filter(|p| {
            creature_power_uids.contains(&p.id)
                && crate::afflictions::classify(p.kind).is_debuff()
                && !crate::afflictions::is_temporary_power_debuff(p.kind)
        })
        .map(|p| p.id)
        .collect();
    for pid in debuff_ids {
        state.powers.retain(|p| p.id != pid);
        state.player.powers.retain(|u| u.0 != pid);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != pid);
        }
    }
}

/// C# `Creature.RemoveAllPowersAfterDeath()` strips every power whose
/// `ShouldPowerBeRemovedAfterOwnerDeath()` returns the base `true`, keeping
/// only the overrides that return `false` (Adaptable, PainfulStabs, …). For
/// the TestSubject this leaves the dead form-1/form-2 corpse holding ONLY its
/// survivor powers — Enrage (and any player-applied debuff like Vulnerable)
/// is removed, so the corpse matches C#'s `powers.len == 1` (just Adaptable)
/// while it awaits the next-turn RESPAWN revive.
///
/// Scoped to creatures carrying a REVIVE power — `Adaptable` (TestSubject) or
/// `Reattach` (Decimillipede segments) — only those carry a death that leaves
/// a power-bearing corpse at this pin. Rust otherwise retains every
/// monster-corpse power under the c8why NN-slot policy, and a general
/// death-strip would shift many unrelated corpses. The survivor allowlist is
/// exactly the kinds whose C# power overrides
/// `ShouldPowerBeRemovedAfterOwnerDeath() => false` that such a creature can
/// hold: Adaptable, PainfulStabs, Nemesis (TestSubject) and Reattach
/// (Decimillipede). A dead Decimillipede segment thus keeps only `[Reattach]`,
/// matching C# (Strength / Poison / TagTeam are stripped). Bead
/// `sts2-rs-jnf4a`.
/// C# `ReattachPower.AfterDeath` (`ReattachPower.cs:50-69`): when a
/// Decimillipede segment dies while at least one other segment is still
/// alive, the corpse is retained and its move machine is forced onto the
/// DEAD no-op (`MonsterModel.SetMoveImmediate`, `MonsterModel.cs:346-358`),
/// replacing the telegraphed move. There is NO heal here — the revive
/// happens later via the REATTACH move at the corpse's own roster slot
/// (`DecimillipedeSegment.ReattachMove` → `ReattachPower.DoReattach`).
/// When every other segment is already dead the power does nothing and the
/// death is fatal (`ShouldOwnerDeathTriggerFatal`), which the alive-count
/// `check_combat_end` already models. Bead `sts2-rs-8lwnq`.
fn arm_reattach_dead_state(state: &mut CombatState, dead: CreatureUid) {
    use crate::PowerKind;
    // Perf gate: no live Reattach power ⇒ `carries_reattach` is provably
    // false for every creature and this fn was a pure no-op walk.
    if !state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Reattach)
    {
        return;
    }
    let carries_reattach = |state: &CombatState, c: &crate::Creature| {
        c.powers.iter().any(|puid| {
            state
                .powers
                .get_by_id(puid.0)
                .is_some_and(|p| p.kind == PowerKind::Reattach)
        })
    };
    let Some(idx) = state.creatures.iter().position(|c| c.id == dead.0) else {
        return;
    };
    if !carries_reattach(state, &state.creatures[idx]) {
        return;
    }
    let another_segment_alive = state.enemy_uids.iter().any(|uid| {
        *uid != dead
            && state
                .creatures
                .iter()
                .any(|c| c.id == uid.0 && c.current_hp > 0 && carries_reattach(state, c))
    });
    if !another_segment_alive {
        return;
    }
    let creature = &mut state.creatures[idx];
    let Some(ai) = creature.ai.as_mut() else {
        return;
    };
    if ai.force_current_state_by_name("DEAD") {
        // C# builds `DEAD_MOVE` with an empty `params AbstractIntent[]`
        // (`DecimillipedeSegment.cs:152`), so the corpse telegraphs NOTHING —
        // clear whatever it was telegraphing when it died. It still takes its
        // turn: `Creature.TakeTurn` is not gated on intents, and
        // `is_acting_reattach_corpse` admits it to the acting snapshot on its
        // own merits rather than via a fabricated placeholder intent.
        creature.intent = None;
    }
}

/// C# `IllusionPower.AfterDeath` (`IllusionPower.cs:76-89`): a dying illusion
/// replaces its telegraphed move with
/// `new MoveState("REVIVE_MOVE", ReviveMove, new HealIntent())` via
/// `SetMoveImmediate`, so from the instant it dies until it revives the corpse
/// telegraphs **Heal** — not whatever it was about to do when it was killed.
///
/// The revive itself is already modeled, in `combat_loop::enemy_turn_exec`
/// step 2.5: a 0-HP Illusion carrier is healed to `max_hp` and that consumes
/// its turn. Only the telegraph was missing, which made every reviving illusion
/// show a stale intent to the player (and to the mirror) for a full round.
/// FOGMOG's EyeWithTeeth and Vantom's Parafrights are the live repros — the
/// wire reports `Heal` on the corpse while the engine still showed the
/// pre-death `Attack`.
///
/// Scoped exactly like [`remove_debuffs_from_dead_illusion`] (the sibling
/// `AfterDeath` witness that C# runs just *before* this one): gated on the DEAD
/// creature owning `Illusion`, not on any Illusion existing. Stamping the
/// intent does not admit the corpse to `acting_enemies` — that snapshot gates
/// on `current_hp > 0 || is_acting_reattach_corpse`, neither of which a 0-HP
/// illusion satisfies — so this is telegraph-only and changes no line of play.
/// Step 2.5's revive re-rolls the follow-up intent once `current_hp > 0`.
fn arm_illusion_revive_telegraph(state: &mut CombatState, dead: CreatureUid) {
    // Perf gate: no live Illusion power ⇒ the carrier check is provably false.
    if !state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Illusion)
    {
        return;
    }
    let Some(idx) = state.creatures.iter().position(|c| c.id == dead.0) else {
        return;
    };
    let carries_illusion = state.creatures[idx].powers.iter().any(|puid| {
        state
            .powers
            .get_by_id(puid.0)
            .is_some_and(|p| p.kind == crate::PowerKind::Illusion)
    });
    if !carries_illusion {
        return;
    }
    state.creatures[idx].intent = Some(crate::Intent::Heal);
}

/// C# `TestSubject.TriggerDeadState` (`TestSubject.cs:167-172`) calls
/// `SetMoveImmediate(DeadState, forceTransition: true)`, and `DeadState` is
/// `new MoveState("RESPAWN_MOVE", RespawnMove, new HealIntent(), new
/// BuffIntent())` (`TestSubject.cs:193`). So an Adaptable corpse telegraphs its
/// respawn from the moment it dies — the same shape as
/// [`arm_illusion_revive_telegraph`], in the third revive family.
///
/// The engine advances its move machine onto RESPAWN at *revive* time
/// (`combat_loop::enemy_turn_exec` step 2.6) rather than at death, so between
/// the two the corpse kept advertising whatever it was killed mid-telegraph.
/// The move table itself was already right; being right about the move a
/// creature will perform is not the same as being armed onto it, and only the
/// second is observable while the corpse waits.
///
/// This arms the LEADING `Heal` only, and that does not fully close the class.
/// The move carries two intents and the wire reports both: `SerializeIntents`
/// loops over every entry in `NextMove.Intents`, the adapter translates the
/// whole list, and `combat_mirror.observables()` compares the whole list. So a
/// world sending `[Heal, Buff]` still meets a mirror sending `[Heal]`. That
/// residual is the pre-existing single-intent limit — `Creature::intent` is one
/// `Option<Intent>` and `state_to_dict` emits a 0-or-1 list — which applies to
/// every multi-intent move (`BULK_MOVE`, `SKULL_BASH_MOVE`, …), not something
/// this function introduces or can fix. Arming the leading intent changes these
/// records from "corpse telegraphs a stale attack" to "corpse telegraphs the
/// right first half", which is strictly closer and still not equal.
///
/// Telegraph-only: the corpse's admission to `acting_enemies` is unchanged (it
/// gates on `current_hp > 0 || is_acting_reattach_corpse`), and step 2.6
/// re-rolls the post-revive intent once the form is back.
fn arm_adaptable_respawn_telegraph(state: &mut CombatState, dead: CreatureUid) {
    // Perf gate: no live Adaptable power ⇒ the carrier check is provably false.
    if !state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Adaptable)
    {
        return;
    }
    let Some(idx) = state.creatures.iter().position(|c| c.id == dead.0) else {
        return;
    };
    let carries_adaptable = state.creatures[idx].powers.iter().any(|puid| {
        state
            .powers
            .get_by_id(puid.0)
            .is_some_and(|p| p.kind == crate::PowerKind::Adaptable)
    });
    if !carries_adaptable {
        return;
    }
    state.creatures[idx].intent = Some(crate::Intent::Heal);
}

pub(crate) fn strip_powers_on_adaptable_death(state: &mut CombatState, dead: CreatureUid) {
    use crate::PowerKind;
    // Perf gate: no live Adaptable/Reattach power ⇒ `has_revive_power` is
    // provably false.
    if !state
        .powers
        .present_kinds()
        .intersects(&ADAPTABLE_OR_REATTACH_MASK)
    {
        return;
    }
    let Some(creature) = state.creatures.iter().find(|c| c.id == dead.0) else {
        return;
    };
    let has_revive_power = creature.powers.iter().any(|puid| {
        state
            .powers
            .get_by_id(puid.0)
            .is_some_and(|p| matches!(p.kind, PowerKind::Adaptable | PowerKind::Reattach))
    });
    if !has_revive_power {
        return;
    }
    let survives = |kind: PowerKind| {
        matches!(
            kind,
            PowerKind::Adaptable
                | PowerKind::PainfulStabs
                | PowerKind::Nemesis
                | PowerKind::Reattach
        )
    };
    let creature_power_uids: smallvec::SmallVec<[std::num::NonZeroU32; 8]> =
        creature.powers.iter().map(|u| u.0).collect();
    let strip_ids: smallvec::SmallVec<[std::num::NonZeroU32; 8]> = state
        .powers
        .iter()
        .filter(|p| creature_power_uids.contains(&p.id) && !survives(p.kind))
        .map(|p| p.id)
        .collect();
    for pid in strip_ids {
        state.powers.retain(|p| p.id != pid);
        state.player.powers.retain(|u| u.0 != pid);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != pid);
        }
    }
}

/// Apply `raw_damage` to the creature `target` references. Returns the HP
/// loss actually inflicted (block-absorbed damage is not counted in the
/// return value, matching Python `DamageResult.hp_lost`).
///
/// **Bead sts2-rs-v7d (P2-2c) signature growth:** previously
/// `apply_damage(state, target, raw_damage)`; now also takes `dealer:
/// CreatureRef` and `props: ValueProps` so the listener arms have the
/// owner-target + `is_powered` guards Python relies on. Canonical
/// player-attacks-enemy fixtures pass `dealer = CreatureRef::Player`,
/// `props = ValueProps::MOVE` (the Python recorder's default — see
/// `record_fixture.py:322`).
///
/// **Pipeline order** matches Python `damage.py:106-172`:
///   1. Run the full `modify_damage` pipeline (additive + multiplicative +
///      cap + single floor + clamp at zero).
///   2. Fire `before_damage_received` event hook on every listener.
///   3. Subtract `min(final_damage, target.block)` from `target.block`.
///   4. Run the `modify_hp_lost` pipeline on the unblocked residual
///      (additive + multiplicative + cap + single floor + clamp at zero).
///      Phase-2 has zero active listeners so this is identity; Phase-7
///      adds Buffer / `HardenedShell` / `TungstenRod` arms.
///   5. Subtract the post-`modify_hp_lost` amount from `target.current_hp`
///      (clamped at 0).
///   6. Fire `after_damage_received` + `after_damage_given` event hooks.
///
/// **Out of scope for nj8:**
///   * Per-listener `modify_hp_lost` arm bodies (Buffer / `HardenedShell` /
///     `TungstenRod` / Intangible-safety-cap / Boot) — Phase 7.
///   * Block-debuff / on-damage-blocked side effects — bead sts2-rs-aot.
///   * Attack-context dealer threading — bead sts2-rs-6w8 wires `DamageCtx`
///     fields when the first additive listener (Strength) lands.
///   * `unblockable` flag (Python `props & ValueProp.UNBLOCKABLE`) — there
///     is no Unblockable card in the VTHIN-2 fixture, so the pipeline
///     unconditionally absorbs through block.
pub fn apply_damage(
    state: &mut CombatState,
    dealer: CreatureRef,
    target: CreatureUid,
    raw_damage: i32,
    props: ValueProps,
    card: Option<CardId>,
) -> i32 {
    apply_damage_full(state, dealer, target, raw_damage, props, card).hp_loss
}

/// Resolve the originating card **instance** uid for a `ModifyDamageCtx`,
/// consistent with the `card: Option<CardId>` already threaded by the
/// caller. The card currently mid-resolution is on top of `state.player.play`
/// (`combat_loop::play_card` moves `Hand → Play` before dispatch); we return
/// that uid only when its `card_id` equals the passed `card`, so the uid and
/// id always agree. Returns `None` for non-card damage (`card == None`) or
/// when the proximate source isn't the top-of-`Play` card (e.g. a relic
/// listener that passes an explicit `card_id`). Bead `sts2-rs-nwz1n`.
#[inline]
fn originating_card_uid(state: &CombatState, card: Option<CardId>) -> Option<crate::CardUid> {
    let card_id = card?;
    let uid = crate::card_effects::resolving_card_uid(state)?;
    let idx = uid.0.get() as usize - 1;
    if state.cards.get(idx).is_some_and(|c| c.card_id == card_id) {
        Some(uid)
    } else {
        None
    }
}

/// Result of a single [`apply_damage_full`] call.
///
/// Mirrors the relevant fields of C#
/// `MegaCrit.Sts2.Core.Entities.Creatures/DamageResult.cs` so callers can
/// compose whatever sum the source card uses. Different cards read different
/// combinations: Fisticuffs gains Block from `TotalDamage + OverkillDamage`,
/// while BlightStrike applies Doom from `TotalDamage` alone (no overkill).
///
/// * `hp_loss` — realized HP loss, clamped to the target's remaining HP
///   (C# `DamageResult.UnblockedDamage`; `currentHp - CurrentHp`). Matches the
///   scalar [`apply_damage`] returns.
/// * `blocked` — damage soaked into the target's Block (C# `BlockedDamage`).
/// * `overkill` — damage beyond what was needed to kill (C# `OverkillDamage`;
///   `0` on a non-lethal hit).
/// * `total_damage` — convenience = `blocked + hp_loss`, exactly C#
///   `DamageResult.TotalDamage` (`BlockedDamage + UnblockedDamage`). Does NOT
///   include overkill — add `overkill` explicitly for the Fisticuffs-style
///   `TotalDamage + OverkillDamage` sum.
///
/// Bead `sts2-rs-67yqf` / `sts2-rs-6cnok`: Fisticuffs gains Block equal to
/// `total_damage + overkill` (incl. block-absorbed and overkill), which
/// [`apply_damage`]'s clamped HP-loss scalar cannot express.
#[derive(Debug, Clone, Copy)]
pub struct DamageOutcome {
    pub hp_loss: i32,
    pub blocked: i32,
    pub overkill: i32,
    pub total_damage: i32,
}

/// Compute the fully-modified outgoing damage for a single dealer→target hit
/// WITHOUT applying it — the relic-additive fold plus the hooks `modify_damage`
/// pipeline (additive → multiplicative → cap → single floor → `max(0, _)`).
/// This is exactly the C# `Hook.ModifyDamage` stage (`CreatureCmd.cs:141`),
/// split out from [`apply_damage_full`] so multi-target callers can lock EVERY
/// target's damage against the constant pre-death dealer state BEFORE any kill
/// is processed: C# `CreatureCmd.Damage` computes every target's `ModifyDamage`
/// in its first loop (`CreatureCmd.cs:134-151`), then calls `Kill` once after
/// the whole loop (`CreatureCmd.cs:285`), so a mid-AoE death (Shrink removal,
/// Possess give-back, …) cannot retroactively change a surviving target's
/// already-computed damage. Read-only over `state`. Bead `sts2-rs-8gzvn`.
#[must_use]
pub fn compute_modified_damage(
    state: &CombatState,
    dealer: CreatureRef,
    target: CreatureUid,
    raw_damage: i32,
    props: ValueProps,
    card: Option<CardId>,
) -> i32 {
    let ctx = DamageCtx {
        dealer,
        target: CreatureRef::Enemy(target),
        props,
    };

    // Full damage pipeline (additive → mul → cap → single floor → max(0, _)).
    // `modify_damage` floors once at the end via `DamageMul::apply` (rational
    // `div_euclid`), matching Python's `math.floor` exactly.
    //
    // Bead `sts2-rs-6jk` (j87-a): fold the relic_effects-family additive
    // contribution INTO `raw_damage` before the hooks-family pipeline runs so
    // multiplicative + cap + floor apply to the combined sum. Mirrors Python
    // `hooks.py:79-82` where all `modify_damage_additive` owners sum to a
    // single delta consumed by the multi-stage pipeline. Hooks-family relic
    // listeners (legacy `iter_relic_listeners` arms) still run inside
    // `hooks::modify_damage`; the two registries compose left-to-right.
    // J01b: a dead channel (no owned handler) folds to the additive identity
    // `0` without building the context.
    let relic_additive = if relic_channel_live(state, PlannedChannel::ModifyDamageAdditive) {
        let relic_additive_md_ctx = crate::relic_effects::ModifyDamageCtx {
            dealer: dealer.to_creature_uid(state),
            target,
            base_damage: raw_damage,
            props,
            // Bead sts2-rs-4nh5: threaded from caller — `Some(card_id)` on
            // card-sourced damage, `None` on non-card paths (Thorns/FlameBarrier
            // reflection, recursive listener calls).
            card,
            // Bead sts2-rs-nwz1n: the played instance uid — the top-of-`Play`
            // card when its id matches `card`. Lets MiniatureCannon read the
            // SPECIFIC instance's upgrade flag instead of the first same-id copy.
            card_uid: originating_card_uid(state, card),
        };
        crate::relic_effects::dispatch_modify_damage_additive(state, &relic_additive_md_ctx)
    } else {
        0
    };
    let raw_damage_with_relic_additive = raw_damage.saturating_add(relic_additive);
    hooks::modify_damage(state, raw_damage_with_relic_additive, &ctx)
}

/// [`compute_modified_damage`], plus the `AfterModifyingDamageAmount` flashes
/// that same hit earns — the pair C# `CreatureCmd.cs:261-262` runs back to
/// back, once per target, before any `Kill`.
///
/// They are one function rather than two calls at each site because the flash
/// selection is defined against the pipeline's own intermediate amount: a
/// caller that computed the damage with one base and flashed against another
/// would light the wrong powers, and nothing downstream would show it. Every
/// real damage resolution goes through here; the previews call
/// [`compute_modified_damage`] directly and stay silent.
pub fn compute_modified_damage_flashing(
    state: &mut CombatState,
    dealer: CreatureRef,
    target: CreatureUid,
    raw_damage: i32,
    props: ValueProps,
    card: Option<CardId>,
) -> i32 {
    // CreatureCmd.Damage filters dead targets before modifiers or hooks.
    if state.creatures[creature_index_for(state, target)].current_hp <= 0 {
        return 0;
    }
    let final_damage = compute_modified_damage(state, dealer, target, raw_damage, props, card);
    if crate::event_log::armed(state) {
        // Recomputing the relic-additive fold rather than threading it out of
        // `compute_modified_damage`: it is a pure function of the same
        // arguments, and this whole branch is cold (armed only).
        let relic_additive_md_ctx = crate::relic_effects::ModifyDamageCtx {
            dealer: dealer.to_creature_uid(state),
            target,
            base_damage: raw_damage,
            props,
            card,
            card_uid: originating_card_uid(state, card),
        };
        let base = raw_damage.saturating_add(
            crate::relic_effects::dispatch_modify_damage_additive(state, &relic_additive_md_ctx),
        );
        flash_damage_amount_modifiers(state, dealer, CreatureRef::Enemy(target), base, props);
    }
    final_damage
}

/// Fire `Hook.AfterModifyingDamageAmount` for one resolved hit — the
/// `PowerFlashed` half of `CreatureCmd.cs:262`, which sits between
/// `Hook.ModifyDamage` and `Hook.BeforeDamageReceived`.
///
/// Emission only. `base_after_relic_additive` must be the SAME number the hit's
/// own `hooks::modify_damage` call received — i.e. `raw_damage` with the
/// `relic_effects` additive already folded in — because the cap stage's
/// listener selection is defined against the amount that pipeline produced.
/// Passing the un-folded raw damage would light the wrong powers on any board
/// carrying a damage relic. [`hooks::damage_amount_flashes`] carries the
/// selection rule and the reasoning behind it.
///
/// Gated on the event log being armed, like `flash_power` itself — so under
/// search this is one branch and no listener walk. That is why the selection
/// walk it calls is allowed to be a second, cold traversal rather than
/// something threaded through `modify_damage`'s hot single-pass fold.
///
/// Not called from any preview: `preview_damage_to_player`,
/// `cards::preview`, `player_modified_card_damage` and the staged card preview
/// in `sts2-py` all run the same pipeline to show a number, and the game
/// dispatches this hook only from `CreatureCmd.Damage` with
/// `CardPreviewMode.None`.
pub fn flash_damage_amount_modifiers(
    state: &mut CombatState,
    dealer: CreatureRef,
    target: CreatureRef,
    base_after_relic_additive: i32,
    props: ValueProps,
) {
    if !crate::event_log::armed(state) {
        return;
    }
    let ctx = hooks::DamageCtx {
        dealer,
        target,
        props,
    };
    for (owner, kind) in hooks::damage_amount_flashes(state, base_after_relic_additive, &ctx) {
        match owner {
            CreatureRef::Player => crate::event_log::flash_player_power(state, kind),
            CreatureRef::Enemy(uid) => crate::event_log::flash_power(state, uid, kind),
            // An ally's powers can reach the fold, but no ally-owned Intangible
            // / HardToKill / Slow exists at v0.107.1 and `flash_power` is
            // keyed on a creature uid an ally slot does not carry. Dropping it
            // is the same choice `LogCreature` makes elsewhere.
            CreatureRef::Ally(_) | CreatureRef::Nobody => {}
        }
    }
}

/// Apply an already-[`compute_modified_damage`]'d `final_damage` to `target`,
/// running every C# `CreatureCmd.Damage` stage from `BeforeDamageReceived`
/// onward: block absorption → `modify_hp_lost` → HP write → power-side death
/// witnesses → after-damage hooks → relic `AfterDeath`. Paired with
/// [`compute_modified_damage`] so AoE callers can resolve all targets' damage
/// up-front (see that fn's note on C# loop-then-Kill ordering);
/// [`apply_damage_full`] composes the two for the single-hit path. Bead
/// `sts2-rs-8gzvn`.
pub fn apply_damage_with_final(
    state: &mut CombatState,
    dealer: CreatureRef,
    target: CreatureUid,
    final_damage: i32,
    props: ValueProps,
) -> DamageOutcome {
    apply_damage_with_final_impl(state, dealer, target, final_damage, props, false)
}

/// Direct-kill variant mirroring C# `CreatureCmd.Kill` → `Creature.LoseHpInternal`
/// (`Creature.cs:374`): a DIRECT HP zero that bypasses the ENTIRE `ModifyHpLost`
/// pipeline. Unlike [`apply_damage_with_final`], the post-block HP loss is NOT
/// run through `modify_hp_lost` — so the Slippery / Intangible 1-cap
/// ([`crate::hooks::modify_hp_lost_cap`]) and Buffer's `DamageMul::ZERO` absorb
/// (plus its per-event stack decrement) never see the lethal tick. The
/// `just_died` death teardown (Step 5 onward) is shared and still fires. Used by
/// the enemy [`crate::hooks::doom_kill`] path so a doomed Slippery / Intangible /
/// Buffered enemy dies outright, matching C# `DoomKill`. Bead `sts2-rs-doom-slippery`.
pub fn apply_damage_direct_kill(
    state: &mut CombatState,
    dealer: CreatureRef,
    target: CreatureUid,
    final_damage: i32,
    props: ValueProps,
) -> DamageOutcome {
    apply_damage_with_final_impl(state, dealer, target, final_damage, props, true)
}

/// Human-readable dealer label for [`crate::damage_trace`]. Enemies resolve
/// to their species so a trace row reads `enemy:Rocket` rather than a uid the
/// reader would have to cross-reference against the roster.
pub(crate) fn describe_dealer_for_trace(state: &CombatState, dealer: CreatureRef) -> String {
    describe_dealer(state, dealer)
}

fn describe_dealer(state: &CombatState, dealer: CreatureRef) -> String {
    match dealer {
        CreatureRef::Player => "player".to_string(),
        CreatureRef::Ally(i) => format!("ally:{i}"),
        CreatureRef::Enemy(uid) => match state.creatures.iter().find(|c| c.id == uid.0) {
            Some(c) => format!("enemy:{}", c.monster_id.0),
            None => format!("enemy:#{}", uid.0.get()),
        },
        CreatureRef::Nobody => "nobody".to_string(),
    }
}

fn apply_damage_with_final_impl(
    state: &mut CombatState,
    dealer: CreatureRef,
    target: CreatureUid,
    final_damage: i32,
    props: ValueProps,
    bypass_hp_loss_cap: bool,
) -> DamageOutcome {
    // Retained corpses are not damage targets either. In particular, a
    // multi-hit attack must not trigger their before-damage Thorns again.
    if state.creatures[creature_index_for(state, target)].current_hp <= 0 {
        return DamageOutcome {
            hp_loss: 0,
            blocked: 0,
            overkill: 0,
            total_damage: 0,
        };
    }
    let target_ref = CreatureRef::Enemy(target);
    let ctx = DamageCtx {
        dealer,
        target: target_ref,
        props,
    };
    let hp_lost_ctx = HpLostCtx {
        dealer,
        target: target_ref,
        props,
    };

    // Step 2: before-damage-received event hook. Bead sts2-rs-3to (P2-2b)
    // promoted the dispatcher to `(state: &mut CombatState, ...)`: listeners
    // produce deferred-effect queue entries (Thorns: reflective
    // `Effect::DealHpLoss` to dealer) which the dispatcher drains internally
    // after the read-only walk via `apply_effects(state, &effects)`.
    hooks::dispatch_before_damage_received(state, &ctx);

    // Step 3-5: block absorption → modify_hp_lost pipeline → HP write. The
    // mutable creature borrow has to come AFTER the read-only
    // `dispatch_before_damage_received` call (which takes `&CombatState`),
    // and we have to compute `unblocked` before re-borrowing `state` for the
    // `modify_hp_lost` dispatcher walk. Layout:
    //
    //   - Inner block 1 (mut borrow): block absorption, compute unblocked.
    //   - `modify_hp_lost(state, ...)` (read-only borrow): apply HP-loss chain.
    //   - Inner block 2 (mut borrow): write HP loss to creature.
    //
    // **Phase-2 unblockable note**: bead 3to threads `ValueProps::UNBLOCKABLE`
    // through the canonical fixtures (Poison's UNBLOCKABLE | UNPOWERED tick).
    // Python's `damage.py:135-137` reads `unblockable = bool(props & UNBLOCKABLE)`
    // and skips block absorption entirely. The Rust path now mirrors that —
    // an UNBLOCKABLE hit treats `creature.block` as zero for absorption
    // purposes (block stays untouched).
    // Resolve the target's arena index ONCE for the whole call. The creatures
    // vec is append-only in production (corpses are retained; spawns push),
    // so the index stays valid across the intervening hook dispatches —
    // re-resolving it per step was three identical linear scans per hit.
    let creature_idx = creature_index_for(state, target);

    let (unblocked, target_fully_blocked_arm) = {
        let creature = &mut state.creatures[creature_idx];
        let unblockable = props.contains(hooks::ValueProps::UNBLOCKABLE);
        let absorbed = if unblockable {
            0
        } else {
            final_damage.min(creature.block).max(0)
        };
        creature.block -= absorbed;
        // C# `WasFullyBlocked` block-arm (CreatureCmd.cs:154): `!Unblockable &&
        // (blockedDamage > 0 || target.Block > 0)` against the post-absorption
        // block. The `unblockedDamage == 0` residual is folded in at the
        // dispatch call below. (BowlbugRock's Imbalanced is enemy-only and never
        // hosted on this creature-target path at this pin, but the value is
        // computed faithfully for symmetry with the player path — bead
        // `sts2-rs-0l7dd`.)
        let fully_blocked_arm = !unblockable && (absorbed > 0 || creature.block > 0);
        ((final_damage - absorbed).max(0), fully_blocked_arm)
    };

    // Step 4: modify_hp_lost pipeline. Mirrors Python's guarded call at
    // damage.py:140-141 (`if combat is not None and remaining > 0`); the
    // `remaining > 0` short-circuit is preserved here so a zero-damage hit
    // doesn't touch the dispatcher walk (matches Python byte-for-byte).
    let modified_hp_loss = if unblocked > 0 {
        if bypass_hp_loss_cap {
            // Direct kill (C# `LoseHpInternal`): the lethal HP loss bypasses the
            // ENTIRE `ModifyHpLost` pipeline — no Slippery / Intangible 1-cap, no
            // Buffer `DamageMul::ZERO` absorb, no relic-side fold. The unblocked
            // amount (already the full `current_hp` for a doom kill, since the
            // UNBLOCKABLE prop zeroes block absorption) is written straight down.
            unblocked
        } else {
            let after_hooks = hooks::modify_hp_lost(state, unblocked, &hp_lost_ctx);
            // Bead `sts2-rs-6jk` (j87-a): apply the relic_effects-family
            // `modify_hp_lost` fold AFTER the hooks-family pipeline so relic-side
            // listeners see the post-cap/post-clamp amount and transform it
            // left-to-right. `relic_effects::dispatch_modify_hp_lost` is a fold
            // returning the new amount; the BTreeMap walk order is deterministic
            // by `RelicId`. J01b: a dead channel leaves the amount unchanged.
            if relic_channel_live(state, PlannedChannel::ModifyHpLost) {
                let ml_ctx = crate::relic_effects::ModifyHpLostCtx {
                    target,
                    dealer: dealer.to_creature_uid(state),
                    props,
                };
                crate::relic_effects::dispatch_modify_hp_lost(state, &ml_ctx, after_hooks)
            } else {
                after_hooks
            }
        }
    } else {
        0
    };

    // Bead `sts2-rs-q6bix.6` (Lucky Tonic): BufferPower consumes one stack
    // per HP-loss EVENT it absorbs. C# splits this across two hooks —
    // `BufferPower.ModifyHpLostAfterOstyLate` returns `0m` (the absorb,
    // wired into `modify_hp_lost_multiplicative` as `DamageMul::ZERO`) and
    // `AfterModifyingHpLostAfterOsty` does `PowerCmd.Decrement(this)`. The
    // Rust `modify_hp_lost` pipeline is read-only and cannot mutate, so the
    // decrement happens here, post-pipeline, gated on the SAME condition
    // the multiplicative arm uses (an active Buffer owned by the target).
    // The enclosing `unblocked > 0` guard mirrors C#: Buffer only fires
    // when an HP-loss event actually reaches the modify stage (block alone
    // does not consume a Buffer stack). Removal-at-zero matches the
    // Counter-power teardown used by `try_consume_artifact_for_debuff`.
    if unblocked > 0 && !bypass_hp_loss_cap {
        decrement_buffer_on_target(state, hp_lost_ctx.target);
        flash_intangible_on_target(state, hp_lost_ctx.target);
        flash_hardened_shell_on_target(state, hp_lost_ctx.target);
    }

    // Step 5: write HP loss.
    let (hp_loss, blocked, just_died, pre_hp, post_hp) = {
        let creature = &mut state.creatures[creature_idx];
        let pre_hp = creature.current_hp;
        let hp_loss = modified_hp_loss.min(creature.current_hp.max(0));
        creature.current_hp -= modified_hp_loss;
        if creature.current_hp < 0 {
            creature.current_hp = 0;
        }
        // Bead `sts2-rs-ar9ll` (dvqa1.1): record the positive→zero
        // transition so the cross-creature AfterDeath dispatch (filed
        // sts2-rs-79tv9) can react. Player deaths are excluded — only
        // monsters get sibling reactions in the C# AfterDeath surface.
        let just_died = pre_hp > 0 && creature.current_hp <= 0;
        // `final_damage` was reduced by the absorbed amount above; the
        // absorbed value is `final_damage - unblocked` (computed in the
        // inner block earlier — `let unblocked = …`). Re-derive it
        // here from the locals already in scope:
        (
            hp_loss,
            final_damage - unblocked,
            just_died,
            pre_hp,
            creature.current_hp,
        )
    };
    // Forensics only — see `damage_trace`. Placed immediately after the HP
    // write so the row reflects THIS hit, before any death teardown or
    // AfterDeath sibling reaction adds its own (separately traced) damage.
    if crate::damage_trace::armed() {
        let (target_id, dealer_label) = {
            let c = &state.creatures[creature_idx];
            let id = if state.player.creature_uid == Some(target) {
                "player".to_string()
            } else {
                c.monster_id.0.to_string()
            };
            (id, describe_dealer(state, dealer))
        };
        crate::damage_trace::record(
            dealer_label,
            target.0.get(),
            target_id,
            final_damage,
            blocked,
            hp_loss,
            pre_hp,
            post_hp,
            i32::from(state.round),
        );
    }
    // Game-style event log (C# `DamageReceivedEntry`), recorded at the same
    // pin and for the same reason: this row is THIS hit, before any death
    // teardown or AfterDeath sibling reaction appends its own.
    if crate::event_log::armed(state) {
        let receiver = crate::event_log::LogCreature::of_uid(state, target);
        let dealer_who = crate::event_log::LogCreature::of_dealer(state, dealer);
        crate::event_log::record(
            state,
            crate::event_log::EventLogEntry::DamageReceived {
                dealer: Some(dealer_who),
                receiver,
                // C# `DamageResult.UnblockedDamage` — what got past block and
                // the modify pipeline, UNCLAMPED. `hp_loss` (the clamped
                // write) would silently under-report an overkill; the HP pair
                // below already carries the actual write.
                unblocked: modified_hp_loss,
                blocked,
                hp_before: pre_hp,
                hp_after: post_hp,
            },
        );
    }
    if just_died
        && state.player.creature_uid != Some(target)
        && !state.combat_log.newly_dead_this_turn.contains(&target)
    {
        state.combat_log.newly_dead_this_turn.push(target);
    }
    // Bead `sts2-rs-RP13`: C# `SwipePower.BeforeDeath` restores any card the
    // thief stole when it DIES (vs escapes). Reclaim here at the damage-death
    // chokepoint so a killed `ThievingHopper`'s stolen card is NOT removed
    // from the run deck at `exit_combat`; escapes never reach this point and
    // so stay un-reclaimed (= permanently removed, matching C#).
    if just_died {
        state.reclaim_swipe_on_death(target);
        // GH #604: C# `HeistPower.BeforeDeath` hands the stolen gold back as a
        // combat reward when the carrier is KILLED. Same chokepoint, same
        // kill-vs-escape distinction as the Swipe reclaim above.
        state.recover_heist_on_death(target);
    }
    // Bead `sts2-rs-q6bix` (ConstrictPower.AfterDeath): when the creature
    // that APPLIED a Constrict dies, the Constrict drops off its target.
    // C# `MegaCrit.Sts2.Core.Models.Powers/ConstrictPower.cs:24-31`:
    // `if (!wasRemovalPrevented && creature == base.Applier) Remove(this)`.
    // Constrict is applied only by monsters (e.g. SlitheringStrangler) at
    // this pin, so a dead applier means a dead monster and the player's
    // lingering Constrict must be removed. Routed through the death-record
    // chokepoint so it fires on EVERY kill path (player card, poison,
    // thorns, enemy turn). NB: the C# `!wasRemovalPrevented` guard (an
    // Illusion that revives keeps the Constrict it applied) is not modeled
    // — no Constrict-applying creature is an Illusion at this pin.
    // The dying creature's side, resolved once for the power-side AfterDeath
    // witnesses below and the relic-side `after_death` bus fired near the end
    // of this function (bead `sts2-rs-rmrzx`). Allies are player-side; an
    // unresolved uid defaults to player-side (no enemy arm fires for it).
    let dead_side = if state.player.creature_uid == Some(target) {
        crate::Side::Player
    } else if state.enemy_uids.contains(&target) {
        crate::Side::Enemy
    } else {
        crate::Side::Player
    };
    if just_died {
        remove_constrict_from_dead_applier(state, target);
        remove_shrink_from_dead_applier(state, target);
        remove_hex_from_dead_applier(state, target);
        remove_dampen_from_dead_applier(state, target);
        // C# `IllusionPower.ShouldPowerBeRemovedOnDeath` strips the dying
        // illusion's Debuff powers before the revive arms (bead sts2-rs-9fygi).
        remove_debuffs_from_dead_illusion(state, target);
        // …and then `IllusionPower.AfterDeath` arms REVIVE_MOVE, whose single
        // `HealIntent` is what the corpse telegraphs until it revives. Ordered
        // after the strip above to match C#, where `RemoveAllPowersAfterDeath`
        // runs before the `AfterDeath` witnesses.
        arm_illusion_revive_telegraph(state, target);
        // C# `RemoveAllPowersAfterDeath` on a TestSubject corpse strips Enrage
        // (and any player debuff) so the form-1/form-2 corpse holds only its
        // survivor powers while it awaits the next-turn RESPAWN revive
        // (bead sts2-rs-jnf4a). Scoped to Adaptable carriers (TestSubject only).
        strip_powers_on_adaptable_death(state, target);
        // …and `TriggerDeadState` arms RESPAWN_MOVE, whose leading `HealIntent`
        // is what the corpse telegraphs until it respawns (TestSubject.cs:167-193).
        arm_adaptable_respawn_telegraph(state, target);
        // C# `ReattachPower.AfterDeath` arms the dead Decimillipede
        // segment's revive cycle (bead sts2-rs-8lwnq).
        arm_reattach_dead_state(state, target);
        // Bead `sts2-rs-ybtmu`: cross-creature power `AfterDeath` witness
        // dispatch (CrabRagePower / SurroundedPower). Fired from the same
        // death-record chokepoint so it triggers on every kill path.
        hooks::dispatch_after_death(
            state,
            &hooks::AfterDeathCtx {
                dead: target,
                dead_side,
            },
        );
        // Bead `sts2-rs-zo6fk`: per-death on-death spawn for the just-killed
        // enemy. C# `CreatureCmd.Kill` fires the dying creature's power
        // `AfterDeath` immediately (CreatureCmd.cs:285), and
        // `StockPower.AfterDeath` spawns a fresh Axebot at the dead bot's
        // slot the instant it dies — so a mid-combat kill of one of two
        // Axebots keeps two live enemies. The combat-end `fire_pending`
        // sweep (check_combat_end) only runs when every enemy is dead, which
        // is too late for a respawn that must happen while a sibling is
        // still alive. `fire_for_owner` zeroes the triggering power, so the
        // combat-end sweep never double-fires it. Enemy deaths only (the
        // player carries no enemy-death-spawn power).
        if dead_side == crate::Side::Enemy {
            crate::enemy_death_effects::fire_for_owner(state, target);
        }
    }

    // Bead `sts2-rs-c7ak`: record per-target hit into the in-flight
    // [`ActiveAttack`] frame (if any). `HandDrill` reads
    // `results.last().blocked > 0 && target.block == 0` to detect a
    // block-break on `after_damage_given`.
    state.record_attack_hit(target, blocked, hp_loss);

    // Bead `sts2-rs-ybtmu` (Group A): power-side `AfterBlockBroken` witness
    // dispatch (BurrowedPower). C# fires `Hook.AfterBlockBroken` per damage
    // result when `WasBlockBroken` is true — `target.Block <= 0 &&
    // blockedDamage > 0` (CreatureCmd.cs:155). Mirror that condition exactly:
    // some damage was absorbed into block (`blocked > 0`) AND the target's
    // block is now depleted. Fired from the same chokepoint as HandDrill's
    // block-break read so it triggers on every attack path (player card,
    // thorns, enemy turn) and on any turn — block-breaks usually happen on
    // the player's turn, before any death. Ordered before the after-damage
    // hooks below, matching the C# result loop (AfterBlockBroken precedes
    // AfterDamageGiven). The owner-mismatch guard inside the Burrowed arm
    // no-ops the (common) player-as-target case.
    // Compute C# `DamageResult.WasBlockBroken` once (CreatureCmd.cs:153):
    // some damage was absorbed into block (`blocked > 0`) AND the target's
    // block is now depleted (`<= 0`). Reused by the power-side
    // AfterBlockBroken dispatch below and the relic-side `after_damage_given`
    // bus (HandDrill) further down.
    let was_block_broken = if blocked > 0 {
        state.creatures[creature_idx].block <= 0
    } else {
        false
    };
    if was_block_broken {
        hooks::dispatch_after_block_broken(state, &hooks::AfterBlockBrokenCtx { broken: target });
    }

    // Bead `sts2-rs-jm6e` (ozy.4-rider): tally a player-as-dealer
    // powered hit for `BeatIntoShape`'s `forge_per_hit` rider
    // (`sts2-rl-agent@01c094d:sts2_env/cards/regent.py:599-607`).
    // Mirrors Python's `record_damage_event` at
    // `sts2_env/core/damage.py:161` (called unconditionally — block
    // is irrelevant) plus the `props.is_powered()` filter applied at
    // read time in `combat.py:1519-1531`. Player-uid view: only
    // increment when the player is the dealer; the only Phase-6
    // consumer (BeatIntoShape) reads the player-totals scalar via
    // [`crate::CombatState::count_player_powered_hits_this_turn`].
    if matches!(dealer, CreatureRef::Player) && props.is_powered() {
        state.player.powered_hits_this_turn = state.player.powered_hits_this_turn.saturating_add(1);
        // Per-target tally for `BeatIntoShape`'s strictly-per-target
        // forge multiplier (C# `BeatIntoShape.cs:27-31` counts
        // `DamageReceivedEntry`s with `e.Receiver == target`). The
        // combat-wide scalar above over-counts in multi-enemy fights;
        // key the count by receiver so the read is exact. Issue #177.
        if let Some(entry) = state
            .player
            .powered_hits_by_target_this_turn
            .iter_mut()
            .find(|(uid, _)| *uid == target)
        {
            entry.1 = entry.1.saturating_add(1);
        } else {
            state
                .player
                .powered_hits_by_target_this_turn
                .push((target, 1));
        }
        // C# `VigorPower.BeforeAttack` latches `commandToModify` on the FIRST
        // powered attack command; thereafter `ModifyDamageAdditive` only boosts
        // that command's card. Mirror it: on the first powered player hit while
        // a live Vigor exists and nothing is latched yet, record the active card
        // (`play.last()`) so subsequent commands this play (e.g. Uproar's
        // auto-play) are excluded. The additive arm already returned `Amount`
        // for THIS first hit (latch was still `None`); latching now scopes the
        // rest. Bead `sts2-rs-vigor-cmd`.
        if state.player.vigor_latch_card.is_none()
            && state.player_power_amount(crate::PowerKind::Vigor) > 0
        {
            state.player.vigor_latch_card = crate::card_effects::resolving_card_uid(state);
        }
    }

    // Necrobinder: tally an Osty-as-dealer attack so Rattle's hit count
    // (`1 + ostyAttacksThisTurn`) scales. C# counts `CreatureAttackedEntry`
    // whose `Actor == Owner.Osty` this turn; the Osty is routed as the dealer
    // `CreatureRef::Enemy(pet_uid)` (see `card_effects::osty_dealer_ref`).
    // Bead `sts2-rs-osty-attacks-this-turn`.
    if let (CreatureRef::Enemy(dealer_uid), Some(pet_uid)) = (dealer, state.player.pet_uid) {
        if dealer_uid == pet_uid {
            state.combat_log.osty_attacks_this_turn =
                state.combat_log.osty_attacks_this_turn.saturating_add(1);
        }
    }

    // Step 6: after-damage event hooks. Bead sts2-rs-3to (P2-2b) wires
    // FlameBarrier's reflective `Effect::DealHpLoss` through the same
    // collect-and-drain pattern as `before_damage_received`.
    // Bead `sts2-rs-6ynt.6.3` threads `hp_loss` so Inferno's
    // post-walk reaction can guard on `damage > 0`.
    // C# `DamageResult.WasFullyBlocked` (CreatureCmd.cs:154): block was
    // INVOLVED (`blocked>0 || target.Block>0`) and no HP got through. Computed
    // before the given-side dispatch so the Envenom / PaperCuts arms can gate
    // on the faithful flag (a 0-damage attack on a 0-block enemy is NOT fully
    // blocked).
    let was_fully_blocked = target_fully_blocked_arm && modified_hp_loss == 0;
    // `target_ref` is always `CreatureRef::Enemy`, so the enemy-target
    // dispatches skip the player-target lanes' kinds in their liveness test
    // (J06).
    // Bead `sts2-rs-ybtmu`: pass the post-block `unblocked` residual (not
    // pre-block `final_damage`) so the Envenom / PaperCuts arms gate on C#'s
    // `UnblockedDamage > 0` — a fully-blocked powered hit triggers neither.
    //
    // The `total_damage` arg is C# `DamageResult.TotalDamage` = `BlockedDamage
    // + UnblockedDamage`, where `UnblockedDamage` is the REALIZED HP loss set by
    // `Creature.LoseHpInternal` AFTER the post-block `ModifyHpLost` pipeline
    // (Slippery / Intangible 1-cap + overkill clamp) — NOT the pre-cap
    // `final_damage`. The two agree for an ordinary hit (`final_damage ==
    // blocked + unblocked` and `hp_loss == unblocked`) but DIVERGE when a
    // HP-loss cap fires: a Slippery(1) Inklet loses 1 HP, so C# TotalDamage =
    // 0 + 1 = 1 and ReaperForm's `DoomPower(TotalDamage * Amount)` is 1, where
    // the pre-cap `final_damage` would wrongly mark Doom 2 (#179, Necrobinder
    // seed 782). Pass the same `blocked + hp_loss` the returned
    // `DamageResult.total_damage` carries; ReaperForm is its only consumer
    // (bead `sts2-rs-4wppb`).
    hooks::dispatch_after_damage_given_enemy_target(
        state,
        &ctx,
        unblocked,
        blocked.saturating_add(hp_loss),
        was_fully_blocked,
    );

    // Bead `sts2-rs-6jk` (j87-a): parallel relic_effects channel. The
    // `DamageGivenCtx` payload uses non-optional `CreatureUid` for both
    // slots, so we skip the dispatch entirely if either ref can't resolve
    // (e.g. an ally dealer at this pin — no ally→uid bridge yet). J01b: and
    // when the channel is dead (no owned handler).
    if relic_channel_live(state, PlannedChannel::AfterDamageGiven) {
        if let (Some(dealer_uid), Some(target_uid)) = (
            dealer.to_creature_uid(state),
            target_ref.to_creature_uid(state),
        ) {
            let dg_ctx = crate::relic_effects::DamageGivenCtx {
                dealer: dealer_uid,
                target: target_uid,
                damage: final_damage,
                props,
                was_block_broken,
            };
            let _ = crate::relic_effects::dispatch_after_damage_given(state, &dg_ctx);
        }
    }

    // CreatureCmd.Damage always runs the given-side hooks, including lethal
    // hits, then runs received only for a nonlethal result or a receiver
    // already healed by an earlier reaction. Death prevention runs later.
    if !just_died || state.creatures[creature_idx].current_hp > 0 {
        hooks::dispatch_after_damage_received_enemy_target(state, &ctx, hp_loss);
    }

    // Bead `sts2-rs-rmrzx`: relic-side `AfterDeath` bus. C# fires
    // `RelicModel.AfterDeath` from `CreatureCmd.Kill` (`CreatureCmd.cs:285`),
    // i.e. AFTER `AfterDamageGiven`/`AfterDamageReceived` for the lethal hit —
    // so this is placed after the damage-given dispatches, not at the early
    // power-side chokepoint above. Gated on `just_died` so it fires exactly
    // once per death (GremlinHorn: enemy death → +1 energy + draw 1).
    if just_died {
        let _ = crate::relic_effects::dispatch_after_death(
            state,
            &hooks::AfterDeathCtx {
                dead: target,
                dead_side,
            },
        );
    }

    // C# `DamageResult` split (`DamageResult.cs`): `UnblockedDamage` is the
    // clamped HP loss, `OverkillDamage` is the residual beyond it on a lethal
    // hit. `modified_hp_loss` is the unclamped post-block, post-`modify_hp_lost`
    // damage that reached HP (= `UnblockedDamage + OverkillDamage`), so the
    // overkill portion is `modified_hp_loss - hp_loss`.
    //
    // Gate it on `just_died`: C# `Creature.LoseHpInternal` (Creature.cs:377-385)
    // sets `OverkillDamage = flag ? max(num - currentHp, 0) : 0` where
    // `flag = CurrentHp > 0 && amount >= CurrentHp` — overkill is reported ONLY
    // when the hit KILLS a LIVING target. Rust's `just_died`
    // (`pre_hp > 0 && current_hp <= 0`) is that exact predicate. Without the
    // gate, a hit on an already-0-HP corpse (Rust retains dead enemies in the
    // array, unlike C#) reported `overkill = modified_hp_loss` (full damage),
    // since `hp_loss` clamps to 0 — which Fisticuffs / Omnislice
    // (`block`/`splash = total_damage + overkill`) turned into an over-grant of
    // the full hit. Bead `sts2-rs-xch4i`.
    let overkill = if just_died {
        (modified_hp_loss - hp_loss).max(0)
    } else {
        0
    };
    DamageOutcome {
        hp_loss,
        blocked,
        overkill,
        // C# `DamageResult.TotalDamage` (`DamageResult.cs:17`:
        // `TotalDamage => BlockedDamage + UnblockedDamage`).
        total_damage: blocked.saturating_add(hp_loss),
    }
}

/// Single-hit entry point: [`compute_modified_damage`] then
/// [`apply_damage_with_final`]. Behaviourally identical to the pre-`8gzvn`
/// monolithic `apply_damage_full`; the split exists only so multi-target
/// callers can stage all `ModifyDamage` calls ahead of the first kill.
/// Like [`apply_damage`] but returns a [`DamageOutcome`] exposing the
/// unclamped post-block damage (incl. overkill) alongside the clamped HP-loss.
pub fn apply_damage_full(
    state: &mut CombatState,
    dealer: CreatureRef,
    target: CreatureUid,
    raw_damage: i32,
    props: ValueProps,
    card: Option<CardId>,
) -> DamageOutcome {
    let final_damage =
        compute_modified_damage_flashing(state, dealer, target, raw_damage, props, card);
    apply_damage_with_final(state, dealer, target, final_damage, props)
}

/// Fractional-base twin of [`compute_modified_damage`]: the relic-additive fold
/// plus [`hooks::modify_damage_frac`], over a RATIONAL base `base_num/base_den`
/// (single floor at the end). Used by Thrash's decimal `Damage.BaseValue`.
/// Bead `sts2-rs-thrash-decimal`.
#[must_use]
pub fn compute_modified_damage_frac(
    state: &CombatState,
    dealer: CreatureRef,
    target: CreatureUid,
    base_num: i64,
    base_den: i64,
    props: ValueProps,
    card: Option<CardId>,
) -> i32 {
    let ctx = DamageCtx {
        dealer,
        target: CreatureRef::Enemy(target),
        props,
    };
    // Relic-additive (MiniatureCannon &c.) reads the integer base; fold it into
    // the rational numerator over the same denominator before the pipeline.
    let int_base = i32::try_from(base_num.div_euclid(base_den)).unwrap_or(i32::MAX);
    let relic_additive = if relic_channel_live(state, PlannedChannel::ModifyDamageAdditive) {
        let relic_additive_md_ctx = crate::relic_effects::ModifyDamageCtx {
            dealer: dealer.to_creature_uid(state),
            target,
            base_damage: int_base,
            props,
            card,
            card_uid: originating_card_uid(state, card),
        };
        crate::relic_effects::dispatch_modify_damage_additive(state, &relic_additive_md_ctx)
    } else {
        0
    };
    let base_num = base_num.saturating_add(i64::from(relic_additive).saturating_mul(base_den));
    hooks::modify_damage_frac(state, base_num, base_den, &ctx)
}

/// Fractional-base twin of [`apply_damage_full`]: composes
/// [`compute_modified_damage_frac`] (rational base → single floor) with
/// [`apply_damage_with_final`] (block / HP / hooks on the resulting integer).
/// Bead `sts2-rs-thrash-decimal`.
pub fn apply_damage_full_frac(
    state: &mut CombatState,
    dealer: CreatureRef,
    target: CreatureUid,
    base_num: i64,
    base_den: i64,
    props: ValueProps,
    card: Option<CardId>,
) -> DamageOutcome {
    let final_damage =
        compute_modified_damage_frac(state, dealer, target, base_num, base_den, props, card);
    apply_damage_with_final(state, dealer, target, final_damage, props)
}

/// Decrement one active [`crate::PowerKind::Buffer`] stack owned by `target`
/// (removing the power at zero), mirroring C#
/// `BufferPower.AfterModifyingHpLostAfterOsty` → `PowerCmd.Decrement`. The
/// owner is resolved via [`hooks::owner_of_listener`] so this matches the
/// gate the `modify_hp_lost_multiplicative` Buffer arm uses. Removal-at-zero
/// clears the power from the global table, the player's legacy list, and
/// every creature's mirror list — the Counter-power teardown shape used by
/// `try_consume_artifact_for_debuff`. No-op when `target` has no active
/// Buffer. Bead `sts2-rs-q6bix.6`.
///
/// Shared with the player-damage path
/// ([`crate::combat_loop::apply_damage_to_player`]), which routes player
/// HP loss outside [`apply_damage_full`] (the latter hardcodes
/// `CreatureRef::Enemy`), so the Lucky-Tonic Buffer decrement has to fire
/// from both sites.
pub(crate) fn decrement_buffer_on_target(state: &mut CombatState, target: CreatureRef) {
    // Perf gate: this runs on every hit that reaches the modify-hp stage;
    // with no live Buffer power the candidate collect below is provably
    // empty, so skip the arena walk entirely.
    if !state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Buffer)
    {
        return;
    }
    // Collect candidate Buffer power ids first to avoid borrowing
    // `state.powers` across the `owner_of_listener` resolution.
    let candidates: smallvec::SmallVec<[std::num::NonZeroU32; 4]> = state
        .powers
        .iter()
        .filter(|p| p.kind == crate::PowerKind::Buffer && p.amount > 0)
        .map(|p| p.id)
        .collect();
    let Some(pid) = candidates.into_iter().find(|&pid| {
        hooks::owner_of_listener(state, hooks::ListenerRef::Power(crate::PowerUid(pid)))
            == Some(target)
    }) else {
        return;
    };
    let mut remove = false;
    if let Some(p) = state.powers.get_mut_by_id(pid) {
        p.amount -= 1;
        remove = p.amount == 0;
    }
    if remove {
        state.powers.retain(|p| p.id != pid);
        state.player.powers.retain(|u| u.0 != pid);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != pid);
        }
    }
}

/// C# `IntangiblePower.AfterModifyingHpLostAfterOsty` (`IntangiblePower.cs:37`)
/// flashes here — the same precedent as [`decrement_buffer_on_target`]'s
/// `BufferPower.AfterModifyingHpLostAfterOsty`: the paired "After" hook takes
/// no target/amount parameter of its own, so C# fires it whenever the
/// hp-lost pipeline ran for a creature that owns the power, regardless of
/// whether the cap actually reduced anything. Rust's `modify_hp_lost` /
/// `modify_hp_lost_pre_osty` are read-only folds and cannot emit, so — like
/// the Buffer decrement — the flash lands post-pipeline, under whatever guard
/// the caller already uses to run the pipeline at all, rather than a guard of
/// its own. Two callers, and their guards are NOT the same predicate:
///
/// - enemy side (`apply_damage_with_final_impl`, `damage.rs:760`):
///   `unblocked > 0 && !bypass_hp_loss_cap`. Both halves are load-bearing.
///   `bypass_hp_loss_cap` is the direct-kill path (C# `LoseHpInternal`) and
///   it skips the ENTIRE `ModifyHpLost` pipeline — no cap, no Buffer absorb,
///   no relic fold (`damage.rs:722`) — so an Intangible or HardenedShell
///   owner taken out by a doom kill must not flash either.
/// - player side ([`crate::combat_loop::apply_damage_to_player`], `:4890`):
///   `unblocked > 0` alone. That path calls `hooks::modify_hp_lost_pre_osty`
///   directly rather than through this module's `apply_damage_with_final`,
///   and has no `bypass_hp_loss_cap` at all (grep: the identifier does not
///   occur anywhere in `combat_loop.rs`).
pub(crate) fn flash_intangible_on_target(state: &mut CombatState, target: CreatureRef) {
    flash_hp_lost_cap_power_on_target(state, target, crate::PowerKind::Intangible);
}

/// [`flash_intangible_on_target`]'s generalization: C# `HardenedShellPower.cs:47`
/// (`AfterModifyingHpLostBeforeOsty`) is the SAME shape of paired "After" hook
/// with no target/amount of its own, tied to `ModifyHpLostBeforeOstyLate`
/// rather than `ModifyHpLostAfterOsty` — but Rust folds BOTH C# methods into
/// the same [`hooks::modify_hp_lost_cap`] per-listener fold (see that arm's
/// `HardenedShell` branch), consumed by the same `modify_hp_lost` /
/// `modify_hp_lost_pre_osty` pipelines. The BeforeOsty/AfterOsty split is a C#
/// timing distinction Rust's single cap fold does not reproduce, so the same
/// post-pipeline flash site serves both powers identically.
pub(crate) fn flash_hardened_shell_on_target(state: &mut CombatState, target: CreatureRef) {
    flash_hp_lost_cap_power_on_target(state, target, crate::PowerKind::HardenedShell);
}

fn flash_hp_lost_cap_power_on_target(
    state: &mut CombatState,
    target: CreatureRef,
    kind: crate::PowerKind,
) {
    if !crate::event_log::armed(state) {
        return;
    }
    if !state.powers.present_kinds().contains(kind) {
        return;
    }
    // Collect candidates first — same reasoning as `decrement_buffer_on_target`.
    let candidates: smallvec::SmallVec<[std::num::NonZeroU32; 4]> = state
        .powers
        .iter()
        .filter(|p| p.kind == kind)
        .map(|p| p.id)
        .collect();
    let owns = candidates.into_iter().any(|pid| {
        hooks::owner_of_listener(state, hooks::ListenerRef::Power(crate::PowerUid(pid)))
            == Some(target)
    });
    if !owns {
        return;
    }
    match target {
        CreatureRef::Player => {
            crate::event_log::flash_player_power(state, kind);
        }
        CreatureRef::Enemy(uid) => {
            crate::event_log::flash_power(state, uid, kind);
        }
        CreatureRef::Ally(_) | CreatureRef::Nobody => {}
    }
}

/// Resolve a [`CreatureUid`] to its index in `state.creatures`. The arena is
/// 1-based (`uid.get() == 1` → index 0) so `Option<CreatureUid>` is
/// niche-packed and `id == 0` is a hard error. Phase 4 (bead sts2-rs-pg5)
/// will replace this with the real `CreatureStore::index_of` once the arena
/// supports stable handles independent of vec position.
///
/// **VTHIN-2 simplification:** at this pin the cultist is at index 0 so
/// the `id` and the index agree. Once enemies can be added/removed mid-
/// combat (P4-3), the arena will need a separate id→index map.
fn creature_index_for(state: &CombatState, target: CreatureUid) -> usize {
    crate::creature_pos(&state.creatures, target.0)
        .unwrap_or_else(|| panic!("apply_damage: creature uid {} not found", target.get()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Creature, IntentHistory, MonsterKind, Power, PowerKind, PowerUid};
    use smallvec::SmallVec;
    use std::num::NonZeroU32;

    fn cultist_uid() -> CreatureUid {
        CreatureUid::new(1).expect("non-zero")
    }

    fn hit_hook_scenario(kind: PowerKind) -> CombatState {
        let mut state = vulnerable_scenario();
        state.powers = crate::PowerStore::default();
        state.powers.push(Power {
            id: NonZeroU32::new(1).unwrap(),
            kind,
            amount: 5,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state
    }

    #[test]
    fn fidelity_hit_hooks_thorns_reflects_lethal_hit_but_not_corpse_hits() {
        let mut state = hit_hook_scenario(PowerKind::Thorns);
        state.creatures[0].current_hp = 3;
        assert_eq!(
            apply_damage(
                &mut state,
                CreatureRef::Player,
                cultist_uid(),
                4,
                ValueProps::MOVE,
                None
            ),
            3
        );
        assert_eq!(
            state.player.current_hp, 45,
            "before-damage Thorns survives a lethal hit"
        );
        // The arena retains the corpse. Dismantle's second hit must not
        // dispatch Thorns, consume block or create another damage event.
        state.creatures[0].block = 7;
        assert_eq!(
            apply_damage(
                &mut state,
                CreatureRef::Player,
                cultist_uid(),
                4,
                ValueProps::MOVE,
                None
            ),
            0
        );
        assert_eq!(
            state.player.current_hp, 45,
            "a later corpse hit has no hooks"
        );
        assert_eq!(state.creatures[0].block, 7);
    }

    #[test]
    fn fidelity_hit_hooks_flame_barrier_skips_lethal_received() {
        for hp in [3, 5] {
            let mut state = hit_hook_scenario(PowerKind::FlameBarrier);
            state.creatures[0].current_hp = hp;
            apply_damage(
                &mut state,
                CreatureRef::Player,
                cultist_uid(),
                4,
                ValueProps::MOVE,
                None,
            );
            assert_eq!(state.player.current_hp, if hp == 3 { 50 } else { 45 });
        }
    }

    #[test]
    fn fidelity_hit_hooks_player_lethal_received_is_skipped() {
        let mut state = hit_hook_scenario(PowerKind::FlameBarrier);
        state.creatures[0].powers.clear();
        state.player.powers.push(PowerUid::new(1).unwrap());
        crate::combat_loop::apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(cultist_uid()),
            50,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(state.player.current_hp, 0);
        assert_eq!(
            state.creatures[0].current_hp, 48,
            "no fatal received-side retaliation"
        );
    }

    #[test]
    #[serial_test::serial(global_registry)]
    fn fidelity_hit_hooks_fatal_received_relics_and_hp_change_are_distinct() {
        crate::relic_effects::demon_tongue::install();
        crate::relic_effects::red_skull::install();
        for reflected in [false, true] {
            let mut state = vulnerable_scenario();
            let _ = state.init_player_creature();
            for (id, kind) in [
                (1, crate::RelicKind::DemonTongue),
                (2, crate::RelicKind::RedSkull),
            ] {
                state.relics.push(crate::Relic {
                    id: NonZeroU32::new(id).unwrap(),
                    kind,
                    counter: 0,
                    is_wax: false,
                    is_melted: false,
                });
            }
            if reflected {
                hooks::apply_effect(
                    &mut state,
                    hooks::Effect::DealHpLoss {
                        dealer: CreatureRef::Enemy(cultist_uid()),
                        target: CreatureRef::Player,
                        amount: 50,
                        props: ValueProps::UNPOWERED,
                    },
                );
            } else {
                crate::combat_loop::apply_damage_to_player(
                    &mut state,
                    CreatureRef::Enemy(cultist_uid()),
                    50,
                    ValueProps::MOVE,
                    None,
                );
            }
            assert_eq!(
                state.player.current_hp, 0,
                "Demon Tongue must not save a fatal hit"
            );
            assert_eq!(state.relics[0].counter, 0, "received relic never triggered");
            assert_eq!(
                state.relics[1].counter, 1,
                "Red Skull observes the HP change even on death"
            );
            // A subsequent effect must not re-enter any damage stage on
            // the dead player (the ordinary path would otherwise call
            // received with WasTargetKilled=false and Demon Tongue heals).
            state.player.block = 7;
            if reflected {
                hooks::apply_effect(
                    &mut state,
                    hooks::Effect::DealHpLoss {
                        dealer: CreatureRef::Enemy(cultist_uid()),
                        target: CreatureRef::Player,
                        amount: 50,
                        props: ValueProps::UNPOWERED,
                    },
                );
            } else {
                crate::combat_loop::apply_damage_to_player(
                    &mut state,
                    CreatureRef::Enemy(cultist_uid()),
                    50,
                    ValueProps::MOVE,
                    None,
                );
            }
            assert_eq!(state.player.current_hp, 0);
            assert_eq!(
                state.player.block, 7,
                "corpse damage does not consume block"
            );
        }
    }

    #[test]
    fn fidelity_hit_hooks_given_still_fires_on_lethal_hit() {
        let mut state = hit_hook_scenario(PowerKind::PaperCuts);
        let _ = state.init_player_creature();
        crate::combat_loop::apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(cultist_uid()),
            50,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(state.player.current_hp, 0);
        assert_eq!(
            state.player.max_hp, 45,
            "given-side Paper Cuts still observes the lethal result"
        );
    }

    /// Build a state with a primary player at 50/50 and one Cultist at 48/48
    /// with a Vulnerable(2) power attached. Mirrors the VTHIN-2 fixture's
    /// initial state so the unit test exercises the same numeric path the
    /// parity harness asserts byte-equality against.
    fn vulnerable_scenario() -> CombatState {
        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        state.player.energy = 0;
        state.player.max_energy = 3;

        let power_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: power_uid.0,
            kind: PowerKind::Vulnerable,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });

        let mut cultist_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        cultist_powers.push(power_uid);
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: cultist_powers,
            species: Some(MonsterKind::Nibbit),
            // VTHIN-2 fixture intentionally has `ai is None` on the
            // cultist — see CASS b-motyob3q-wtg5g0 finding (3). Setting
            // intent=None here keeps the parity exporter from emitting
            // intent / intent_damage / intent_hits fields, preserving
            // the byte-equality assertion in the parity harness.
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());
        state
    }

    /// 6 raw damage → Vulnerable (×1.5) → 9 final damage → Cultist 48 - 9 = 39.
    /// This is the canonical VTHIN-2 numeric assertion: the rounding-audit
    /// invariant locked in by 58w (`floor(6 * 3/2) = 9`) survives end-to-end
    /// through the dispatcher walk over the cultist's powers segment of
    /// `iter_power_listeners`.
    #[test]
    fn vulnerable_amplifies_six_to_nine() {
        let mut state = vulnerable_scenario();
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 9);
        assert_eq!(state.creatures[0].current_hp, 39);
        assert_eq!(state.creatures[0].block, 0);
    }

    /// Bead `sts2-rs-8gzvn`: the `compute_modified_damage` +
    /// `apply_damage_with_final` split must be behaviour-preserving — the
    /// two-call form an AoE caller uses has to land the SAME final damage as
    /// the monolithic `apply_damage_full`. Locks the VTHIN-2 numeric path
    /// (6 raw → Vulnerable ×1.5 → 9) through the split so a future refactor of
    /// either half can't silently drift the AoE pre-compute off the single-hit
    /// pipeline.
    #[test]
    fn compute_then_apply_matches_apply_damage_full() {
        // `compute_modified_damage` is read-only: it returns 9 without
        // touching the cultist's HP.
        let state_ro = vulnerable_scenario();
        let final_damage = compute_modified_damage(
            &state_ro,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(final_damage, 9);
        assert_eq!(
            state_ro.creatures[0].current_hp, 48,
            "compute must not mutate"
        );

        // Applying the pre-computed 9 lands exactly the monolithic result.
        let mut split = vulnerable_scenario();
        let out = apply_damage_with_final(
            &mut split,
            CreatureRef::Player,
            cultist_uid(),
            final_damage,
            ValueProps::MOVE,
        );
        assert_eq!(out.hp_loss, 9);
        assert_eq!(split.creatures[0].current_hp, 39);
    }

    /// Surfaced by a cargo-mutants survey (2026-06-19): the fractional-base
    /// twin `compute_modified_damage_frac` (Thrash's decimal
    /// `Damage.BaseValue`, bead `sts2-rs-thrash-decimal`) had NO value-pinning
    /// test — replacing its whole body with a constant `0`/`1`/`-1` survived
    /// the suite while the integer twin's identical mutants were all caught.
    /// Pin it two ways: (1) with `base_den == 1` it must equal the integer
    /// twin on the VTHIN-2 path (6 → Vulnerable ×1.5 → 9); (2) a genuinely
    /// fractional base must defer the single floor to the end —
    /// `floor(6.5 × 3/2) = floor(9.75) = 9`, which an early-floored
    /// (`7.5 → 7`) or constant implementation could not reproduce.
    #[test]
    fn compute_modified_damage_frac_pins_fractional_base() {
        let state = vulnerable_scenario();

        // (1) `base_den == 1` is the integer twin, exactly.
        let int_twin = compute_modified_damage(
            &state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        let frac_den1 = compute_modified_damage_frac(
            &state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            1,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(frac_den1, int_twin, "den=1 must match the integer twin");
        assert_eq!(frac_den1, 9);

        // (2) genuinely fractional base 13/2 = 6.5; Vulnerable ×1.5 folded in
        // rational space then floored ONCE → floor(9.75) = 9.
        let frac = compute_modified_damage_frac(
            &state,
            CreatureRef::Player,
            cultist_uid(),
            13,
            2,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(frac, 9, "floor(6.5 * 3/2) = floor(9.75) = 9");

        // `compute_*` is read-only: no HP mutation.
        assert_eq!(state.creatures[0].current_hp, 48, "compute must not mutate");
    }

    /// Block absorbs damage before HP loss; residual hits HP.
    #[test]
    fn block_absorbs_then_hp_loses() {
        let mut state = vulnerable_scenario();
        state.creatures[0].block = 5;
        // 6 raw → 9 after Vulnerable → 5 absorbed by block → 4 hp loss.
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 4);
        assert_eq!(state.creatures[0].block, 0);
        assert_eq!(state.creatures[0].current_hp, 44);
    }

    /// HP cannot go below zero.
    #[test]
    fn hp_clamps_at_zero() {
        let mut state = vulnerable_scenario();
        state.creatures[0].current_hp = 5;
        // 6 raw → 9 after Vulnerable → 5 hp loss (clamped, even though
        // unblocked damage was 9). `current_hp` ends at 0.
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 5);
        assert_eq!(state.creatures[0].current_hp, 0);
    }

    /// Bead `sts2-rs-xch4i`: C# `Creature.LoseHpInternal` gates
    /// `OverkillDamage` on `flag = CurrentHp > 0 && amount >= CurrentHp`, so a
    /// hit on an already-dead (0-HP) corpse reports ZERO overkill — not the
    /// full damage. Rust retains corpses in the array, so the pre-gate
    /// `modified_hp_loss - hp_loss` over-reported overkill = the whole hit,
    /// which Fisticuffs / Omnislice (`total_damage + overkill`) over-granted.
    #[test]
    fn overkill_is_zero_when_hitting_a_corpse() {
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 0, // already dead
            max_hp: 48,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());
        let out = apply_damage_full(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            10,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            out.overkill, 0,
            "no overkill on a 0-HP corpse (flag CurrentHp>0)"
        );
        assert_eq!(out.total_damage, 0, "no HP to lose → TotalDamage 0");
    }

    /// The gate must NOT suppress legitimate lethal overkill: a LIVING target
    /// killed by the hit still reports the excess (C# `flag` is true here).
    #[test]
    fn overkill_reported_on_living_lethal_hit() {
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 4,
            max_hp: 48,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());
        let out = apply_damage_full(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            10,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(out.total_damage, 4, "4 HP lost");
        assert_eq!(out.overkill, 6, "10 dealt − 4 HP = 6 overkill");
    }

    /// Bead `sts2-rs-8rn0c`: `ValueProps::UNBLOCKABLE` skips block
    /// absorption entirely (damage.rs:141-148) — block stays untouched
    /// and full damage routes to HP. Mirrors Python damage.py:135-137.
    #[test]
    fn unblockable_bypasses_block_entirely() {
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 8,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());

        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            10,
            ValueProps::MOVE.union(ValueProps::UNBLOCKABLE),
            None,
        );
        assert_eq!(hp_loss, 10, "full damage routes to HP under UNBLOCKABLE");
        assert_eq!(
            state.creatures[0].block, 8,
            "block untouched under UNBLOCKABLE"
        );
        assert_eq!(state.creatures[0].current_hp, 38);
    }

    /// Control for `unblockable_bypasses_block_entirely`: the same hit
    /// without the UNBLOCKABLE flag must absorb block first.
    #[test]
    fn non_unblockable_absorbs_block_first() {
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 8,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());

        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            10,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 2, "8 absorbed by block, 2 to HP");
        assert_eq!(state.creatures[0].block, 0);
        assert_eq!(state.creatures[0].current_hp, 46);
    }

    /// Bead `sts2-rs-gdyrs`: with no in-flight `ActiveAttack` frame,
    /// `apply_damage`'s `record_attack_hit` call (damage.rs:193) is a
    /// no-op — `state.active_attack` stays `None`.
    #[test]
    fn record_attack_hit_is_noop_without_active_frame() {
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());
        assert!(state.active_attack().is_none());

        let _ = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        assert!(
            state.active_attack().is_none(),
            "no frame opened by apply_damage; recording is a no-op"
        );
    }

    /// Bead `sts2-rs-gdyrs`: when an `ActiveAttack` frame is in flight,
    /// `apply_damage` appends one `AttackResult` per hit with the
    /// correct `blocked` + `hp_loss` so `HandDrill`'s block-break
    /// listener can read `results.last()`. Cover both block-absorption
    /// and the post-hit block-break detection gate
    /// (`blocked > 0 && target.block == 0`).
    #[test]
    fn record_attack_hit_appends_blocked_and_hp_loss_to_frame() {
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 8,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());
        state.begin_attack(None, Some(cultist_uid()));

        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            10,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 2);

        let frame = state
            .active_attack()
            .expect("frame still in flight after one hit");
        assert_eq!(frame.results.len(), 1);
        let r = frame.results[0];
        assert_eq!(r.target, cultist_uid());
        assert_eq!(r.blocked, 8, "8 absorbed by block before HP");
        assert_eq!(r.hp_loss, 2);
        // HandDrill block-break gate: blocked > 0 AND post-hit block == 0.
        assert!(r.blocked > 0);
        assert_eq!(state.creatures[0].block, 0);
    }

    /// With no Vulnerable, raw damage passes through unchanged.
    #[test]
    fn no_vulnerable_no_amplification() {
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());

        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 6);
        assert_eq!(state.creatures[0].current_hp, 42);
    }

    /// **VTHIN-2 byte-equality preserved (P2-4 sts2-rs-nj8):** the
    /// canonical Cultist + Vulnerable + 6 damage scenario must produce
    /// identical numbers (cultist HP 48 → 39, `hp_loss=9`) AFTER inserting
    /// the `modify_hp_lost` step into `apply_damage`. Phase-2 has no
    /// active `modify_hp_lost` listeners, so the chain is identity and the
    /// observable end state is unchanged from VTHIN-2.
    ///
    /// This is the load-bearing parity assertion for nj8 — the
    /// `parity_harness::vthin2_vulnerable_damage_matches_python` test
    /// covers the same path but at the JSON-serialization level; this
    /// unit test isolates the `apply_damage` primitive itself.
    #[test]
    fn vthin2_byte_equality_preserved_after_modify_hp_lost_insertion() {
        let mut state = vulnerable_scenario();
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        // Identical to VTHIN-2 invariants:
        assert_eq!(hp_loss, 9, "VTHIN-2 expected 9 hp loss");
        assert_eq!(
            state.creatures[0].current_hp, 39,
            "VTHIN-2 expected cultist HP 48 - 9 = 39"
        );
        assert_eq!(state.creatures[0].block, 0);
    }

    /// **Zero damage short-circuits the `modify_hp_lost` pipeline:**
    /// mirrors Python's `if combat is not None and remaining > 0` guard at
    /// `damage.py:140-141`. With a fully-blocked hit (block ≥ damage), the
    /// HP-loss chain is bypassed entirely (matching Python). Once Phase-7
    /// listeners exist, this short-circuit prevents listeners from firing
    /// on a no-op hit.
    #[test]
    fn modify_hp_lost_short_circuits_on_zero_unblocked() {
        let mut state = vulnerable_scenario();
        state.creatures[0].block = 100; // way more than 9 amplified damage
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 0);
        assert_eq!(state.creatures[0].current_hp, 48);
        // Block consumed the 9 amplified damage; 91 block remaining.
        assert_eq!(state.creatures[0].block, 91);
    }

    /// **No-listener pipeline is identity:** without any active
    /// `modify_hp_lost` listeners, the chain returns its input unchanged
    /// (additive=0, mul=ONE, cap=MAX). We exercise this through
    /// `apply_damage` with raw damage that bypasses block — the unblocked
    /// amount equals the HP loss.
    #[test]
    fn modify_hp_lost_no_listener_passthrough() {
        let mut state = vulnerable_scenario();
        // Strip Vulnerable so the damage pipeline is identity too.
        state.creatures[0].powers.clear();
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            7,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 7);
        assert_eq!(state.creatures[0].current_hp, 41);
    }

    // -----------------------------------------------------------------
    // P2-2c sts2-rs-v7d: DamageCtx context expansion via apply_damage.
    // -----------------------------------------------------------------

    /// **`apply_damage` round-trips `props` through `DamageCtx`**: with an
    /// unpowered (`ValueProps::NONE`) hit, Vulnerable's `is_powered` guard
    /// fails → no 3/2 multiplier → 6 raw damage stays at 6 (cultist
    /// 48 - 6 = 42), instead of the powered 9. Locks that the props
    /// argument actually flows through the listener arm.
    #[test]
    fn apply_damage_signature_round_trips_props_through_ctx() {
        let mut state = vulnerable_scenario();
        // Unpowered hit → Vulnerable does NOT fire → 6 raw stays 6.
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::NONE,
            None,
        );
        assert_eq!(
            hp_loss, 6,
            "non-MOVE props should bypass Vulnerable: 6 raw stays 6"
        );
        assert_eq!(state.creatures[0].current_hp, 42);
    }

    /// **Wrong dealer bypasses Strength**: Strength on the player +
    /// `apply_damage` with an enemy as the dealer → Strength does NOT
    /// fire (would have added +amount). The dealer field actually flows
    /// through.
    #[test]
    fn apply_damage_routes_dealer_through_to_strength_guard() {
        let mut state = CombatState::default();
        // Player has Strength(3).
        let str_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: str_uid.0,
            kind: PowerKind::Strength,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(str_uid);
        // Cultist with no powers.
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());

        // Enemy attacks (dealer != player) → Strength does NOT add +3.
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Enemy(CreatureUid::new(2).unwrap()),
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            hp_loss, 6,
            "Strength on player should NOT buff enemy attacks (dealer guard)"
        );
        assert_eq!(state.creatures[0].current_hp, 42);
    }

    // -----------------------------------------------------------------
    // P2-2b sts2-rs-3to: deferred-effect queue end-to-end behaviors.
    //
    // These tests exercise the full `apply_damage` → `dispatch_*` →
    // `apply_effects` chain to confirm Thorns / FlameBarrier / Poison /
    // Artifact produce the right state mutations after the listener walk
    // completes. Each test mirrors a Python fixture path documented in the
    // per-power Python source citations in `hooks.rs` listener arms.
    // -----------------------------------------------------------------

    /// **Thorns reflects HP-loss damage to the attacker.** With Thorns(3) on
    /// a cultist + a powered attack from the player for 5, the cultist takes
    /// 5 damage AND the player takes 3 HP-loss reflective damage. Mirrors
    /// Python `damage_reactions.py:47-64`.
    ///
    /// **Recursion semantics**: the reflective hit on the player carries
    /// `UNPOWERED | SKIP_HURT_ANIM`, so any Thorns the player might have
    /// would NOT re-fire (its listener guard short-circuits on
    /// `props.is_powered()`). Verified by adding Thorns(2) to the player
    /// in a sibling test.
    #[test]
    fn thorns_deals_reflective_hp_loss_to_attacker() {
        use crate::PowerUid;
        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        // Cultist with Thorns(3).
        let thorns_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: thorns_uid.0,
            kind: PowerKind::Thorns,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let mut cultist_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        cultist_powers.push(thorns_uid);
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: cultist_powers,
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());

        // Powered attack from player: 5 damage on cultist; Thorns reflects 3
        // back to player.
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            5,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 5, "cultist takes 5 (no Vulnerable)");
        assert_eq!(state.creatures[0].current_hp, 48 - 5);
        assert_eq!(
            state.player.current_hp,
            50 - 3,
            "player takes 3 HP-loss from Thorns reflection"
        );
    }

    /// **Thorns recursion termination**: even when the attacker also has
    /// Thorns, the reflective-damage chain terminates at depth 1 because
    /// the reflected hit carries `UNPOWERED` — Thorns' listener guards on
    /// `props.is_powered()` short-circuits before emitting another reflection.
    /// Mirrors Python's recursion semantics in `damage.py:106` +
    /// `damage_reactions.py:58`.
    #[test]
    fn thorns_does_not_recursively_trigger_attacker_thorns() {
        use crate::PowerUid;
        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        // Player has Thorns(7) — should NOT fire on the cultist's reflective hit.
        let player_thorns_uid = PowerUid::new(2).expect("non-zero");
        state.powers.push(Power {
            id: player_thorns_uid.0,
            kind: PowerKind::Thorns,
            amount: 7,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(player_thorns_uid);

        // Cultist with Thorns(3).
        let cultist_thorns_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: cultist_thorns_uid.0,
            kind: PowerKind::Thorns,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let mut cultist_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        cultist_powers.push(cultist_thorns_uid);
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: cultist_powers,
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());

        // Player attacks for 5 → cultist takes 5 → Thorns(3) reflects to
        // player (UNPOWERED) → player takes 3 → player's Thorns(7) does NOT
        // fire (UNPOWERED guard) → no recursion to cultist.
        apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            5,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(state.creatures[0].current_hp, 48 - 5, "cultist took 5");
        assert_eq!(
            state.player.current_hp,
            50 - 3,
            "player took 3 from cultist's Thorns; player's Thorns(7) did NOT re-reflect"
        );
    }

    /// **`FlameBarrier` reflects damage on `after_damage_received`.** Same
    /// shape as Thorns but fires AFTER the HP write. With `FlameBarrier(5)`
    /// on a cultist + a powered attack from the player for 4, the cultist
    /// takes 4 AND the player takes 5 HP-loss reflective damage.
    #[test]
    fn flame_barrier_reflects_damage_when_attacked() {
        use crate::PowerUid;
        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        // Cultist with FlameBarrier(5).
        let fb_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: fb_uid.0,
            kind: PowerKind::FlameBarrier,
            amount: 5,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let mut cultist_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        cultist_powers.push(fb_uid);
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: cultist_powers,
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());

        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            4,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 4, "cultist takes 4 (no Vulnerable)");
        assert_eq!(state.creatures[0].current_hp, 48 - 4);
        assert_eq!(
            state.player.current_hp,
            50 - 5,
            "player takes 5 HP-loss from FlameBarrier reflection"
        );
    }

    /// **Poison ticks the target at turn-start AND decrements amount.**
    /// Direct dispatcher test — uses `dispatch_after_side_turn_start`
    /// against a player with Poison(4). After firing on `Side::Player` turn
    /// start: player HP -= 4 AND `Poison.amount` becomes 3.
    /// Mirrors Python `turn_effects.py::PoisonPower.after_side_turn_start:491-511`.
    #[test]
    fn poison_ticks_player_at_turn_start_and_decrements_amount() {
        use crate::hooks::{self, TurnSideCtx};
        use crate::{PowerUid, Side};
        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        let poison_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: poison_uid.0,
            kind: PowerKind::Poison,
            amount: 4,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(poison_uid);

        hooks::dispatch_after_side_turn_start(&mut state, &TurnSideCtx { side: Side::Player });

        assert_eq!(
            state.player.current_hp,
            50 - 4,
            "player should take 4 HP-loss from Poison tick (UNBLOCKABLE | UNPOWERED)"
        );
        // Amount tick-down: 4 -> 3.
        let amount = state
            .powers
            .iter()
            .find(|p| p.id == poison_uid.0)
            .unwrap()
            .amount;
        assert_eq!(amount, 3, "Poison amount should decrement from 4 to 3");
    }

    /// **Poison on enemy ticks on enemy turn start, NOT player turn start.**
    /// Mirrors Python's `if side != owner.side: return` guard.
    #[test]
    fn poison_on_enemy_does_not_tick_on_player_turn_start() {
        use crate::hooks::{self, TurnSideCtx};
        use crate::{PowerUid, Side};
        let mut state = CombatState::default();
        let poison_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: poison_uid.0,
            kind: PowerKind::Poison,
            amount: 4,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let mut cultist_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        cultist_powers.push(poison_uid);
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: cultist_powers,
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());

        // Player turn start: Poison on enemy should NOT fire.
        hooks::dispatch_after_side_turn_start(&mut state, &TurnSideCtx { side: Side::Player });
        assert_eq!(
            state.creatures[0].current_hp, 48,
            "cultist HP unchanged on player turn start"
        );
        let amount = state
            .powers
            .iter()
            .find(|p| p.id == poison_uid.0)
            .unwrap()
            .amount;
        assert_eq!(
            amount, 4,
            "Poison amount unchanged on wrong-side turn start"
        );

        // Enemy turn start: NOW Poison fires.
        hooks::dispatch_after_side_turn_start(&mut state, &TurnSideCtx { side: Side::Enemy });
        assert_eq!(
            state.creatures[0].current_hp,
            48 - 4,
            "cultist took 4 Poison"
        );
        let amount = state
            .powers
            .iter()
            .find(|p| p.id == poison_uid.0)
            .unwrap()
            .amount;
        assert_eq!(amount, 3, "Poison amount decremented after firing");
    }

    /// **Effect queue order: Thorns reflects before `FlameBarrier`.** With
    /// multiple reflective-damage listeners on the same target, the player
    /// takes the sum of both reflections. Order: `before_damage_received`
    /// fires Thorns, `after_damage_received` fires `FlameBarrier` — both
    /// reflections land deterministically.
    #[test]
    fn effect_queue_order_thorns_before_flame_barrier() {
        use crate::PowerUid;
        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        // Cultist with Thorns(2) AND FlameBarrier(3).
        let thorns_uid = PowerUid::new(1).expect("non-zero");
        let fb_uid = PowerUid::new(2).expect("non-zero");
        state.powers.push(Power {
            id: thorns_uid.0,
            kind: PowerKind::Thorns,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.powers.push(Power {
            id: fb_uid.0,
            kind: PowerKind::FlameBarrier,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let mut cultist_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        cultist_powers.push(thorns_uid);
        cultist_powers.push(fb_uid);
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: cultist_powers,
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());

        apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            5,
            ValueProps::MOVE,
            None,
        );
        // Thorns reflects 2 BEFORE the HP write; FlameBarrier reflects 3
        // AFTER. Total reflective damage on player: 2 + 3 = 5.
        assert_eq!(
            state.player.current_hp,
            50 - 2 - 3,
            "player should take 2 (Thorns) + 3 (FlameBarrier) reflective"
        );
        // Cultist still takes the original 5 damage.
        assert_eq!(state.creatures[0].current_hp, 48 - 5);
    }

    /// **VTHIN-2 byte-equality preserved**: the canonical Cultist +
    /// Vulnerable + 6 damage scenario produces identical numbers
    /// (cultist HP 48 → 39, `hp_loss=9`) AFTER the deferred-effect-queue
    /// refactor. Phase-2's canonical fixture has no reflective-damage
    /// listeners, so the effect queue is always empty and the numeric
    /// path is unchanged from VTHIN-2 / nj8.
    #[test]
    fn vthin2_byte_equality_preserved_after_3to_refactor() {
        let mut state = vulnerable_scenario();
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 9, "VTHIN-2 expected 9 hp loss");
        assert_eq!(
            state.creatures[0].current_hp, 39,
            "VTHIN-2 expected cultist HP 48 - 9 = 39"
        );
        assert_eq!(state.creatures[0].block, 0);
    }

    /// Bead `sts2-rs-12xvk`: SlipperyPower caps incoming damage at 1
    /// when target == owner, and consumes 1 stack per landed hit. Per
    /// C# `MegaCrit.Sts2.Core.Models.Powers/SlipperyPower.cs`.
    fn slippery_scenario(stacks: i32) -> (CombatState, CreatureUid) {
        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        state.player.energy = 0;
        state.player.max_energy = 3;

        let power_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: power_uid.0,
            kind: PowerKind::Slippery,
            amount: stacks,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });

        let mut vantom_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        vantom_powers.push(power_uid);
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 173,
            max_hp: 173,
            block: 0,
            powers: vantom_powers,
            species: Some(MonsterKind::Vantom),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Vantom"),
            ai: None,
        });
        let uid = CreatureUid::new(1).expect("non-zero");
        state.enemy_uids.push(uid);
        (state, uid)
    }

    /// **`SurroundedPower` back-attack multiplier** (`sts2-rs-ybtmu`): the
    /// surrounded player takes 1.5× damage from the arm on its *back* (the
    /// side opposite its facing), 1× from the arm it faces. Flipping the
    /// facing swaps which arm gets the bonus.
    #[test]
    fn surrounded_amplifies_back_attack_by_one_and_a_half() {
        use crate::PowerUid;
        let mut state = CombatState::default();
        state.player.current_hp = 200;
        state.player.max_hp = 200;
        let player_uid = state.init_player_creature();

        // Player carries Surrounded facing Right (counter == 0).
        let surrounded = PowerUid::new(1).expect("nz");
        state.powers.push(Power {
            id: surrounded.0,
            kind: PowerKind::Surrounded,
            amount: 1,
            counter: 0, // SURROUNDED_FACING_RIGHT
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(surrounded);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == player_uid.0) {
            c.powers.push(surrounded);
            c.block = 0;
        }

        // Crusher (BackAttackLeft) and Rocket (BackAttackRight).
        let bal = PowerUid::new(2).expect("nz");
        let bar = PowerUid::new(3).expect("nz");
        state.powers.push(Power {
            id: bal.0,
            kind: PowerKind::BackAttackLeft,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.powers.push(Power {
            id: bar.0,
            kind: PowerKind::BackAttackRight,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let mut crusher_p: SmallVec<[PowerUid; 8]> = SmallVec::new();
        crusher_p.push(bal);
        state.creatures.push(Creature {
            id: NonZeroU32::new(10).expect("nz"),
            current_hp: 200,
            max_hp: 200,
            block: 0,
            powers: crusher_p,
            species: Some(MonsterKind::Crusher),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Crusher"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid::new(10).expect("nz"));
        let mut rocket_p: SmallVec<[PowerUid; 8]> = SmallVec::new();
        rocket_p.push(bar);
        state.creatures.push(Creature {
            id: NonZeroU32::new(11).expect("nz"),
            current_hp: 200,
            max_hp: 200,
            block: 0,
            powers: rocket_p,
            species: Some(MonsterKind::Rocket),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Rocket"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid::new(11).expect("nz"));

        let crusher = CreatureRef::Enemy(CreatureUid::new(10).expect("nz"));
        let rocket = CreatureRef::Enemy(CreatureUid::new(11).expect("nz"));
        // Enemy-attacks-player must route through `apply_damage_to_player`,
        // which builds `ctx.target = CreatureRef::Player` (the bare
        // `apply_damage` helper hard-codes `Enemy(target)`). Measure realized
        // damage via the player's HP delta, resetting HP + block each round.
        let hit_player = |state: &mut CombatState, dealer: CreatureRef| -> i32 {
            state.player.current_hp = 200;
            state.player.block = 0;
            if let Some(c) = state.creatures.iter_mut().find(|c| c.id == player_uid.0) {
                c.current_hp = 200;
                c.block = 0;
            }
            let before = state.player.current_hp;
            let _ = crate::combat_loop::apply_damage_to_player(
                state,
                dealer,
                10,
                ValueProps::MOVE,
                None,
            );
            before - state.player.current_hp
        };

        // Facing Right: the LEFT arm (Crusher) is at the player's back → 1.5×.
        assert_eq!(
            hit_player(&mut state, crusher),
            15,
            "facing Right: Crusher back-attacks for 1.5×"
        );
        // The faced arm (Rocket) deals normal damage.
        assert_eq!(
            hit_player(&mut state, rocket),
            10,
            "facing Right: faced Rocket deals 1×"
        );

        // Flip facing to Left: now Rocket is at the player's back.
        if let Some(p) = state.powers.get_mut_by_id(surrounded.0) {
            p.counter = 1; // SURROUNDED_FACING_LEFT
        }
        assert_eq!(
            hit_player(&mut state, rocket),
            15,
            "facing Left: Rocket back-attacks for 1.5×"
        );
        assert_eq!(
            hit_player(&mut state, crusher),
            10,
            "facing Left: faced Crusher deals 1×"
        );
    }

    #[test]
    fn slippery_caps_single_hit_at_one_damage() {
        let (mut state, vantom_uid) = slippery_scenario(9);
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            vantom_uid,
            11, // Breakthrough+Inflame at floor 17
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 1, "Slippery caps damage at 1");
        assert_eq!(state.creatures[0].current_hp, 172, "173 - 1 = 172");
        assert_eq!(
            state.powers[0].amount, 8,
            "1 stack consumed by the landed hit (9 → 8)"
        );
    }

    /// `#179` (Necrobinder seed 782): ReaperForm's `DoomPower(TotalDamage *
    /// Amount)` must read C# `DamageResult.TotalDamage` = `BlockedDamage +
    /// realized-HP-loss`, NOT the pre-cap `final_damage`. A Slippery(1) target
    /// caps its HP loss to 1, so a powered 5-damage hit yields TotalDamage 1 →
    /// Doom 1, where the pre-cap value would wrongly mark Doom 5.
    #[test]
    fn reaper_form_doom_reads_slippery_capped_total_not_precap_damage() {
        let (mut state, vantom_uid) = slippery_scenario(9);
        // Player carries ReaperForm(1); the after_damage_given arm reads it via
        // `player_power_uids()`, which unions in `state.player.powers`.
        let reaper_uid = PowerUid::new(2).expect("non-zero");
        state.powers.push(Power {
            id: reaper_uid.0,
            kind: PowerKind::ReaperForm,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(reaper_uid);

        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            vantom_uid,
            5, // pre-cap powered damage
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 1, "Slippery caps the realized HP loss at 1");

        // The Vantom must carry exactly one Doom stack of amount 1 (= capped
        // TotalDamage 0 blocked + 1 hp_loss), not the pre-cap 5.
        let doom_amount: i32 = state.creatures[0]
            .powers
            .iter()
            .filter_map(|u| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == u.0 && p.kind == PowerKind::Doom)
            })
            .map(|p| p.amount)
            .sum();
        assert_eq!(
            doom_amount, 1,
            "ReaperForm Doom = capped TotalDamage (1), not pre-cap final_damage (5)"
        );
    }

    #[test]
    fn slippery_consumes_one_stack_per_hit_across_multiple_attacks() {
        let (mut state, vantom_uid) = slippery_scenario(3);
        for _ in 0..3 {
            let hp_loss = apply_damage(
                &mut state,
                CreatureRef::Player,
                vantom_uid,
                100,
                ValueProps::MOVE,
                None,
            );
            assert_eq!(hp_loss, 1, "each hit capped at 1 while Slippery > 0");
        }
        // Bead `sts2-rs-2w966`: the power is removed once its stack reaches 0
        // (C# `ShouldRemoveDueToAmount`), so no Slippery instance remains.
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Slippery),
            "all 3 stacks consumed → Slippery removed at 0"
        );
        // 4th hit lands at full damage now
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            vantom_uid,
            10,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            hp_loss, 10,
            "Slippery at 0 no longer caps — full damage lands"
        );
    }

    /// **v0.107.1 Slippery retiming (`§1a.8`, bead `…wb8lk.1`).** Slippery no
    /// longer caps the *pre-block* damage at 1; it caps the *post-block HP
    /// loss* at 1 (`SlipperyPower.cs::ModifyHpLostAfterOsty`). A partially
    /// blocked hit therefore soaks the full damage into Block first, then the
    /// leftover HP loss is clamped to 1. (v0.103.2 capped the damage to 1
    /// pre-block, so Block barely depleted.)
    #[test]
    fn slippery_v0107_caps_hp_loss_after_block_not_pre_block_damage() {
        let (mut state, vantom_uid) = slippery_scenario(9);
        state.creatures[0].block = 5;
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            vantom_uid,
            11,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 1, "6 unblocked → HP loss capped at 1");
        assert_eq!(
            state.creatures[0].block, 0,
            "full 11 hits Block first (5 → 0), NOT capped to 1 pre-block"
        );
        assert_eq!(state.creatures[0].current_hp, 172, "173 - 1 = 172");
        assert_eq!(
            state.powers[0].amount, 8,
            "stack consumed: UnblockedDamage(6) >= 1"
        );
    }

    /// **v0.107.1 Slippery + fully-blocked hit.** When Block fully absorbs the
    /// hit (`UnblockedDamage == 0`) the stack is NOT consumed — v0.107.1
    /// changed the consume guard from `TotalDamage != 0` to `UnblockedDamage
    /// >= 1`. (v0.103.2 capped damage to 1, so a blocked hit still showed
    /// `TotalDamage == 1` and consumed a stack.)
    #[test]
    fn slippery_v0107_fully_blocked_hit_does_not_consume_stack() {
        let (mut state, vantom_uid) = slippery_scenario(9);
        state.creatures[0].block = 20;
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            vantom_uid,
            11,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 0, "fully blocked → 0 HP loss");
        assert_eq!(
            state.creatures[0].block, 9,
            "20 - 11 = 9 (full hit on Block)"
        );
        assert_eq!(state.creatures[0].current_hp, 173, "no HP lost");
        assert_eq!(
            state.powers[0].amount, 9,
            "no stack consumed when UnblockedDamage == 0"
        );
    }

    /// **Multi-hit attack: per-hit Vulnerable amplification + Thorns
    /// reflection.** A multi-hit card runs one `apply_damage` per hit, so
    /// each hit must independently (a) be amplified by the target's
    /// Vulnerable — C# `VulnerablePower.ModifyDamageMultiplicative` (×1.5,
    /// fires on every damage event) — and (b) trigger the target's Thorns
    /// reflection back to the attacker — C# `ThornsPower.BeforeDamageReceived`
    /// reflects `Amount` per powered hit with `Unpowered` (no recursion).
    /// Neither power decays per hit (Vulnerable is a turn-duration, Thorns a
    /// persistent counter), so all three hits see the full effect.
    #[test]
    fn multi_hit_attack_amplifies_and_reflects_per_hit() {
        use crate::PowerUid;
        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 50;

        let vuln_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: vuln_uid.0,
            kind: PowerKind::Vulnerable,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let thorns_uid = PowerUid::new(2).expect("non-zero");
        state.powers.push(Power {
            id: thorns_uid.0,
            kind: PowerKind::Thorns,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });

        let mut cultist_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        cultist_powers.push(vuln_uid);
        cultist_powers.push(thorns_uid);
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: cultist_powers,
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());

        // 3-hit attack, 6 raw per hit.
        for hit in 1..=3 {
            let hp_loss = apply_damage(
                &mut state,
                CreatureRef::Player,
                cultist_uid(),
                6,
                ValueProps::MOVE,
                None,
            );
            assert_eq!(
                hp_loss, 9,
                "hit {hit}: 6 raw × 1.5 Vulnerable = 9 every hit"
            );
            assert_eq!(
                state.player.current_hp,
                50 - 3 * hit,
                "hit {hit}: Thorns(3) reflects once per hit"
            );
        }
        assert_eq!(
            state.creatures[0].current_hp,
            48 - 27,
            "3 hits × 9 = 27 total to the Vulnerable target"
        );
        // Neither power decayed across the multi-hit loop.
        let amount_of = |k: PowerKind| state.powers.iter().find(|p| p.kind == k).map(|p| p.amount);
        assert_eq!(
            amount_of(PowerKind::Vulnerable),
            Some(2),
            "Vulnerable is a turn-duration — it does not decay per hit"
        );
        assert_eq!(
            amount_of(PowerKind::Thorns),
            Some(3),
            "Thorns is a persistent counter — it does not decay per hit"
        );
    }

    /// **Multi-hit attack into Slippery + Vulnerable: the cap applies AFTER
    /// amplification, Slippery decays per hit, Vulnerable persists.** C#
    /// `SlipperyPower.ModifyDamageCap` returns `1` — it clamps the *final*
    /// post-multiplicative damage — and `AfterDamageReceived` decrements one
    /// stack per landed hit. `VulnerablePower` multiplies ×1.5 every hit and
    /// does not decay per hit. So the Slippery-covered hits land for 1 (the
    /// amplified 9 clamped down), each consuming a stack; once Slippery
    /// reaches 0 the next hit lands at the full amplified 9 — proving
    /// Vulnerable survived the capped hits.
    #[test]
    fn slippery_caps_post_vulnerable_then_full_amplified_hit_lands() {
        use crate::PowerUid;
        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;

        let slip_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: slip_uid.0,
            kind: PowerKind::Slippery,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let vuln_uid = PowerUid::new(2).expect("non-zero");
        state.powers.push(Power {
            id: vuln_uid.0,
            kind: PowerKind::Vulnerable,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });

        let mut vantom_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        vantom_powers.push(slip_uid);
        vantom_powers.push(vuln_uid);
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 173,
            max_hp: 173,
            block: 0,
            powers: vantom_powers,
            species: Some(MonsterKind::Vantom),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Vantom"),
            ai: None,
        });
        state.enemy_uids.push(cultist_uid());

        let slippery_amount = |s: &CombatState| {
            s.powers
                .iter()
                .find(|p| p.kind == PowerKind::Slippery)
                .map(|p| p.amount)
        };

        // Hits 1 & 2: 6 raw → 9 (Vulnerable) → clamped to 1 by Slippery; one
        // stack consumed each landed hit.
        for hit in 1..=2 {
            let hp_loss = apply_damage(
                &mut state,
                CreatureRef::Player,
                cultist_uid(),
                6,
                ValueProps::MOVE,
                None,
            );
            assert_eq!(
                hp_loss, 1,
                "hit {hit}: Slippery clamps the amplified 9 down to 1"
            );
            // Bead `sts2-rs-2w966`: at 0 the power is removed (C#
            // `ShouldRemoveDueToAmount`), so the lookup yields `None` rather
            // than `Some(0)` after the second stack is consumed.
            let expected = (2 - hit != 0).then_some(2 - hit);
            assert_eq!(
                slippery_amount(&state),
                expected,
                "hit {hit}: one Slippery stack consumed per landed hit"
            );
        }
        assert_eq!(
            slippery_amount(&state),
            None,
            "both Slippery stacks consumed → Slippery removed at 0"
        );

        // Hit 3: Slippery exhausted → no cap → Vulnerable still amplifies 6→9.
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            hp_loss, 9,
            "Slippery at 0 no longer caps; Vulnerable(2) still amplifies 6→9"
        );
        assert_eq!(
            state.creatures[0].current_hp,
            173 - 1 - 1 - 9,
            "1 + 1 + 9 = 11 total across the 3-hit attack"
        );
        assert_eq!(
            state
                .powers
                .iter()
                .find(|p| p.kind == PowerKind::Vulnerable)
                .map(|p| p.amount),
            Some(2),
            "Vulnerable persists across all hits — only Slippery decays per hit"
        );
    }

    /// **Tracking multiplies the owner's powered attack into a Weak
    /// target.** C# `TrackingPower.ModifyDamageMultiplicative` returns
    /// `base.Amount` (×N) iff the owner's powered, card-sourced attack
    /// lands on a Weak target. Here the player (dealer) holds Tracking(2)
    /// and the enemy holds Weak, so a powered 6-damage hit is doubled to
    /// 12; without Weak on the target, or on an unpowered hit, Tracking is
    /// identity. Bead `sts2-rs-sy070`.
    #[test]
    fn tracking_doubles_powered_attack_into_weak_target() {
        use crate::PowerUid;

        fn scenario(target_has_weak: bool) -> (CombatState, CreatureUid) {
            let mut state = CombatState::default();
            state.player.current_hp = 50;
            state.player.max_hp = 50;

            // Player (dealer) holds Tracking(2).
            let tracking_uid = PowerUid::new(1).expect("non-zero");
            state.powers.push(Power {
                id: tracking_uid.0,
                kind: PowerKind::Tracking,
                amount: 2,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            state.player.powers.push(tracking_uid);

            let mut enemy_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
            if target_has_weak {
                let weak_uid = PowerUid::new(2).expect("non-zero");
                state.powers.push(Power {
                    id: weak_uid.0,
                    kind: PowerKind::Weak,
                    amount: 1,
                    counter: 0,
                    applier: None,
                    skip_next_tick: false,
                });
                enemy_powers.push(weak_uid);
            }
            state.creatures.push(Creature {
                id: NonZeroU32::new(1).expect("non-zero"),
                current_hp: 48,
                max_hp: 48,
                block: 0,
                powers: enemy_powers,
                species: Some(MonsterKind::Nibbit),
                intent: None,
                intent_history: IntentHistory::default(),
                monster_id: crate::MonsterId("Nibbit"),
                ai: None,
            });
            state.enemy_uids.push(cultist_uid());
            (state, cultist_uid())
        }

        // Powered attack into a Weak target → Tracking(2) doubles 6 → 12.
        let (mut state, uid) = scenario(true);
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            hp_loss, 12,
            "Tracking(2) doubles a powered attack into a Weak target"
        );

        // Same attack, target NOT Weak → Tracking is identity.
        let (mut state, uid) = scenario(false);
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            hp_loss, 6,
            "Tracking does not fire when the target lacks Weak"
        );

        // Unpowered hit into a Weak target → Tracking is identity.
        let (mut state, uid) = scenario(true);
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            6,
            ValueProps::UNPOWERED,
            None,
        );
        assert_eq!(hp_loss, 6, "Tracking does not fire on an unpowered hit");
    }

    #[test]
    fn slippery_does_not_cap_damage_on_non_owner() {
        let (mut state, _vantom_uid) = slippery_scenario(9);
        // Apply damage to the PLAYER, not Vantom. Slippery on Vantom
        // should not affect player damage.
        let player_uid = CreatureUid::new(2).expect("non-zero");
        state.creatures.push(Creature {
            id: NonZeroU32::new(2).expect("non-zero"),
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Vantom),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Vantom"),
            ai: None,
        });
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            player_uid,
            10,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            hp_loss, 10,
            "Slippery on Vantom does not affect other target"
        );
        assert_eq!(state.powers[0].amount, 9, "no stacks consumed");
    }

    /// Bead `sts2-rs-ar9ll` (dvqa1.1): a positive→zero HP transition on
    /// an enemy gets recorded into `combat_log.newly_dead_this_turn`,
    /// and the same uid is not double-recorded if `apply_damage` is
    /// invoked again on the already-dead creature.
    #[test]
    fn newly_dead_this_turn_records_enemy_zero_transition() {
        let mut state = vulnerable_scenario();
        // 48 HP → 6 raw damage ×1.5 (Vulnerable) → 9 hp_loss → 39 HP. Not dead.
        let _ = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            6,
            ValueProps::UNPOWERED,
            None,
        );
        assert!(
            state.combat_log.newly_dead_this_turn.is_empty(),
            "non-lethal hit does not record a death"
        );

        // Lethal: 200 raw damage definitively kills (post-Vulnerable
        // pipeline scaling, the absolute floor is well above 39 remaining HP).
        let _ = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            200,
            ValueProps::UNPOWERED,
            None,
        );
        let dead_uid_after_lethal = state.creatures[0].current_hp;
        assert!(
            dead_uid_after_lethal <= 0,
            "sanity: 200 raw damage reduced HP to <= 0 (got {dead_uid_after_lethal})"
        );
        assert_eq!(
            state.combat_log.newly_dead_this_turn.as_slice(),
            &[cultist_uid()],
            "lethal hit records the dead creature's uid"
        );

        // Posthumous hit must not double-record. pre_hp is already 0,
        // so the positive→zero transition does not fire again.
        let _ = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid(),
            5,
            ValueProps::UNPOWERED,
            None,
        );
        assert_eq!(
            state.combat_log.newly_dead_this_turn.as_slice(),
            &[cultist_uid()],
            "posthumous hit does not re-record"
        );

        // reset_per_turn drains the buffer (defense-in-depth — the
        // dispatch loop normally drains eagerly).
        state.combat_log.reset_per_turn();
        assert!(state.combat_log.newly_dead_this_turn.is_empty());
    }

    /// Bead `sts2-rs-q6bix` (ConstrictPower.AfterDeath): the player's
    /// Constrict is removed when the monster that applied it dies — C#
    /// `ConstrictPower.cs:24-31`. Repro for the coverage-sweep arena
    /// (SlitheringStrangler): after the snake dies, C# clears the
    /// player's Constrict while the old Rust path kept it bleeding.
    #[test]
    fn constrict_removed_when_applier_dies() {
        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 50;

        let monster_uid = cultist_uid();
        // Player carries Constrict(3) applied BY the monster.
        let cpow = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: cpow.0,
            kind: PowerKind::Constrict,
            amount: 3,
            counter: 0,
            applier: Some(monster_uid),
            skip_next_tick: false,
        });
        state.player.powers.push(cpow);

        // A low-HP monster that applied the Constrict.
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 5,
            max_hp: 5,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(monster_uid);

        // Player kills the monster.
        let _ = apply_damage(
            &mut state,
            CreatureRef::Player,
            monster_uid,
            99,
            ValueProps::MOVE,
            None,
        );

        assert!(state.creatures[0].current_hp <= 0, "monster is dead");
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Constrict),
            "Constrict removed from the central power store when its applier dies"
        );
        assert!(
            state.player.powers.is_empty(),
            "Constrict unlinked from player.powers when its applier dies"
        );
    }

    /// **`HardToKillPower` caps incoming damage at `Amount`**
    /// (`sts2-rs-8e8mf`): C# `HardToKillPower.ModifyDamageCap` returns
    /// `base.Amount` when `target == base.Owner`; otherwise `decimal.MaxValue`.
    /// Root of the DRAMATIC_ENTRANCE "deals 6 extra damage to all enemies"
    /// divergence — DRAMATIC_ENTRANCE+1 deals 15 raw but Exoskeleton's
    /// `HardToKill(9)` should cap each hit at 9; Rust was applying 15 (no cap),
    /// hence +6 per monster × 4 monsters.
    ///
    /// This test builds an enemy with `HardToKill(9)` and checks that a 15-raw
    /// attack lands for 9 (the cap), not 15.
    ///
    /// Reference: `decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Powers/
    /// HardToKillPower.cs:ModifyDamageCap`.
    #[test]
    fn hard_to_kill_caps_incoming_damage_at_amount() {
        use crate::{IntentHistory, PowerUid};

        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;

        let htk_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: htk_uid.0,
            kind: PowerKind::HardToKill,
            amount: 9,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });

        let mut enemy_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        enemy_powers.push(htk_uid);
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: enemy_powers,
            species: Some(MonsterKind::Exoskeleton),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Exoskeleton"),
            ai: None,
        });
        let enemy_uid = CreatureUid::new(1).expect("non-zero");
        state.enemy_uids.push(enemy_uid);

        // Deal 15 raw damage (DRAMATIC_ENTRANCE+1 amount); HardToKill(9) should
        // cap the landed damage at 9, leaving the enemy at 50 − 9 = 41 HP.
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemy_uid,
            15,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 9, "HardToKill(9) caps 15-raw hit at 9");
        assert_eq!(
            state.creatures[0].current_hp, 41,
            "enemy HP after capped hit"
        );

        // A hit already at or below the cap lands uncapped.
        let hp_loss2 = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemy_uid,
            7,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss2, 7, "hit ≤ cap lands at full strength");
        assert_eq!(state.creatures[0].current_hp, 34, "41 − 7 = 34");
    }

    /// **`HardToKillPower` does NOT cap damage dealt to a different creature**
    /// (`sts2-rs-8e8mf`): C# `ModifyDamageCap` guards on `target == base.Owner`
    /// — the cap is an inbound filter on the owning creature only; it must not
    /// cap damage routed to an unrelated target (e.g. reflected Thorns hits,
    /// or a second enemy in the same encounter).
    #[test]
    fn hard_to_kill_does_not_cap_damage_on_non_owner() {
        use crate::{IntentHistory, PowerUid};

        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;

        // Enemy 1 carries HardToKill(9).
        let htk_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: htk_uid.0,
            kind: PowerKind::HardToKill,
            amount: 9,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let mut e1_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        e1_powers.push(htk_uid);
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: e1_powers,
            species: Some(MonsterKind::Exoskeleton),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Exoskeleton"),
            ai: None,
        });
        state
            .enemy_uids
            .push(CreatureUid::new(1).expect("non-zero"));

        // Enemy 2 has no powers.
        state.creatures.push(Creature {
            id: NonZeroU32::new(2).expect("non-zero"),
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        let e2_uid = CreatureUid::new(2).expect("non-zero");
        state.enemy_uids.push(e2_uid);

        // 15-raw hit to enemy 2 (no HardToKill) — must land at full 15.
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            e2_uid,
            15,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            hp_loss, 15,
            "HardToKill on enemy 1 must not cap damage to enemy 2"
        );
    }

    // ---------------------------------------------------------------------
    // cargo-mutants gap closure (2026-06-20): the death-record helpers
    // (`remove_*_from_dead_applier`, `remove_debuffs_from_dead_illusion`,
    // `strip_powers_on_adaptable_death`), `originating_card_uid`, and
    // `decrement_buffer_on_target` ran in integration paths but had NO
    // unit assertion pinning their filter/unlink logic — a scoped sweep
    // of `damage.rs` left 90+ of their `&&`/`==`/`!=`/retain mutants alive.
    // The tests below pin each helper's contract directly.
    // ---------------------------------------------------------------------

    fn cuid(n: u32) -> CreatureUid {
        CreatureUid::new(n).expect("non-zero creature uid")
    }

    fn puid(n: u32) -> PowerUid {
        PowerUid::new(n).expect("non-zero power uid")
    }

    fn mk_power(id: u32, kind: PowerKind, applier: Option<CreatureUid>) -> Power {
        Power {
            id: NonZeroU32::new(id).expect("non-zero power id"),
            kind,
            amount: 1,
            counter: 0,
            applier,
            skip_next_tick: false,
        }
    }

    fn bystander(id: u32, powers: SmallVec<[PowerUid; 8]>) -> Creature {
        Creature {
            id: NonZeroU32::new(id).expect("non-zero creature id"),
            current_hp: 10,
            max_hp: 10,
            block: 0,
            powers,
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        }
    }

    fn power_ids(state: &CombatState) -> Vec<u32> {
        state.powers.iter().map(|p| p.id.get()).collect()
    }

    /// Shared contract for the four `remove_<debuff>_from_dead_applier`
    /// helpers: a `kind` power applied by `dead` is unlinked from all three
    /// power surfaces (central rows, player mirror, creature mirrors), while
    /// (a) the SAME kind applied by a live/other creature or with no applier
    /// and (b) a DIFFERENT kind applied by `dead` are both left untouched.
    /// Discriminates every `&&`→`||`, `==`/`!=`, and retain mutant in the
    /// filter + unlink loop.
    fn assert_dead_applier_unlink(kind: PowerKind, remover: fn(&mut CombatState, CreatureUid)) {
        let dead = cuid(1);
        let other = cuid(2);
        let mut state = CombatState::default();
        // P1: kind by dead → REMOVED. P2: kind by other → kept. P3: kind, no
        // applier → kept. P4: other kind by dead → kept.
        state.powers.push(mk_power(1, kind, Some(dead)));
        state.powers.push(mk_power(2, kind, Some(other)));
        state.powers.push(mk_power(3, kind, None));
        state
            .powers
            .push(mk_power(4, PowerKind::Strength, Some(dead)));
        state.player.powers = smallvec::smallvec![puid(1), puid(4)];
        state
            .creatures
            .push(bystander(2, smallvec::smallvec![puid(1), puid(4)]));

        remover(&mut state, dead);

        assert_eq!(
            power_ids(&state),
            vec![2, 3, 4],
            "{kind:?}: only the dead-applier instance is removed from central rows"
        );
        assert_eq!(
            state.player.powers.as_slice(),
            &[puid(4)],
            "{kind:?}: player mirror keeps the unrelated power, drops the removed pid"
        );
        assert_eq!(
            state.creatures[0].powers.as_slice(),
            &[puid(4)],
            "{kind:?}: creature mirror keeps the unrelated power, drops the removed pid"
        );
    }

    #[test]
    fn constrict_removed_only_from_dead_applier() {
        assert_dead_applier_unlink(PowerKind::Constrict, remove_constrict_from_dead_applier);
    }

    #[test]
    fn hex_removed_only_from_dead_applier() {
        assert_dead_applier_unlink(PowerKind::Hex, remove_hex_from_dead_applier);
    }

    #[test]
    fn dampen_removed_only_from_dead_applier() {
        assert_dead_applier_unlink(PowerKind::Dampen, remove_dampen_from_dead_applier);
    }

    #[test]
    fn shrink_removed_only_from_dead_applier() {
        assert_dead_applier_unlink(PowerKind::Shrink, remove_shrink_from_dead_applier);
    }

    /// A dying Illusion strips its OWN Debuff powers (Vulnerable) from every
    /// surface while keeping its buffs (Strength) and the structural Illusion
    /// power, and never touches a sibling creature's debuff.
    #[test]
    fn dead_illusion_strips_only_its_own_debuffs() {
        let dead = cuid(1);
        let mut state = CombatState::default();
        // creatures[0] = the dying illusion (id 1); a sibling (id 2) holds a
        // Vulnerable that must survive (it is not the dead creature's power).
        state
            .creatures
            .push(bystander(1, smallvec::smallvec![puid(1), puid(2), puid(3)]));
        state
            .creatures
            .push(bystander(2, smallvec::smallvec![puid(4)]));
        state.powers.push(mk_power(1, PowerKind::Illusion, None)); // structural → kept
        state.powers.push(mk_power(2, PowerKind::Vulnerable, None)); // debuff → stripped
        state.powers.push(mk_power(3, PowerKind::Strength, None)); // buff → kept
        state.powers.push(mk_power(4, PowerKind::Vulnerable, None)); // sibling's → kept
        state.player.powers = smallvec::smallvec![puid(2), puid(3)];

        remove_debuffs_from_dead_illusion(&mut state, dead);

        assert_eq!(
            power_ids(&state),
            vec![1, 3, 4],
            "only the dying illusion's own debuff (pid 2) is stripped"
        );
        assert_eq!(
            state.player.powers.as_slice(),
            &[puid(3)],
            "player mirror drops the stripped debuff, keeps the buff"
        );
        assert_eq!(
            state.creatures[0].powers.as_slice(),
            &[puid(1), puid(3)],
            "illusion keeps its Illusion + Strength, loses Vulnerable"
        );
        assert_eq!(
            state.creatures[1].powers.as_slice(),
            &[puid(4)],
            "sibling's Vulnerable is untouched"
        );
    }

    /// A dying Illusion replaces its telegraph with `Heal`: C#
    /// `IllusionPower.AfterDeath` arms `REVIVE_MOVE` (a lone `HealIntent`) via
    /// `SetMoveImmediate`, so the corpse advertises the revive rather than the
    /// attack it was killed mid-telegraph. Repro: Vantom's Parafright, whose
    /// wire corpse reports `Heal` while the engine kept showing `Attack 16`.
    #[test]
    fn dead_illusion_telegraphs_its_revive() {
        let dead = cuid(1);
        let mut state = CombatState::default();
        state
            .creatures
            .push(bystander(1, smallvec::smallvec![puid(1)]));
        state.powers.push(mk_power(1, PowerKind::Illusion, None));
        state.creatures[0].current_hp = 0;
        state.creatures[0].intent = Some(crate::Intent::Attack { damage: 16 });

        arm_illusion_revive_telegraph(&mut state, dead);

        assert_eq!(
            state.creatures[0].intent,
            Some(crate::Intent::Heal),
            "REVIVE_MOVE telegraphs a HealIntent, replacing the pre-death attack"
        );
    }

    /// A dying TestSubject telegraphs its respawn: C# `TriggerDeadState` arms
    /// `RESPAWN_MOVE` (`HealIntent` first) via `SetMoveImmediate`
    /// (TestSubject.cs:167-193). Regression for the v18 corpus, where 6 records
    /// showed `TestSubject#dead0` as `Attack 30` / `Attack 10x4` in the mirror
    /// against `Heal` on the wire — the third revive family, missed because the
    /// RESPAWN move table was verified without checking that the corpse is
    /// actually armed onto it at death.
    #[test]
    fn dead_test_subject_telegraphs_its_respawn() {
        let dead = cuid(1);
        let mut state = CombatState::default();
        state
            .creatures
            .push(bystander(1, smallvec::smallvec![puid(1)]));
        state.powers.push(mk_power(1, PowerKind::Adaptable, None));
        state.creatures[0].current_hp = 0;
        state.creatures[0].intent = Some(crate::Intent::Attack { damage: 30 });

        arm_adaptable_respawn_telegraph(&mut state, dead);

        assert_eq!(
            state.creatures[0].intent,
            Some(crate::Intent::Heal),
            "RESPAWN_MOVE leads with a HealIntent, replacing the pre-death attack"
        );
    }

    /// The telegraph is gated on the DEAD creature owning `Illusion`, not on
    /// any Illusion being live — the same scoping as
    /// `remove_debuffs_from_dead_illusion`. Without this a normal monster dying
    /// alongside a FOGMOG illusion would sprout a phantom `Heal`.
    #[test]
    fn dead_non_illusion_keeps_its_telegraph_even_beside_an_illusion() {
        let dead = cuid(2);
        let mut state = CombatState::default();
        // creatures[0] is the illusion (alive); creatures[1] is the plain
        // monster that actually died.
        state
            .creatures
            .push(bystander(1, smallvec::smallvec![puid(1)]));
        state
            .creatures
            .push(bystander(2, smallvec::smallvec![puid(2)]));
        state.powers.push(mk_power(1, PowerKind::Illusion, None));
        state.powers.push(mk_power(2, PowerKind::Strength, None));
        state.creatures[1].current_hp = 0;
        state.creatures[1].intent = Some(crate::Intent::Attack { damage: 16 });

        arm_illusion_revive_telegraph(&mut state, dead);

        assert_eq!(
            state.creatures[1].intent,
            Some(crate::Intent::Attack { damage: 16 }),
            "a non-Illusion corpse keeps whatever it was telegraphing"
        );
    }

    /// A dying Illusion KEEPS temporary-power debuffs (Mangle) while still
    /// stripping plain Debuffs (Vulnerable). C#
    /// `IllusionPower.ShouldPowerBeRemovedOnDeath` returns
    /// `!(power is ITemporaryPower)` for Debuff-typed powers, so a
    /// player-applied MANGLE survives the illusion's death. Regression for
    /// seed 2232 (`MONSTER_STATE|OMNISLICE`): an OMNISLICE splash kill of a
    /// Mangle'd illusion was wrongly dropping the Mangle power.
    #[test]
    fn dead_illusion_keeps_temporary_power_debuffs() {
        let dead = cuid(1);
        let mut state = CombatState::default();
        state
            .creatures
            .push(bystander(1, smallvec::smallvec![puid(1), puid(2), puid(3)]));
        state.powers.push(mk_power(1, PowerKind::Illusion, None)); // structural → kept
        state.powers.push(mk_power(2, PowerKind::Vulnerable, None)); // plain debuff → stripped
        state.powers.push(mk_power(3, PowerKind::ManglePower, None)); // temporary debuff → kept

        remove_debuffs_from_dead_illusion(&mut state, dead);

        assert_eq!(
            power_ids(&state),
            vec![1, 3],
            "Mangle (ITemporaryPower) survives; only the plain Vulnerable is stripped"
        );
        assert_eq!(
            state.creatures[0].powers.as_slice(),
            &[puid(1), puid(3)],
            "illusion keeps Illusion + Mangle"
        );
    }

    /// A dying NON-illusion creature keeps its debuffs even when a sibling
    /// elsewhere carries an Illusion power — the strip is gated on the DEAD
    /// creature owning the Illusion, not on any Illusion existing.
    #[test]
    fn dead_non_illusion_keeps_its_debuffs() {
        let dead = cuid(1);
        let mut state = CombatState::default();
        state
            .creatures
            .push(bystander(1, smallvec::smallvec![puid(2)])); // dead, no illusion
        state
            .creatures
            .push(bystander(2, smallvec::smallvec![puid(1)])); // sibling has illusion
        state.powers.push(mk_power(1, PowerKind::Illusion, None));
        state.powers.push(mk_power(2, PowerKind::Vulnerable, None));

        remove_debuffs_from_dead_illusion(&mut state, dead);

        assert_eq!(
            power_ids(&state),
            vec![1, 2],
            "no strip: the dead creature carries no Illusion of its own"
        );
    }

    /// A dying Adaptable carrier (TestSubject) keeps only its survivor powers
    /// (Adaptable, PainfulStabs, Nemesis, Reattach) and is stripped of
    /// everything else (Enrage, player debuffs) from every surface; a sibling
    /// non-carrier is untouched.
    #[test]
    fn adaptable_death_strips_non_survivor_powers() {
        let dead = cuid(1);
        let mut state = CombatState::default();
        state.creatures.push(bystander(
            1,
            smallvec::smallvec![puid(1), puid(2), puid(3), puid(4)],
        ));
        state
            .creatures
            .push(bystander(2, smallvec::smallvec![puid(5)]));
        state.powers.push(mk_power(1, PowerKind::Adaptable, None)); // survivor
        state.powers.push(mk_power(2, PowerKind::Enrage, None)); // stripped
        state
            .powers
            .push(mk_power(3, PowerKind::PainfulStabs, None)); // survivor
        state.powers.push(mk_power(4, PowerKind::Vulnerable, None)); // stripped
        state.powers.push(mk_power(5, PowerKind::Enrage, None)); // sibling's → kept
        state.player.powers = smallvec::smallvec![puid(2), puid(3)];

        strip_powers_on_adaptable_death(&mut state, dead);

        assert_eq!(
            power_ids(&state),
            vec![1, 3, 5],
            "non-survivor powers (Enrage pid2, Vulnerable pid4) stripped; survivors + sibling kept"
        );
        assert_eq!(
            state.player.powers.as_slice(),
            &[puid(3)],
            "player mirror keeps PainfulStabs survivor, drops stripped Enrage"
        );
        assert_eq!(
            state.creatures[0].powers.as_slice(),
            &[puid(1), puid(3)],
            "corpse keeps only Adaptable + PainfulStabs survivors"
        );
        assert_eq!(
            state.creatures[1].powers.as_slice(),
            &[puid(5)],
            "sibling non-carrier is untouched"
        );
    }

    /// A creature WITHOUT a revive power (Adaptable/Reattach) is never
    /// stripped, even though it owns non-survivor powers.
    ///
    /// NB: the `has_revive_power` `p.id == puid.0` mutant (`==`→`!=`) at
    /// `strip_powers_on_adaptable_death` is a genuine EQUIVALENT mutant — the
    /// `any`-over-all-powers walk always finds the Adaptable row via a sibling
    /// power's index, so no reachable state distinguishes it. cargo-mutants
    /// reports it as a survivor; that is expected, not a coverage gap.
    #[test]
    fn non_revive_death_strips_nothing() {
        let dead = cuid(1);
        let mut state = CombatState::default();
        state
            .creatures
            .push(bystander(1, smallvec::smallvec![puid(1), puid(2)]));
        state.powers.push(mk_power(1, PowerKind::Enrage, None));
        state.powers.push(mk_power(2, PowerKind::Vulnerable, None));

        strip_powers_on_adaptable_death(&mut state, dead);

        assert_eq!(
            power_ids(&state),
            vec![1, 2],
            "no revive power → no strip, all powers retained"
        );
    }

    /// `originating_card_uid` returns the top-of-`Play` uid only when its
    /// `card_id` matches the passed card, and `None` otherwise. Pins the
    /// `card?` guard, the `uid - 1` index math, and the id-equality check.
    #[test]
    fn originating_card_uid_matches_top_of_play() {
        use crate::cards::{CardId, CardTags, CardType, TargetType};
        let mk_card = |raw: u32, card_id: CardId| crate::Card {
            uid: crate::CardUid::new(raw).expect("non-zero"),
            card_id,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        };
        let mut state = CombatState::default();
        state.cards.push(mk_card(1, CardId::StrikeIronclad));
        state.cards.push(mk_card(2, CardId::DefendIronclad));
        // Top-of-play is uid 2 (DefendIronclad).
        state.player.play = smallvec::smallvec![crate::CardUid::new(2).unwrap()];

        // Non-card damage → None (the `card?` guard).
        assert_eq!(originating_card_uid(&state, None), None);
        // Matching id → the top-of-play uid (idx = 2 - 1 = 1 → cards[1]).
        assert_eq!(
            originating_card_uid(&state, Some(CardId::DefendIronclad)),
            Some(crate::CardUid::new(2).unwrap()),
            "matching card id resolves to the top-of-play instance"
        );
        // id mismatch (top-of-play is Defend, asked for Strike) → None.
        assert_eq!(
            originating_card_uid(&state, Some(CardId::StrikeIronclad)),
            None,
            "id mismatch against the top-of-play card resolves to None"
        );
    }

    /// `decrement_buffer_on_target` drops one stack of the target's Buffer
    /// and, when it reaches zero, unlinks it from every surface — leaving
    /// unrelated powers in place.
    #[test]
    fn decrement_buffer_removes_at_zero() {
        let mut state = CombatState::default();
        // Buffer owned by the player (resolved via `owner_of_listener` →
        // present in `state.player.powers`), amount 1 so the decrement
        // removes it.
        state.powers.push(mk_power(1, PowerKind::Buffer, None));
        state.powers.push(mk_power(2, PowerKind::Strength, None));
        state.player.powers = smallvec::smallvec![puid(1), puid(2)];
        state
            .creatures
            .push(bystander(1, smallvec::smallvec![puid(1), puid(2)]));

        decrement_buffer_on_target(&mut state, CreatureRef::Player);

        assert_eq!(
            power_ids(&state),
            vec![2],
            "Buffer at amount 1 is removed on decrement; Strength kept"
        );
        assert_eq!(
            state.player.powers.as_slice(),
            &[puid(2)],
            "player mirror drops the spent Buffer, keeps Strength"
        );
        assert_eq!(
            state.creatures[0].powers.as_slice(),
            &[puid(2)],
            "creature mirror drops the spent Buffer, keeps Strength"
        );
    }

    /// A Buffer at amount 0 is NOT a decrement candidate (the `amount > 0`
    /// filter) — the function is a no-op and leaves the dead stack alone.
    #[test]
    fn decrement_buffer_ignores_zero_amount() {
        let mut state = CombatState::default();
        let mut zero_buffer = mk_power(1, PowerKind::Buffer, None);
        zero_buffer.amount = 0;
        state.powers.push(zero_buffer);
        state.player.powers = smallvec::smallvec![puid(1)];

        decrement_buffer_on_target(&mut state, CreatureRef::Player);

        assert_eq!(
            state
                .powers
                .get_by_id(NonZeroU32::new(1).unwrap())
                .map(|p| p.amount),
            Some(0),
            "an amount-0 Buffer is not a candidate; left untouched (not decremented to -1)"
        );
    }

    // ----- apply_damage_with_final branch gates (cargo-mutants) -----

    /// Build a single living enemy at `hp`/`block` (id 1) registered in
    /// `enemy_uids`, returning the populated state.
    fn enemy_target_state(hp: i32, block: i32, powers: SmallVec<[PowerUid; 8]>) -> CombatState {
        let mut state = CombatState::default();
        let mut c = bystander(1, powers);
        c.current_hp = hp;
        c.max_hp = hp;
        c.block = block;
        state.creatures.push(c);
        state.enemy_uids.push(cuid(1));
        state
    }

    /// A FULLY-blocked hit (unblocked == 0) must NOT consume a Buffer stack —
    /// Buffer only spends on an HP-loss event that reaches the modify stage,
    /// not on pure block absorption (the `unblocked > 0` gate at the
    /// decrement call site).
    #[test]
    fn buffer_not_decremented_on_fully_blocked_hit() {
        // Enemy with 20 block owns a Buffer(2). A 5-damage hit is fully
        // absorbed by block → unblocked == 0 → Buffer untouched.
        let mut state = enemy_target_state(50, 20, smallvec::smallvec![puid(10)]);
        let mut buffer = mk_power(10, PowerKind::Buffer, None);
        buffer.amount = 2;
        state.powers.push(buffer);

        let out = apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            cuid(1),
            5,
            ValueProps::MOVE,
        );

        assert_eq!(out.hp_loss, 0, "fully blocked → no HP loss");
        assert_eq!(out.blocked, 5, "5 absorbed into block");
        assert_eq!(
            state
                .powers
                .get_by_id(NonZeroU32::new(10).unwrap())
                .map(|p| p.amount),
            Some(2),
            "Buffer stack NOT consumed by a fully-blocked hit"
        );
    }

    /// An UNBLOCKED hit DOES consume a Buffer stack — the decrement fires
    /// whenever `unblocked > 0`, even though Buffer's modify-hp-lost arm zeros
    /// the realized HP loss. Complements the fully-blocked control above: that
    /// test pins the `> 0`→`>= 0`/`== 0` mutants, this one pins `> 0`→`< 0`
    /// (which would never decrement).
    #[test]
    fn buffer_decremented_on_unblocked_hit() {
        let mut state = enemy_target_state(50, 0, smallvec::smallvec![puid(10)]);
        let mut buffer = mk_power(10, PowerKind::Buffer, None);
        buffer.amount = 2;
        state.powers.push(buffer);

        let _ = apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            cuid(1),
            6,
            ValueProps::MOVE,
        );

        assert_eq!(
            state
                .powers
                .get_by_id(NonZeroU32::new(10).unwrap())
                .map(|p| p.amount),
            Some(1),
            "an unblocked hit consumes exactly one Buffer stack"
        );
    }

    /// A powered player hit latches the active card for Vigor when a live
    /// Vigor exists and nothing is latched yet (the `is_none() && amount > 0`
    /// gate, true branch).
    #[test]
    fn vigor_latches_active_card_on_powered_player_hit() {
        let mut state = enemy_target_state(50, 0, SmallVec::new());
        // Player holds Vigor(3); the active card is uid 5 on top of Play.
        let mut vigor = mk_power(10, PowerKind::Vigor, None);
        vigor.amount = 3;
        state.powers.push(vigor);
        state.player.powers = smallvec::smallvec![puid(10)];
        state.player.play = smallvec::smallvec![crate::CardUid::new(5).unwrap()];

        let _ = apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            cuid(1),
            6,
            ValueProps::MOVE,
        );

        assert_eq!(
            state.player.vigor_latch_card,
            Some(crate::CardUid::new(5).unwrap()),
            "first powered hit with live Vigor latches the active card"
        );
    }

    /// Control for the Vigor latch: with NO active Vigor the latch stays
    /// `None` even on a powered player hit (the `amount > 0` half of the gate,
    /// false branch). Kills the `&&`→`||` and `> 0`→`>= 0` mutants that would
    /// latch regardless of Vigor.
    #[test]
    fn no_vigor_latch_without_active_vigor() {
        let mut state = enemy_target_state(50, 0, SmallVec::new());
        state.player.play = smallvec::smallvec![crate::CardUid::new(5).unwrap()];

        let _ = apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            cuid(1),
            6,
            ValueProps::MOVE,
        );

        assert_eq!(
            state.player.vigor_latch_card, None,
            "no live Vigor → no latch"
        );
    }

    /// An Osty-as-dealer attack bumps `osty_attacks_this_turn` (Necrobinder
    /// Rattle scaling). Pins the `dealer_uid == pet_uid` equality.
    #[test]
    fn osty_attack_increments_counter_when_pet_is_dealer() {
        let mut state = enemy_target_state(50, 0, SmallVec::new());
        state.player.pet_uid = Some(cuid(2));
        assert_eq!(state.combat_log.osty_attacks_this_turn, 0);

        let _ = apply_damage_with_final(
            &mut state,
            CreatureRef::Enemy(cuid(2)),
            cuid(1),
            6,
            ValueProps::MOVE,
        );

        assert_eq!(
            state.combat_log.osty_attacks_this_turn, 1,
            "a pet-dealer attack increments the Osty attack counter"
        );
    }

    // ----- arm_reattach_dead_state (Decimillipede revive arm) -----

    fn dead_state_noop(_: &mut CombatState, _: CreatureUid) {}

    /// A two-state AI (`atk` initial, plus a `DEAD` move-state) so
    /// `force_current_state_by_name("DEAD")` can resolve.
    fn reattach_ai() -> crate::monster_ai::MonsterAi {
        use crate::monster_ai::{MonsterAi, MonsterState, MoveState};
        let mut states = std::collections::BTreeMap::new();
        states.insert(
            "atk",
            MonsterState::Move(MoveState {
                id: "atk",
                effect: dead_state_noop,
                intents: smallvec::smallvec![crate::Intent::Attack { damage: 1 }],
                follow_up: Some("atk"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "DEAD",
            MonsterState::Move(MoveState {
                id: "DEAD",
                effect: dead_state_noop,
                intents: smallvec::smallvec![crate::Intent::Unknown],
                follow_up: Some("DEAD"),
                must_perform_once: false,
            }),
        );
        MonsterAi::new(states, "atk")
    }

    fn segment(
        id: u32,
        hp: i32,
        power_uid: PowerUid,
        ai: Option<crate::monster_ai::MonsterAi>,
    ) -> Creature {
        Creature {
            id: NonZeroU32::new(id).expect("non-zero creature id"),
            current_hp: hp,
            max_hp: 10,
            block: 0,
            powers: smallvec::smallvec![power_uid],
            species: Some(MonsterKind::Nibbit),
            intent: Some(crate::Intent::Attack { damage: 1 }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai,
        }
    }

    /// A dead Reattach segment with at least one OTHER live Reattach segment
    /// is forced onto the `DEAD` no-op move instead of dying outright — the
    /// corpse awaits its later REATTACH revive. `DEAD_MOVE` carries no
    /// intents in C#, so the corpse's telegraph is CLEARED, not replaced
    /// with a placeholder; `is_acting_reattach_corpse` is what keeps it
    /// taking turns.
    #[test]
    fn reattach_arms_dead_state_when_a_live_segment_remains() {
        let mut state = CombatState::default();
        state
            .creatures
            .push(segment(1, 0, puid(1), Some(reattach_ai()))); // dead segment
        state.creatures.push(segment(2, 10, puid(2), None)); // live sibling
        state.powers.push(mk_power(1, PowerKind::Reattach, None));
        state.powers.push(mk_power(2, PowerKind::Reattach, None));
        state.enemy_uids.push(cuid(1));
        state.enemy_uids.push(cuid(2));

        arm_reattach_dead_state(&mut state, cuid(1));

        assert_eq!(
            state.creatures[0].intent, None,
            "DEAD_MOVE telegraphs nothing, so arming clears the intent"
        );
        assert_eq!(
            state.creatures[0].ai.as_ref().unwrap().current_state_id(),
            "DEAD",
            "dead segment's AI is forced onto the DEAD state"
        );
    }

    /// No arming when the only other living enemy is NOT a Reattach segment —
    /// the death is fatal and the corpse keeps its telegraphed move.
    ///
    /// NB: the `carries_reattach` `p.id == puid.0` mutant (`==`→`!=`) is an
    /// EQUIVALENT mutant: with two Reattach rows in the global table each
    /// creature's check is satisfied by the OTHER row under `!=`, so no
    /// reachable state distinguishes it.
    #[test]
    fn reattach_does_not_arm_without_a_sibling_segment() {
        let mut state = CombatState::default();
        state
            .creatures
            .push(segment(1, 0, puid(1), Some(reattach_ai())));
        state.creatures.push(segment(2, 10, puid(3), None)); // live, but no Reattach
        state.powers.push(mk_power(1, PowerKind::Reattach, None));
        state.powers.push(mk_power(3, PowerKind::Strength, None));
        state.enemy_uids.push(cuid(1));
        state.enemy_uids.push(cuid(2));

        arm_reattach_dead_state(&mut state, cuid(1));

        assert_eq!(
            state.creatures[0].intent,
            Some(crate::Intent::Attack { damage: 1 }),
            "no sibling segment → intent untouched"
        );
        assert_eq!(
            state.creatures[0].ai.as_ref().unwrap().current_state_id(),
            "atk",
            "no sibling segment → AI not forced to DEAD"
        );
    }

    /// No arming when the other Reattach segment is itself already dead
    /// (`current_hp > 0` must hold for the sibling to count as alive).
    #[test]
    fn reattach_does_not_arm_when_sibling_segment_is_dead() {
        let mut state = CombatState::default();
        state
            .creatures
            .push(segment(1, 0, puid(1), Some(reattach_ai())));
        state.creatures.push(segment(2, 0, puid(2), None)); // Reattach but dead
        state.powers.push(mk_power(1, PowerKind::Reattach, None));
        state.powers.push(mk_power(2, PowerKind::Reattach, None));
        state.enemy_uids.push(cuid(1));
        state.enemy_uids.push(cuid(2));

        arm_reattach_dead_state(&mut state, cuid(1));

        assert_eq!(
            state.creatures[0].intent,
            Some(crate::Intent::Attack { damage: 1 }),
            "all other segments dead → no arming"
        );
        assert_eq!(
            state.creatures[0].ai.as_ref().unwrap().current_state_id(),
            "atk",
            "all other segments dead → AI not forced to DEAD"
        );
    }

    // ---------------------------------------------------------------------
    // Integration gates: apply_damage_with_final branches that are only
    // observable through a reacting power/relic (cargo-mutants left these
    // alive because no test in the crate exercised the downstream feature).
    // ---------------------------------------------------------------------

    /// Killing an enemy that carries a registered on-death-spawn power
    /// (StockPower / Axebot) fires `enemy_death_effects::fire_for_owner` —
    /// gated on `dead_side == Side::Enemy` at damage.rs:650. The mutant
    /// (`==`→`!=`) would skip the respawn when an enemy dies.
    #[test]
    fn enemy_death_with_stock_fires_on_death_spawn() {
        // Register a marker handler for this (per-nextest-process) run.
        crate::enemy_death_effects::register(PowerKind::Stock, stock_death_spawn_marker);

        let mut state = enemy_target_state(5, 0, smallvec::smallvec![puid(30)]);
        let mut stock = mk_power(30, PowerKind::Stock, None);
        stock.amount = 2;
        state.powers.push(stock);

        let before = state.enemy_uids.len();
        let _ = apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            cuid(1),
            99,
            ValueProps::MOVE,
        );

        assert!(
            state
                .creatures
                .iter()
                .any(|c| c.id == cuid(1).0 && c.current_hp <= 0),
            "the Stock-carrying enemy died"
        );
        assert_eq!(
            state.enemy_uids.len(),
            before + 1,
            "an enemy death with Stock fires the on-death spawn"
        );
        assert!(
            state.enemy_uids.contains(&cuid(99)),
            "the spawned replacement was registered"
        );
    }

    fn stock_death_spawn_marker(state: &mut CombatState, _owner: CreatureUid) {
        state.creatures.push(bystander(99, SmallVec::new()));
        state.enemy_uids.push(cuid(99));
    }

    /// Breaking an enemy's block fires `AfterBlockBroken` (Burrowed → forced
    /// DIZZY_MOVE + power removal). Pins `was_block_broken = blocked > 0 &&
    /// target_block_now <= 0` (damage.rs:678/683): a block-break stuns, while
    /// a hit on a 0-block enemy does NOT (the `blocked > 0` guard).
    fn burrowed_ai() -> crate::monster_ai::MonsterAi {
        use crate::monster_ai::{MonsterAi, MonsterState, MoveState};
        let mut states = std::collections::BTreeMap::new();
        states.insert(
            "atk",
            MonsterState::Move(MoveState {
                id: "atk",
                effect: dead_state_noop,
                intents: smallvec::smallvec![crate::Intent::Attack { damage: 1 }],
                follow_up: Some("atk"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "DIZZY_MOVE",
            MonsterState::Move(MoveState {
                id: "DIZZY_MOVE",
                effect: dead_state_noop,
                intents: smallvec::smallvec![crate::Intent::Unknown],
                follow_up: Some("DIZZY_MOVE"),
                must_perform_once: false,
            }),
        );
        MonsterAi::new(states, "atk")
    }

    fn burrowed_enemy_state(block: i32) -> CombatState {
        let mut state = CombatState::default();
        let mut c = bystander(1, smallvec::smallvec![puid(30)]);
        c.current_hp = 50;
        c.max_hp = 50;
        c.block = block;
        c.ai = Some(burrowed_ai());
        state.creatures.push(c);
        state.enemy_uids.push(cuid(1));
        state.powers.push(mk_power(30, PowerKind::Burrowed, None));
        state
    }

    fn has_burrowed(state: &CombatState) -> bool {
        state.creatures[0].powers.iter().any(|u| {
            state
                .powers
                .iter()
                .any(|p| p.id == u.0 && p.kind == PowerKind::Burrowed)
        })
    }

    #[test]
    fn block_break_stuns_burrowed_owner() {
        // block 8, deal 10 → block fully broken (→0), 2 HP through.
        let mut state = burrowed_enemy_state(8);
        let _ = apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            cuid(1),
            10,
            ValueProps::MOVE,
        );

        assert_eq!(
            state.creatures[0].ai.as_ref().unwrap().current_state_id(),
            "DIZZY_MOVE",
            "block-break unburrows the owner into the stun move"
        );
        assert!(!has_burrowed(&state), "Burrowed is removed on block-break");
    }

    #[test]
    fn no_block_break_when_enemy_has_no_block() {
        // block 0, deal 10 → no block to break (blocked == 0).
        let mut state = burrowed_enemy_state(0);
        let _ = apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            cuid(1),
            10,
            ValueProps::MOVE,
        );

        assert_eq!(
            state.creatures[0].ai.as_ref().unwrap().current_state_id(),
            "atk",
            "a hit on a 0-block enemy is not a block-break → no stun"
        );
        assert!(
            has_burrowed(&state),
            "Burrowed persists without a block-break"
        );
    }
}
