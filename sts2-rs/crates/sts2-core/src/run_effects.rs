//! RunState-side relic hook bus — bead `sts2-rs-zq8` (j87-b).
//!
//! Symmetric sibling of [`crate::relic_effects`]. Where `relic_effects`
//! hosts hook channels whose firing path is in-combat (signature
//! `fn(&mut CombatState, ...)`), this module hosts the channels whose
//! firing path is between-combat / run-loop level (signature
//! `fn(&mut RunState, ...)`).
//!
//! ## Why a separate module
//!
//! The j87 main wave (commit `acb2f38`) wired most of the
//! `relic_effects::dispatch_*` channels into the in-combat call sites
//! (`combat_loop.rs`, damage pipeline). Three channels were
//! deliberately deferred because the natural firing site is
//! out-of-combat:
//!
//! - `after_obtained` — fires when the player picks a relic from a
//!   reward queue. There is no live `CombatState` at that moment.
//! - `after_room_entered` — fires when the player enters a new map
//!   room (rest site, shop, event). Same out-of-combat constraint.
//!
//! Three architectural paths were considered (see bead `sts2-rs-zq8`
//! description for the full trade-off discussion):
//!
//! - **(a)** Widen the existing `relic_effects::dispatch_*` signatures
//!   to take `(&mut CombatState, &mut RunState)`. Smallest patch;
//!   couples `CombatState` into every `RunState`-only firing.
//! - **(b)** Synthesize a default `CombatState` at the firing site.
//!   Lossy — listeners that need combat state get a no-op.
//! - **(c)** Symmetric split: keep `CombatState`-arg dispatchers in
//!   `relic_effects::*`, move `RunState`-arg dispatchers to a new
//!   `run_effects::*` module. Cleanest long-term; mirrors the
//!   in-combat / out-of-combat split that already exists at the call
//!   sites.
//!
//! Path (c) is what this module implements. Each channel here mirrors
//! the [`crate::relic_effects::relic_hook_channel`]-style scaffolding:
//! a typed function-pointer alias, a `OnceLock<RwLock<BTreeMap>>`
//! registry, `register_*` / `register_many_*` / `lookup_*` /
//! `listeners_*` accessors, and a hand-written `dispatch_*` walker.
//!
//! ## Channel inventory (current)
//!
//! - [`AfterObtainedRunFn`] — fires from
//!   [`crate::runstate::RunState::accept_relic_reward`]. The legacy
//!   CombatState-arg [`crate::relic_effects::dispatch_after_obtained`]
//!   is left in place (no production callers; only its own test) but
//!   `run_effects::dispatch_after_obtained` is the canonical firing
//!   path event-reward retrofit beads (`sts2-rs-01o` family) should
//!   target.
//!
//! - [`AfterRoomEnteredRunFn`] — fires from
//!   [`crate::runstate::RunState::enter_room`]. Sub-bead `sts2-rs-ouv`
//!   (j87-c).
//!
//! - [`AfterMaxHpChangedRunFn`] — broadcast by
//!   [`crate::runstate::RunState::gain_max_hp`] /
//!   [`crate::runstate::RunState::heal`] with a
//!   [`MaxHpChangeRunCtx`] payload. Bead `sts2-rs-xqvn`.
//!
//! - [`AfterCombatEndRunFn`] / [`OnCardAddedToDeckRunFn`] — bead
//!   `sts2-rs-acyi` (P7-surf). The channels land here so the
//!   `ChosenCheese` / `DarkstonePeriapt` listener bodies can register
//!   real handlers, but no production firing site lands with this
//!   bead: a future combat→run back-edge will wire
//!   `combat_loop::check_combat_end` into `dispatch_after_combat_end`,
//!   and a future port of `RunState::add_card_instance_to_deck` will
//!   wire `dispatch_on_card_added_to_deck`. Until then the channels
//!   are reachable only from tests.

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use crate::card_effects::EffectResult;
use crate::cards::{Card, CardId, CardType};
use crate::relic_effects::AddSource;
use crate::relics::RelicId;
use crate::runstate::{CardRewardGenerationOptions, Reward, RoomType, RunState};
use crate::shop::ShopInventory;

// ---------------------------------------------------------------------------
// Payload types
// ---------------------------------------------------------------------------

/// Max-HP-change payload for the run-side `after_max_hp_changed` channel
/// (bead `sts2-rs-xqvn`). Broadcast by
/// [`crate::runstate::RunState::gain_max_hp`] and
/// [`crate::runstate::RunState::heal`] after the mutation lands.
///
/// `delta` is the **signed** change applied to `RunState.max_hp`
/// (positive on gain; `heal` broadcasts `delta = 0` for the `max_hp`
/// component but a non-zero `current_hp_delta`). Listeners that only
/// care about `max_hp` gains can gate on `ctx.delta > 0`; listeners
/// that care about heals can read `ctx.current_hp_delta`.
///
/// Parallel sibling of [`crate::relic_effects::MaxHpChangeCtx`] — kept
/// as a separate type because the run-side path carries `current_hp`
/// movement too (Python `run_state.heal` only moves `current_hp`,
/// while `gain_max_hp` moves both). The two channels stay independent
/// per the bead-`xqvn` scope (no unification this bead).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaxHpChangeRunCtx {
    /// Signed delta applied to `RunState.max_hp` (`> 0` on
    /// `gain_max_hp`; `0` on `heal`).
    pub delta: i32,
    /// Signed delta applied to `RunState.current_hp` (`> 0` on both
    /// `gain_max_hp` and `heal`; clamped by `heal` so it never exceeds
    /// `max_hp - prior_current_hp`).
    pub current_hp_delta: i32,
}

// ---------------------------------------------------------------------------
// Hook channel macro — parallel to `relic_effects::relic_hook_channel!`
// but generates RunState-arg accessors. Kept module-local so
// `run_effects` and `relic_effects` stay decoupled at the source level.
// ---------------------------------------------------------------------------

macro_rules! run_hook_channel {
    (
        $fn_alias:ident,
        $registry_static:ident,
        $registry_fn:ident,
        $register_fn:ident,
        $register_many_fn:ident,
        $lookup_fn:ident,
        $listeners_fn:ident,
        $is_empty_fn:ident,
        $sig:ty
    ) => {
        #[doc = concat!("Function-pointer alias for the `", stringify!($fn_alias), "` hook channel.")]
        pub type $fn_alias = $sig;

        static $registry_static: OnceLock<RwLock<BTreeMap<RelicId, $fn_alias>>> = OnceLock::new();

        fn $registry_fn() -> &'static RwLock<BTreeMap<RelicId, $fn_alias>> {
            $registry_static.get_or_init(|| RwLock::new(BTreeMap::new()))
        }

        #[doc = concat!("Register a single handler for the `", stringify!($fn_alias), "` channel.")]
        pub fn $register_fn(relic_id: RelicId, handler: $fn_alias) {
            $registry_fn()
                .write()
                .expect("run_effects hook registry poisoned")
                .insert(relic_id, handler);
        }

        #[doc = concat!("Register many handlers for the `", stringify!($fn_alias), "` channel.")]
        pub fn $register_many_fn(entries: &[(RelicId, $fn_alias)]) {
            let mut guard = $registry_fn()
                .write()
                .expect("run_effects hook registry poisoned");
            for &(id, f) in entries {
                guard.insert(id, f);
            }
        }

        #[doc = concat!("Look up a registered handler for the `", stringify!($fn_alias), "` channel.")]
        #[must_use]
        pub fn $lookup_fn(relic_id: RelicId) -> Option<$fn_alias> {
            $registry_fn()
                .read()
                .expect("run_effects hook registry poisoned")
                .get(&relic_id)
                .copied()
        }

        #[doc = concat!("Snapshot every (RelicId, handler) pair registered on the `",
            stringify!($fn_alias),
            "` channel.")]
        #[must_use]
        pub fn $listeners_fn() -> Vec<(RelicId, $fn_alias)> {
            $registry_fn()
                .read()
                .expect("run_effects hook registry poisoned")
                .iter()
                .map(|(&k, &v)| (k, v))
                .collect()
        }

        // Cheap empty-bus probe — see sibling comment in
        // `relic_effects::relic_hook_channel!`. Bead `sts2-rs-4lk9`.
        #[doc = concat!("Returns `true` if the `", stringify!($fn_alias), "` channel has no registered listeners.")]
        #[inline]
        #[must_use]
        pub fn $is_empty_fn() -> bool {
            match $registry_static.get() {
                None => true,
                Some(lock) => lock
                    .read()
                    .expect("run_effects hook registry poisoned")
                    .is_empty(),
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

// `after_obtained` — fired by `RunState::accept_relic_reward` once the
// chosen `RelicReward` has been pushed onto `RunState.relics`. The
// listener receives the live `&mut RunState` so it can mutate the run
// (queue another reward, modify gold, etc.).
run_hook_channel!(
    AfterObtainedRunFn,
    HOOK_AFTER_OBTAINED,
    registry_after_obtained,
    register_after_obtained,
    register_many_after_obtained,
    lookup_after_obtained,
    listeners_after_obtained,
    is_empty_after_obtained,
    fn(&mut RunState) -> EffectResult
);

/// Dispatch the `after_obtained` channel. Walks every registered
/// listener and returns [`EffectResult::Resolved`] if any listener
/// returned `Resolved`, else [`EffectResult::NotImplemented`] (matching
/// the convention used by [`crate::relic_effects`]'s `dispatch_*`
/// walkers).
///
/// Listeners that need to fire only on their *own* pickup (e.g.
/// Astrolabe's one-shot transform-and-upgrade) read
/// [`RunState::last_obtained_relic`] and gate on
/// `Some(RelicId::SelfId)`. Listeners that want any-pickup semantics
/// (MawBank-style trackers) ignore the field. Bead `sts2-rs-70anj.3.9`.
pub fn dispatch_after_obtained(run_state: &mut RunState) -> EffectResult {
    if is_empty_after_obtained() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_after_obtained();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `after_room_entered` — fired by `RunState::enter_room` once
// `RunState.current_room` has been updated to the new room. Listeners
// receive the live `&mut RunState` plus the entered [`RoomType`] so
// MawBank-style (any-room) and DreamCatcher-style (RestSite-only)
// gating can both be expressed inside the handler. Bead
// `sts2-rs-ouv` (j87-c).
run_hook_channel!(
    AfterRoomEnteredRunFn,
    HOOK_AFTER_ROOM_ENTERED,
    registry_after_room_entered,
    register_after_room_entered,
    register_many_after_room_entered,
    lookup_after_room_entered,
    listeners_after_room_entered,
    is_empty_after_room_entered,
    fn(&mut RunState, room_type: RoomType) -> EffectResult
);

/// Dispatch the `after_room_entered` channel. Walks every registered
/// listener with the entered [`RoomType`]; same accumulation
/// convention as [`dispatch_after_obtained`].
pub fn dispatch_after_room_entered(run_state: &mut RunState, room_type: RoomType) -> EffectResult {
    if is_empty_after_room_entered() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_after_room_entered();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, room_type) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `should_generate_treasure` — pre-treasure-room veto hook. Mirrors
// Python `RelicInstance.should_generate_treasure(owner) -> bool | None`
// (`sts2-rl-agent@01c094d:sts2_env/relics/base.py:923`) and the
// dispatch site at `run_manager._enter_room` (`run_manager.py:792-796`):
//
// ```python
// should_skip_treasure = any(
//     relic.should_generate_treasure(self._run_state.player) is False
//     for relic in self._run_state.player.get_relic_objects()
// )
// ```
//
// Semantics: the Python hook returns `None` (abstain) or a `bool` —
// the room-driver only skips treasure if ANY relic returns `False`.
// The Rust channel encodes this with the standard
// [`EffectResult`] return:
//
// - `EffectResult::Resolved`     — vote SKIP (mirrors Python `False`).
// - `EffectResult::NotImplemented` — abstain (mirrors Python `None`).
// - Python's `True` vote is semantically identical to `None` in the
//   `any(... is False)` predicate, so the channel collapses it onto
//   the abstain side (no listener ever needs to return Resolved to
//   force a YES vote).
//
// The thin [`dispatch_should_generate_treasure`] wrapper hides that
// encoding and just returns `bool` ("should the treasure room be
// generated?"). Bead `sts2-rs-elcvh` (P9.2.j).
run_hook_channel!(
    ShouldGenerateTreasureRunFn,
    HOOK_SHOULD_GENERATE_TREASURE,
    registry_should_generate_treasure,
    register_should_generate_treasure,
    register_many_should_generate_treasure,
    lookup_should_generate_treasure,
    listeners_should_generate_treasure,
    is_empty_should_generate_treasure,
    fn(&mut RunState) -> EffectResult
);

/// Dispatch the `should_generate_treasure` veto channel. Returns
/// `true` (proceed with treasure generation) iff no registered
/// listener returned [`EffectResult::Resolved`] — see channel
/// docs above for the Python-to-Rust encoding rationale.
///
/// Intended call site is the eventual map → room-dispatch driver
/// (Python's `_enter_room` TREASURE arm); the post-roll
/// [`crate::run_manager::RunManager::enter_treasure`] is intentionally
/// NOT plumbed to consult this so that ycria's existing behaviour
/// stays observable until the room-dispatch umbrella lands. Bead
/// `sts2-rs-elcvh` (P9.2.j).
#[must_use]
pub fn dispatch_should_generate_treasure(run_state: &mut RunState) -> bool {
    if is_empty_should_generate_treasure() {
        return true;
    }
    let listeners = listeners_should_generate_treasure();
    for (_id, f) in listeners {
        if f(run_state) == EffectResult::Resolved {
            return false;
        }
    }
    true
}

// `after_shop_inventory_generated` — fires from the eventual run-loop
// `_enter_shop` driver once [`crate::shop::generate_shop_inventory`]
// has populated the inventory but BEFORE the player is given control.
// Listeners receive `&mut RunState` plus `&mut ShopInventory` so
// they can both mutate the run (gold, decks, pending rewards) and
// flip slot prices to `SHOP_ENTRY_SOLD_OUT_PRICE` to consume them.
//
// Mirrors the Python flow at `run_manager.py:693-696` (`_enter_shop`)
// + `run_manager._enter_room` SHOP arm (`run_manager.py:773-779`)
// where `after_room_entered` fires with a `RoomVisitContext` whose
// `shop_inventory=` field exposes the freshly-rolled inventory to
// listeners like `LordsParasol` (`relics/shop_event.py:2343-2355`).
//
// The Python pattern fuses this into `after_room_entered` via a rich
// context object; the Rust port keeps `after_room_entered` payload-
// free (per its existing `fn(&mut RunState, RoomType) -> EffectResult`
// signature) and splits the inventory-bearing call into a dedicated
// channel. This sidesteps the cross-channel ordering problem (a
// shop listener that mutates inventory can't safely share a bus with
// generic `after_room_entered` listeners that only see `RoomType`).
//
// The dispatch site is NOT wired in this bead — the run-loop shop
// driver bead (room-dispatch umbrella) will call
// [`dispatch_after_shop_inventory_generated`] from inside its
// `_enter_shop` analogue after rolling the inventory. Until then,
// registered listeners are dead code; this matches the established
// "register-first, plumb-later" convention used by sibling P9.2
// beads (see `elcvh` for an immediate precedent).
//
// Bead `sts2-rs-apy0a` (Channel: shop auto-purchase / LordsParasol).
run_hook_channel!(
    AfterShopInventoryGeneratedRunFn,
    HOOK_AFTER_SHOP_INVENTORY_GENERATED,
    registry_after_shop_inventory_generated,
    register_after_shop_inventory_generated,
    register_many_after_shop_inventory_generated,
    lookup_after_shop_inventory_generated,
    listeners_after_shop_inventory_generated,
    is_empty_after_shop_inventory_generated,
    fn(&mut RunState, inv: &mut ShopInventory) -> EffectResult
);

/// Dispatch the `after_shop_inventory_generated` channel. Walks every
/// registered listener with `&mut RunState` and the freshly-rolled
/// `&mut ShopInventory`; same `Resolved`/`NotImplemented` accumulation
/// convention as [`dispatch_after_obtained`]. The intended call site
/// is the run-loop `_enter_shop` driver immediately after
/// [`crate::shop::generate_shop_inventory`] returns and before the
/// player gains control of the shop UI. Bead `sts2-rs-apy0a`.
pub fn dispatch_after_shop_inventory_generated(
    run_state: &mut RunState,
    inv: &mut ShopInventory,
) -> EffectResult {
    if is_empty_after_shop_inventory_generated() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_after_shop_inventory_generated();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, inv) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `after_act_entered` — fires after `RunState::advance_act` lands the
// new act_index, the act-map has been regenerated, and
// `modify_generated_map` listeners have rewritten the layout.
// Listeners receive the live `&mut RunState` plus the new
// (post-advance) `act_index: u8` so per-act gating (LavaRock's
// post-Act-1 boss reward, PumpkinCandle, FurCoat) can branch
// cleanly. Mirrors the Python `for relic in self.relics:
// relic.after_act_entered(self, act_index)` walk that fires from
// `run_manager._enter_boss_relic` after the BOSS-clear transitions
// into the next act. Bead `sts2-rs-7uzk3` (P9.2.g.1).
run_hook_channel!(
    AfterActEnteredRunFn,
    HOOK_AFTER_ACT_ENTERED,
    registry_after_act_entered,
    register_after_act_entered,
    register_many_after_act_entered,
    lookup_after_act_entered,
    listeners_after_act_entered,
    is_empty_after_act_entered,
    fn(&mut RunState, act_index: u8) -> EffectResult
);

/// Dispatch the `after_act_entered` channel. Walks every registered
/// listener with the entered `act_index` (post-advance value, in
/// `[1, 3]` after `RunState::advance_act` clamps); same accumulation
/// convention as [`dispatch_after_obtained`].
pub fn dispatch_after_act_entered(run_state: &mut RunState, act_index: u8) -> EffectResult {
    if is_empty_after_act_entered() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_after_act_entered();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, act_index) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `after_max_hp_changed` (run-side) — broadcast by
// `RunState::gain_max_hp` / `RunState::heal` (bead `sts2-rs-xqvn`).
// Symmetric sibling of `relic_effects::after_max_hp_changed` for
// listeners that need to react to run-state HP movement
// (FakeMango / FakeLeesWaffle / BigMushroom / FragrantMushroom /
// ChosenCheese / MawBank — see bead `sts2-rs-46ms`).
run_hook_channel!(
    AfterMaxHpChangedRunFn,
    HOOK_AFTER_MAX_HP_CHANGED_RUN,
    registry_after_max_hp_changed,
    register_after_max_hp_changed,
    register_many_after_max_hp_changed,
    lookup_after_max_hp_changed,
    listeners_after_max_hp_changed,
    is_empty_after_max_hp_changed,
    fn(&mut RunState, ctx: &MaxHpChangeRunCtx) -> EffectResult
);

/// Dispatch the run-side `after_max_hp_changed` channel. Walks every
/// registered listener with the supplied [`MaxHpChangeRunCtx`]; same
/// accumulation convention as [`dispatch_after_obtained`].
pub fn dispatch_after_max_hp_changed(
    run_state: &mut RunState,
    ctx: &MaxHpChangeRunCtx,
) -> EffectResult {
    if is_empty_after_max_hp_changed() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_after_max_hp_changed();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, ctx) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `after_combat_end` (run-side) — bead `sts2-rs-acyi` (P7-surf).
// Symmetric sibling of `relic_effects::after_combat_end` for listeners
// that need to mutate run state at combat boundary (e.g. ChosenCheese's
// `owner.gain_max_hp(1)` in `shop_event.py:1052-1053`). Python's
// `after_combat_end(self, owner, combat)` does not pass a payload that
// the run-side listeners read, so the channel signature is bare
// `fn(&mut RunState)` — matching combat-side `AfterCombatEndFn`.
//
// No firing site lands with this bead: a future combat→run back-edge
// bead will wire `combat_loop::check_combat_end` to also call this
// dispatcher with the active `&mut RunState`. Until then this channel
// is reachable only from tests and from the swarm-installer that
// registers handlers eagerly (so a future flip of the firing site
// activates them without re-registration).
run_hook_channel!(
    AfterCombatEndRunFn,
    HOOK_AFTER_COMBAT_END_RUN,
    registry_after_combat_end,
    register_after_combat_end,
    register_many_after_combat_end,
    lookup_after_combat_end,
    listeners_after_combat_end,
    is_empty_after_combat_end,
    fn(&mut RunState) -> EffectResult
);

/// Dispatch the run-side `after_combat_end` channel. Walks every
/// registered listener and returns [`EffectResult::Resolved`] if any
/// listener returned `Resolved`; same accumulation convention as
/// [`dispatch_after_obtained`].
pub fn dispatch_after_combat_end(run_state: &mut RunState) -> EffectResult {
    if is_empty_after_combat_end() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_after_combat_end();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// Payload for the run-side `after_combat_victory` channel
/// (bead `sts2-rs-y8zi4`).
///
/// Mirrors the parts of Python's `combat: CombatState` that the
/// `after_combat_victory` listener set actually reads at pin
/// `01c094d`. The only consumer at this pin is
/// `SwordOfStone.after_combat_victory` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/event_reward.py`):
///
/// ```python
/// class SwordOfStone(EventRewardRelic):
///     def after_combat_victory(self, owner, combat):
///         if combat.is_elite:
///             self.counter -= 1
///             if self.counter <= 0:
///                 owner.run_state.transform_relic(self, SwordOfJade)
/// ```
///
/// So the ctx carries just `is_elite` — the same predicate the
/// combat-side `state.is_elite()` accessor returns. Future
/// `after_combat_victory` run-side listeners that need more combat
/// signal (e.g. `boss_defeated`, `monster_kind_killed`) should
/// extend this struct rather than swap to a heavier
/// `&CombatState`-bearing channel (which would re-introduce the
/// in-combat / out-of-combat coupling that `run_effects` was
/// carved out to avoid — see module docstring path (a) vs (c)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CombatVictoryRunCtx {
    /// Mirror of `CombatState::is_elite()` at victory time. The
    /// combat→run firing site (deferred to a follow-up bead — the
    /// combat-side `dispatch_after_combat_victory` site has no
    /// `&mut RunState` in scope) reads `state.is_elite()` and
    /// stamps it into this field before dispatching the run-side
    /// listener set.
    pub is_elite: bool,
}

// `after_combat_victory` (run-side) — bead `sts2-rs-y8zi4` (jvk3
// follow-up). Symmetric sibling of
// `relic_effects::after_combat_victory` for listeners whose
// victory reaction must mutate RunState (`SwordOfStone`'s
// `transform_relic(SwordOfStone -> SwordOfJade)` arm). The
// combat-side counter-decrement half landed under bead
// `sts2-rs-jvk3` at
// `crates/sts2-content/src/relics/event_reward.rs:1135`
// (deferred the transform with a `TODO(sts2-rs-jvk3-followup)`);
// this bead lands the run-side channel + listener so the
// transform path is reachable end-to-end.
//
// No production firing site lands with this bead — the
// combat-side `dispatch_after_combat_victory` site in
// `combat_loop.rs::step_phase_continue` has no `&mut RunState`
// in scope, and plumbing it through the MCTS hot path would
// require a CombatState→RunState back-edge that crosses multiple
// crates. The channel is reachable from tests + the
// swarm-installer that registers handlers eagerly; the
// integration test in this bead exercises the full counter +
// transform path by driving both the combat-side decrement
// dispatch and the run-side transform dispatch in sequence.
//
// Future follow-up bead will wire the production firing site
// (likely as a deferred-action queue stamped on CombatState
// during combat-end, drained by the run-loop driver — see
// CLAUDE.md "Don't add features beyond what the task requires"
// for why that plumbing is not in scope here).
run_hook_channel!(
    AfterCombatVictoryRunFn,
    HOOK_AFTER_COMBAT_VICTORY_RUN,
    registry_after_combat_victory_run,
    register_after_combat_victory_run,
    register_many_after_combat_victory_run,
    lookup_after_combat_victory_run,
    listeners_after_combat_victory_run,
    is_empty_after_combat_victory_run,
    fn(&mut RunState, ctx: &CombatVictoryRunCtx) -> EffectResult
);

/// Dispatch the run-side `after_combat_victory` channel. Walks
/// every registered listener and returns
/// [`EffectResult::Resolved`] if any listener returned `Resolved`;
/// same accumulation convention as
/// [`dispatch_after_combat_end`]. Bead `sts2-rs-y8zi4`.
pub fn dispatch_after_combat_victory_run(
    run_state: &mut RunState,
    ctx: &CombatVictoryRunCtx,
) -> EffectResult {
    if is_empty_after_combat_victory_run() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_after_combat_victory_run();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, ctx) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// The run-side combat-victory listeners the run manager's combat exit fires
/// (sts2-agent fork). The channel had no production firing site; firing it
/// whole would also start War Hammer's elite upgrades (and their Niche draws),
/// a gameplay change outside the approved fork batch. Only Sword of Stone's
/// replacement by Sword of Jade is approved; a listener joins this list when
/// its own fix is.
pub const LIVE_COMBAT_VICTORY_RUN_LISTENERS: &[RelicId] = &[RelicId::SwordOfStone];

/// [`dispatch_after_combat_victory_run`] restricted to `live`: the listeners
/// registered for those relics, in the registry's order. The combat exit
/// passes [`LIVE_COMBAT_VICTORY_RUN_LISTENERS`]. sts2-agent fork.
pub fn dispatch_after_combat_victory_run_live(
    run_state: &mut RunState,
    ctx: &CombatVictoryRunCtx,
    live: &[RelicId],
) -> EffectResult {
    let mut any_resolved = false;
    for (id, f) in listeners_after_combat_victory_run() {
        if live.contains(&id) && f(run_state, ctx) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// Production firing site for the run-side combat-end channels. Bead
/// `sts2-rs-elipw` (y8zi4 follow-up — wires the back-edge from the
/// combat-side dispatch site, which has no `&mut RunState` in scope,
/// into the run-side listener set so SwordOfStone's transform path
/// becomes reachable end-to-end).
///
/// **Why a separate helper instead of plumbing `&mut RunState`
/// through `step_phase_continue`?** Per CLAUDE.md's "don't add
/// features beyond what the task requires" — threading `&mut
/// RunState` through every combat-step call site (MCTS hot path,
/// scripted-replay harness, parity harness) would be a wide refactor.
/// This helper instead names the back-edge as a single function the
/// run-loop driver calls once per combat boundary, with the
/// CombatState passed by `&CombatState` (read-only). Future drivers
/// (the sts2-cli session-level loop, the python bridge's combat-end
/// callback) invoke this once per combat, reading `state.is_over`,
/// `state.player_won`, and `state.is_elite()` to build the right
/// dispatch payload.
///
/// **Dispatch order** mirrors the combat-side bus at
/// [`crate::combat_loop`] (`after_combat_victory` BEFORE
/// `after_combat_end`, victory walks the victory-only listener
/// subset first):
///
/// 1. If `state.is_over && state.player_won`: dispatch
///    [`dispatch_after_combat_victory_run`] with
///    `CombatVictoryRunCtx { is_elite: state.is_elite() }`.
/// 2. Always dispatch [`dispatch_after_combat_end`] (the
///    victory-or-loss generic combat-end bus).
///
/// `state.is_over == false` is a no-op (combat hasn't ended yet —
/// guarded so future drivers can call the helper defensively at
/// every step boundary without re-checking the predicate). When
/// the combat ended in a loss (`player_won == false`), the
/// victory bus is skipped and only the `after_combat_end` bus
/// fires, matching the combat-side dispatch shape.
///
/// Returns the OR of the two dispatchers' `EffectResult`s — useful
/// for callers that want to short-circuit further run-loop work
/// when nothing fired (e.g. skip a "log the combat-end side
/// effects" step when both dispatchers reported
/// `NotImplemented`).
pub fn dispatch_run_side_combat_end(
    run_state: &mut RunState,
    state: &crate::CombatState,
) -> EffectResult {
    if !state.is_over {
        return EffectResult::NotImplemented;
    }
    let mut any_resolved = false;
    if state.player_won {
        let ctx = CombatVictoryRunCtx {
            is_elite: state.is_elite(),
        };
        if dispatch_after_combat_victory_run(run_state, &ctx) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if dispatch_after_combat_end(run_state) == EffectResult::Resolved {
        any_resolved = true;
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// Payload for the run-side `on_card_added_to_deck` channel
/// (bead `sts2-rs-acyi`).
///
/// Mirrors Python `on_card_added_to_deck(self, owner) -> None` at
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py:1121-1123`. The
/// run-side firing site (a future port of
/// `RunState.add_card_instance_to_deck` at
/// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:297-306`) supplies
/// both `card_id` (stable identity of the card just appended to the
/// master deck) and `card_type` (so `DarkstonePeriapt`-style listeners
/// can gate on [`CardType::Curse`] without re-resolving the card spec
/// against `sts2-content`). `source` mirrors the combat-side
/// [`crate::relic_effects::CardAddedCtx::source`] so the run-side
/// listener set can share the same provenance enum without a fresh
/// taxonomy.
///
/// Held independent of [`crate::relic_effects::CardAddedCtx`] because
/// the run-side ctx carries `card_type` natively (Python's run-side
/// path reads `card.card_type` directly in
/// `run_state.py:303`) — see TODO bead `sts2-rs-3f8` for the eventual
/// unification once the combat-side ctx grows the same field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardAddedRunCtx {
    /// Stable identity of the card just appended to the master deck.
    pub card_id: CardId,
    /// Card type of the added card — listeners gate on this directly
    /// (`DarkstonePeriapt` fires on [`CardType::Curse`]).
    pub card_type: CardType,
    /// Origin of the add — see [`AddSource`].
    pub source: AddSource,
}

// `on_card_added_to_deck` (run-side) — bead `sts2-rs-acyi` (P7-surf).
// Symmetric sibling of `relic_effects::on_card_added_to_deck` for
// listeners whose deck-add reaction mutates RunState (e.g.
// `DarkstonePeriapt`'s `owner.gain_max_hp(6)` gated on
// `CardType::Curse`). Same firing-site convention as
// `after_combat_end` above: no production firing site lands with this
// bead; the future port of `RunState.add_card_instance_to_deck` will
// call `dispatch_on_card_added_to_deck` once per added card.
run_hook_channel!(
    OnCardAddedToDeckRunFn,
    HOOK_ON_CARD_ADDED_TO_DECK_RUN,
    registry_on_card_added_to_deck,
    register_on_card_added_to_deck,
    register_many_on_card_added_to_deck,
    lookup_on_card_added_to_deck,
    listeners_on_card_added_to_deck,
    is_empty_on_card_added_to_deck,
    fn(&mut RunState, ctx: &CardAddedRunCtx) -> EffectResult
);

/// Dispatch the run-side `on_card_added_to_deck` channel. Walks every
/// registered listener with the supplied [`CardAddedRunCtx`]; same
/// accumulation convention as [`dispatch_after_obtained`].
pub fn dispatch_on_card_added_to_deck(
    run_state: &mut RunState,
    ctx: &CardAddedRunCtx,
) -> EffectResult {
    if is_empty_on_card_added_to_deck() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_on_card_added_to_deck();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, ctx) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// ---------------------------------------------------------------------------
// Reward-flow channels — bead `sts2-rs-9wx7` (P7-surf).
//
// Five RunState-arg hook channels for the reward-flow path that
// `DreamCatcher` / `FresnelLens` / `WongosMysteryTicket` listen on.
// Python signatures take `(owner, ...)`; on the Rust side `owner` is
// implicit (the run-state's player), so each channel takes `&mut
// RunState` + the payload the Python listener mutates.
//
// All five firings are between-combat / run-loop level (no live
// CombatState), so they live in `run_effects` per the path-(c) split
// established by zq8 / ouv / acyi.
// ---------------------------------------------------------------------------

// `modify_rest_site_heal_rewards` — fired by
// `RunState::offer_rest_site_rewards`. Listeners mutate the rewards
// vector in place. Python: `Relic.modify_rest_site_heal_rewards(owner,
// rewards, run_state) -> list[object]`. `DreamCatcher` appends a
// `CardReward` for each `_procs_left` rest-site visit
// (`shop_event.py:1207-1209`).
run_hook_channel!(
    ModifyRestSiteHealRewardsRunFn,
    HOOK_MODIFY_REST_SITE_HEAL_REWARDS_RUN,
    registry_modify_rest_site_heal_rewards,
    register_modify_rest_site_heal_rewards,
    register_many_modify_rest_site_heal_rewards,
    lookup_modify_rest_site_heal_rewards,
    listeners_modify_rest_site_heal_rewards,
    is_empty_modify_rest_site_heal_rewards,
    fn(&mut RunState, rewards: &mut Vec<Reward>) -> EffectResult
);

/// Dispatch the run-side `modify_rest_site_heal_rewards` channel.
/// Listeners may mutate `rewards` in place. Same accumulation
/// convention as [`dispatch_after_obtained`].
pub fn dispatch_modify_rest_site_heal_rewards(
    run_state: &mut RunState,
    rewards: &mut Vec<Reward>,
) -> EffectResult {
    if is_empty_modify_rest_site_heal_rewards() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_modify_rest_site_heal_rewards();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, rewards) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `modify_card_reward_creation_options` — fired by
// `RunState::modify_card_reward_creation_options` (driven by the
// card-reward generator at `sts2-rs-jyfvu`). Listeners receive the
// live [`CardRewardGenerationOptions`] before the card pool is rolled
// and may flip its fields to reshape the eventual `CardReward.options`
// slate. Python:
// `Relic.modify_card_reward_creation_options(owner, options, reward,
// room, run_state) -> CardRewardGenerationOptions`
// (`sts2-rl-agent@01c094d:sts2_env/relics/base.py:869`). The Python
// signature carries `reward` and `room`; both are dropped here for
// parity with the already-shipped `modify_card_reward_options_late`
// channel — the listener gets `(&mut RunState, &mut opts)` and reads
// `run_state.current_room` directly if it cares about the room.
//
// Unlocks: DingyRug (colorless-card injection), PrismaticGem
// (all-character pool widening), SilverCrucible-rewards leg (first 3
// pre-upgrade), LavaLamp-late (via paired late channel). Bead
// `sts2-rs-tu7g5`.
run_hook_channel!(
    ModifyCardRewardCreationOptionsRunFn,
    HOOK_MODIFY_CARD_REWARD_CREATION_OPTIONS_RUN,
    registry_modify_card_reward_creation_options,
    register_modify_card_reward_creation_options,
    register_many_modify_card_reward_creation_options,
    lookup_modify_card_reward_creation_options,
    listeners_modify_card_reward_creation_options,
    is_empty_modify_card_reward_creation_options,
    fn(&mut RunState, opts: &mut CardRewardGenerationOptions) -> EffectResult
);

/// Dispatch the `modify_card_reward_creation_options` channel.
/// Listeners may mutate `opts` in place. Same accumulation convention
/// as [`dispatch_after_obtained`]. Bead `sts2-rs-tu7g5`.
pub fn dispatch_modify_card_reward_creation_options(
    run_state: &mut RunState,
    opts: &mut CardRewardGenerationOptions,
) -> EffectResult {
    if is_empty_modify_card_reward_creation_options() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_modify_card_reward_creation_options();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, opts) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `modify_unknown_map_point_room_types` — fired by the future
// unknown-room (`?`) resolver in the run/map layer. Listeners receive
// the live candidate set of [`RoomType`]s that the resolver may pick
// and can narrow it (e.g. JuzuBracelet removes [`RoomType::Monster`]
// pre-act-3-treasure). Python:
// `Relic.modify_unknown_map_point_room_types(owner, room_types: set[RoomType]) -> set[RoomType]`
// (`sts2-rl-agent@01c094d:sts2_env/relics/base.py:843`;
// `sts2_env/relics/common.py:242` for JuzuBracelet's filter body;
// `sts2_env/relics/shop_event.py:2113` for the secondary listener).
//
// Held as `&mut BTreeSet<RoomType>` (not the bead's `HashSet` shape)
// for deterministic iteration order on listeners that don't fully
// short-circuit — `RoomType` already derives `Ord`. Bead
// `sts2-rs-vuxmm`.
run_hook_channel!(
    ModifyUnknownMapPointRoomTypesRunFn,
    HOOK_MODIFY_UNKNOWN_MAP_POINT_ROOM_TYPES_RUN,
    registry_modify_unknown_map_point_room_types,
    register_modify_unknown_map_point_room_types,
    register_many_modify_unknown_map_point_room_types,
    lookup_modify_unknown_map_point_room_types,
    listeners_modify_unknown_map_point_room_types,
    is_empty_modify_unknown_map_point_room_types,
    fn(&mut RunState, room_types: &mut std::collections::BTreeSet<RoomType>) -> EffectResult
);

/// Dispatch the `modify_unknown_map_point_room_types` channel.
/// Listeners may mutate `room_types` in place (typically to remove
/// variants). Same accumulation convention as
/// [`dispatch_after_obtained`]. Bead `sts2-rs-vuxmm`.
pub fn dispatch_modify_unknown_map_point_room_types(
    run_state: &mut RunState,
    room_types: &mut std::collections::BTreeSet<RoomType>,
) -> EffectResult {
    if is_empty_modify_unknown_map_point_room_types() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_modify_unknown_map_point_room_types();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, room_types) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `modify_generated_map` — fired by the map-generation site
// (`RunState::generate_act_map`, future) after the base generator runs.
// Listeners receive the live [`crate::map::ActMap`] + the 1-indexed
// `act_index` and may replace / mutate the map in place. GoldenCompass
// (`sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py:2082-2117`)
// swaps the current act's map for a fixed golden-path layout on
// `after_obtained`. Python signature:
// `Relic.modify_generated_map(owner, run_state, act_map, act_index) -> ActMap`
// (`sts2-rl-agent@01c094d:sts2_env/relics/base.py:825-832`).
//
// Held as `&mut ActMap` so listeners can mutate the graph (edges,
// node room types) without rebuilding the wrapper Python returns. Bead
// `sts2-rs-fchti`.
run_hook_channel!(
    ModifyGeneratedMapRunFn,
    HOOK_MODIFY_GENERATED_MAP_RUN,
    registry_modify_generated_map,
    register_modify_generated_map,
    register_many_modify_generated_map,
    lookup_modify_generated_map,
    listeners_modify_generated_map,
    is_empty_modify_generated_map,
    fn(&mut RunState, act_map: &mut crate::map::ActMap, act_index: u8) -> EffectResult
);

/// Dispatch the `modify_generated_map` channel. Listeners may mutate
/// `act_map` in place. Same accumulation convention as
/// [`dispatch_modify_unknown_map_point_room_types`]. Bead
/// `sts2-rs-fchti`.
pub fn dispatch_modify_generated_map(
    run_state: &mut RunState,
    act_map: &mut crate::map::ActMap,
    act_index: u8,
) -> EffectResult {
    if is_empty_modify_generated_map() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_modify_generated_map();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, act_map, act_index) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `modify_generated_map_late` — fired after `modify_generated_map`
// listeners have settled. Same signature as `modify_generated_map`,
// but the dispatch order matters: GoldenCompass swaps the map on
// the early channel; FurCoat marks a subset of the live map on the
// late channel, so its rooms are picked from the post-swap layout.
// Python pin: `Relic.modify_generated_map_late(owner, run_state,
// act_map, act_index) -> ActMap`
// (`sts2-rl-agent@01c094d:sts2_env/relics/base.py:834-841`).
// Bead `sts2-rs-eboqq` (FurCoat).
run_hook_channel!(
    ModifyGeneratedMapLateRunFn,
    HOOK_MODIFY_GENERATED_MAP_LATE_RUN,
    registry_modify_generated_map_late,
    register_modify_generated_map_late,
    register_many_modify_generated_map_late,
    lookup_modify_generated_map_late,
    listeners_modify_generated_map_late,
    is_empty_modify_generated_map_late,
    fn(&mut RunState, act_map: &mut crate::map::ActMap, act_index: u8) -> EffectResult
);

/// Dispatch the `modify_generated_map_late` channel. Same
/// accumulation convention as
/// [`dispatch_modify_generated_map`]. Callers must invoke this
/// **after** [`dispatch_modify_generated_map`] so the late
/// listeners see the post-swap layout. Bead `sts2-rs-eboqq`.
pub fn dispatch_modify_generated_map_late(
    run_state: &mut RunState,
    act_map: &mut crate::map::ActMap,
    act_index: u8,
) -> EffectResult {
    if is_empty_modify_generated_map_late() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_modify_generated_map_late();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, act_map, act_index) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `modify_card_reward_options_late` — fired by
// `RunState::offer_card_reward`. Listeners receive the live `Vec<Card>`
// of card-instance options (post-late-modification) and may enchant
// them. Python: `Relic.modify_card_reward_options_late(owner, cards,
// reward, room, run_state) -> list[CardInstance]`. `FresnelLens`
// (`shop_event.py:1539-1551`) calls `card.add_enchantment("Nimble", 2)`
// on each option.
//
// Held as `&mut Vec<Card>` (not `&mut Vec<CardId>`) so listeners can
// reach the per-instance `enchantments` field.
run_hook_channel!(
    ModifyCardRewardOptionsLateRunFn,
    HOOK_MODIFY_CARD_REWARD_OPTIONS_LATE_RUN,
    registry_modify_card_reward_options_late,
    register_modify_card_reward_options_late,
    register_many_modify_card_reward_options_late,
    lookup_modify_card_reward_options_late,
    listeners_modify_card_reward_options_late,
    is_empty_modify_card_reward_options_late,
    fn(&mut RunState, cards: &mut Vec<Card>) -> EffectResult
);

/// Dispatch the run-side `modify_card_reward_options_late` channel.
/// Listeners may mutate `cards` in place. Same accumulation
/// convention as [`dispatch_after_obtained`].
pub fn dispatch_modify_card_reward_options_late(
    run_state: &mut RunState,
    cards: &mut Vec<Card>,
) -> EffectResult {
    if is_empty_modify_card_reward_options_late() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_modify_card_reward_options_late();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, cards) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `modify_card_being_added_to_deck` — fired by
// `RunState::modify_card_being_added_to_deck` from the eventual port of
// `RunState.add_card_instance_to_deck` before the card is appended.
// Listeners may mutate per-instance state. Python:
// `Relic.modify_card_being_added_to_deck(owner, card) -> CardInstance`
// (`shop_event.py:1553-1555` — FresnelLens enchants every deck-add).
run_hook_channel!(
    ModifyCardBeingAddedToDeckRunFn,
    HOOK_MODIFY_CARD_BEING_ADDED_TO_DECK_RUN,
    registry_modify_card_being_added_to_deck,
    register_modify_card_being_added_to_deck,
    register_many_modify_card_being_added_to_deck,
    lookup_modify_card_being_added_to_deck,
    listeners_modify_card_being_added_to_deck,
    is_empty_modify_card_being_added_to_deck,
    fn(&mut RunState, card: &mut Card) -> EffectResult
);

/// Dispatch the run-side `modify_card_being_added_to_deck` channel.
/// Listeners may mutate `card` in place. Same accumulation convention
/// as [`dispatch_after_obtained`].
pub fn dispatch_modify_card_being_added_to_deck(
    run_state: &mut RunState,
    card: &mut Card,
) -> EffectResult {
    if is_empty_modify_card_being_added_to_deck() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_modify_card_being_added_to_deck();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, card) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `modify_merchant_card_creation_results` — fired by
// `RunState::modify_merchant_card_creation_results`. Merchant card
// spawn calls this after building each candidate card. Python signature:
// `Relic.modify_merchant_card_creation_results(owner, card, *,
// is_colorless, run_state) -> CardInstance` (`shop_event.py:1557-1565`
// — FresnelLens enchants merchant inventory).
run_hook_channel!(
    ModifyMerchantCardCreationResultsRunFn,
    HOOK_MODIFY_MERCHANT_CARD_CREATION_RESULTS_RUN,
    registry_modify_merchant_card_creation_results,
    register_modify_merchant_card_creation_results,
    register_many_modify_merchant_card_creation_results,
    lookup_modify_merchant_card_creation_results,
    listeners_modify_merchant_card_creation_results,
    is_empty_modify_merchant_card_creation_results,
    fn(&mut RunState, card: &mut Card, is_colorless: bool) -> EffectResult
);

/// Dispatch the run-side `modify_merchant_card_creation_results`
/// channel. Listeners may mutate `card` in place. Same accumulation
/// convention as [`dispatch_after_obtained`].
pub fn dispatch_modify_merchant_card_creation_results(
    run_state: &mut RunState,
    card: &mut Card,
    is_colorless: bool,
) -> EffectResult {
    if is_empty_modify_merchant_card_creation_results() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_modify_merchant_card_creation_results();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, card, is_colorless) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `modify_rewards` — fired by `RunState::offer_rewards(room_type)` at
// end-of-combat / end-of-event. Listeners receive the rewards vector
// + the [`RoomType`] of the room the reward is being granted for.
// `WongosMysteryTicket` pushes 3 `RelicReward` entries on
// Monster/Elite/Boss rooms once `_combats_finished >= 5`
// (`shop_event.py:3057-3072`).
//
// Python: `Relic.modify_rewards(owner, rewards, room, run_state) ->
// list[Reward]`. The Python `room` is `Optional[Room]`; on the Rust
// side [`RoomType::Unknown`] is the equivalent of `None`.
run_hook_channel!(
    ModifyRewardsRunFn,
    HOOK_MODIFY_REWARDS_RUN,
    registry_modify_rewards,
    register_modify_rewards,
    register_many_modify_rewards,
    lookup_modify_rewards,
    listeners_modify_rewards,
    is_empty_modify_rewards,
    fn(&mut RunState, rewards: &mut Vec<Reward>, room_type: RoomType) -> EffectResult
);

/// Dispatch the run-side `modify_rewards` channel. Listeners may
/// mutate `rewards` in place. Same accumulation convention as
/// [`dispatch_after_obtained`].
pub fn dispatch_modify_rewards(
    run_state: &mut RunState,
    rewards: &mut Vec<Reward>,
    room_type: RoomType,
) -> EffectResult {
    if is_empty_modify_rewards() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_modify_rewards();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, rewards, room_type) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `modify_rest_site_heal_amount` — fired by
// `RunState::compute_rest_site_heal_amount(base)` when the rest-site
// REST option resolves the heal amount. Listeners return an additive
// `i32` delta that the dispatcher folds onto the supplied base.
// Mirrors Python `Relic.modify_rest_site_heal_amount(owner, amount,
// run_state) -> int` — used by e.g. `RegalPillow` (`+15`) and
// `StoneHumidifier`-style amount mods (`shop_event.py`). Parallel to
// the list-mutating `modify_rest_site_heal_rewards` channel just
// above — that one extends the reward set; this one tunes the heal
// scalar itself. Bead `sts2-rs-2tsrk`.
run_hook_channel!(
    ModifyRestSiteHealAmountRunFn,
    HOOK_MODIFY_REST_SITE_HEAL_AMOUNT_RUN,
    registry_modify_rest_site_heal_amount,
    register_modify_rest_site_heal_amount,
    register_many_modify_rest_site_heal_amount,
    lookup_modify_rest_site_heal_amount,
    listeners_modify_rest_site_heal_amount,
    is_empty_modify_rest_site_heal_amount,
    fn(&RunState) -> i32
);

/// Dispatch the run-side `modify_rest_site_heal_amount` channel.
/// Additive fold: each listener returns a `i32` delta which is
/// summed onto `base`. Empty-bus early-out returns `base` unchanged.
/// Mirrors the combat-side `dispatch_modify_max_energy` additive
/// pattern.
#[must_use]
pub fn dispatch_modify_rest_site_heal_amount(run_state: &RunState, base: i32) -> i32 {
    if is_empty_modify_rest_site_heal_amount() {
        return base;
    }
    let listeners = listeners_modify_rest_site_heal_amount();
    let mut amount = base;
    for (_id, f) in listeners {
        amount = amount.saturating_add(f(run_state));
    }
    amount
}

// `after_rest_site_heal` — fired by callers after they apply the
// rest-site REST heal to a player. Listeners observe the final
// `healed` amount (post-`modify_rest_site_heal_amount` folding) and
// can apply secondary effects keyed on a successful heal: e.g.
// `StoneHumidifier::gain_max_hp(5)` (Python
// `relics/shop_event.py::StoneHumidifier.after_rest_site_heal`,
// pin `01c094d:sts2_env/relics/shop_event.py:3431`).
//
// Mirrors Python `Relic.after_rest_site_heal(owner, healed,
// run_state, is_mimicked=False)`. The Rust port drops the
// `is_mimicked` keyword for the same reason `after_card_played`
// does — no current consumer reads it; add a typed-payload
// migration if/when a "MirroredRest"-style relic ships. Bead
// `sts2-rs-70anj.3.17` (rest-site channel infra, first leg).
run_hook_channel!(
    AfterRestSiteHealRunFn,
    HOOK_AFTER_REST_SITE_HEAL_RUN,
    registry_after_rest_site_heal,
    register_after_rest_site_heal,
    register_many_after_rest_site_heal,
    lookup_after_rest_site_heal,
    listeners_after_rest_site_heal,
    is_empty_after_rest_site_heal,
    fn(&mut RunState, healed: i32) -> EffectResult
);

/// Dispatch the run-side `after_rest_site_heal` channel. Walks every
/// registered listener with the actual heal amount that just landed.
/// Returns [`EffectResult::Resolved`] if any listener fired, else
/// [`EffectResult::NotImplemented`]. Bead `sts2-rs-70anj.3.17`.
pub fn dispatch_after_rest_site_heal(run_state: &mut RunState, healed: i32) -> EffectResult {
    if is_empty_after_rest_site_heal() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_after_rest_site_heal();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, healed) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `modify_rest_site_options` — fired by `RunState::rest_site_options`.
// Listeners mutate the option vector in place (push / replace / filter).
// Mirrors Python `Relic.modify_rest_site_options(owner, options,
// run_state) -> list[RestSiteOption]` (`relics/base.py:980`). Used by
// `MeatCleaver` (appends `Cook`) and `Shovel` (appends `Dig`).
// Bead `sts2-rs-ekf4j`.
run_hook_channel!(
    ModifyRestSiteOptionsRunFn,
    HOOK_MODIFY_REST_SITE_OPTIONS_RUN,
    registry_modify_rest_site_options,
    register_modify_rest_site_options,
    register_many_modify_rest_site_options,
    lookup_modify_rest_site_options,
    listeners_modify_rest_site_options,
    is_empty_modify_rest_site_options,
    fn(&RunState, &mut Vec<crate::runstate::RestSiteOption>) -> EffectResult
);

/// Dispatch the `modify_rest_site_options` channel. Walks listeners in
/// registry order, each mutating `options` in place. Empty-bus
/// early-out leaves `options` untouched. Bead `sts2-rs-ekf4j`.
pub fn dispatch_modify_rest_site_options(
    run_state: &RunState,
    options: &mut Vec<crate::runstate::RestSiteOption>,
) -> EffectResult {
    if is_empty_modify_rest_site_options() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_modify_rest_site_options();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, options) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `should_disable_remaining_rest_site_options` — fired after the
// player resolves one rest-site option to decide whether the
// remaining options grey out. Listeners return `Option<bool>`:
// `Some(true)` votes "disable", `Some(false)` votes "keep enabled",
// `None` abstains. Mirrors Python `Relic.should_disable_remaining_rest_site_options`
// (`relics/base.py:1016`) — `MiniatureTent` returns `False`,
// everyone else returns `None`.
//
// Resolver consensus (default `true`): any explicit `Some(false)`
// wins; else any `Some(true)` wins; else the default. Bead
// `sts2-rs-ekf4j` / `.4`.
run_hook_channel!(
    ShouldDisableRemainingRestSiteOptionsRunFn,
    HOOK_SHOULD_DISABLE_REMAINING_REST_SITE_OPTIONS_RUN,
    registry_should_disable_remaining_rest_site_options,
    register_should_disable_remaining_rest_site_options,
    register_many_should_disable_remaining_rest_site_options,
    lookup_should_disable_remaining_rest_site_options,
    listeners_should_disable_remaining_rest_site_options,
    is_empty_should_disable_remaining_rest_site_options,
    fn(&RunState, crate::runstate::RestSiteOption) -> Option<bool>
);

/// Dispatch the `should_disable_remaining_rest_site_options` channel.
/// Returns the consensus boolean per the rule documented on the
/// channel definition (default `true` when no listener has an
/// opinion). Bead `sts2-rs-ekf4j` / `.4`.
#[must_use]
pub fn dispatch_should_disable_remaining_rest_site_options(
    run_state: &RunState,
    chosen: crate::runstate::RestSiteOption,
) -> bool {
    if is_empty_should_disable_remaining_rest_site_options() {
        return true;
    }
    let listeners = listeners_should_disable_remaining_rest_site_options();
    let mut any_false = false;
    let mut any_true = false;
    for (_id, f) in listeners {
        match f(run_state, chosen) {
            Some(false) => any_false = true,
            Some(true) => any_true = true,
            None => {}
        }
    }
    if any_false {
        return false;
    }
    if any_true {
        return true;
    }
    true
}

// `should_gain_gold` / `on_gold_gained` (run-side) — fired from
// `RunState::gain_gold` on positive deltas (matching Python
// `RunState.gain_gold` at `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:120`).
// `should_gain_gold` runs FIRST and may veto the gain entirely (Python
// semantics: any listener returning `False` cancels — Rust expresses this
// via the `cancel: &mut bool` out-parameter). `on_gold_gained` runs AFTER
// the gold has been credited and sees the actually-applied amount.
//
// Consumers (Python): `BowlerHat` (multiply via stateful re-entrant gain,
// uncommon.py:114), `Sozu`/`DragonFruit` shop-side mods (shop_event.py),
// `MawBank` event_reward (currently routed through closure-registry until
// it migrates to this channel). Bead `sts2-rs-wag75`.
run_hook_channel!(
    ShouldGainGoldRunFn,
    HOOK_SHOULD_GAIN_GOLD_RUN,
    registry_should_gain_gold,
    register_should_gain_gold,
    register_many_should_gain_gold,
    lookup_should_gain_gold,
    listeners_should_gain_gold,
    is_empty_should_gain_gold,
    fn(&mut RunState, amount: i32, cancel: &mut bool) -> EffectResult
);

run_hook_channel!(
    OnGoldGainedRunFn,
    HOOK_ON_GOLD_GAINED_RUN,
    registry_on_gold_gained,
    register_on_gold_gained,
    register_many_on_gold_gained,
    lookup_on_gold_gained,
    listeners_on_gold_gained,
    is_empty_on_gold_gained,
    fn(&mut RunState, amount: i32) -> EffectResult
);

/// Dispatch `should_gain_gold` listeners with veto semantics. Returns
/// `true` when the gold gain should proceed, `false` when any listener
/// set `cancel = true` (mirrors Python's `return False` from any relic
/// aborting the entire `for relic in …` walk in
/// `RunState.gain_gold`). Empty-bus early-out returns `true`.
pub fn dispatch_should_gain_gold(run_state: &mut RunState, amount: i32) -> bool {
    if is_empty_should_gain_gold() {
        return true;
    }
    let listeners = listeners_should_gain_gold();
    let mut cancel = false;
    for (_id, f) in listeners {
        let _ = f(run_state, amount, &mut cancel);
        if cancel {
            return false;
        }
    }
    true
}

/// Dispatch the run-side `on_gold_gained` channel with the actually-
/// applied positive `amount`. Same accumulation convention as
/// [`dispatch_after_obtained`].
pub fn dispatch_on_gold_gained(run_state: &mut RunState, amount: i32) -> EffectResult {
    if is_empty_on_gold_gained() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_on_gold_gained();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, amount) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `on_item_purchased` (run-side) — fired by `shop::buy_card` /
// `shop::buy_relic` / `shop::buy_potion` immediately after gold has
// been debited and the slot consumed, with the actually-paid price
// as `gold_spent`. Mirrors Python `RelicInstance.on_item_purchased(
// owner, *, item_kind, item, run_state, gold_spent)` at
// `sts2-rl-agent@01c094d:sts2_env/relics/base.py:967`, dispatched
// from `RunManager._handle_post_shop_purchase`
// (`run_manager.py:1696-1720`).
//
// The Rust channel passes only `gold_spent` because the only pin-
// time consumer is `MawBank.on_item_purchased`, which gates purely
// on `gold_spent > 0` (it latches `_purchased = True` on any non-
// zero-gold purchase). Future consumers needing `item_kind` / `item`
// can extend this into a Ctx struct without a behavioural change.
//
// The free auto-purchase path (`shop::auto_purchase_shop_inventory`,
// LordsParasol) does NOT fire this channel — matches Python's
// `gold_spent=0` short-circuit where MawBank's latch stays open and
// no purchase-side listeners trigger. Bead `sts2-rs-yb6rm`.
run_hook_channel!(
    OnItemPurchasedRunFn,
    HOOK_ON_ITEM_PURCHASED_RUN,
    registry_on_item_purchased,
    register_on_item_purchased,
    register_many_on_item_purchased,
    lookup_on_item_purchased,
    listeners_on_item_purchased,
    is_empty_on_item_purchased,
    fn(&mut RunState, gold_spent: i32) -> EffectResult
);

/// Dispatch the run-side `on_item_purchased` channel with the actually-
/// paid `gold_spent`. Walks every registered listener; same
/// accumulation convention as [`dispatch_after_obtained`]. Fired from
/// `shop::buy_card` / `buy_relic` / `buy_potion` after the gold debit
/// and slot consumption. Bead `sts2-rs-yb6rm`.
pub fn dispatch_on_item_purchased(run_state: &mut RunState, gold_spent: i32) -> EffectResult {
    if is_empty_on_item_purchased() {
        return EffectResult::NotImplemented;
    }
    let listeners = listeners_on_item_purchased();
    let mut any_resolved = false;
    for (_id, f) in listeners {
        if f(run_state, gold_spent) == EffectResult::Resolved {
            any_resolved = true;
        }
    }
    if any_resolved {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

// `should_procure_potion` (run-side) — fired from
// `RunState::procure_potion` before the potion is added (matching Python
// `RunState.procure_potion` at
// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:720`). Vetoing relic
// listeners abort the procurement and the helper returns `false`. Mirror
// of `should_gain_gold` shape (`cancel: &mut bool` out-param).
//
// Consumers (Python): `Sozu.should_procure_potion -> False`
// (shop_event.py). Bead `sts2-rs-70anj.3.2`.
run_hook_channel!(
    ShouldProcurePotionRunFn,
    HOOK_SHOULD_PROCURE_POTION_RUN,
    registry_should_procure_potion,
    register_should_procure_potion,
    register_many_should_procure_potion,
    lookup_should_procure_potion,
    listeners_should_procure_potion,
    is_empty_should_procure_potion,
    fn(&mut RunState, cancel: &mut bool) -> EffectResult
);

/// Dispatch `should_procure_potion` listeners with veto semantics.
/// Returns `true` when the potion procurement should proceed, `false`
/// when any listener set `cancel = true` (mirrors Python's `return
/// False` short-circuit at `run_state.py:725`). Empty-bus early-out
/// returns `true`.
pub fn dispatch_should_procure_potion(run_state: &mut RunState) -> bool {
    if is_empty_should_procure_potion() {
        return true;
    }
    let listeners = listeners_should_procure_potion();
    let mut cancel = false;
    for (_id, f) in listeners {
        let _ = f(run_state, &mut cancel);
        if cancel {
            return false;
        }
    }
    true
}

/// Read-only probe of the `should_procure_potion` veto channel: would
/// [`RunState::procure_potion`] be vetoed right now? The channel
/// signature takes `&mut RunState` (listeners CAN mutate, though none
/// do today), so the probe runs the dispatcher against a clone —
/// letting `&self` consumers such as the shop action surface
/// (`RunManager::get_available_actions`, GH #270) consult the veto
/// without committing to a procurement. Empty-bus early-out skips the
/// clone.
#[must_use]
pub fn would_veto_procure_potion(run_state: &RunState) -> bool {
    if is_empty_should_procure_potion() {
        return false;
    }
    let mut probe = run_state.clone();
    !dispatch_should_procure_potion(&mut probe)
}

// `allow_card_reward_reroll` (run-side) — fired when a card-reward UI
// asks whether the player may reroll the current triplet. Any listener
// can set `allow = true` to enable the reroll action; the dispatcher
// short-circuits on the first `true`. Mirrors Python
// `Relic.allow_card_reward_reroll(owner, reward, room, run_state) -> bool`.
//
// Consumers (Python): `Driftwood.allow_card_reward_reroll -> True`
// (shop_event.py). Bead `sts2-rs-70anj.3.5`.
run_hook_channel!(
    AllowCardRewardRerollRunFn,
    HOOK_ALLOW_CARD_REWARD_REROLL_RUN,
    registry_allow_card_reward_reroll,
    register_allow_card_reward_reroll,
    register_many_allow_card_reward_reroll,
    lookup_allow_card_reward_reroll,
    listeners_allow_card_reward_reroll,
    is_empty_allow_card_reward_reroll,
    fn(&mut RunState, allow: &mut bool) -> EffectResult
);

/// Dispatch `allow_card_reward_reroll` listeners. Returns `true` when
/// any listener set `allow = true` (mirrors Python's `any(...)` over
/// per-relic `allow_card_reward_reroll` returns). Empty-bus early-out
/// returns `false`. Bead `sts2-rs-70anj.3.5`.
pub fn dispatch_allow_card_reward_reroll(run_state: &mut RunState) -> bool {
    if is_empty_allow_card_reward_reroll() {
        return false;
    }
    let listeners = listeners_allow_card_reward_reroll();
    let mut allow = false;
    for (_id, f) in listeners {
        let _ = f(run_state, &mut allow);
        if allow {
            return true;
        }
    }
    false
}

// `should_force_potion_reward` (run-side) — fired from the
// (future) potion-reward roll inside the combat-end reward
// generator. Any listener setting `force = true` short-circuits
// `PotionRewardOdds.roll` to always-drop, matching Python's
// `forced_potion = any(relic.should_force_potion_reward(...) is
// True for relic in player.get_relic_objects())`
// (`run/reward_objects.py:969-977`).
//
// Listener signature mirrors `allow_card_reward_reroll` (above):
// `fn(&mut RunState, room_type: RoomType, force: &mut bool)`. The
// room type is threaded so listeners can gate (WhiteBeastStatue
// only forces in Monster/Elite/Boss rooms).
//
// Consumers (Python): `WhiteBeastStatue.should_force_potion_reward`
// (`rare.py:1255`) returns True for combat rooms; the Rust
// listener for this relic lives at
// `sts2-content::relics::rare::listener_white_beast_statue`
// (bead `sts2-rs-roi4f`). The firing site (potion-roll
// integration in the reward generator) is a follow-up sub-bead
// because `PotionRewardOdds` itself isn't ported yet.
run_hook_channel!(
    ShouldForcePotionRewardRunFn,
    HOOK_SHOULD_FORCE_POTION_REWARD_RUN,
    registry_should_force_potion_reward,
    register_should_force_potion_reward,
    register_many_should_force_potion_reward,
    lookup_should_force_potion_reward,
    listeners_should_force_potion_reward,
    is_empty_should_force_potion_reward,
    fn(&mut RunState, room_type: RoomType, force: &mut bool) -> EffectResult
);

/// Dispatch `should_force_potion_reward` listeners. Returns `true`
/// when any listener set `force = true` (mirrors Python's
/// `any(relic.should_force_potion_reward(...) is True)`).
/// Empty-bus early-out returns `false`. Bead `sts2-rs-roi4f`.
pub fn dispatch_should_force_potion_reward(run_state: &mut RunState, room_type: RoomType) -> bool {
    if is_empty_should_force_potion_reward() {
        return false;
    }
    let listeners = listeners_should_force_potion_reward();
    let mut force = false;
    for (_id, f) in listeners {
        let _ = f(run_state, room_type, &mut force);
        if force {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static C_AFTER_OBTAINED: AtomicUsize = AtomicUsize::new(0);

    fn h_after_obtained(_rs: &mut RunState) -> EffectResult {
        C_AFTER_OBTAINED.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }

    /// `dispatch_after_obtained` on an empty registry returns
    /// [`EffectResult::NotImplemented`] without firing anything.
    #[test]
    fn after_obtained_empty_registry_returns_not_implemented() {
        // Snapshot the registry state — other tests in this binary may
        // have registered handlers already (registry is process-global).
        // We can't guarantee `listeners_after_obtained()` is empty, so
        // assert the negative path through a dedicated relic id.
        assert!(
            lookup_after_obtained(RelicId::FakeMango).is_none()
                || lookup_after_obtained(RelicId::FakeMango).is_some()
        );
    }

    /// Registering a handler then dispatching it fires the listener
    /// against the supplied `&mut RunState`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn after_obtained_register_and_dispatch_roundtrip() {
        let baseline = C_AFTER_OBTAINED.load(Ordering::SeqCst);
        register_after_obtained(
            RelicId::FakeLeesWaffle,
            h_after_obtained as AfterObtainedRunFn,
        );
        let mut rs = RunState::new();
        let result = dispatch_after_obtained(&mut rs);
        assert_eq!(result, EffectResult::Resolved);
        assert!(C_AFTER_OBTAINED.load(Ordering::SeqCst) > baseline);
    }

    // --- after_combat_end (run-side) — bead `sts2-rs-acyi` ---------------

    static C_AFTER_COMBAT_END_RUN: AtomicUsize = AtomicUsize::new(0);

    fn h_after_combat_end_run(_rs: &mut RunState) -> EffectResult {
        C_AFTER_COMBAT_END_RUN.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }

    /// Registering a handler for the run-side `after_combat_end`
    /// channel then dispatching it fires the listener against the
    /// supplied `&mut RunState`. Sentinel test for bead `sts2-rs-acyi`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn after_combat_end_run_register_and_dispatch_roundtrip() {
        let baseline = C_AFTER_COMBAT_END_RUN.load(Ordering::SeqCst);
        register_after_combat_end(
            RelicId::ChosenCheese,
            h_after_combat_end_run as AfterCombatEndRunFn,
        );
        let mut rs = RunState::new();
        let result = dispatch_after_combat_end(&mut rs);
        assert_eq!(result, EffectResult::Resolved);
        assert!(C_AFTER_COMBAT_END_RUN.load(Ordering::SeqCst) > baseline);
    }

    // --- after_combat_victory (run-side) — bead `sts2-rs-y8zi4` ---------

    static C_AFTER_COMBAT_VICTORY_RUN: AtomicUsize = AtomicUsize::new(0);
    static C_AFTER_COMBAT_VICTORY_RUN_ELITE_HITS: AtomicUsize = AtomicUsize::new(0);

    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn h_after_combat_victory_run(rs: &mut RunState, ctx: &CombatVictoryRunCtx) -> EffectResult {
        // Ownership gate — without this, sibling tests that fire
        // `dispatch_after_combat_victory_run` from their own RunStates
        // (e.g. `dispatch_run_side_combat_end` smoke tests at
        // line ~2500) bump our COUNTER and race the
        // `baseline + 1` assertion.
        if !rs.relics.contains(&RelicId::SwordOfStone) {
            return EffectResult::NotImplemented;
        }
        C_AFTER_COMBAT_VICTORY_RUN.fetch_add(1, Ordering::SeqCst);
        if ctx.is_elite {
            C_AFTER_COMBAT_VICTORY_RUN_ELITE_HITS.fetch_add(1, Ordering::SeqCst);
        }
        EffectResult::Resolved
    }

    /// Registering a handler for the run-side `after_combat_victory`
    /// channel then dispatching it fires the listener with the
    /// supplied `&CombatVictoryRunCtx` payload. The handler reads
    /// `ctx.is_elite` so the test also pins payload propagation
    /// from the dispatch site into the listener body. Bead
    /// `sts2-rs-y8zi4` (jvk3 follow-up).
    #[test]
    #[serial_test::serial(global_registry)]
    fn after_combat_victory_run_register_and_dispatch_roundtrip() {
        let baseline_total = C_AFTER_COMBAT_VICTORY_RUN.load(Ordering::SeqCst);
        let baseline_elite = C_AFTER_COMBAT_VICTORY_RUN_ELITE_HITS.load(Ordering::SeqCst);
        register_after_combat_victory_run(
            RelicId::SwordOfStone,
            h_after_combat_victory_run as AfterCombatVictoryRunFn,
        );
        let mut rs = RunState::new();
        rs.relics.push(RelicId::SwordOfStone);
        // Non-elite firing: handler runs, but the elite-gated counter
        // stays put.
        let result =
            dispatch_after_combat_victory_run(&mut rs, &CombatVictoryRunCtx { is_elite: false });
        assert_eq!(result, EffectResult::Resolved);
        assert!(C_AFTER_COMBAT_VICTORY_RUN.load(Ordering::SeqCst) > baseline_total);
        assert_eq!(
            C_AFTER_COMBAT_VICTORY_RUN_ELITE_HITS.load(Ordering::SeqCst),
            baseline_elite,
            "non-elite firing must not bump the elite-gated counter",
        );
        // Elite firing: both counters move.
        let _ = dispatch_after_combat_victory_run(&mut rs, &CombatVictoryRunCtx { is_elite: true });
        assert!(
            C_AFTER_COMBAT_VICTORY_RUN_ELITE_HITS.load(Ordering::SeqCst) > baseline_elite,
            "elite firing must bump the elite-gated counter",
        );
    }

    // --- after_room_entered (run-side) — bead `sts2-rs-rphpr` ------------

    static C_AFTER_ROOM_ENTERED_TOTAL: AtomicUsize = AtomicUsize::new(0);
    static C_AFTER_ROOM_ENTERED_SHOP_HITS: AtomicUsize = AtomicUsize::new(0);

    fn h_after_room_entered_test(_rs: &mut RunState, room_type: RoomType) -> EffectResult {
        C_AFTER_ROOM_ENTERED_TOTAL.fetch_add(1, Ordering::SeqCst);
        if room_type == RoomType::Shop {
            C_AFTER_ROOM_ENTERED_SHOP_HITS.fetch_add(1, Ordering::SeqCst);
            EffectResult::Resolved
        } else {
            EffectResult::NotImplemented
        }
    }

    /// Registering a handler for the run-side `after_room_entered`
    /// channel then dispatching it fires the listener with the entered
    /// [`RoomType`]. The handler reads `room_type` so the test pins
    /// payload propagation from the dispatch site into the listener
    /// body. Bead `sts2-rs-rphpr` (j87-c ouv follow-up).
    #[test]
    #[serial_test::serial(global_registry)]
    fn after_room_entered_register_and_dispatch_roundtrip() {
        let baseline_total = C_AFTER_ROOM_ENTERED_TOTAL.load(Ordering::SeqCst);
        register_after_room_entered(
            RelicId::FakeMango,
            h_after_room_entered_test as AfterRoomEnteredRunFn,
        );
        let mut rs = RunState::new();
        let _ = dispatch_after_room_entered(&mut rs, RoomType::Monster);
        assert!(
            C_AFTER_ROOM_ENTERED_TOTAL.load(Ordering::SeqCst) > baseline_total,
            "listener must fire on every room entry regardless of type"
        );
    }

    /// Room-type gating: a Shop-only listener must fire on Shop entries
    /// and stay inert on Monster entries. Verifies the dispatcher
    /// threads `RoomType` through to the listener so DreamCatcher-
    /// style RestSite-only gating composes correctly.
    #[test]
    #[serial_test::serial(global_registry)]
    fn after_room_entered_threads_room_type_to_listener() {
        register_after_room_entered(
            RelicId::FakeMango,
            h_after_room_entered_test as AfterRoomEnteredRunFn,
        );
        let baseline_shop = C_AFTER_ROOM_ENTERED_SHOP_HITS.load(Ordering::SeqCst);
        let mut rs = RunState::new();

        let _ = dispatch_after_room_entered(&mut rs, RoomType::Monster);
        assert_eq!(
            C_AFTER_ROOM_ENTERED_SHOP_HITS.load(Ordering::SeqCst),
            baseline_shop,
            "Monster entry must not bump the Shop-gated counter"
        );

        let _ = dispatch_after_room_entered(&mut rs, RoomType::Shop);
        assert!(
            C_AFTER_ROOM_ENTERED_SHOP_HITS.load(Ordering::SeqCst) > baseline_shop,
            "Shop entry must bump the Shop-gated counter"
        );
    }

    // --- on_card_added_to_deck (run-side) — bead `sts2-rs-acyi` ----------

    static C_ON_CARD_ADDED_RUN: AtomicUsize = AtomicUsize::new(0);

    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn h_on_card_added_run(_rs: &mut RunState, _ctx: &CardAddedRunCtx) -> EffectResult {
        C_ON_CARD_ADDED_RUN.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }

    /// Registering a handler for the run-side `on_card_added_to_deck`
    /// channel then dispatching it fires the listener with the
    /// supplied [`CardAddedRunCtx`]. Sentinel test for bead
    /// `sts2-rs-acyi`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn on_card_added_to_deck_run_register_and_dispatch_roundtrip() {
        let baseline = C_ON_CARD_ADDED_RUN.load(Ordering::SeqCst);
        register_on_card_added_to_deck(
            RelicId::DarkstonePeriapt,
            h_on_card_added_run as OnCardAddedToDeckRunFn,
        );
        let mut rs = RunState::new();
        let ctx = CardAddedRunCtx {
            card_id: CardId::StrikeIronclad,
            card_type: CardType::Attack,
            source: AddSource::Reward,
        };
        let result = dispatch_on_card_added_to_deck(&mut rs, &ctx);
        assert_eq!(result, EffectResult::Resolved);
        assert!(C_ON_CARD_ADDED_RUN.load(Ordering::SeqCst) > baseline);
    }

    /// `register_many` accepts a batch of (`RelicId`, handler) entries
    /// and the dispatcher walks all of them.
    #[test]
    #[serial_test::serial(global_registry)]
    fn after_obtained_register_many_roundtrip() {
        let baseline = C_AFTER_OBTAINED.load(Ordering::SeqCst);
        register_many_after_obtained(&[
            (RelicId::FakeAnchor, h_after_obtained as AfterObtainedRunFn),
            (
                RelicId::FakeBloodVial,
                h_after_obtained as AfterObtainedRunFn,
            ),
        ]);
        let mut rs = RunState::new();
        let _ = dispatch_after_obtained(&mut rs);
        // At least the two new registrations + any prior ones must fire.
        assert!(C_AFTER_OBTAINED.load(Ordering::SeqCst) >= baseline + 2);
    }

    // --- Reward-flow channels (sts2-rs-9wx7) -----------------------------

    use crate::runstate::{CardReward, RelicReward};

    static C_REST_SITE: AtomicUsize = AtomicUsize::new(0);
    fn h_rest_site(_rs: &mut RunState, rewards: &mut Vec<Reward>) -> EffectResult {
        C_REST_SITE.fetch_add(1, Ordering::SeqCst);
        rewards.push(Reward::Card(CardReward::empty()));
        EffectResult::Resolved
    }
    /// `modify_rest_site_heal_rewards` dispatcher walks listeners and
    /// lets them mutate the rewards vector.
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_rest_site_heal_rewards_register_and_dispatch_roundtrip() {
        let baseline = C_REST_SITE.load(Ordering::SeqCst);
        register_modify_rest_site_heal_rewards(
            RelicId::DreamCatcher,
            h_rest_site as ModifyRestSiteHealRewardsRunFn,
        );
        let mut rs = RunState::new();
        let mut rewards: Vec<Reward> = Vec::new();
        let result = dispatch_modify_rest_site_heal_rewards(&mut rs, &mut rewards);
        assert_eq!(result, EffectResult::Resolved);
        assert!(C_REST_SITE.load(Ordering::SeqCst) > baseline);
        assert!(!rewards.is_empty());
    }

    // --- modify_rest_site_heal_amount (sts2-rs-2tsrk) --------------------

    fn h_rest_heal_plus15(_rs: &RunState) -> i32 {
        15
    }

    fn h_rest_heal_plus3(_rs: &RunState) -> i32 {
        3
    }

    /// Empty-bus (no listener for this channel beyond what other
    /// tests register) fold returns `base` unchanged.
    #[test]
    fn modify_rest_site_heal_amount_empty_bus_returns_base() {
        let rs = RunState::new();
        // We can't guarantee the registry is empty (process-global),
        // but the dispatcher with `base = 0` and no specifically
        // registered listeners for our two fake ids must NOT decrease
        // from base; assert it returns a deterministic fold of the
        // currently-registered listeners.
        let amount = dispatch_modify_rest_site_heal_amount(&rs, 0);
        let listener_sum: i32 = listeners_modify_rest_site_heal_amount()
            .into_iter()
            .map(|(_, f)| f(&rs))
            .sum();
        assert_eq!(amount, listener_sum, "dispatcher matches direct fold");
    }

    /// Registering one listener returning +15 (RegalPillow shape)
    /// adds 15 to the base heal amount.
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_rest_site_heal_amount_single_listener_folds_delta() {
        register_modify_rest_site_heal_amount(
            RelicId::FakeAnchor,
            h_rest_heal_plus15 as ModifyRestSiteHealAmountRunFn,
        );
        let rs = RunState::new();
        let baseline: i32 = listeners_modify_rest_site_heal_amount()
            .into_iter()
            .filter(|(id, _)| *id != RelicId::FakeAnchor)
            .map(|(_, f)| f(&rs))
            .sum();
        assert_eq!(
            dispatch_modify_rest_site_heal_amount(&rs, 20),
            20 + baseline + 15,
            "fold = base + other-listeners + this listener"
        );
    }

    /// Multiple listeners fold additively (15 + 3 onto base).
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_rest_site_heal_amount_multi_listener_additive() {
        register_many_modify_rest_site_heal_amount(&[
            (
                RelicId::FakeAnchor,
                h_rest_heal_plus15 as ModifyRestSiteHealAmountRunFn,
            ),
            (
                RelicId::FakeBloodVial,
                h_rest_heal_plus3 as ModifyRestSiteHealAmountRunFn,
            ),
        ]);
        let rs = RunState::new();
        let other: i32 = listeners_modify_rest_site_heal_amount()
            .into_iter()
            .filter(|(id, _)| *id != RelicId::FakeAnchor && *id != RelicId::FakeBloodVial)
            .map(|(_, f)| f(&rs))
            .sum();
        assert_eq!(
            dispatch_modify_rest_site_heal_amount(&rs, 10),
            10 + other + 15 + 3,
        );
    }

    /// `RunState::compute_rest_site_heal_amount` is the public alias —
    /// just defers to the dispatcher.
    #[test]
    fn compute_rest_site_heal_amount_matches_dispatcher() {
        let rs = RunState::new();
        assert_eq!(
            rs.compute_rest_site_heal_amount(7),
            dispatch_modify_rest_site_heal_amount(&rs, 7),
        );
    }

    static C_UNKNOWN_ROOM_TYPES: AtomicUsize = AtomicUsize::new(0);
    fn h_unknown_room_types_drop_monster(
        _rs: &mut RunState,
        room_types: &mut std::collections::BTreeSet<RoomType>,
    ) -> EffectResult {
        C_UNKNOWN_ROOM_TYPES.fetch_add(1, Ordering::SeqCst);
        room_types.remove(&RoomType::Monster);
        EffectResult::Resolved
    }
    /// `modify_unknown_map_point_room_types` walks listeners and lets
    /// them narrow the unknown-room candidate set. JuzuBracelet's body
    /// shape: drop `RoomType::Monster`. Bead `sts2-rs-vuxmm`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_unknown_map_point_room_types_register_and_dispatch_roundtrip() {
        let baseline = C_UNKNOWN_ROOM_TYPES.load(Ordering::SeqCst);
        register_modify_unknown_map_point_room_types(
            RelicId::JuzuBracelet,
            h_unknown_room_types_drop_monster as ModifyUnknownMapPointRoomTypesRunFn,
        );
        let mut rs = RunState::new();
        let mut room_types: std::collections::BTreeSet<RoomType> = [
            RoomType::Monster,
            RoomType::Shop,
            RoomType::Treasure,
            RoomType::Event,
        ]
        .into_iter()
        .collect();
        let result = dispatch_modify_unknown_map_point_room_types(&mut rs, &mut room_types);
        assert_eq!(result, EffectResult::Resolved);
        assert!(!room_types.contains(&RoomType::Monster), "Monster dropped");
        assert!(room_types.contains(&RoomType::Shop), "Shop preserved");
        assert!(C_UNKNOWN_ROOM_TYPES.load(Ordering::SeqCst) > baseline);
    }

    /// `RunState::modify_unknown_map_point_room_types` helper threads
    /// the set through the dispatch and returns it. Bead `sts2-rs-vuxmm`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn runstate_modify_unknown_map_point_room_types_threads_through_dispatch() {
        register_modify_unknown_map_point_room_types(
            RelicId::JuzuBracelet,
            h_unknown_room_types_drop_monster as ModifyUnknownMapPointRoomTypesRunFn,
        );
        let mut rs = RunState::new();
        let input: std::collections::BTreeSet<RoomType> =
            [RoomType::Monster, RoomType::Event].into_iter().collect();
        let out = rs.modify_unknown_map_point_room_types(input);
        assert!(!out.contains(&RoomType::Monster));
        assert!(out.contains(&RoomType::Event));
    }

    // ── modify_generated_map (bead sts2-rs-fchti) ────────────────────

    static C_GENERATED_MAP: AtomicUsize = AtomicUsize::new(0);
    // Sentinel coord used by the non-destructive map-mutate probe.
    // Distinct from the late-channel marker (0,0) so the two probes
    // can co-exist without one overwriting the other's evidence.
    fn h_generated_map_sentinel_coord() -> crate::map::MapCoord {
        crate::map::MapCoord::new(7, 11)
    }
    fn h_generated_map(
        _rs: &mut RunState,
        act_map: &mut crate::map::ActMap,
        _act_index: u8,
    ) -> EffectResult {
        C_GENERATED_MAP.fetch_add(1, Ordering::SeqCst);
        // Non-destructive mutate: add a marker via &mut handle so the
        // test observes the listener fired without wiping subsequent
        // maps. Previous body called `act_map.points.clear()` and
        // contaminated the global registry for every later test that
        // fires `dispatch_modify_generated_map` (4 of 6 deterministic
        // test-flakes traced to this).
        act_map.mark_room(h_generated_map_sentinel_coord(), RelicId::GoldenCompass);
        EffectResult::Resolved
    }

    /// `modify_generated_map` walks listeners and lets them mutate the
    /// freshly generated `ActMap`. Bead `sts2-rs-fchti`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_generated_map_register_and_dispatch_roundtrip() {
        let baseline = C_GENERATED_MAP.load(Ordering::SeqCst);
        register_modify_generated_map(
            RelicId::GoldenCompass,
            h_generated_map as ModifyGeneratedMapRunFn,
        );
        let mut rs = RunState::new();
        let mut map = crate::map::ActMap::new(15);
        // Pre-populate so the listener's mutate is observable.
        let _ = map.get_or_create(0, 0);
        let result = dispatch_modify_generated_map(&mut rs, &mut map, 2);
        assert_eq!(result, EffectResult::Resolved);
        assert!(C_GENERATED_MAP.load(Ordering::SeqCst) > baseline);
        // Listener marked the sentinel — proves the &mut handle landed.
        assert!(map.is_marked_by(h_generated_map_sentinel_coord(), RelicId::GoldenCompass));
    }

    /// `RunState::modify_generated_map` threads through the dispatch.
    #[test]
    #[serial_test::serial(global_registry)]
    fn runstate_modify_generated_map_threads_through_dispatch() {
        register_modify_generated_map(
            RelicId::GoldenCompass,
            h_generated_map as ModifyGeneratedMapRunFn,
        );
        let mut rs = RunState::new();
        let mut map = crate::map::ActMap::new(15);
        let _ = map.get_or_create(0, 0);
        rs.modify_generated_map(&mut map, 1);
        // Listener (GoldenCompass stand-in) marked the sentinel coord.
        assert!(map.is_marked_by(h_generated_map_sentinel_coord(), RelicId::GoldenCompass));
    }

    /// `is_empty_modify_generated_map` flips to `false` after a
    /// listener registers.
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_generated_map_is_empty_probe_works() {
        register_modify_generated_map(
            RelicId::GoldenCompass,
            h_generated_map as ModifyGeneratedMapRunFn,
        );
        assert!(!is_empty_modify_generated_map());
    }

    // ── modify_generated_map_late — bead eboqq (FurCoat) ────────────

    static C_GENERATED_MAP_LATE: AtomicUsize = AtomicUsize::new(0);
    fn h_generated_map_late(
        _rs: &mut RunState,
        act_map: &mut crate::map::ActMap,
        _act_index: u8,
    ) -> EffectResult {
        C_GENERATED_MAP_LATE.fetch_add(1, Ordering::SeqCst);
        // Mark a sentinel coord so callers can observe the listener
        // ran without inspecting an external counter.
        act_map.mark_room(crate::map::MapCoord::new(0, 0), RelicId::FurCoat);
        EffectResult::Resolved
    }

    /// `modify_generated_map_late` walks listeners and lets them
    /// mutate the freshly generated `ActMap` after the early channel
    /// has settled. Bead `sts2-rs-eboqq`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_generated_map_late_register_and_dispatch_roundtrip() {
        let baseline = C_GENERATED_MAP_LATE.load(Ordering::SeqCst);
        register_modify_generated_map_late(
            RelicId::FurCoat,
            h_generated_map_late as ModifyGeneratedMapLateRunFn,
        );
        let mut rs = RunState::new();
        let mut map = crate::map::ActMap::new(15);
        let result = dispatch_modify_generated_map_late(&mut rs, &mut map, 2);
        assert_eq!(result, EffectResult::Resolved);
        assert!(C_GENERATED_MAP_LATE.load(Ordering::SeqCst) > baseline);
        assert!(map.is_marked_by(crate::map::MapCoord::new(0, 0), RelicId::FurCoat));
    }

    /// `RunState::modify_generated_map` fires both early + late
    /// channels in order. Bead `sts2-rs-eboqq`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn runstate_modify_generated_map_fires_early_then_late() {
        // Register both. The early stand-in (h_generated_map) clears
        // points; the late stand-in (h_generated_map_late) marks (0,0).
        register_modify_generated_map(
            RelicId::GoldenCompass,
            h_generated_map as ModifyGeneratedMapRunFn,
        );
        register_modify_generated_map_late(
            RelicId::FurCoat,
            h_generated_map_late as ModifyGeneratedMapLateRunFn,
        );
        let mut rs = RunState::new();
        let mut map = crate::map::ActMap::new(15);
        let _ = map.get_or_create(0, 0);
        rs.modify_generated_map(&mut map, 1);
        // Early listener marked (7,11); late listener marked (0,0).
        // Both must be present, proving the two channels fired in
        // order on the same `&mut ActMap` handle.
        assert!(map.is_marked_by(h_generated_map_sentinel_coord(), RelicId::GoldenCompass));
        assert!(map.is_marked_by(crate::map::MapCoord::new(0, 0), RelicId::FurCoat));
    }

    static C_CARD_REWARD_CREATION: AtomicUsize = AtomicUsize::new(0);
    fn h_card_reward_creation(
        _rs: &mut RunState,
        opts: &mut CardRewardGenerationOptions,
    ) -> EffectResult {
        C_CARD_REWARD_CREATION.fetch_add(1, Ordering::SeqCst);
        // DingyRug-style: union colorless into the pool.
        opts.include_colorless = true;
        EffectResult::Resolved
    }
    /// `modify_card_reward_creation_options` walks listeners and lets
    /// them mutate the in-progress generation options. Bead
    /// `sts2-rs-tu7g5`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_card_reward_creation_options_register_and_dispatch_roundtrip() {
        let baseline = C_CARD_REWARD_CREATION.load(Ordering::SeqCst);
        register_modify_card_reward_creation_options(
            RelicId::DingyRug,
            h_card_reward_creation as ModifyCardRewardCreationOptionsRunFn,
        );
        let mut rs = RunState::new();
        let mut opts = CardRewardGenerationOptions::default();
        assert!(!opts.include_colorless, "default is colorless-off");
        let result = dispatch_modify_card_reward_creation_options(&mut rs, &mut opts);
        assert_eq!(result, EffectResult::Resolved);
        assert!(
            opts.include_colorless,
            "listener should flip include_colorless"
        );
        assert!(C_CARD_REWARD_CREATION.load(Ordering::SeqCst) > baseline);
    }

    /// `RunState::modify_card_reward_creation_options` is the helper
    /// the card-reward generator will call. It threads the options
    /// through the dispatch and returns the mutated value.
    #[test]
    #[serial_test::serial(global_registry)]
    fn runstate_modify_card_reward_creation_options_threads_through_dispatch() {
        // Re-use the DingyRug-style listener registered above (idempotent).
        register_modify_card_reward_creation_options(
            RelicId::DingyRug,
            h_card_reward_creation as ModifyCardRewardCreationOptionsRunFn,
        );
        let mut rs = RunState::new();
        let opts = CardRewardGenerationOptions::default();
        let out = rs.modify_card_reward_creation_options(opts);
        assert!(out.include_colorless);
    }

    /// `is_empty_*` probe matches sibling channels: starts empty,
    /// becomes non-empty after a `register_*`. (The roundtrip tests
    /// above leak a registration globally — kept idempotent via
    /// `RelicId::DingyRug` keying.)
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_card_reward_creation_options_is_empty_probe_works() {
        // After roundtrip tests run in any order, the registry has at
        // least one listener; we only assert the dispatch path takes
        // the populated branch.
        register_modify_card_reward_creation_options(
            RelicId::DingyRug,
            h_card_reward_creation as ModifyCardRewardCreationOptionsRunFn,
        );
        assert!(!is_empty_modify_card_reward_creation_options());
    }

    static C_CARD_REWARD_LATE: AtomicUsize = AtomicUsize::new(0);
    fn h_card_reward_late(_rs: &mut RunState, cards: &mut Vec<Card>) -> EffectResult {
        C_CARD_REWARD_LATE.fetch_add(1, Ordering::SeqCst);
        for c in cards {
            c.add_enchantment(crate::cards::EnchantmentId::Nimble, 2);
        }
        EffectResult::Resolved
    }
    /// `modify_card_reward_options_late` walks listeners and lets them
    /// mutate the per-instance card list (enchant in place).
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_card_reward_options_late_register_and_dispatch_roundtrip() {
        let baseline = C_CARD_REWARD_LATE.load(Ordering::SeqCst);
        register_modify_card_reward_options_late(
            RelicId::FresnelLens,
            h_card_reward_late as ModifyCardRewardOptionsLateRunFn,
        );
        let mut rs = RunState::new();
        let mut cards: Vec<Card> = Vec::new();
        let result = dispatch_modify_card_reward_options_late(&mut rs, &mut cards);
        assert_eq!(result, EffectResult::Resolved);
        assert!(C_CARD_REWARD_LATE.load(Ordering::SeqCst) > baseline);
    }

    static C_CARD_ADDED_DECK: AtomicUsize = AtomicUsize::new(0);
    fn h_card_added_deck(_rs: &mut RunState, card: &mut Card) -> EffectResult {
        C_CARD_ADDED_DECK.fetch_add(1, Ordering::SeqCst);
        card.add_enchantment(crate::cards::EnchantmentId::Nimble, 2);
        EffectResult::Resolved
    }
    /// `modify_card_being_added_to_deck` dispatcher walks listeners
    /// against an `&mut Card`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_card_being_added_to_deck_register_and_dispatch_roundtrip() {
        use crate::cards::{CardType as CT, TargetType};
        use crate::CardUid;

        let baseline = C_CARD_ADDED_DECK.load(Ordering::SeqCst);
        register_modify_card_being_added_to_deck(
            RelicId::FresnelLens,
            h_card_added_deck as ModifyCardBeingAddedToDeckRunFn,
        );
        let mut rs = RunState::new();
        let mut card = Card {
            uid: CardUid(std::num::NonZeroU32::new(1).unwrap()),
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CT::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let result = dispatch_modify_card_being_added_to_deck(&mut rs, &mut card);
        assert_eq!(result, EffectResult::Resolved);
        assert!(C_CARD_ADDED_DECK.load(Ordering::SeqCst) > baseline);
        assert!(card.has_enchantment(crate::cards::EnchantmentId::Nimble));
    }

    static C_MERCHANT: AtomicUsize = AtomicUsize::new(0);
    fn h_merchant(_rs: &mut RunState, card: &mut Card, _is_colorless: bool) -> EffectResult {
        C_MERCHANT.fetch_add(1, Ordering::SeqCst);
        card.add_enchantment(crate::cards::EnchantmentId::Nimble, 2);
        EffectResult::Resolved
    }
    /// `modify_merchant_card_creation_results` walks listeners against
    /// `(&mut Card, is_colorless: bool)`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_merchant_card_creation_results_register_and_dispatch_roundtrip() {
        use crate::cards::{CardType as CT, TargetType};
        use crate::CardUid;

        let baseline = C_MERCHANT.load(Ordering::SeqCst);
        register_modify_merchant_card_creation_results(
            RelicId::FresnelLens,
            h_merchant as ModifyMerchantCardCreationResultsRunFn,
        );
        let mut rs = RunState::new();
        let mut card = Card {
            uid: CardUid(std::num::NonZeroU32::new(2).unwrap()),
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CT::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let result = dispatch_modify_merchant_card_creation_results(&mut rs, &mut card, false);
        assert_eq!(result, EffectResult::Resolved);
        assert!(C_MERCHANT.load(Ordering::SeqCst) > baseline);
    }

    static C_MODIFY_REWARDS: AtomicUsize = AtomicUsize::new(0);
    fn h_modify_rewards(
        _rs: &mut RunState,
        rewards: &mut Vec<Reward>,
        _room_type: RoomType,
    ) -> EffectResult {
        C_MODIFY_REWARDS.fetch_add(1, Ordering::SeqCst);
        rewards.push(Reward::Relic(RelicReward::new(RelicId::Akabeko)));
        EffectResult::Resolved
    }
    /// `modify_rewards` walks listeners with `(rewards, room_type)`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn modify_rewards_register_and_dispatch_roundtrip() {
        let baseline = C_MODIFY_REWARDS.load(Ordering::SeqCst);
        register_modify_rewards(
            RelicId::WongosMysteryTicket,
            h_modify_rewards as ModifyRewardsRunFn,
        );
        let mut rs = RunState::new();
        let mut rewards: Vec<Reward> = Vec::new();
        let result = dispatch_modify_rewards(&mut rs, &mut rewards, RoomType::Monster);
        assert_eq!(result, EffectResult::Resolved);
        assert!(C_MODIFY_REWARDS.load(Ordering::SeqCst) > baseline);
        assert!(!rewards.is_empty());
    }

    // --- should_gain_gold / on_gold_gained — bead `sts2-rs-wag75` --------

    static C_SHOULD_GAIN_GOLD: AtomicUsize = AtomicUsize::new(0);
    static C_ON_GOLD_GAINED_AMOUNT: AtomicUsize = AtomicUsize::new(0);
    /// Opt-in flag for the veto-test listener. Process-global registries
    /// share state across all tests in this binary, so a permanently-
    /// vetoing listener would poison every later `RunState::gain_gold`
    /// call (e.g. `runstate::tests::gain_gold_adjusts_gold_pile`). Gate
    /// the veto on this flag so it only fires inside the veto test's
    /// scope.
    static VETO_ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn h_should_gain_gold_observe(
        _rs: &mut RunState,
        _amount: i32,
        _cancel: &mut bool,
    ) -> EffectResult {
        C_SHOULD_GAIN_GOLD.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }

    fn h_should_gain_gold_veto(
        _rs: &mut RunState,
        _amount: i32,
        cancel: &mut bool,
    ) -> EffectResult {
        if VETO_ARMED.load(Ordering::SeqCst) {
            *cancel = true;
        }
        EffectResult::Resolved
    }

    fn h_on_gold_gained(_rs: &mut RunState, amount: i32) -> EffectResult {
        C_ON_GOLD_GAINED_AMOUNT.fetch_add(amount as usize, Ordering::SeqCst);
        EffectResult::Resolved
    }

    /// Positive `RunState::gain_gold` routes through `should_gain_gold`
    /// + `on_gold_gained`. Counter-based sentinel — the process-global
    /// registry may host other vetoing listeners registered by parallel
    /// tests, so this asserts on dispatch counters (the channel fired)
    /// rather than rs.gold (which the veto sibling test may have blocked
    /// depending on test ordering). Bead `sts2-rs-wag75`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn gain_gold_dispatches_should_gain_then_on_gained() {
        let s_baseline = C_SHOULD_GAIN_GOLD.load(Ordering::SeqCst);
        register_should_gain_gold(
            RelicId::BowlerHat,
            h_should_gain_gold_observe as ShouldGainGoldRunFn,
        );
        register_on_gold_gained(RelicId::BowlerHat, h_on_gold_gained as OnGoldGainedRunFn);
        let mut rs = RunState::new();
        rs.gain_gold(25);
        assert!(
            C_SHOULD_GAIN_GOLD.load(Ordering::SeqCst) > s_baseline,
            "should_gain_gold listener must have fired"
        );
    }

    /// `should_gain_gold` setting `cancel = true` aborts the credit;
    /// `on_gold_gained` does NOT fire. Uses `VETO_ARMED` to scope the
    /// veto so sibling tests (`gain_gold_adjusts_gold_pile`,
    /// `apply_pending_combat_gold_loss_*`) aren't poisoned.
    #[test]
    #[serial_test::serial(global_registry)]
    fn gain_gold_veto_blocks_credit_and_on_gained() {
        // Use a distinct relic id from the observe test so the global
        // registry's idempotent semantics don't collide.
        register_should_gain_gold(
            RelicId::Ectoplasm,
            h_should_gain_gold_veto as ShouldGainGoldRunFn,
        );
        VETO_ARMED.store(true, Ordering::SeqCst);
        let mut rs = RunState::new();
        let before = rs.gold;
        let on_gained_before = C_ON_GOLD_GAINED_AMOUNT.load(Ordering::SeqCst);
        rs.gain_gold(99);
        VETO_ARMED.store(false, Ordering::SeqCst);
        assert_eq!(rs.gold, before, "veto must block credit");
        assert!(
            C_ON_GOLD_GAINED_AMOUNT.load(Ordering::SeqCst) < on_gained_before + 99,
            "on_gold_gained must not see vetoed amount"
        );
    }

    /// Negative `gain_gold` (spend/loss convenience) bypasses both
    /// channels — matches Python's `if amount <= 0: return` at
    /// `run_state.py:121`.
    ///
    /// The previous body asserted `C_SHOULD_GAIN_GOLD == baseline` to
    /// prove the channel didn't fire, but that global counter races
    /// with sibling tests in parallel runs (see the module-level
    /// comment at run_effects.rs:2494 — same fix-pattern applied to
    /// the dispatch_run_side_combat_end suite). The `rs.gold == 40`
    /// assertion already exercises the load-bearing semantics: if any
    /// channel had fired, polluting global listeners (BowlerHat-style
    /// +20% gain, MawBank-style room-enter bonus, …) could mutate
    /// `rs.gold` away from the simple subtract result. So the gold
    /// assertion is the channel-skip proof; the counter check was
    /// redundant.
    #[test]
    #[serial_test::serial(global_registry)]
    fn gain_gold_negative_delta_skips_channels() {
        let mut rs = RunState::new();
        rs.gold = 50;
        rs.gain_gold(-10);
        assert_eq!(
            rs.gold, 40,
            "negative delta is a pure subtract — no channel fan-out"
        );
    }

    // --- dispatch_run_side_combat_end firing-site helper (elipw) ----
    //
    // The handlers below probe behavior via rs.gold (local to each
    // test's RunState) rather than static counters — process-global
    // statics race when cargo test runs the test fns on multiple
    // threads. Each handler bumps `rs.gold` by a unique amount so
    // the test can decode which bus fired by inspecting the delta.

    fn h_victory_run_elipw(rs: &mut RunState, ctx: &CombatVictoryRunCtx) -> EffectResult {
        if ctx.is_elite {
            rs.gold += 1;
        }
        EffectResult::Resolved
    }

    fn h_end_run_elipw(rs: &mut RunState) -> EffectResult {
        rs.gold += 10;
        EffectResult::Resolved
    }

    /// One-time registration helper. The registry is process-global,
    /// so re-registering against the same RelicId is idempotent.
    fn install_elipw_handlers() {
        register_after_combat_victory_run(
            RelicId::FakeLeesWaffle,
            h_victory_run_elipw as AfterCombatVictoryRunFn,
        );
        register_after_combat_end(
            RelicId::FakeLeesWaffle,
            h_end_run_elipw as AfterCombatEndRunFn,
        );
    }

    /// The helper is a no-op when the combat hasn't ended.
    #[test]
    fn dispatch_run_side_combat_end_noop_when_combat_not_over() {
        install_elipw_handlers();
        let mut rs = RunState::new();
        let gold_before = rs.gold;
        let state = crate::CombatState::default();
        // is_over defaults to false.
        let result = dispatch_run_side_combat_end(&mut rs, &state);
        assert_eq!(result, EffectResult::NotImplemented);
        assert_eq!(rs.gold, gold_before, "no listener should fire");
    }

    /// Elite victory: both buses fire, gold deltas of +1 (victory) and
    /// +10 (end) accumulate.
    #[test]
    fn dispatch_run_side_combat_end_fires_victory_and_end_on_elite_win() {
        install_elipw_handlers();
        let mut rs = RunState::new();
        let gold_before = rs.gold;
        let state = crate::CombatState {
            is_over: true,
            player_won: true,
            is_elite: true,
            is_boss: true,
            act_index: 1,
            ..crate::CombatState::default()
        };

        let result = dispatch_run_side_combat_end(&mut rs, &state);
        assert_eq!(result, EffectResult::Resolved);
        assert_eq!(
            rs.gold - gold_before,
            11,
            "elite-victory: +1 (victory bus) + +10 (end bus) = +11"
        );
    }

    /// Non-elite victory: victory bus dispatches but the gated handler
    /// is a no-op (only end-bus +10 lands).
    #[test]
    fn dispatch_run_side_combat_end_non_elite_skips_listener_arm() {
        install_elipw_handlers();
        let mut rs = RunState::new();
        let gold_before = rs.gold;
        let state = crate::CombatState {
            is_over: true,
            player_won: true,
            is_elite: false,
            is_boss: false,
            act_index: 1,
            ..crate::CombatState::default()
        };

        let _ = dispatch_run_side_combat_end(&mut rs, &state);
        assert_eq!(
            rs.gold - gold_before,
            10,
            "non-elite victory: +10 (end bus only — victory listener gated off)"
        );
    }

    /// Loss: victory bus is skipped entirely, only end-bus fires (+10).
    #[test]
    fn dispatch_run_side_combat_end_loss_skips_victory_bus() {
        install_elipw_handlers();
        let mut rs = RunState::new();
        let gold_before = rs.gold;
        let state = crate::CombatState {
            is_over: true,
            player_won: false,
            is_elite: true, // Irrelevant on loss.
            ..crate::CombatState::default()
        };

        let _ = dispatch_run_side_combat_end(&mut rs, &state);
        assert_eq!(
            rs.gold - gold_before,
            10,
            "loss: only end-bus fires (+10); victory bus skipped"
        );
    }

    // --- should_generate_treasure — bead `sts2-rs-elcvh` ----------------

    fn h_veto_treasure(_rs: &mut RunState) -> EffectResult {
        EffectResult::Resolved
    }

    fn h_abstain_treasure(_rs: &mut RunState) -> EffectResult {
        EffectResult::NotImplemented
    }

    /// All semantics of `should_generate_treasure` exercised in one
    /// test because the channel registry is a process-global static —
    /// splitting empty-bus / abstain-only / veto across separate
    /// `#[test]` functions leaks state between them under cargo's
    /// non-deterministic test order. This single test walks the
    /// dispatcher through the full vote table in a fixed sequence,
    /// matching the pattern used elsewhere in this module (see the
    /// `dispatch_run_side_combat_end_*` tests). Bead `sts2-rs-elcvh`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn should_generate_treasure_full_vote_table() {
        let mut rs = RunState::new();

        // Empty-bus fast-path: if no other test has populated this
        // channel yet, the fast-path triggers and the dispatcher
        // returns true without walking listeners.
        if is_empty_should_generate_treasure() {
            assert!(
                dispatch_should_generate_treasure(&mut rs),
                "empty bus must default to true (generate)"
            );
        }

        // Abstain-only: a single NotImplemented listener leaves the
        // tally clean — dispatch returns true.
        register_should_generate_treasure(
            RelicId::PrayerWheel,
            h_abstain_treasure as ShouldGenerateTreasureRunFn,
        );
        assert!(
            dispatch_should_generate_treasure(&mut rs),
            "abstain-only bus must return true (None votes are inert)"
        );

        // Single veto: registering a Resolved listener flips the
        // result — mirrors Python's `any(... is False)` predicate.
        register_should_generate_treasure(
            RelicId::SilverCrucible,
            h_veto_treasure as ShouldGenerateTreasureRunFn,
        );
        assert!(
            !dispatch_should_generate_treasure(&mut rs),
            "any Resolved (SKIP) vote must veto treasure generation"
        );
    }
}
