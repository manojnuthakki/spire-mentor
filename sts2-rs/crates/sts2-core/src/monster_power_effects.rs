//! Monster-power on-trigger dispatch lanes — bead `sts2-rs-hfpy1`
//! (9l5c2 sub-1).
//!
//! Parallel to [`crate::relic_effects`] but keyed by [`PowerKind`]
//! instead of `RelicId`. Each channel here lets a per-`PowerKind`
//! handler subscribe to one of three creature-power lifecycle events
//! that the relic-side lane does not cover for monster powers:
//!
//! 1. **`AfterApplied`** — fires when an instance of the power is
//!    added to a creature's `powers` vector (the moment the C# pin
//!    fires `OnAppliedToOwner`/`AfterApplied`). Listeners use this
//!    to run an "on-spawn" walk (e.g. HungerPower afflicting every
//!    existing player Attack/Skill card with Devoured).
//! 2. **`AfterCardEnteredCombat`** — fires per card added to the
//!    player's pool while the power is still on the creature. The
//!    relic-side `dispatch_after_card_entered_combat` covers relics;
//!    this channel covers monster powers that observe the same hook
//!    (HungerPower / GraspPower / future analogs).
//! 3. **`AfterRemoved`** — fires when the power is stripped from the
//!    creature's `powers` vector mid-combat (Doormaker phase swap is
//!    the canonical site at this pin). Listeners run a revert walk
//!    (HungerPower stripping the `Exhaust` keyword + clearing the
//!    `Devoured` affliction on cards where `applied_exhaust == true`).
//!
//! Scope + rationale: docs/audits/2026-05-29-9l5c2-hungerpower-hook-dispatch-scope.md
//! ("Option A: parallel monster-power dispatch lane").
//!
//! This bead only adds the **registries + dispatchers + empty-bus
//! probes**. Wiring HungerPower's three hook bodies against the lane
//! is bead `sts2-rs-ig66w` (9l5c2 sub-2); GraspPower (Weighted) is
//! bead `sts2-rs-6ww9w`. The call sites that fire the dispatchers
//! (apply / power-removal / card-entered-combat) are added by those
//! follow-up beads — this primitive ships unused.

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use crate::card_effects::EffectResult;
use crate::{CardUid, CombatState, CreatureUid, PowerKind};

// ---------------------------------------------------------------------------
// Channel: AfterApplied
// ---------------------------------------------------------------------------

/// Handler signature for `after_applied_power`. Called once per
/// `(creature, kind)` pair when an instance of `kind` is first
/// installed on `owner`'s `Creature.powers`. `applier` is the
/// creature that applied the power (matches C#
/// `OnAppliedToOwner(applier)`); `None` for system-applied / spawn
/// installs.
pub type AfterAppliedPowerFn =
    fn(&mut CombatState, owner: CreatureUid, applier: Option<CreatureUid>) -> EffectResult;

static REGISTRY_AFTER_APPLIED_POWER: OnceLock<RwLock<BTreeMap<PowerKind, AfterAppliedPowerFn>>> =
    OnceLock::new();

fn registry_after_applied_power() -> &'static RwLock<BTreeMap<PowerKind, AfterAppliedPowerFn>> {
    REGISTRY_AFTER_APPLIED_POWER.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// Register a handler on the `after_applied_power` channel for `kind`.
pub fn register_after_applied_for_power(kind: PowerKind, handler: AfterAppliedPowerFn) {
    registry_after_applied_power()
        .write()
        .expect("monster-power hook registry poisoned")
        .insert(kind, handler);
}

/// Look up the registered `after_applied_power` handler for `kind`.
#[must_use]
pub fn lookup_after_applied_for_power(kind: PowerKind) -> Option<AfterAppliedPowerFn> {
    registry_after_applied_power()
        .read()
        .expect("monster-power hook registry poisoned")
        .get(&kind)
        .copied()
}

/// `true` if no listeners are registered on the `after_applied_power`
/// channel. Hot-path early-out, mirrors the relic-side `is_empty_*`
/// probes.
#[inline]
#[must_use]
pub fn is_empty_after_applied_power() -> bool {
    match REGISTRY_AFTER_APPLIED_POWER.get() {
        None => true,
        Some(lock) => lock
            .read()
            .expect("monster-power hook registry poisoned")
            .is_empty(),
    }
}

/// Fire `after_applied_power(state, owner, applier)` to the listener
/// registered for `kind`, if any. Call site: immediately after
/// `apply_power_to_creature` (or the swap-phase install) installs the
/// power on `owner` — must be invoked AFTER the `Power` entry has been
/// pushed into `state.powers` and `owner.powers`, so the listener can
/// observe the freshly-installed power if it needs to read its `amount`.
pub fn dispatch_after_applied_for_power(
    state: &mut CombatState,
    owner: CreatureUid,
    kind: PowerKind,
    applier: Option<CreatureUid>,
) -> EffectResult {
    if is_empty_after_applied_power() {
        return EffectResult::NotImplemented;
    }
    let Some(handler) = lookup_after_applied_for_power(kind) else {
        return EffectResult::NotImplemented;
    };
    handler(state, owner, applier)
}

// ---------------------------------------------------------------------------
// Channel: AfterCardEnteredCombat
// ---------------------------------------------------------------------------

/// Handler signature for `after_card_entered_combat_power`. Called for
/// each `(owner, kind)` pair on every alive creature whose `powers`
/// includes `kind`, after a new card is added to the player's combat
/// pool. Symmetric with [`crate::relic_effects::dispatch_after_card_entered_combat`].
pub type AfterCardEnteredCombatPowerFn =
    fn(&mut CombatState, owner: CreatureUid, card_uid: CardUid) -> EffectResult;

static REGISTRY_AFTER_CARD_ENTERED_COMBAT_POWER: OnceLock<
    RwLock<BTreeMap<PowerKind, AfterCardEnteredCombatPowerFn>>,
> = OnceLock::new();

fn registry_after_card_entered_combat_power(
) -> &'static RwLock<BTreeMap<PowerKind, AfterCardEnteredCombatPowerFn>> {
    REGISTRY_AFTER_CARD_ENTERED_COMBAT_POWER.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// Register a handler on the `after_card_entered_combat_power` channel.
pub fn register_after_card_entered_combat_for_power(
    kind: PowerKind,
    handler: AfterCardEnteredCombatPowerFn,
) {
    registry_after_card_entered_combat_power()
        .write()
        .expect("monster-power hook registry poisoned")
        .insert(kind, handler);
}

/// Look up the registered handler for `kind`.
#[must_use]
pub fn lookup_after_card_entered_combat_for_power(
    kind: PowerKind,
) -> Option<AfterCardEnteredCombatPowerFn> {
    registry_after_card_entered_combat_power()
        .read()
        .expect("monster-power hook registry poisoned")
        .get(&kind)
        .copied()
}

/// `true` if no listeners are registered on the channel.
#[inline]
#[must_use]
pub fn is_empty_after_card_entered_combat_power() -> bool {
    match REGISTRY_AFTER_CARD_ENTERED_COMBAT_POWER.get() {
        None => true,
        Some(lock) => lock
            .read()
            .expect("monster-power hook registry poisoned")
            .is_empty(),
    }
}

/// Walk every creature's `powers` vector and fire the registered
/// `after_card_entered_combat_power` handler for each installed
/// `PowerKind`. Call site: the same point as the existing relic-side
/// `dispatch_after_card_entered_combat` (added when bead `ig66w` wires
/// HungerPower).
///
/// Iteration order: creatures in `state.creatures` index order, then
/// powers in `Creature.powers` index order — deterministic and
/// matches the existing iteration discipline of dispatchers like
/// [`crate::lib::CombatState::get_random_enemy`]. Within a single
/// creature only one listener per `PowerKind` fires (multi-stack
/// powers are collapsed by the BTreeMap key — same convention as
/// the relic-side lane, which fires once per `RelicId` regardless of
/// stack count).
pub fn dispatch_after_card_entered_combat_on_monster_powers(
    state: &mut CombatState,
    card_uid: CardUid,
) -> EffectResult {
    // C# `SmoggyPower.AfterCardEnteredCombat` (`SmoggyPower.cs:39-45`) rides
    // this same hook, but its listener sits on the PLAYER's mirror+legacy power
    // union, which the registry walk below cannot see — see the SmoggyPower
    // section at the bottom of this module for why. Run it as a dedicated leg
    // BEFORE the empty-registry early-out so every call site of this bus (the
    // `fire_after_card_entered_combat` bundle plus the four content sites that
    // inline it) covers Smoggy without a per-site edit.
    let smoggy = smoggy_after_card_entered_combat(state, card_uid);
    // `TangledPower.AfterCardEnteredCombat` sits on the player too (same leg).
    let tangled = tangled_after_card_entered_combat(state, card_uid);
    let player_legs = if tangled == EffectResult::Resolved {
        tangled
    } else {
        smoggy
    };
    if is_empty_after_card_entered_combat_power() {
        return player_legs;
    }
    // Snapshot the (owner, kind) pairs first so the per-handler call
    // can take `&mut CombatState` without holding an immutable borrow.
    // Mirrors the snapshot-then-iterate pattern in the relic-side
    // `dispatch_after_card_entered_combat`. Two passes: first collect
    // `(owner, puid)` pairs from `state.creatures`, then resolve each
    // `puid` against `state.powers` — avoids the double-borrow that
    // would otherwise happen when the inner closure captures `state`.
    let owner_puids: Vec<(CreatureUid, crate::PowerUid)> = state
        .creatures
        .iter()
        .filter(|c| c.current_hp > 0)
        .flat_map(|c| {
            let owner = CreatureUid(c.id);
            c.powers.iter().copied().map(move |puid| (owner, puid))
        })
        .collect();
    let pairs: Vec<(CreatureUid, PowerKind)> = owner_puids
        .into_iter()
        .filter_map(|(owner, puid)| {
            state
                .powers
                .iter()
                .find(|p| p.id == puid.0)
                .map(|p| (owner, p.kind))
        })
        .collect();
    let mut any_resolved = smoggy == EffectResult::Resolved;
    for (owner, kind) in pairs {
        if let Some(handler) = lookup_after_card_entered_combat_for_power(kind) {
            if handler(state, owner, card_uid) == EffectResult::Resolved {
                any_resolved = true;
            }
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// ---------------------------------------------------------------------------
// Channel: AfterRemoved
// ---------------------------------------------------------------------------

/// Handler signature for `after_removed_power`. Called once when an
/// instance of the power is removed from `old_owner`'s `Creature.powers`
/// (either via explicit `state.powers.retain` or via creature death,
/// per the caller's choice). Mirrors C# `AfflictionPower.AfterRemoved`
/// hook signature (`HungerPower.cs:81-90`).
pub type AfterRemovedPowerFn = fn(&mut CombatState, old_owner: CreatureUid) -> EffectResult;

static REGISTRY_AFTER_REMOVED_POWER: OnceLock<RwLock<BTreeMap<PowerKind, AfterRemovedPowerFn>>> =
    OnceLock::new();

fn registry_after_removed_power() -> &'static RwLock<BTreeMap<PowerKind, AfterRemovedPowerFn>> {
    REGISTRY_AFTER_REMOVED_POWER.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// Register a handler on the `after_removed_power` channel.
pub fn register_after_removed_for_power(kind: PowerKind, handler: AfterRemovedPowerFn) {
    registry_after_removed_power()
        .write()
        .expect("monster-power hook registry poisoned")
        .insert(kind, handler);
}

/// Look up the registered handler for `kind`.
#[must_use]
pub fn lookup_after_removed_for_power(kind: PowerKind) -> Option<AfterRemovedPowerFn> {
    registry_after_removed_power()
        .read()
        .expect("monster-power hook registry poisoned")
        .get(&kind)
        .copied()
}

/// `true` if no listeners are registered on the channel.
#[inline]
#[must_use]
pub fn is_empty_after_removed_power() -> bool {
    match REGISTRY_AFTER_REMOVED_POWER.get() {
        None => true,
        Some(lock) => lock
            .read()
            .expect("monster-power hook registry poisoned")
            .is_empty(),
    }
}

/// Fire `after_removed_power(state, old_owner)` to the listener
/// registered for `kind`, if any. Call site: immediately BEFORE the
/// `state.powers.retain` (or equivalent removal) prunes the power, so
/// the listener can still observe `state.powers` entries for the
/// instance being removed if it needs to read residual state. Bead
/// `ig66w` (HungerPower) and bead `6ww9w` (GraspPower) wire this in
/// `doormaker_swap_phase_power`.
pub fn dispatch_after_removed_for_power(
    state: &mut CombatState,
    old_owner: CreatureUid,
    kind: PowerKind,
) -> EffectResult {
    if is_empty_after_removed_power() {
        return EffectResult::NotImplemented;
    }
    let Some(handler) = lookup_after_removed_for_power(kind) else {
        return EffectResult::NotImplemented;
    };
    handler(state, old_owner)
}

// ---------------------------------------------------------------------------
// Channel: AfterDamageReceived
// ---------------------------------------------------------------------------

/// Handler signature for `after_damage_received_power`. Fired for each
/// `PowerKind` installed on the creature that just received a damage
/// event (`owner`), with the `dealer`, value-`props`, and the realised
/// `unblocked_damage` of the hit (C# `DamageResult.UnblockedDamage`).
/// Mirrors C# `PowerModel.AfterDamageReceived` where `target ==
/// base.Owner`. Callers dispatch unconditionally on every damage event —
/// even fully-blocked hits — because C# does too; listeners that C#
/// gates on `result.UnblockedDamage > 0` (PlowPower.cs:30) apply that
/// gate themselves, while listeners whose C# body ignores the
/// `DamageResult` (PersonalHivePower, CurlUp's latch) ignore the scalar.
/// Beads `sts2-rs-ybtmu`, unblocked threading for the Plow gate.
pub type AfterDamageReceivedPowerFn = fn(
    &mut CombatState,
    owner: CreatureUid,
    dealer: crate::hooks::CreatureRef,
    props: crate::hooks::ValueProps,
    unblocked_damage: i32,
) -> EffectResult;

static REGISTRY_AFTER_DAMAGE_RECEIVED_POWER: OnceLock<
    RwLock<BTreeMap<PowerKind, AfterDamageReceivedPowerFn>>,
> = OnceLock::new();

fn registry_after_damage_received_power(
) -> &'static RwLock<BTreeMap<PowerKind, AfterDamageReceivedPowerFn>> {
    REGISTRY_AFTER_DAMAGE_RECEIVED_POWER.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// Register a handler on the `after_damage_received_power` channel.
///
/// `kind` must be in the after-damage-received hook's trigger set
/// (`hooks::AFTER_DAMAGE_RECEIVED_TRIGGERS`): the dispatcher skips this
/// channel when no trigger kind is live, so an unlisted key's handler would
/// never run. Debug builds assert it here, where the keys become known.
pub fn register_after_damage_received_for_power(
    kind: PowerKind,
    handler: AfterDamageReceivedPowerFn,
) {
    debug_assert!(
        crate::hooks::PowerHook::AfterDamageReceivedEnemyTarget
            .triggers()
            .contains(kind),
        "PowerKind::{kind:?} has an after_damage_received handler but is not in \
         hooks::AFTER_DAMAGE_RECEIVED_TRIGGERS, so the dispatcher would skip it"
    );
    registry_after_damage_received_power()
        .write()
        .expect("monster-power hook registry poisoned")
        .insert(kind, handler);
}

/// Look up the registered handler for `kind`.
#[must_use]
pub fn lookup_after_damage_received_for_power(
    kind: PowerKind,
) -> Option<AfterDamageReceivedPowerFn> {
    registry_after_damage_received_power()
        .read()
        .expect("monster-power hook registry poisoned")
        .get(&kind)
        .copied()
}

/// `true` if no listeners are registered on the channel.
#[inline]
#[must_use]
pub fn is_empty_after_damage_received_power() -> bool {
    match REGISTRY_AFTER_DAMAGE_RECEIVED_POWER.get() {
        None => true,
        Some(lock) => lock
            .read()
            .expect("monster-power hook registry poisoned")
            .is_empty(),
    }
}

/// Fire the registered `after_damage_received_power` handler for every
/// `PowerKind` on the `target` creature (the creature that received the
/// damage event). C# `AfterDamageReceived` only fires on the damaged
/// creature's own powers, so — unlike the card-entered-combat lane —
/// this dispatcher is scoped to `target`'s `powers` vector, not a
/// whole-board walk. Call site: `hooks::dispatch_after_damage_received`,
/// before the player-only post-walk reactions. Bead `sts2-rs-ybtmu`.
pub fn dispatch_after_damage_received_on_monster_powers(
    state: &mut CombatState,
    target: CreatureUid,
    dealer: crate::hooks::CreatureRef,
    props: crate::hooks::ValueProps,
    unblocked_damage: i32,
) -> EffectResult {
    if is_empty_after_damage_received_power() {
        return EffectResult::NotImplemented;
    }
    // Snapshot the target's power kinds first so the per-handler call can
    // take `&mut CombatState` without holding an immutable borrow.
    let kinds: Vec<PowerKind> = {
        let Some(c) = state.creatures.iter().find(|c| c.id == target.0) else {
            return EffectResult::NotImplemented;
        };
        c.powers
            .iter()
            .filter_map(|pu| state.powers.get_by_id(pu.0).map(|p| p.kind))
            .collect()
    };
    let mut any_resolved = false;
    for kind in kinds {
        if let Some(handler) = lookup_after_damage_received_for_power(kind) {
            if handler(state, target, dealer, props, unblocked_damage) == EffectResult::Resolved {
                any_resolved = true;
            }
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// Every card UID across the player's draw / hand / discard / exhaust piles,
/// in that order. Shared helper for the per-card power-hook bodies below.
fn all_player_card_uids(state: &CombatState) -> smallvec::SmallVec<[CardUid; 32]> {
    let mut out: smallvec::SmallVec<[CardUid; 32]> = smallvec::SmallVec::new();
    out.extend(state.player.draw.iter().copied());
    out.extend(state.player.hand.iter().copied());
    out.extend(state.player.discard.iter().copied());
    out.extend(state.player.exhaust.iter().copied());
    // C# `PlayerCombatState.AllPiles` (PlayerCombatState.cs:73-88) includes the
    // Play pile, so a per-card power hook (Dampen downgrade/re-upgrade,
    // MonarchsGaze) must also cover the card CURRENTLY being played. Omitting it
    // left a mid-play card un-downgraded/un-re-upgraded (flash-of-steel seed
    // 5619). Bead follow-up to `sts2-rs-mw4fy`.
    out.extend(state.player.play.iter().copied());
    out
}

// ---------------------------------------------------------------------------
// DampenPower: downgrade all upgraded player cards when applied, re-upgrade
// when removed (C# `DampenPower.cs:30-76`). Bead `sts2-rs-mw4fy`.
// ---------------------------------------------------------------------------

fn dampen_after_applied(
    state: &mut CombatState,
    owner: CreatureUid,
    _applier: Option<CreatureUid>,
) -> EffectResult {
    // C# `DampenPower.cs:37` (`AfterApplied`) flashes unconditionally
    // whenever the power is applied — no guard beyond being applied at all.
    crate::event_log::flash_power(state, owner, PowerKind::Dampen);
    // Iterate all player cards across all piles and downgrade any that are
    // currently upgraded. Mark them with DampenDowngradedMarker so we can
    // re-upgrade them when the power is removed.
    let uids = all_player_card_uids(state);
    for uid in uids {
        if let Some(card) = state.cards.iter_mut().find(|c| c.uid == uid) {
            if card.upgraded {
                card.downgrade();
                // Mark this card as downgraded by Dampen so we can re-upgrade
                // it when Dampen is removed.
                card.set_combat_var(crate::cards::VarKey::DampenDowngradedMarker, 1);
            }
        }
    }
    EffectResult::Resolved
}

fn dampen_after_removed(state: &mut CombatState, _old_owner: CreatureUid) -> EffectResult {
    // When Dampen is removed, re-upgrade any cards that were marked as
    // downgraded by Dampen during the AfterApplied phase.
    let uids = all_player_card_uids(state);
    for uid in uids {
        if let Some(card) = state.cards.iter_mut().find(|c| c.uid == uid) {
            if card.combat_var(crate::cards::VarKey::DampenDowngradedMarker) != 0 {
                card.upgrade();
                // Clear the marker.
                card.set_combat_var(crate::cards::VarKey::DampenDowngradedMarker, 0);
            }
        }
    }
    EffectResult::Resolved
}

/// Idempotent installer for the DampenPower hook bodies.
pub fn install_dampen_power_handlers() {
    register_after_applied_for_power(PowerKind::Dampen, dampen_after_applied);
    register_after_removed_for_power(PowerKind::Dampen, dampen_after_removed);
}

// ---------------------------------------------------------------------------
// VitalSparkPower hook bodies (v0.107.1 rewrite, bead `…wb8lk.4`)
// ---------------------------------------------------------------------------
//
// C# source: `MegaCrit.Sts2.Core.Models.Powers/VitalSparkPower.cs` (v0.107.1).
// VitalSpark (Infested Prism, enemy power) is the in-combat source of the
// Tainted keyword. It no longer grants energy (v0.103.2 behavior removed):
//   * BeforeCombatStart       → afflict every ally Skill card with Tainted.
//   * AfterCardEnteredCombat  → afflict a newly-added, unafflicted Skill card.
//   * AfterCardPlayed         → stack TaintedPower on the player who played a
//                               Tainted card (wired in `hooks`).
//   * AfterRemoved            → clear every Tainted card affliction.
//   * AfterPowerAmountChanged → re-sync card affliction amounts (DEFERRED: no
//                               in-game mechanic changes VitalSpark's amount
//                               mid-combat, so this hook is unreachable in the
//                               single-player sim; see fidelity notes).

/// The first VitalSpark amount installed on `owner` (Counter stack), or 0.
fn vital_spark_amount(state: &CombatState, owner: CreatureUid) -> i32 {
    state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().find_map(|pu| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == pu.0 && p.kind == PowerKind::VitalSpark)
                    .map(|p| p.amount)
            })
        })
        .unwrap_or(0)
}

/// Afflict a single card with Tainted(amount). The Skill-only filter is
/// enforced by [`crate::cards::Card::set_affliction`] (Tainted's
/// `CanAfflictCardType`). When `only_if_unafflicted` is set (the
/// AfterCardEnteredCombat path, which mirrors C#'s `card.Affliction == null`
/// guard) an already-afflicted card is skipped; the BeforeCombatStart path
/// passes `false` (C# afflicts every Skill card unconditionally).
fn vital_spark_afflict_card(
    state: &mut CombatState,
    amount: i32,
    card_uid: CardUid,
    only_if_unafflicted: bool,
) {
    use crate::card_afflictions::{CardAffliction, CardAfflictionKind};
    let idx = (card_uid.0.get() as usize).saturating_sub(1);
    let Some(card) = state.cards.get_mut(idx) else {
        return;
    };
    if only_if_unafflicted && card.affliction.is_some() {
        return;
    }
    let _ = card.set_affliction(CardAffliction::new(CardAfflictionKind::Tainted, amount));
}

/// C# `VitalSparkPower.BeforeCombatStart`: for each VitalSpark instance, afflict
/// every ally (player) Skill card with Tainted(Amount). Called from
/// [`crate::combat_loop::start_combat`] after the generic before-combat-start
/// dispatch — VitalSpark is installed on the enemy at spawn (`AfterAddedToRoom`),
/// before the player's combat deck exists, so the tainting must wait for combat
/// start. Dedicated (not registry-driven) because the generic
/// `before_combat_start` channel takes `&CombatState` and cannot mutate cards.
pub fn vital_spark_before_combat_start(state: &mut CombatState) {
    // Each alive creature's VitalSpark amount (typically one — Infested Prism).
    let amounts: smallvec::SmallVec<[i32; 2]> = state
        .creatures
        .iter()
        .filter(|c| c.current_hp > 0)
        .flat_map(|c| c.powers.iter().copied())
        .filter_map(|pu| {
            state
                .powers
                .iter()
                .find(|p| p.id == pu.0 && p.kind == PowerKind::VitalSpark)
                .map(|p| p.amount)
        })
        .collect();
    if amounts.is_empty() {
        return;
    }
    let uids = all_player_card_uids(state);
    for amount in amounts {
        for uid in uids.iter().copied() {
            vital_spark_afflict_card(state, amount, uid, /*only_if_unafflicted=*/ false);
        }
    }
}

/// C# `VitalSparkPower.AfterCardEnteredCombat`: a newly-added card with no
/// affliction and `Type == Skill` gets Tainted(Amount).
fn vital_spark_after_card_entered_combat(
    state: &mut CombatState,
    owner: CreatureUid,
    card_uid: CardUid,
) -> EffectResult {
    let amount = vital_spark_amount(state, owner);
    if amount <= 0 {
        return EffectResult::NotImplemented;
    }
    vital_spark_afflict_card(state, amount, card_uid, /*only_if_unafflicted=*/ true);
    EffectResult::Resolved
}

/// C# `VitalSparkPower.AfterRemoved`: clear every ally card's Tainted affliction.
fn vital_spark_after_removed(state: &mut CombatState, _old_owner: CreatureUid) -> EffectResult {
    use crate::card_afflictions::CardAfflictionKind;
    let uids = all_player_card_uids(state);
    for uid in uids {
        let idx = (uid.0.get() as usize).saturating_sub(1);
        let Some(card) = state.cards.get_mut(idx) else {
            continue;
        };
        if matches!(
            card.affliction,
            Some(crate::card_afflictions::CardAffliction {
                kind: CardAfflictionKind::Tainted,
                ..
            })
        ) {
            let _ = card.clear_affliction();
        }
    }
    EffectResult::Resolved
}

/// Idempotent installer for the VitalSparkPower registry hook bodies
/// (AfterCardEnteredCombat + AfterRemoved). The BeforeCombatStart +
/// AfterCardPlayed legs are dedicated core functions (see
/// [`vital_spark_before_combat_start`] and `hooks::vital_spark_after_card_played`).
pub fn install_vital_spark_power_handlers() {
    register_after_card_entered_combat_for_power(
        PowerKind::VitalSpark,
        vital_spark_after_card_entered_combat,
    );
    register_after_removed_for_power(PowerKind::VitalSpark, vital_spark_after_removed);
}

// ---------------------------------------------------------------------------
// SmoggyPower hook bodies (v0.107.1)
// ---------------------------------------------------------------------------
//
// C# source: `MegaCrit.Sts2.Core.Models.Powers/SmoggyPower.cs` (v0.107.1);
// `Type => Debuff`, `StackType => Single`. LivingFog's ADVANCED_GAS applies it
// to the player (`sts2-content/src/monsters/act1_underdocks.rs:836-838`), and
// it is the sole source of the `Smog` card-affliction — `Smog.cs` is a pure
// marker whose doc-comment reads "This Affliction has no logic of its own. All
// logic lives in SmoggyPower".
//
// The four hooks, and where each lives:
//
//   * `AfterCardPlayed` (`SmoggyPower.cs:22-37`) — the owner playing a SKILL
//     afflicts every unafflicted Skill in `Owner.Player.PlayerCombatState
//     .AllCards` with `Smog(1)`. Net effect: one Skill per turn.
//     → [`smoggy_after_card_played`], fired from `hooks::dispatch_after_card_played`.
//   * `AfterCardEnteredCombat` (`SmoggyPower.cs:39-45`) — a card ENTERING
//     combat unafflicted, as a Skill, while a Skill play has already started
//     this turn, is afflicted too (otherwise a generated Skill would dodge the
//     lockout). → [`smoggy_after_card_entered_combat`], fired from
//     [`dispatch_after_card_entered_combat_on_monster_powers`].
//   * `AfterSideTurnEnd` (`SmoggyPower.cs:47-62`) — at the OWNER's side turn
//     end, every `Smog` affliction in `AllCards` is cleared.
//     → [`smoggy_after_side_turn_end`], fired from `combat_loop`'s player
//     turn-end phase two.
//   * `ShouldPlay` (`SmoggyPower.cs:64-71`) — a `Smog`-afflicted card of the
//     owner cannot be played. → `combat_loop::player_should_play_blocked`
//     (the engine's ShouldPlay union).
//
// **Why these are dedicated functions and not registry listeners.** Unlike
// VitalSpark (an ENEMY-owned power the registry walk finds by iterating
// `state.creatures[..].powers`), Smoggy sits on the PLAYER, and both real
// application paths write the LEGACY player-power surface rather than the d03
// mirror Creature: `apply_power_to_player_via_legacy_surface`
// (`act1_underdocks.rs::living_fog_advanced_gas_effect`) and the live-board
// wire reconstruction (`sts2-py/src/state_dict.rs:3041-3059`, which pushes onto
// `state.player.powers`). A registry listener keyed on `PowerKind::Smoggy`
// would therefore never fire on exactly the states this port exists to model.
// Gating on [`CombatState::player_power_amount`] (the mirror+legacy union)
// mirrors the player-owned PhantomBlades listener that
// `card_effects::fire_after_card_entered_combat` already carries inline.
//
// Single-player scope: C# guards each hook on the card's owner being the
// power's owner (`cardPlay.Card.Owner.Creature != base.Owner`,
// `card.Owner == base.Owner.Player`). Every card in `state.cards` reachable
// through the player's piles is player-owned in this engine, so the guard is
// structurally satisfied by walking [`all_player_card_uids`].

/// `true` while the player carries [`PowerKind::Smoggy`] (mirror+legacy union).
/// C# has no explicit presence test — the hook bodies only run because a
/// `SmoggyPower` instance is subscribed — so this is the Rust stand-in for
/// "the listener exists".
fn smoggy_active(state: &CombatState) -> bool {
    state.player_power_amount(PowerKind::Smoggy) > 0
}

/// Afflict one card with `Smog(1)` iff it is an unafflicted Skill — C#'s
/// `item.Type == CardType.Skill && item.Affliction == null` guard
/// (`SmoggyPower.cs:32`) / the `AfterCardEnteredCombat` triple-guard
/// (`SmoggyPower.cs:41`). A card that already carries ANY affliction keeps its
/// original one (C# `CardModel.Affliction` is a single slot and the guard skips
/// occupied slots outright).
fn smog_afflict_if_unafflicted_skill(state: &mut CombatState, card_uid: CardUid) -> bool {
    use crate::card_afflictions::{CardAffliction, CardAfflictionKind};
    let idx = (card_uid.0.get() as usize).saturating_sub(1);
    let Some(card) = state.cards.get_mut(idx) else {
        return false;
    };
    if card.affliction.is_some() || card.card_type != crate::cards::CardType::Skill {
        return false;
    }
    // `CardCmd.Afflict<Smog>(item, 1m)` — amount 1, matching the C# literal.
    card.set_affliction(CardAffliction::new(CardAfflictionKind::Smog, 1))
}

/// C# `CombatManager.Instance.History.CardPlaysStarted.Any(e =>
/// e.HappenedThisTurn(CombatState) && e.CardPlay.Card.Type == CardType.Skill &&
/// e.CardPlay.Card.Owner.Creature == Owner)` (`SmoggyPower.cs:41`).
///
/// The engine keeps no typed card-play history, so the predicate is
/// reconstructed from the two pieces of state that together cover "a Skill play
/// has STARTED this turn":
///
/// 1. [`PlayerCombatState::cards_played_this_turn`]'s Skill bucket — plays that
///    have already FINISHED this turn (tallied at play end).
/// 2. The Play pile — `play_card` moves the card Hand→Play *before* its effect
///    dispatch (`combat_loop.rs:5964`), so a Skill whose `OnPlay` generates a
///    card is in flight there. This is precisely the window where C#'s
///    started-vs-finished distinction is observable.
fn player_skill_play_started_this_turn(state: &CombatState) -> bool {
    if state.player.cards_played_this_turn[crate::cards::CardType::Skill.index()] > 0 {
        return true;
    }
    state.player.play.iter().any(|uid| {
        let idx = (uid.0.get() as usize).saturating_sub(1);
        state
            .cards
            .get(idx)
            .is_some_and(|c| c.card_type == crate::cards::CardType::Skill)
    })
}

/// C# `SmoggyPower.AfterCardPlayed` (`SmoggyPower.cs:22-37`): when the owner
/// plays a **Skill**, every Skill in `AllCards` that has no affliction yet is
/// afflicted with `Smog(1)` — locking out the rest of the turn's Skills via
/// [`crate::combat_loop::player_should_play_blocked`]. `played_card_type` is
/// the played card's type from the `AfterCardPlayed` context (C#
/// `cardPlay.Card.Type`); a `None` (no-card-known sentinel) is a no-op.
pub fn smoggy_after_card_played(
    state: &mut CombatState,
    played_card_type: Option<crate::cards::CardType>,
) {
    if played_card_type != Some(crate::cards::CardType::Skill) {
        return;
    }
    if !smoggy_active(state) {
        return;
    }
    // C# `SmoggyPower.cs:28` flashes inside this same guard, before the
    // afflict loop.
    crate::event_log::flash_player_power(state, PowerKind::Smoggy);
    let uids = all_player_card_uids(state);
    for uid in uids {
        smog_afflict_if_unafflicted_skill(state, uid);
    }
}

/// C# `SmoggyPower.AfterCardEnteredCombat` (`SmoggyPower.cs:39-45`): a card
/// entering combat afresh gets `Smog(1)` iff it is the owner's, unafflicted, a
/// Skill, and a Skill play has already started this turn. Without this leg a
/// Skill generated after the turn's Skill play (Discovery / a transform / a
/// status spawn) would be freely playable while every pre-existing Skill was
/// locked.
pub fn smoggy_after_card_entered_combat(
    state: &mut CombatState,
    card_uid: CardUid,
) -> EffectResult {
    if !smoggy_active(state) || !player_skill_play_started_this_turn(state) {
        return EffectResult::NotImplemented;
    }
    if smog_afflict_if_unafflicted_skill(state, card_uid) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// C# `SmoggyPower.AfterSideTurnEnd` (`SmoggyPower.cs:47-62`): at the end of
/// the side turn the owner participates in, clear the `Smog` affliction from
/// every card in `AllCards` — so next turn's first Skill is playable again.
/// Only `Smog` is cleared; a card carrying a different affliction is untouched.
///
/// The owner is always the player at this pin (only LivingFog's ADVANCED_GAS
/// applies Smoggy, and it targets the player), so the C# `participants.Contains
/// (Owner)` guard reduces to "fire on the PLAYER's side turn end" — the call
/// site in `combat_loop`'s player turn-end phase two.
pub fn smoggy_after_side_turn_end(state: &mut CombatState) {
    use crate::card_afflictions::{CardAffliction, CardAfflictionKind};
    if !smoggy_active(state) {
        return;
    }
    let uids = all_player_card_uids(state);
    for uid in uids {
        let idx = (uid.0.get() as usize).saturating_sub(1);
        let Some(card) = state.cards.get_mut(idx) else {
            continue;
        };
        if matches!(
            card.affliction,
            Some(CardAffliction {
                kind: CardAfflictionKind::Smog,
                ..
            })
        ) {
            let _ = card.clear_affliction();
        }
    }
}

// ---------------------------------------------------------------------------
// TangledPower hook bodies
// ---------------------------------------------------------------------------
//
// C# source: `MegaCrit.Sts2.Core.Models.Powers/TangledPower.cs`. Vine
// Shambler's GRASPING_VINES applies it to the player through the legacy
// player surface (as Smoggy, so these are dedicated functions, not registry
// listeners). `Entangled` is a pure marker (`Afflictions/Entangled.cs`); the
// power holds its logic:
//
//   * `AfterApplied` (a fresh instance only, `PowerCmd.Apply`) — every player
//     Attack in `AllCards` with no affliction is Entangled.
//     → [`tangled_after_applied`], called by the applier.
//   * `AfterCardEnteredCombat` — an unafflicted Attack entering combat is
//     Entangled. → [`tangled_after_card_entered_combat`], on the
//     card-entered bus.
//   * `AfterRemoved` — every Entangled card is cleared.
//     → [`tangled_after_removed`], from `Effect::RemovePower`.
//   * `TryModifyEnergyCostInCombat` — an Entangled card costs `Amount` more
//     (the `PowerKind::Tangled` arm in `hooks::modify_card_cost`).

/// Entangle one card iff it is an Attack with no affliction (C#
/// `card.Affliction == null && card.Type == CardType.Attack`; the AfterApplied
/// path reaches the same set through `AfflictionModel.CanAfflict`, Entangled
/// not being stackable).
fn entangle_if_unafflicted_attack(state: &mut CombatState, card_uid: CardUid) -> bool {
    use crate::card_afflictions::{CardAffliction, CardAfflictionKind};
    let idx = (card_uid.0.get() as usize).saturating_sub(1);
    let Some(card) = state.cards.get_mut(idx) else {
        return false;
    };
    if card.affliction.is_some() || card.card_type != crate::cards::CardType::Attack {
        return false;
    }
    // `CardCmd.Afflict<Entangled>(item, 1m)`.
    card.set_affliction(CardAffliction::new(CardAfflictionKind::Entangled, 1))
}

/// C# `TangledPower.AfterApplied` (`TangledPower.cs:23-30`): Entangle every
/// unafflicted player Attack in `AllCards`. The applier calls it when it puts
/// a fresh Tangled instance on the player (C# fires `AfterApplied` only then,
/// not when an existing instance stacks).
pub fn tangled_after_applied(state: &mut CombatState) {
    for uid in all_player_card_uids(state) {
        entangle_if_unafflicted_attack(state, uid);
    }
}

/// C# `TangledPower.AfterCardEnteredCombat` (`TangledPower.cs:32-38`).
fn tangled_after_card_entered_combat(state: &mut CombatState, card_uid: CardUid) -> EffectResult {
    if state.player_power_amount(PowerKind::Tangled) <= 0 {
        return EffectResult::NotImplemented;
    }
    if entangle_if_unafflicted_attack(state, card_uid) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// C# `TangledPower.AfterRemoved` (`TangledPower.cs:49-57`): clear every
/// Entangled card. Called once the player's Tangled is gone.
pub fn tangled_after_removed(state: &mut CombatState) {
    use crate::card_afflictions::{CardAffliction, CardAfflictionKind};
    for uid in all_player_card_uids(state) {
        let idx = (uid.0.get() as usize).saturating_sub(1);
        let Some(card) = state.cards.get_mut(idx) else {
            continue;
        };
        if matches!(
            card.affliction,
            Some(CardAffliction {
                kind: CardAfflictionKind::Entangled,
                ..
            })
        ) {
            let _ = card.clear_affliction();
        }
    }
}

// ---------------------------------------------------------------------------
// Test-support: clear all registries (for nextest's process-global state)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) fn clear_all_for_test() {
    if let Some(lock) = REGISTRY_AFTER_APPLIED_POWER.get() {
        lock.write().unwrap().clear();
    }
    if let Some(lock) = REGISTRY_AFTER_CARD_ENTERED_COMBAT_POWER.get() {
        lock.write().unwrap().clear();
    }
    if let Some(lock) = REGISTRY_AFTER_REMOVED_POWER.get() {
        lock.write().unwrap().clear();
    }
    if let Some(lock) = REGISTRY_AFTER_DAMAGE_RECEIVED_POWER.get() {
        lock.write().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_applied(
        _state: &mut CombatState,
        _owner: CreatureUid,
        _applier: Option<CreatureUid>,
    ) -> EffectResult {
        EffectResult::Resolved
    }
    fn dummy_card_entered(
        _state: &mut CombatState,
        _owner: CreatureUid,
        _card_uid: CardUid,
    ) -> EffectResult {
        EffectResult::Resolved
    }
    fn dummy_removed(_state: &mut CombatState, _old_owner: CreatureUid) -> EffectResult {
        EffectResult::Resolved
    }

    #[test]
    fn registries_start_empty() {
        clear_all_for_test();
        assert!(is_empty_after_applied_power());
        assert!(is_empty_after_card_entered_combat_power());
        assert!(is_empty_after_removed_power());
    }

    #[test]
    fn register_then_lookup_round_trips() {
        clear_all_for_test();
        register_after_applied_for_power(PowerKind::Tainted, dummy_applied);
        register_after_card_entered_combat_for_power(PowerKind::Tainted, dummy_card_entered);
        register_after_removed_for_power(PowerKind::Tainted, dummy_removed);

        assert!(!is_empty_after_applied_power());
        assert!(!is_empty_after_card_entered_combat_power());
        assert!(!is_empty_after_removed_power());

        assert!(lookup_after_applied_for_power(PowerKind::Tainted).is_some());
        assert!(lookup_after_card_entered_combat_for_power(PowerKind::Tainted).is_some());
        assert!(lookup_after_removed_for_power(PowerKind::Tainted).is_some());

        // Unregistered kind returns None.
        assert!(lookup_after_applied_for_power(PowerKind::WitheringPresence).is_none());

        clear_all_for_test();
    }

    #[test]
    fn register_overwrites_per_kind() {
        clear_all_for_test();
        fn other(_: &mut CombatState, _: CreatureUid, _: Option<CreatureUid>) -> EffectResult {
            EffectResult::NotImplemented
        }
        register_after_applied_for_power(PowerKind::Tainted, dummy_applied);
        register_after_applied_for_power(PowerKind::Tainted, other);
        let handler = lookup_after_applied_for_power(PowerKind::Tainted).unwrap();
        // Can't compare fn pointers in stable Rust reliably; verify via
        // behavior (the second registration's body returns NotImplemented).
        let mut state = CombatState::default();
        let result = handler(&mut state, CreatureUid::new(1).unwrap(), None);
        assert_eq!(result, EffectResult::NotImplemented);
        clear_all_for_test();
    }

    // -----------------------------------------------------------------------
    // SmoggyPower (`SmoggyPower.cs`, v0.107.1)
    // -----------------------------------------------------------------------

    use crate::card_afflictions::{CardAffliction, CardAfflictionKind};
    use crate::cards::{Card, CardType};
    use crate::CardUid;

    /// Push a card at `uid` (the arena is indexed by `uid - 1`) into the draw
    /// pile and return its uid.
    fn push_card(state: &mut CombatState, uid: u32, card_type: CardType) -> CardUid {
        let uid = CardUid::new(uid).unwrap();
        state.cards.push(Card {
            uid,
            card_type,
            ..Card::pod_defaults()
        });
        state.player.draw.push(uid);
        uid
    }

    /// Install `Smoggy(1)` on the LEGACY player-power surface — the surface
    /// both real appliers use (LivingFog ADVANCED_GAS via
    /// `apply_power_to_player_via_legacy_surface`, and the live-board wire
    /// reconstruction).
    fn give_player_smoggy(state: &mut CombatState) {
        let id = std::num::NonZeroU32::new(
            state
                .powers
                .iter()
                .map(|p| p.id.get())
                .max()
                .map_or(1, |m| m + 1),
        )
        .unwrap();
        state.powers.push(crate::Power {
            id,
            kind: PowerKind::Smoggy,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(id));
    }

    fn is_smog(state: &CombatState, uid: CardUid) -> bool {
        matches!(
            state.cards[uid.0.get() as usize - 1].affliction,
            Some(CardAffliction {
                kind: CardAfflictionKind::Smog,
                ..
            })
        )
    }

    // -----------------------------------------------------------------------
    // TangledPower (`TangledPower.cs`)
    // -----------------------------------------------------------------------

    /// Tangled(1) on the legacy player surface, as GRASPING_VINES applies it;
    /// returns the power's id.
    fn give_player_tangled(state: &mut CombatState) -> std::num::NonZeroU32 {
        let id = std::num::NonZeroU32::new(
            state
                .powers
                .iter()
                .map(|p| p.id.get())
                .max()
                .map_or(1, |m| m + 1),
        )
        .unwrap();
        state.powers.push(crate::Power {
            id,
            kind: PowerKind::Tangled,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(id));
        id
    }

    fn affliction_of(state: &CombatState, uid: CardUid) -> Option<CardAfflictionKind> {
        state.cards[uid.0.get() as usize - 1]
            .affliction
            .map(|a| a.kind)
    }

    /// C# `TangledPower.AfterApplied` (`TangledPower.cs:23-30`): every player
    /// Attack in `AllCards` is afflicted with Entangled; a Skill isn't, and an
    /// Attack already carrying another affliction keeps it (`Entangled` isn't
    /// stackable, so `AfflictionModel.CanAfflict` refuses it).
    #[test]
    fn tangled_applied_entangles_every_unafflicted_attack() {
        let mut state = CombatState::default();
        let drawn = push_card(&mut state, 1, CardType::Attack);
        let in_hand = push_card(&mut state, 2, CardType::Attack);
        state.player.draw.retain(|u| *u != in_hand);
        state.player.hand.push(in_hand);
        let skill = push_card(&mut state, 3, CardType::Skill);
        let smogged = push_card(&mut state, 4, CardType::Attack);
        state.cards[3].affliction = Some(CardAffliction::new(CardAfflictionKind::Smog, 1));
        give_player_tangled(&mut state);
        tangled_after_applied(&mut state);
        assert_eq!(
            affliction_of(&state, drawn),
            Some(CardAfflictionKind::Entangled)
        );
        assert_eq!(
            affliction_of(&state, in_hand),
            Some(CardAfflictionKind::Entangled)
        );
        assert_eq!(affliction_of(&state, skill), None);
        assert_eq!(
            affliction_of(&state, smogged),
            Some(CardAfflictionKind::Smog)
        );
    }

    /// C# `TangledPower.AfterCardEnteredCombat` (`TangledPower.cs:32-38`): an
    /// unafflicted Attack entering combat while Tangled lasts is Entangled; one
    /// entering without Tangled is not.
    #[test]
    fn tangled_entangles_an_attack_entering_combat_while_it_lasts() {
        let mut state = CombatState::default();
        let before = push_card(&mut state, 1, CardType::Attack);
        let _ = dispatch_after_card_entered_combat_on_monster_powers(&mut state, before);
        assert_eq!(affliction_of(&state, before), None);
        give_player_tangled(&mut state);
        let during = push_card(&mut state, 2, CardType::Attack);
        let _ = dispatch_after_card_entered_combat_on_monster_powers(&mut state, during);
        assert_eq!(
            affliction_of(&state, during),
            Some(CardAfflictionKind::Entangled)
        );
        let skill = push_card(&mut state, 3, CardType::Skill);
        let _ = dispatch_after_card_entered_combat_on_monster_powers(&mut state, skill);
        assert_eq!(affliction_of(&state, skill), None);
    }

    /// C# `TangledPower.AfterRemoved` (`TangledPower.cs:49-57`) clears every
    /// Entangled card, and `TryModifyEnergyCostInCombat` (`:59-68`) surcharges
    /// only an Entangled card: an Attack with another affliction costs its
    /// base while Tangled lasts.
    #[test]
    fn tangled_surcharges_entangled_cards_only_and_its_removal_clears_them() {
        let mut state = CombatState::default();
        let attack = push_card(&mut state, 1, CardType::Attack);
        let smogged = push_card(&mut state, 2, CardType::Attack);
        let other = push_card(&mut state, 3, CardType::Skill);
        for c in &mut state.cards {
            c.cost = 1;
        }
        state.cards[1].affliction = Some(CardAffliction::new(CardAfflictionKind::Smog, 1));
        state.cards[2].affliction = Some(CardAffliction::new(CardAfflictionKind::Smog, 1));
        let id = give_player_tangled(&mut state);
        tangled_after_applied(&mut state);
        assert_eq!(crate::hooks::effective_card_cost(&state, attack), 2);
        assert_eq!(crate::hooks::effective_card_cost(&state, smogged), 1);
        crate::hooks::apply_effects(
            &mut state,
            &[crate::hooks::Effect::RemovePower {
                listener: crate::hooks::ListenerRef::Power(crate::PowerUid(id)),
            }],
        );
        assert_eq!(affliction_of(&state, attack), None);
        assert_eq!(
            affliction_of(&state, smogged),
            Some(CardAfflictionKind::Smog)
        );
        assert_eq!(affliction_of(&state, other), Some(CardAfflictionKind::Smog));
        assert_eq!(crate::hooks::effective_card_cost(&state, attack), 1);
    }

    /// (a) C# `SmoggyPower.AfterCardPlayed` (`SmoggyPower.cs:22-37`): playing a
    /// Skill smogs every OTHER unafflicted Skill in `AllCards` — Attacks and
    /// Powers are untouched (`item.Type == CardType.Skill` guard).
    #[test]
    fn smoggy_skill_play_afflicts_skills_but_not_attacks() {
        let mut state = CombatState::default();
        give_player_smoggy(&mut state);
        let skill_a = push_card(&mut state, 1, CardType::Skill);
        let skill_b = push_card(&mut state, 2, CardType::Skill);
        let attack = push_card(&mut state, 3, CardType::Attack);
        let power = push_card(&mut state, 4, CardType::Power);

        // Drive the real bus, not the helper, so the `dispatch_after_card_played`
        // wiring is covered too.
        crate::hooks::dispatch_after_card_played(
            &mut state,
            &crate::hooks::CardCtx {
                card_uid: Some(skill_a),
                card_type: Some(CardType::Skill),
                ..crate::hooks::CardCtx::default()
            },
        );

        assert!(is_smog(&state, skill_a), "the played Skill is smogged too");
        assert!(is_smog(&state, skill_b), "every other Skill is smogged");
        assert!(
            state.cards[attack.0.get() as usize - 1]
                .affliction
                .is_none(),
            "Attacks are not Skills — no Smog"
        );
        assert!(
            state.cards[power.0.get() as usize - 1].affliction.is_none(),
            "Powers are not Skills — no Smog"
        );
    }

    /// Playing an ATTACK does not smog anything — C# returns early unless
    /// `cardPlay.Card.Type == CardType.Skill`.
    #[test]
    fn smoggy_attack_play_afflicts_nothing() {
        let mut state = CombatState::default();
        give_player_smoggy(&mut state);
        let skill = push_card(&mut state, 1, CardType::Skill);
        let attack = push_card(&mut state, 2, CardType::Attack);

        crate::hooks::dispatch_after_card_played(
            &mut state,
            &crate::hooks::CardCtx {
                card_uid: Some(attack),
                card_type: Some(CardType::Attack),
                ..crate::hooks::CardCtx::default()
            },
        );

        assert!(
            state.cards[skill.0.get() as usize - 1].affliction.is_none(),
            "an Attack play must not trigger the Smog spread"
        );
    }

    /// The AfterCardPlayed spread reaches every pile of C#
    /// `PlayerCombatState.AllCards` (Hand / Draw / Discard / Exhaust / Play),
    /// not just the hand.
    #[test]
    fn smoggy_spread_covers_every_pile() {
        let mut state = CombatState::default();
        give_player_smoggy(&mut state);
        // push_card seeds the draw pile; relocate one card into each other pile.
        let draw = push_card(&mut state, 1, CardType::Skill);
        let hand = push_card(&mut state, 2, CardType::Skill);
        let discard = push_card(&mut state, 3, CardType::Skill);
        let exhaust = push_card(&mut state, 4, CardType::Skill);
        let play = push_card(&mut state, 5, CardType::Skill);
        state.player.draw.retain(|u| *u == draw);
        state.player.hand.push(hand);
        state.player.discard.push(discard);
        state.player.exhaust.push(exhaust);
        state.player.play.push(play);

        smoggy_after_card_played(&mut state, Some(CardType::Skill));

        for uid in [draw, hand, discard, exhaust, play] {
            assert!(is_smog(&state, uid), "card {uid:?} must be smogged");
        }
    }

    /// (e) C#'s `item.Affliction == null` guard: a card that already carries a
    /// DIFFERENT affliction keeps it (the single `CardModel.Affliction` slot is
    /// never overwritten).
    #[test]
    fn smoggy_skips_already_afflicted_cards() {
        let mut state = CombatState::default();
        give_player_smoggy(&mut state);
        let bound = push_card(&mut state, 1, CardType::Skill);
        state.cards[0].affliction = Some(CardAffliction::new(CardAfflictionKind::Bound, 2));
        let plain = push_card(&mut state, 2, CardType::Skill);

        smoggy_after_card_played(&mut state, Some(CardType::Skill));

        assert!(
            matches!(
                state.cards[bound.0.get() as usize - 1].affliction,
                Some(CardAffliction {
                    kind: CardAfflictionKind::Bound,
                    stacks: 2,
                    ..
                })
            ),
            "an already-afflicted card keeps its original affliction"
        );
        assert!(is_smog(&state, plain));
    }

    /// (b) C# `SmoggyPower.ShouldPlay` (`SmoggyPower.cs:64-71`): a Smog-carrying
    /// player card is unplayable while the player has SmoggyPower, and playable
    /// again without it. This is the surface the live game reports as
    /// `playable:false` / `unplayable_reason:"BlockedByHook"`.
    #[test]
    fn smoggy_should_play_blocks_smogged_card_only_while_power_is_held() {
        let mut state = CombatState::default();
        let uid = push_card(&mut state, 1, CardType::Skill);
        state.cards[0].affliction = Some(CardAffliction::new(CardAfflictionKind::Smog, 1));
        let smogged = state.cards[uid.0.get() as usize - 1];

        assert!(
            !crate::combat_loop::player_should_play_blocked(&state, &smogged),
            "no SmoggyPower ⇒ a Smog-marked card is still playable"
        );

        give_player_smoggy(&mut state);
        assert!(
            crate::combat_loop::player_should_play_blocked(&state, &smogged),
            "SmoggyPower + Smog affliction ⇒ BlockedByHook"
        );

        // An unafflicted card in the same state stays playable.
        let clean = push_card(&mut state, 2, CardType::Skill);
        let clean_card = state.cards[clean.0.get() as usize - 1];
        assert!(
            !crate::combat_loop::player_should_play_blocked(&state, &clean_card),
            "SmoggyPower alone must not block an unafflicted card"
        );
    }

    /// (c) C# `SmoggyPower.AfterCardEnteredCombat` (`SmoggyPower.cs:39-45`): a
    /// Skill entering combat AFTER a Skill play started this turn is smogged;
    /// with no Skill play yet this turn it is not.
    #[test]
    fn smoggy_after_card_entered_combat_gated_on_a_skill_play_this_turn() {
        clear_all_for_test();
        let mut state = CombatState::default();
        give_player_smoggy(&mut state);
        let early = push_card(&mut state, 1, CardType::Skill);

        // No Skill played yet this turn → the fresh card stays clean.
        let _ = dispatch_after_card_entered_combat_on_monster_powers(&mut state, early);
        assert!(
            state.cards[early.0.get() as usize - 1].affliction.is_none(),
            "no Skill play started this turn ⇒ no Smog on entry"
        );

        // A finished Skill play this turn arms the hook.
        state.player.cards_played_this_turn[CardType::Skill.index()] = 1;
        let late = push_card(&mut state, 2, CardType::Skill);
        let late_attack = push_card(&mut state, 3, CardType::Attack);
        let _ = dispatch_after_card_entered_combat_on_monster_powers(&mut state, late);
        let _ = dispatch_after_card_entered_combat_on_monster_powers(&mut state, late_attack);

        assert!(
            is_smog(&state, late),
            "a Skill entering after this turn's Skill play is smogged"
        );
        assert!(
            state.cards[late_attack.0.get() as usize - 1]
                .affliction
                .is_none(),
            "a non-Skill entering combat is never smogged"
        );
    }

    /// The `CardPlaysStarted` predicate counts STARTED plays, so a Skill whose
    /// own `OnPlay` generates a card (the generator is still in the Play pile)
    /// already arms the entered-combat hook.
    #[test]
    fn smoggy_after_card_entered_combat_sees_an_in_flight_skill_play() {
        clear_all_for_test();
        let mut state = CombatState::default();
        give_player_smoggy(&mut state);
        // In-flight Skill: moved Hand→Play by `play_card` before its dispatch,
        // with the per-turn finished-play buckets still at zero.
        let in_flight = push_card(&mut state, 1, CardType::Skill);
        state.player.draw.retain(|u| *u != in_flight);
        state.player.play.push(in_flight);

        let generated = push_card(&mut state, 2, CardType::Skill);
        let _ = dispatch_after_card_entered_combat_on_monster_powers(&mut state, generated);

        assert!(
            is_smog(&state, generated),
            "an in-flight Skill play counts as started (C# CardPlaysStarted)"
        );
    }

    /// (d) C# `SmoggyPower.AfterSideTurnEnd` (`SmoggyPower.cs:47-62`): the
    /// owner's side turn end clears every Smog affliction — and ONLY Smog.
    #[test]
    fn smoggy_side_turn_end_clears_only_smog() {
        let mut state = CombatState::default();
        give_player_smoggy(&mut state);
        let smogged = push_card(&mut state, 1, CardType::Skill);
        let bound = push_card(&mut state, 2, CardType::Skill);
        state.cards[1].affliction = Some(CardAffliction::new(CardAfflictionKind::Bound, 2));

        smoggy_after_card_played(&mut state, Some(CardType::Skill));
        assert!(is_smog(&state, smogged));

        smoggy_after_side_turn_end(&mut state);

        assert!(
            state.cards[smogged.0.get() as usize - 1]
                .affliction
                .is_none(),
            "side-turn end clears the Smog affliction"
        );
        assert!(
            matches!(
                state.cards[bound.0.get() as usize - 1].affliction,
                Some(CardAffliction {
                    kind: CardAfflictionKind::Bound,
                    ..
                })
            ),
            "a non-Smog affliction survives the Smoggy clear"
        );
        // …and the card is playable again next turn.
        let card = state.cards[smogged.0.get() as usize - 1];
        assert!(!crate::combat_loop::player_should_play_blocked(
            &state, &card
        ));
    }

    /// Without SmoggyPower none of the three reactive legs do anything.
    #[test]
    fn smoggy_legs_are_inert_without_the_power() {
        clear_all_for_test();
        let mut state = CombatState::default();
        let skill = push_card(&mut state, 1, CardType::Skill);
        state.player.cards_played_this_turn[CardType::Skill.index()] = 1;

        smoggy_after_card_played(&mut state, Some(CardType::Skill));
        let _ = dispatch_after_card_entered_combat_on_monster_powers(&mut state, skill);

        assert!(
            state.cards[skill.0.get() as usize - 1].affliction.is_none(),
            "no SmoggyPower ⇒ no Smog anywhere"
        );
    }
}
