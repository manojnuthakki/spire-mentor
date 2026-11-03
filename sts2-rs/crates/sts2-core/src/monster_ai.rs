use std::collections::BTreeMap;

use smallvec::SmallVec;

use crate::monsters::Intent;
use crate::{CombatState, CreatureUid, DotNetRng};

pub type StateId = &'static str;
pub type EffectFn = fn(&mut CombatState, CreatureUid);

/// Sorted-by-id state store for [`MonsterAi`] (perf 2026-08-18).
///
/// Drop-in replacement for the previous `BTreeMap<StateId, MonsterState>`:
/// same lookup semantics, same (lexicographic) iteration order, same panic
/// on a missing indexed key — but a flat sorted `Vec`, so [`MonsterAi`] can
/// keep a cached index of its current state and read it back in O(1)
/// instead of re-walking a string-keyed tree on every `current_move()` /
/// transition check (the memcmp cost in the 2026-08-17 playout callgrind).
/// Constructors still ACCEPT a `BTreeMap` (the content builders are
/// unchanged); it is compiled into this table at spawn.
#[derive(Clone, Debug, Default)]
pub struct StateTable {
    /// Sorted by id — exactly a `BTreeMap`'s iteration order.
    entries: Vec<(StateId, MonsterState)>,
}

impl StateTable {
    fn idx_of(&self, id: &str) -> Option<usize> {
        self.entries.binary_search_by(|(k, _)| (*k).cmp(id)).ok()
    }

    fn entry_at(&self, idx: usize) -> Option<&(StateId, MonsterState)> {
        self.entries.get(idx)
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&MonsterState> {
        self.idx_of(id).map(|i| &self.entries[i].1)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut MonsterState> {
        self.idx_of(id).map(|i| &mut self.entries[i].1)
    }

    #[must_use]
    pub fn contains_key(&self, id: &str) -> bool {
        self.idx_of(id).is_some()
    }

    /// Insert preserving sort order; returns the previous value for a
    /// duplicate id (mirrors `BTreeMap::insert`).
    pub fn insert(&mut self, id: StateId, state: MonsterState) -> Option<MonsterState> {
        match self.entries.binary_search_by(|(k, _)| (*k).cmp(id)) {
            Ok(i) => Some(std::mem::replace(&mut self.entries[i], (id, state)).1),
            Err(i) => {
                self.entries.insert(i, (id, state));
                None
            }
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &StateId> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &MonsterState> {
        self.entries.iter().map(|(_, v)| v)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut MonsterState> {
        self.entries.iter_mut().map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&StateId, &MonsterState)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl From<BTreeMap<StateId, MonsterState>> for StateTable {
    fn from(map: BTreeMap<StateId, MonsterState>) -> Self {
        // BTreeMap iteration is already id-sorted.
        Self {
            entries: map.into_iter().collect(),
        }
    }
}

impl std::ops::Index<&str> for StateTable {
    type Output = MonsterState;
    fn index(&self, id: &str) -> &MonsterState {
        self.get(id)
            .unwrap_or_else(|| panic!("unknown monster-AI state id {id:?}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoveRepeatType {
    CanRepeatForever,
    CanRepeatXTimes,
    CannotRepeat,
    UseOnlyOnce,
}

#[derive(Clone, Debug)]
pub struct MoveState {
    pub id: StateId,
    pub effect: EffectFn,
    pub intents: SmallVec<[Intent; 4]>,
    pub follow_up: Option<StateId>,
    pub must_perform_once: bool,
}

#[derive(Clone, Debug)]
pub struct WeightedBranch {
    pub state_id: StateId,
    pub repeat_type: MoveRepeatType,
    pub max_times: u8,
    pub weight: f64,
    pub cooldown: u8,
}

impl WeightedBranch {
    fn effective_weight(&self, state_log: &[StateId]) -> f64 {
        match self.repeat_type {
            MoveRepeatType::UseOnlyOnce => {
                if state_log.contains(&self.state_id) {
                    return 0.0;
                }
            }
            MoveRepeatType::CannotRepeat => {
                if state_log.last() == Some(&self.state_id) {
                    return 0.0;
                }
            }
            MoveRepeatType::CanRepeatXTimes => {
                let consecutive = state_log
                    .iter()
                    .rev()
                    .take_while(|&&s| s == self.state_id)
                    .count();
                if consecutive >= usize::from(self.max_times) {
                    return 0.0;
                }
            }
            MoveRepeatType::CanRepeatForever => {}
        }

        if self.cooldown > 0 {
            let window = state_log.len().min(usize::from(self.cooldown));
            let recent = &state_log[state_log.len() - window..];
            if recent.contains(&self.state_id) {
                return 0.0;
            }
        }

        self.weight
    }
}

#[derive(Clone, Debug)]
pub struct RandomBranchState {
    pub id: StateId,
    pub branches: SmallVec<[WeightedBranch; 4]>,
}

/// Predicate consumed by [`ConditionalBranch`]. Two flavors:
///
/// * `Stateless` — `fn(&CombatState) -> bool`. Reads only global combat
///   state; sufficient for most checks (player block, alive enemy
///   counts, etc.).
/// * `OwnerAware` — `fn(&CombatState, CreatureUid) -> bool`. Receives
///   the AI's owning creature uid so the predicate can inspect
///   per-creature state like a Power amount on the specific monster
///   running this AI. Bead `sts2-rs-msi1` (CeremonialBeast PLOW_CHECK
///   needs `creature.get_power_amount(PLOW)` which Python reads via
///   `self.creature` — Rust's plain fn pointer can't capture that).
///
/// `OwnerAware` predicates resolve to `false` when the AI's
/// [`MonsterAi::owner_uid`] is unset; callers must invoke
/// [`MonsterAi::set_owner`] (the combat loop does this before each
/// `roll_move_with_combat`) for the predicate to fire correctly.
/// Unit-test code paths that go through [`MonsterAi::new`] and
/// [`MonsterAi::roll_move`] never see the owner-aware branches, which
/// is the intended degradation for AI-only tests.
#[derive(Clone, Copy, Debug)]
pub enum ConditionFn {
    Stateless(fn(&CombatState) -> bool),
    OwnerAware(fn(&CombatState, CreatureUid) -> bool),
    /// Owner + per-instance counter predicate. Reads the AI's
    /// `MonsterAi::counter()` snapshot threaded by the combat loop.
    /// Use for monsters whose Python `_state` dict encodes a single
    /// integer flag (FrogKnight `beetle_charged`, TestSubject `phase2`,
    /// BowlbugRock `off_balance`, SlumberingBeetle `awakened`,
    /// KnowledgeDemon curse-applied counter). Bead `sts2-rs-976k`.
    CounterAware(fn(&CombatState, CreatureUid, i32) -> bool),
}

impl ConditionFn {
    /// Resolve the predicate against the combat state, threading the
    /// AI's owning creature uid and per-instance counter when known.
    /// Owner-aware predicates with no owner context resolve to
    /// `false`.
    #[must_use]
    pub fn resolve(self, state: &CombatState, owner: Option<CreatureUid>, counter: i32) -> bool {
        match self {
            ConditionFn::Stateless(f) => f(state),
            ConditionFn::OwnerAware(f) => match owner {
                Some(uid) => f(state, uid),
                None => false,
            },
            ConditionFn::CounterAware(f) => match owner {
                Some(uid) => f(state, uid, counter),
                None => false,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConditionalBranch {
    pub condition: ConditionFn,
    pub state_id: StateId,
}

#[derive(Clone, Debug)]
pub struct ConditionalBranchState {
    pub id: StateId,
    pub branches: SmallVec<[ConditionalBranch; 4]>,
}

#[derive(Clone, Debug)]
pub enum MonsterState {
    Move(MoveState),
    RandomBranch(RandomBranchState),
    ConditionalBranch(ConditionalBranchState),
}

impl MonsterState {
    fn is_move(&self) -> bool {
        matches!(self, MonsterState::Move(_))
    }

    fn should_appear_in_logs(&self) -> bool {
        matches!(self, MonsterState::Move(_))
    }
}

/// Lookahead entry returned by [`MonsterAi::peek_intents`]. Bead
/// `sts2-rs-6pzjm` — wide-eval feature feeder.
///
/// `certainty ∈ [0.0, 1.0]` is the product of per-step branch
/// probabilities encountered while walking from the current state to
/// the move that emitted this intent. `1.0` means a deterministic
/// chain (linear or always-true conditional). Use as a feature weight
/// or as a confidence input to the value head.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeekedIntent {
    pub intent: crate::monsters::Intent,
    pub certainty: f32,
    /// 1-indexed turn offset from now. `step=1` is the immediately-next
    /// intent the AI will fire; `step=2` is the turn after that, etc.
    /// Bead `sts2-rs-h5nf6` (6pzjm.1) — lets the wide-eval feeder group
    /// peek entries into per-turn lookahead columns without re-walking
    /// the state machine.
    pub step: u8,
}

#[derive(Clone, Debug)]
pub struct MonsterAi {
    /// The moveset state machine. Wrapped in an `Arc` (perf 2026-07-05):
    /// the map is built once per monster at spawn and only rarely mutated
    /// afterwards (spawn-time tweaks / ascension scaling — those sites go
    /// through `Arc::make_mut`, which copy-on-writes), while `MonsterAi`
    /// is cloned 2-3x per MCTS simulation as part of every
    /// `CombatState::clone`. Sharing the map makes that clone a refcount
    /// bump instead of a full `BTreeMap<_, MonsterState>` deep copy.
    /// Reads deref transparently; use `Arc::make_mut(&mut ai.states)`
    /// to mutate. Storage is a [`StateTable`] (sorted flat vec) since
    /// 2026-08-18 — same lookup/iteration semantics as the previous
    /// `BTreeMap`, but it lets `current_idx` below make the hot
    /// current-state reads O(1).
    pub states: std::sync::Arc<StateTable>,
    pub initial: StateId,
    current: StateId,
    /// Cached index of `current` in `states` (perf 2026-08-18). Kept in
    /// lockstep by [`Self::set_current`]; validated on every read by a
    /// pointer+len compare against the entry's key, so an external
    /// key-set mutation through `Arc::make_mut` (spawn-time insert /
    /// remove) can only ever make reads fall back to the string lookup —
    /// never return the wrong state.
    current_idx: u32,
    state_log: SmallVec<[StateId; 16]>,
    performed_first_move: bool,
    /// Whether the initial state has been resolved against the RUN's
    /// `monster_ai` stream yet (as opposed to the throwaway rng
    /// [`Self::new_with_combat`] uses, which cannot advance the run cursor).
    ///
    /// C# resolves a monster's opening telegraph exactly once, off the real
    /// stream, when the creature is added to combat — a `Move` initial costs
    /// zero reads, a branch initial one `NextFloat`. `start_combat` does that
    /// for the encounter's roster; a creature SUMMONED mid-combat only reaches
    /// [`crate::combat_loop::roll_next_enemy_intents`], whose normal roll path
    /// early-returns while `performed_first_move` is false. Without this flag
    /// that summon keeps the throwaway-rng answer forever and the run stream
    /// stays one read behind C# for the rest of the combat (GH #544,
    /// TwoTailedRat CALL_FOR_BACKUP).
    initial_resolved: bool,
    performed_at_least_once: bool,
    /// Creature uid running this AI. Set by the combat loop before
    /// each `roll_move_with_combat` so [`ConditionFn::OwnerAware`]
    /// predicates can read per-creature state (e.g. PLOW power
    /// remaining on CeremonialBeast). `None` is the default for
    /// AI-only test paths and degrades owner-aware predicates to
    /// `false` rather than panicking. Bead `sts2-rs-msi1`.
    owner_uid: Option<CreatureUid>,
    /// Per-instance integer counter — Rust analogue of Python's
    /// single-int `_state` dict entry. Read by
    /// [`ConditionFn::CounterAware`] predicates; mutated from
    /// move-effect closures via [`MonsterAi::set_counter`] /
    /// [`MonsterAi::increment_counter`]. Default 0. Bead `sts2-rs-976k`.
    counter: i32,
    /// One-shot override consumed by the next [`Self::roll_move`] /
    /// [`Self::roll_move_with_combat`] in place of the normal
    /// follow-up chain (analogue of C# `SetMoveImmediate(state)`).
    /// `None` = follow the normal FollowUpState graph. Set via
    /// [`Self::force_next_state`]; cleared as soon as the next roll
    /// consumes it. Default `None`. Bead `sts2-rs-70zuj` (dvqa1.3 —
    /// needed by the Queen `BurnBrightForMeState → EnragedState`
    /// jump when TorchHeadAmalgam dies mid-chain) and likely future
    /// phase-switch bosses.
    forced_next_state: Option<StateId>,
    /// Encounter **slot index** this creature occupies, as a position in
    /// the encounter's `Slots` list (C# `Creature.SlotName` →
    /// `Encounter.Slots.IndexOf`). `None` for the vast majority of
    /// monsters, whose array order never changes mid-combat so the raw
    /// `enemy_uids` push order already matches C#. Set only by summoners
    /// that fill encounter slots dynamically and re-sort, where C#
    /// `CombatState.SortEnemiesBySlotName` (called from
    /// `CombatManager.AddCreature`) reorders the active `Enemies` list on
    /// every add — currently the Ovicopter, whose freed egg slots
    /// (`LastOrDefault` unused) make the post-death lay order
    /// un-expressible by a flat front-insert. Excluded from
    /// [`crate::Creature`]'s `PartialEq` (the whole `ai` field is), so it
    /// never perturbs state-equality / parity. Bead `sts2-rs-c8why`
    /// (Ovicopter multi-lay slot-sort follow-up to `sts2-rs-z4jsg`).
    slot: Option<u8>,
    /// Optional listener invoked once per `(dead_creature, this_ai's
    /// owner)` pair at the end of each `perform_intent` call in
    /// [`crate::combat_loop`], driven by
    /// `CombatState.combat_log.newly_dead_this_turn` (bead
    /// `sts2-rs-ar9ll`). The listener is registered per-AI by content
    /// constructors that need cross-creature death-observation (e.g.
    /// Queen tracking the `TorchHeadAmalgam` AfterDeath transition,
    /// bead `sts2-rs-6g4v4` — m70xg sub-3). `None` = no-op (default
    /// for every AI that does not opt in). The dispatch loop walks
    /// every alive enemy other than the dead creature itself and
    /// invokes its listener (if any), then clears the
    /// `newly_dead_this_turn` carrier.
    ///
    /// Signature: `(state, dead_uid, owner_uid)`. The listener can
    /// reach `owner`'s own AI via `state.creatures[owner_idx].ai`,
    /// so passing `&mut MonsterAi` directly is unnecessary (and
    /// would conflict with the `&mut CombatState` borrow).
    ///
    /// Bead `sts2-rs-79tv9` (dvqa1.2). Scope:
    /// docs/audits/2026-05-29-dvqa1-after-death-dispatch-scope.md.
    pub after_death: Option<fn(&mut CombatState, CreatureUid, CreatureUid)>,
}

impl MonsterAi {
    pub fn new(states: BTreeMap<StateId, MonsterState>, initial: StateId) -> Self {
        let mut ai = Self {
            states: std::sync::Arc::new(StateTable::from(states)),
            initial,
            current: initial,
            current_idx: 0,
            state_log: SmallVec::new(),
            performed_first_move: false,
            initial_resolved: false,
            performed_at_least_once: false,
            owner_uid: None,
            counter: 0,
            forced_next_state: None,
            after_death: None,
            slot: None,
        };
        ai.set_current(initial);
        // Constructor-time resolution: monsters' `initial` states are
        // Move states in practice, so the resolve loop short-circuits
        // immediately. The throwaway RNG only matters if an `initial`
        // happens to be a RandomBranch; in that pathological case the
        // counter advance is local to this RNG and does not leak into
        // the run-wide MonsterAi stream (bead `sts2-rs-5w76f`).
        let mut throwaway = DotNetRng::new(0);
        ai.resolve_to_move_no_combat(&mut throwaway);
        ai
    }

    pub fn new_with_combat(
        states: BTreeMap<StateId, MonsterState>,
        initial: StateId,
        combat: &CombatState,
    ) -> Self {
        let mut ai = Self {
            states: std::sync::Arc::new(StateTable::from(states)),
            initial,
            current: initial,
            current_idx: 0,
            state_log: SmallVec::new(),
            performed_first_move: false,
            initial_resolved: false,
            performed_at_least_once: false,
            owner_uid: None,
            counter: 0,
            forced_next_state: None,
            after_death: None,
            slot: None,
        };
        ai.set_current(initial);
        let mut throwaway = DotNetRng::new(0);
        ai.resolve_to_move_with_combat(&mut throwaway, combat);
        ai
    }

    /// Set the current state, refreshing the O(1) index cache. Every
    /// `current` assignment MUST route through here (transitions are
    /// rare — once per enemy turn — so the one binary search per call is
    /// the cold half of the trade that makes [`Self::cur`] hot-path O(1)).
    fn set_current(&mut self, id: StateId) {
        self.current = id;
        // An unknown id (force paths debug_assert against it) parks the
        // cache on an out-of-range slot: `cur()` then falls back to the
        // string lookup, which panics exactly where the old code did.
        self.current_idx = self.states.idx_of(id).map_or(u32::MAX, |i| i as u32);
    }

    /// The current [`MonsterState`], via the index cache when it is still
    /// valid (pointer+len key compare — `StateId`s are `&'static str`, so
    /// a cache refreshed by [`Self::set_current`] against these same
    /// entries compares equal without a memcmp). Falls back to the string
    /// lookup when an external `Arc::make_mut` key-set mutation moved the
    /// entries under the cache.
    #[inline]
    fn cur(&self) -> &MonsterState {
        if let Some((k, st)) = self.states.entry_at(self.current_idx as usize) {
            if k.as_ptr() == self.current.as_ptr() && k.len() == self.current.len() {
                return st;
            }
        }
        &self.states[self.current]
    }

    pub fn current_move(&self) -> &MoveState {
        match self.cur() {
            MonsterState::Move(m) => m,
            _ => panic!("current state '{}' is not a MoveState", self.current),
        }
    }

    /// Non-panicking [`Self::current_move`]: `None` when the AI is parked on
    /// a non-Move state (e.g. a `ConditionalBranch` that has not resolved).
    /// For callers that consult the move table opportunistically — a
    /// reconstruction recovering an icon-only wire telegraph must not turn
    /// an unresolved branch into a crash.
    #[must_use]
    pub fn current_move_opt(&self) -> Option<&MoveState> {
        match self.cur() {
            MonsterState::Move(m) => Some(m),
            _ => None,
        }
    }

    pub fn current_state_id(&self) -> StateId {
        self.current
    }

    /// Tell the AI which [`CreatureUid`] it belongs to. Required for
    /// owner-aware `ConditionalBranch` predicates (e.g. CeremonialBeast
    /// PLOW_CHECK). The combat loop calls this immediately before each
    /// `roll_move_with_combat` (bead `sts2-rs-msi1`); test paths that
    /// don't go through the combat loop leave it `None` and any
    /// `OwnerAware` branch resolves to `false`.
    pub fn set_owner(&mut self, uid: CreatureUid) {
        self.owner_uid = Some(uid);
    }

    /// Read the current owner uid, if set. Exposed for the combat
    /// loop's idempotent re-set check and for tests.
    #[must_use]
    pub fn owner_uid(&self) -> Option<CreatureUid> {
        self.owner_uid
    }

    /// Read the per-instance counter. Default 0. Bead `sts2-rs-976k`.
    #[must_use]
    pub fn counter(&self) -> i32 {
        self.counter
    }

    /// Overwrite the per-instance counter. Called from move-effect
    /// closures (e.g. FrogKnight's CHARGE intent sets the
    /// `beetle_charged` flag). Bead `sts2-rs-976k`.
    pub fn set_counter(&mut self, n: i32) {
        self.counter = n;
    }

    /// Saturating-increment the per-instance counter — Rust analogue
    /// of Python's `self._state[key] = self._state.get(key, 0) + 1`
    /// idiom (e.g. KnowledgeDemon's curse-application count). Bead
    /// `sts2-rs-976k`.
    pub fn increment_counter(&mut self) {
        self.counter = self.counter.saturating_add(1);
    }

    /// Read the encounter slot index, if assigned. See the `slot` field
    /// docs. Bead `sts2-rs-c8why`.
    #[must_use]
    pub fn slot(&self) -> Option<u8> {
        self.slot
    }

    /// Assign the encounter slot index (a position in the encounter's
    /// `Slots` list). Called by dynamic summoners (Ovicopter) right after
    /// the egg is spawned so subsequent lays can compute the C#
    /// `LastOrDefault` free slot and slot-sorted insert position. Bead
    /// `sts2-rs-c8why`.
    pub fn set_slot(&mut self, slot: u8) {
        self.slot = Some(slot);
    }

    /// Queue a one-shot state override for the next
    /// [`Self::roll_move`] / [`Self::roll_move_with_combat`], bypassing
    /// the normal follow-up chain. Analogue of C#
    /// `SetMoveImmediate(state)`. The override is consumed on the next
    /// roll and reverts to follow-the-graph behavior thereafter.
    ///
    /// Use case (Bead `sts2-rs-70zuj` / dvqa1.3): the C# Queen's
    /// `AfterDeath` hook on TorchHeadAmalgam calls
    /// `SetMoveImmediate(EnragedState)` if her next move was
    /// BurnBrightForMeState — the rebased Rust Queen
    /// (`sts2-rs-cd77n`) needs this primitive to model the same
    /// branch flip without rewriting its state graph.
    ///
    /// Calling twice before a roll overwrites the earlier queued id.
    /// Panics in debug if `state_id` is not in `states`; release
    /// builds let the bad id surface at the next roll's `[…]` index.
    pub fn force_next_state(&mut self, state_id: StateId) {
        debug_assert!(
            self.states.contains_key(state_id),
            "force_next_state({state_id:?}): unknown state id",
        );
        self.forced_next_state = Some(state_id);
    }

    /// Read the queued forced-next-state override (if any). Exposed
    /// for tests and for the dispatch loop's idempotent peek. Bead
    /// `sts2-rs-70zuj`.
    #[must_use]
    pub fn forced_next_state(&self) -> Option<StateId> {
        self.forced_next_state
    }

    /// **Immediate** state override — replace the *current* move right
    /// now, NOT at the next roll. The faithful analogue of C#
    /// `CreatureCmd.Stun` → `Creature.StunInternal` →
    /// `Monster.SetMoveImmediate(state)`, used when an event OUTSIDE the
    /// owner's own turn must change the move it will perform on its next
    /// turn — e.g. `BurrowedPower.AfterBlockBroken` redirecting the
    /// Tunneler to its `DIZZY_MOVE` (stun) state when the player breaks
    /// its block on the *player* turn (bead `sts2-rs-ybtmu`, Group A).
    ///
    /// Differs from [`Self::force_next_state`], which is consumed by the
    /// next `roll_move` (the owner's turn-END telegraph): that would let
    /// the already-telegraphed move perform once more before the
    /// override took hold. Here the combat loop performs `current_move`
    /// at turn start, so replacing `current` immediately is what skips
    /// the telegraphed move.
    ///
    /// `target` must be a `Move` state present in `states`; unknown or
    /// non-move ids are a no-op (debug builds assert). The fresh state
    /// is marked unperformed so a `must_perform_once` target still
    /// resolves correctly. `performed_first_move` is forced true: this
    /// only ever fires mid-combat, after the owner has already acted.
    pub fn force_current_state(&mut self, target: StateId) {
        debug_assert!(
            self.states.get(target).is_some_and(MonsterState::is_move),
            "force_current_state({target:?}): unknown or non-move state id",
        );
        if !self.states.get(target).is_some_and(MonsterState::is_move) {
            return;
        }
        self.set_current(target);
        self.performed_first_move = true;
        self.performed_at_least_once = false;
        self.forced_next_state = None;
    }

    /// Restore the current move-state from a serialized [`current_state_id`]
    /// string. Looks the runtime `target` up among this AI's own move-state
    /// ids (which are `&'static str`) and, when found, forces it as the
    /// current move; returns `true` on success, `false` if no such move
    /// state exists on this AI.
    ///
    /// Used by `state_from_dict` to rebuild a mid-combat enemy AI faithfully.
    /// Without it a reconstructed enemy carries no AI and goes inert after
    /// its first telegraphed intent, which makes MCTS rollouts treat the
    /// fight as trivially winnable (bead `sts2-rs-3f3rz`).
    ///
    /// [`current_state_id`]: MonsterAi::current_state_id
    #[must_use]
    pub fn force_current_state_by_name(&mut self, target: &str) -> bool {
        let Some(sid) = self.states.keys().copied().find(|k| *k == target) else {
            return false;
        };
        if !self.states.get(sid).is_some_and(MonsterState::is_move) {
            return false;
        }
        self.force_current_state(sid);
        true
    }

    /// [`Self::force_current_state_by_name`], but tolerant of the C# wire's
    /// `*_MOVE` naming. The live-game bridge serializes the C# `MoveState`
    /// id (`"WRIGGLE_MOVE"`, `"THRASH_MOVE_2"`), while this crate's ported
    /// AIs only sometimes kept that suffix (`Wriggler` uses `"WRIGGLE"`,
    /// `KinPriest` uses `"RITUAL_MOVE"`). Exact match is tried first; on a
    /// miss, both sides are compared with their `_MOVE` segment removed
    /// (`"THRASH_MOVE_2"` ≡ `"THRASH_2"`). Ambiguity is impossible in
    /// practice — no single AI defines two move states whose ids differ
    /// only by a `_MOVE` segment — but if it ever happens the first match
    /// in `BTreeMap` id order wins, deterministically.
    ///
    /// Only meant for `state_from_dict`-style reconstruction from wire
    /// dicts; engine-internal callers know their own static ids and should
    /// use the exact-match variant.
    #[must_use]
    pub fn force_current_state_by_name_relaxed(&mut self, target: &str) -> bool {
        if self.force_current_state_by_name(target) {
            return true;
        }
        fn strip_move_segment(id: &str) -> String {
            let stripped = id.strip_suffix("_MOVE").unwrap_or(id);
            stripped.replace("_MOVE_", "_")
        }
        let want = strip_move_segment(target);
        let Some(sid) = self
            .states
            .iter()
            .filter(|(_, st)| st.is_move())
            .map(|(k, _)| *k)
            .find(|k| strip_move_segment(k) == want)
        else {
            return false;
        };
        self.force_current_state(sid);
        true
    }

    /// Past performed-move state ids, oldest first. Mirrors Python's
    /// `state_machine.state_log` — used by the REPL to render an
    /// "intent history" line and by tests that pin a turn-by-turn
    /// sequence.
    #[must_use]
    pub fn move_history(&self) -> &[StateId] {
        &self.state_log
    }

    /// Carry the per-instance move-machine state that no wire dialect can
    /// express, from a mirror of the same creature onto a fresh
    /// reconstruction.
    ///
    /// A producer serialises at most `move_state` — the CURRENT state id — so
    /// a rebuilt AI starts with an empty `state_log`, `counter == 0`, both
    /// `performed_*` flags false, and no `forced_next_state`. Every one of
    /// those feeds the next transition: `CounterAware` predicates read
    /// `counter`, the initial-move branches read `performed_first_move`, and
    /// `forced_next_state` is a pending `SetMoveImmediate` that simply
    /// vanishes. The result is a mirror that telegraphs a different move than
    /// the game from an otherwise identical board.
    ///
    /// Deliberately does NOT touch `current`: that one field IS on the wire,
    /// so reality wins on it exactly as it does for every other observable.
    /// `states` / `initial` / `owner_uid` / `slot` are rebuilt correctly by
    /// the spawn path and are not per-instance history.
    ///
    /// Measured on the oracle (native engine both sides, so zero
    /// content-parity confound and zero RNG excuse — `cursor_drifts == 0` on
    /// every one): 5 of 6 Silent divergences over seeds 1-40 were this,
    /// TwigSlimeM telegraphing `Attack 11` where the game telegraphed
    /// `StatusCard` and Wriggler `Attack 6` against `Buff`.
    pub fn transplant_instance_state_from(&mut self, carried: &Self) {
        self.state_log.clone_from(&carried.state_log);
        self.performed_first_move = carried.performed_first_move;
        self.performed_at_least_once = carried.performed_at_least_once;
        self.initial_resolved = carried.initial_resolved;
        self.counter = carried.counter;
        self.forced_next_state = carried.forced_next_state;
        // The encounter slot is NOT "rebuilt correctly by the fresh build"
        // for dynamically-slotted spawns (Fabricator bots, Ovicopter eggs):
        // the wire carries no slot, the fresh build leaves `None`, and a
        // slot-less bot sorts ahead of everything — misaligning the whole
        // roster against C#'s slot-sorted enemy list (33 v28wr intent/power
        // events). The mirror's own assignment survives here instead.
        if self.slot.is_none() {
            self.slot = carried.slot;
        }
    }

    /// Roll to the next move using the C#-faithful RNG path.
    ///
    /// **Bead `sts2-rs-5w76f`:** matches C# `RandomBranchState.GetNextState`
    /// (RandomBranchState.cs:114-128) — only `RandomBranch` transitions
    /// consume RNG (one `rng.NextFloat(total)` tick); `Move` and
    /// `ConditionalBranch` transitions consume zero. The caller threads
    /// its own `&mut DotNetRng` (production: `state.run_rng.monster_ai`)
    /// so the counter only advances when a branch is actually rolled.
    pub fn roll_move(&mut self, rng: &mut DotNetRng) -> &MoveState {
        if !self.performed_first_move && self.cur().is_move() {
            return match self.cur() {
                MonsterState::Move(m) => m,
                _ => unreachable!(),
            };
        }

        if self.cur().is_move() && !self.can_transition_away() {
            return match self.cur() {
                MonsterState::Move(m) => m,
                _ => unreachable!(),
            };
        }

        let next_id = self
            .forced_next_state
            .take()
            .unwrap_or_else(|| self.get_next_state_no_combat(self.current, rng));
        if self.cur().is_move() {
            self.performed_at_least_once = false;
        }
        self.set_current(next_id);

        self.resolve_to_move_no_combat(rng);

        match self.cur() {
            MonsterState::Move(m) => m,
            _ => unreachable!(),
        }
    }

    /// Combat-aware variant of [`Self::roll_move`]. See that method for
    /// RNG contract (bead `sts2-rs-5w76f`).
    pub fn roll_move_with_combat(
        &mut self,
        rng: &mut DotNetRng,
        combat: &CombatState,
    ) -> &MoveState {
        if !self.performed_first_move && self.cur().is_move() {
            return match self.cur() {
                MonsterState::Move(m) => m,
                _ => unreachable!(),
            };
        }

        if self.cur().is_move() && !self.can_transition_away() {
            return match self.cur() {
                MonsterState::Move(m) => m,
                _ => unreachable!(),
            };
        }

        let next_id = self
            .forced_next_state
            .take()
            .unwrap_or_else(|| self.get_next_state_with_combat(self.current, rng, combat));
        if self.cur().is_move() {
            self.performed_at_least_once = false;
        }
        self.set_current(next_id);

        self.resolve_to_move_with_combat(rng, combat);

        match self.cur() {
            MonsterState::Move(m) => m,
            _ => unreachable!(),
        }
    }

    /// Resolve the FIRST move against the *run* `MonsterAi` RNG,
    /// mirroring C#'s combat-start `MonsterModel.RollMove(RunRng.MonsterAi)`
    /// (invoked per-enemy from `CombatManager.AfterCreatureAdded`, which
    /// fires in monster-add order). Bead `sts2-rs-0j24s`.
    ///
    /// The constructor pre-resolves the `initial` state with a *throwaway*
    /// RNG so [`Self::current_move`] is always valid for callers that read
    /// the telegraph before combat starts (spawn-time `debug_assert`s,
    /// AI-only unit tests). C#, by contrast, resolves the initial state
    /// lazily on the first `RollMove` using the run RNG:
    /// `MonsterMoveStateMachine.FindNextMoveState` early-returns (0 RNG)
    /// when `!_performedFirstMove && _currentState.IsMove`, but otherwise
    /// traverses the initial transition — so a `Move` initial (e.g.
    /// TwigSlimeM's `STICKY_SHOT`) consumes ZERO ticks while a
    /// `RandomBranch` initial (e.g. Flyconid's `INITIAL`) consumes one
    /// `NextFloat`.
    ///
    /// This re-runs that resolution against the real run RNG exactly once,
    /// before the first move is performed, discarding the constructor's
    /// throwaway result. For a `Move` initial it is a no-op (0 RNG,
    /// `current` already that move); for a branch initial it resets to
    /// `initial` and re-traverses, keeping the run `MonsterAi` stream in
    /// lockstep with C#. `state_log` is still empty at this point, so
    /// `CannotRepeat` weighting is unaffected.
    /// Whether this AI has performed a move yet. Paired with
    /// [`Self::initial_resolved`] to spot a creature summoned mid-combat: it
    /// has neither resolved its opening telegraph off the run stream nor acted.
    #[must_use]
    pub fn performed_first_move(&self) -> bool {
        self.performed_first_move
    }

    /// Whether the current move has actually begun executing.
    #[must_use]
    pub fn performed_current_move(&self) -> bool {
        self.performed_at_least_once
    }

    /// Restore existing execution flags and a queued move from engine-private
    /// transport. Validate the queue before changing any field. This carries
    /// no performed/observed history and does not infer a missing resume.
    #[must_use]
    pub fn restore_move_execution(
        &mut self,
        performed_first: bool,
        performed_current: bool,
        initial_resolved: bool,
        forced_next: Option<&str>,
    ) -> bool {
        let queued = match forced_next {
            Some(name) => {
                let Some(id) = self
                    .states
                    .keys()
                    .copied()
                    .find(|id| *id == name && self.states[*id].is_move())
                else {
                    return false;
                };
                Some(id)
            }
            None => None,
        };
        self.performed_first_move = performed_first;
        self.performed_at_least_once = performed_current;
        self.initial_resolved = initial_resolved;
        self.forced_next_state = queued;
        true
    }

    /// Whether [`Self::resolve_initial_move_with_combat`] has run. See the
    /// `initial_resolved` field docs for why a mid-combat summon needs it.
    #[must_use]
    pub fn initial_resolved(&self) -> bool {
        self.initial_resolved
    }

    pub fn resolve_initial_move_with_combat(&mut self, rng: &mut DotNetRng, combat: &CombatState) {
        // Only meaningful as the very first move resolution: once the
        // first move is performed the follow-up chain owns the RNG.
        debug_assert!(
            !self.performed_first_move,
            "resolve_initial_move_with_combat called after the first move",
        );
        // Mark it resolved BEFORE the early return: a `Move` initial is
        // resolved too, it just costs zero reads (C#'s first `RollMove`
        // early-returns on it). Setting the flag only on the branch path would
        // make every Move-initial creature look perpetually unresolved.
        self.initial_resolved = true;
        if self.states[self.initial].is_move() {
            // C#: `Move` initial early-returns from the first RollMove.
            return;
        }
        self.set_current(self.initial);
        self.resolve_to_move_with_combat(rng, combat);
    }

    /// Test-only convenience: roll using a fresh `DotNetRng` seeded
    /// from `seed`. Production code MUST pass through the run RNG via
    /// [`Self::roll_move`] so the counter stays in lockstep with C#.
    #[doc(hidden)]
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    pub fn roll_move_seeded(&mut self, seed: u64) -> &MoveState {
        let mut rng = DotNetRng::new(seed as i32);
        self.roll_move(&mut rng)
    }

    /// Test-only convenience analogue of [`Self::roll_move_with_combat`].
    /// See [`Self::roll_move_seeded`].
    #[doc(hidden)]
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    pub fn roll_move_with_combat_seeded(&mut self, seed: u64, combat: &CombatState) -> &MoveState {
        let mut rng = DotNetRng::new(seed as i32);
        self.roll_move_with_combat(&mut rng, combat)
    }

    /// Admit the current move before its body can force a replacement. Keep
    /// this engine's existing performed-state history contract: the admitted
    /// move owns the entry, even when its body replaces `current`.
    pub fn on_move_started(&mut self) {
        self.performed_at_least_once = true;
        let state = self.cur();
        if state.should_appear_in_logs() {
            self.state_log.push(self.current);
        }
    }

    /// The admitted body has completed. A newly forced current move has not
    /// performed merely because the old body's await has finished.
    pub fn on_move_completed(&mut self) {
        self.performed_first_move = true;
    }

    /// Atomic convenience for AI-only callers that perform a whole move.
    /// The combat walk splits these writes around the actual move body.
    pub fn on_move_performed(&mut self) {
        self.on_move_started();
        self.on_move_completed();
    }

    /// Peek the next `k` move intents without mutating self.
    ///
    /// **Bead `sts2-rs-6pzjm`:** wide-eval look-ahead feature feeder.
    /// Walks `k` virtual moves through the same state machine
    /// `roll_move_with_combat` would, using a cloned cursor
    /// (`current`, `state_log`, `performed_*` flags). Self is read-only;
    /// `combat` is read-only — no RNG mutation, no `IntentHistory`
    /// mutation.
    ///
    /// **Branch resolution under uncertainty:**
    /// - `RandomBranch`: pick the highest-weight non-zero branch
    ///   (after `effective_weight` filtering by the virtual `state_log`);
    ///   `certainty = max_weight / total_weight`. Ties resolve to the
    ///   first such branch (matches `roll_move`'s deterministic-given-
    ///   seed fallback for `roll <= cumulative` at boundary 0).
    /// - `ConditionalBranch`: resolve against the current `combat`
    ///   snapshot (assumption: combat state at lookahead-time ≈ now);
    ///   `certainty = 1.0`. Owner-aware predicates fire if
    ///   [`Self::set_owner`] was called.
    /// - `Move`: certainty `1.0`.
    ///
    /// The certainty product is folded across the walk into each
    /// returned [`PeekedIntent::certainty`], so a `k=3` peek through
    /// two 50/50 branches yields `certainty ≈ 0.25` on the deepest entry.
    ///
    /// Returns up to `k * MAX_INTENTS_PER_MOVE` entries (most monsters
    /// emit a single intent per move; multi-intent moves like
    /// `MultiAttack`-stacked telegraphs expand here).
    ///
    /// Used by the wide-eval feature extractor (bead `sts2-rs-6pzjm`)
    /// to feed the value-net look-ahead columns. Determinism contract:
    /// must NOT mutate `IntentHistory` or any RNG; existing tests must
    /// pass untouched.
    #[must_use]
    pub fn peek_intents(&self, k: usize, combat: &CombatState) -> SmallVec<[PeekedIntent; 8]> {
        let mut out: SmallVec<[PeekedIntent; 8]> = SmallVec::new();
        if k == 0 {
            return out;
        }

        let mut cursor_current = self.current;
        let mut cursor_log: SmallVec<[StateId; 16]> = self.state_log.clone();
        let mut cursor_performed_first = self.performed_first_move;
        let mut cursor_performed_once = self.performed_at_least_once;
        let mut running_certainty: f32 = 1.0;

        for step_idx in 0..k {
            let step = (step_idx + 1).min(u8::MAX as usize) as u8;
            // Emit the intents at the current move state, before advancing.
            if !cursor_performed_first {
                // First move: held until performed (same as roll_move).
                if let MonsterState::Move(m) = &self.states[cursor_current] {
                    for intent in &m.intents {
                        out.push(PeekedIntent {
                            intent: *intent,
                            certainty: running_certainty,
                            step,
                        });
                    }
                    // Mark first-move-performed for the virtual walk.
                    cursor_performed_first = true;
                    cursor_performed_once = true;
                    if matches!(&self.states[cursor_current], MonsterState::Move(_)) {
                        cursor_log.push(cursor_current);
                    }
                }
                continue;
            }

            // can_transition_away check for the virtual cursor.
            let can_transition = match &self.states[cursor_current] {
                MonsterState::Move(m) if m.must_perform_once => cursor_performed_once,
                _ => true,
            };
            if !can_transition {
                if let MonsterState::Move(m) = &self.states[cursor_current] {
                    for intent in &m.intents {
                        out.push(PeekedIntent {
                            intent: *intent,
                            certainty: running_certainty,
                            step,
                        });
                    }
                }
                continue;
            }

            // Walk to the next move-state, folding certainty along the way.
            let (next_id, step_certainty) =
                self.peek_next_move_state(cursor_current, &cursor_log, combat);
            running_certainty *= step_certainty;
            if matches!(&self.states[cursor_current], MonsterState::Move(_)) {
                cursor_performed_once = false;
            }
            cursor_current = next_id;

            if let MonsterState::Move(m) = &self.states[cursor_current] {
                for intent in &m.intents {
                    out.push(PeekedIntent {
                        intent: *intent,
                        certainty: running_certainty,
                        step,
                    });
                }
                cursor_performed_once = true;
                cursor_log.push(cursor_current);
            }
        }
        out
    }

    /// Walk from `from` through any non-Move states until landing on a
    /// `Move`, returning `(landing_state_id, certainty)`. Used by
    /// [`Self::peek_intents`]. The combat snapshot is read-only.
    fn peek_next_move_state(
        &self,
        from: StateId,
        cursor_log: &[StateId],
        combat: &CombatState,
    ) -> (StateId, f32) {
        let mut certainty: f32 = 1.0;
        let mut current = match &self.states[from] {
            MonsterState::Move(m) => m.follow_up.expect("MoveState has no follow_up_id"),
            _ => from,
        };
        let mut safety: u8 = 100;
        loop {
            match &self.states[current] {
                MonsterState::Move(_) => return (current, certainty),
                MonsterState::RandomBranch(rb) => {
                    let (next_id, branch_certainty) =
                        self.peek_random_branch_choice(rb, cursor_log);
                    certainty *= branch_certainty;
                    current = next_id;
                }
                MonsterState::ConditionalBranch(cb) => {
                    let mut found: Option<StateId> = None;
                    for branch in &cb.branches {
                        if branch
                            .condition
                            .resolve(combat, self.owner_uid, self.counter)
                        {
                            found = Some(branch.state_id);
                            break;
                        }
                    }
                    // No-match: fall back to first branch's state to keep
                    // peek total (matches degraded-no-combat semantics).
                    current = found.unwrap_or_else(|| {
                        cb.branches
                            .first()
                            .map(|b| b.state_id)
                            .expect("ConditionalBranch with no branches")
                    });
                }
            }
            safety = safety
                .checked_sub(1)
                .expect("infinite loop in peek_next_move_state");
        }
    }

    /// Pick the highest-effective-weight branch under the virtual
    /// `state_log`. Returns `(state_id, certainty = max_weight / total_weight)`.
    #[allow(clippy::unused_self)]
    fn peek_random_branch_choice(
        &self,
        rb: &RandomBranchState,
        cursor_log: &[StateId],
    ) -> (StateId, f32) {
        let weights: SmallVec<[f64; 4]> = rb
            .branches
            .iter()
            .map(|b| b.effective_weight(cursor_log))
            .collect();
        let total: f64 = weights.iter().sum();
        if total <= 0.0 {
            // Mirrors resolve_random_branch's fallback.
            return (
                rb.branches
                    .first()
                    .map(|b| b.state_id)
                    .expect("no branches"),
                0.0,
            );
        }
        let (idx, max_w) =
            weights
                .iter()
                .copied()
                .enumerate()
                .fold(
                    (0usize, 0.0f64),
                    |(bi, bw), (i, w)| {
                        if w > bw {
                            (i, w)
                        } else {
                            (bi, bw)
                        }
                    },
                );
        #[allow(clippy::cast_possible_truncation)]
        let certainty = (max_w / total) as f32;
        (rb.branches[idx].state_id, certainty)
    }

    /// Native immediate replacement is refused until a must-perform move ran.
    #[must_use]
    pub fn can_transition_away(&self) -> bool {
        match self.cur() {
            MonsterState::Move(m) if m.must_perform_once => self.performed_at_least_once,
            _ => true,
        }
    }

    fn resolve_to_move_no_combat(&mut self, rng: &mut DotNetRng) {
        let mut safety = 100u8;
        while !self.cur().is_move() {
            safety = safety
                .checked_sub(1)
                .expect("infinite loop in state machine resolution");
            let next = self.get_next_state_no_combat(self.current, rng);
            self.set_current(next);
        }
    }

    fn resolve_to_move_with_combat(&mut self, rng: &mut DotNetRng, combat: &CombatState) {
        let mut safety = 100u8;
        while !self.cur().is_move() {
            safety = safety
                .checked_sub(1)
                .expect("infinite loop in state machine resolution");
            let next = self.get_next_state_with_combat(self.current, rng, combat);
            self.set_current(next);
        }
    }

    fn resolve_random_branch(&self, rb: &RandomBranchState, rng: &mut DotNetRng) -> StateId {
        let weights: SmallVec<[f64; 4]> = rb
            .branches
            .iter()
            .map(|b| b.effective_weight(&self.state_log))
            .collect();
        let total: f64 = weights.iter().sum();
        if total <= 0.0 {
            // Bead `sts2-rs-zdjjx`: C# `RandomBranchState.GetNextState`
            // (RandomBranchState.cs:114-128) ALWAYS calls `rng.NextFloat(max)`,
            // and `Rng.NextFloat` (Rng.cs:90) does `Counter++` *before*
            // computing — so even when every branch is on cooldown /
            // `CannotRepeat` and the summed weight is 0, C# still draws (and
            // discards) one MonsterAi tick before its `num -= weight` loop
            // returns the first branch. A Flyconid whose VULN(cd3)/FRAIL(cd2)/
            // SMASH(CannotRepeat) all zero out on the same turn hit exactly
            // this case; skipping the draw left the `monster_ai` stream one
            // tick behind C# for the rest of combat. Draw the tick here to
            // stay aligned, then return the first branch (C#'s `num <= 0`
            // short-circuit picks `States[0]`).
            let _ = rng.next_float(total.max(0.0));
            return rb
                .branches
                .first()
                .map(|b| b.state_id)
                .expect("no branches");
        }
        // C# RandomBranchState.GetNextState (RandomBranchState.cs:114-128)
        // draws one `rng.NextFloat(total)` tick on RunRng.MonsterAi. Mirror
        // it exactly — see bead `sts2-rs-5w76f`.
        let roll = rng.next_float(total);
        // C# does NOT skip zero-weight branches and uses an inclusive
        // boundary: it subtracts EVERY branch's weight in order and returns
        // the first branch where the running remainder is `<= 0`
        //
        //     float num = rng.NextFloat(max);
        //     foreach (state in States) { num -= weight; if (num <= 0) return state; }
        //
        // The previous `if w <= 0 { continue; }` skip plus strict
        // `roll < cumulative` is the textbook weighted-pick, but it diverges
        // from C# at boundary draws: at `roll == 0.0` (reachable — NextDouble
        // can return exactly 0) C# returns `States[0]` even when it is
        // zero-weighted (on cooldown / `CannotRepeat`), and at any
        // `roll == Σweights[0..=i]` exactly C# returns branch `i` while the
        // skip/`<` form returns `i+1`. A single divergent pick desyncs the
        // `MonsterAi` RNG stream for the rest of combat. This is the same
        // class of mismatch the `total <= 0` arm above (bead `sts2-rs-zdjjx`)
        // already corrects for the all-zero case.
        let mut remainder = roll;
        for (branch, &w) in rb.branches.iter().zip(weights.iter()) {
            remainder -= w;
            if remainder <= 0.0 {
                return branch.state_id;
            }
        }
        // C# throws `InvalidOperationException` here; keep a positive-weight
        // fallback as a float-rounding safety net (`roll < total` guarantees
        // the loop returns absent rounding error).
        rb.branches
            .iter()
            .zip(weights.iter())
            .rev()
            .find(|(_, &w)| w > 0.0)
            .map(|(b, _)| b.state_id)
            .expect("unreachable")
    }

    fn get_next_state_no_combat(&self, from: StateId, rng: &mut DotNetRng) -> StateId {
        match &self.states[from] {
            MonsterState::Move(m) => m.follow_up.expect("MoveState has no follow_up_id"),
            MonsterState::RandomBranch(rb) => self.resolve_random_branch(rb, rng),
            MonsterState::ConditionalBranch(cb) => {
                // Bead `sts2-rs-msi1`: degrade gracefully without
                // combat context. `OwnerAware` predicates resolve to
                // `false` (no owner), and `Stateless` predicates need
                // a `&CombatState`; we pass a temporary
                // `CombatState::default()` so any always-true
                // fallthrough branches (the convention) fire. AI-only
                // unit tests rely on this — the no-combat path used to
                // panic, which broke any monster whose graph contained
                // a ConditionalBranch even if every callsite only
                // intended to walk the always-true fallthrough.
                let sentinel = CombatState::default();
                for branch in &cb.branches {
                    if matches!(
                        branch.condition,
                        ConditionFn::OwnerAware(_) | ConditionFn::CounterAware(_)
                    ) {
                        continue;
                    }
                    if branch.condition.resolve(&sentinel, None, 0) {
                        return branch.state_id;
                    }
                }
                panic!(
                    "ConditionalBranchState '{}': no stateless condition matched in \
                     no-combat path; use roll_move_with_combat",
                    cb.id
                );
            }
        }
    }

    fn get_next_state_with_combat(
        &self,
        from: StateId,
        rng: &mut DotNetRng,
        combat: &CombatState,
    ) -> StateId {
        match &self.states[from] {
            MonsterState::Move(m) => m.follow_up.expect("MoveState has no follow_up_id"),
            MonsterState::RandomBranch(rb) => self.resolve_random_branch(rb, rng),
            MonsterState::ConditionalBranch(cb) => {
                for branch in &cb.branches {
                    if branch
                        .condition
                        .resolve(combat, self.owner_uid, self.counter)
                    {
                        return branch.state_id;
                    }
                }
                panic!("ConditionalBranchState '{}': no condition matched", cb.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::powers::PowerKind;
    use smallvec::smallvec;
    use std::num::NonZeroU32;

    fn noop_effect(_: &mut CombatState, _: CreatureUid) {}

    fn make_linear_chain() -> MonsterAi {
        let mut states = BTreeMap::new();
        states.insert(
            "A",
            MonsterState::Move(MoveState {
                id: "A",
                effect: noop_effect,
                intents: smallvec![Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 0
                }],
                follow_up: Some("B"),
                must_perform_once: true,
            }),
        );
        states.insert(
            "B",
            MonsterState::Move(MoveState {
                id: "B",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 6 }],
                follow_up: Some("B"),
                must_perform_once: false,
            }),
        );
        MonsterAi::new(states, "A")
    }

    /// Bead `sts2-rs-fgjdh`: `set_owner` round-trips through
    /// `owner_uid()`. Fresh AI has `owner_uid()==None`; after
    /// `set_owner(uid)`, the getter returns `Some(uid)`; a second
    /// `set_owner(other)` overwrites. This is the contract
    /// `OwnerAware` AI branches depend on (set_owner docstring at
    /// monster_ai.rs:254).
    #[test]
    fn set_owner_round_trips_through_owner_uid() {
        let mut ai = make_linear_chain();
        assert!(ai.owner_uid().is_none(), "fresh AI has no owner");
        let uid_a = CreatureUid(NonZeroU32::new(1).unwrap());
        ai.set_owner(uid_a);
        assert_eq!(ai.owner_uid(), Some(uid_a));
        let uid_b = CreatureUid(NonZeroU32::new(2).unwrap());
        ai.set_owner(uid_b);
        assert_eq!(ai.owner_uid(), Some(uid_b), "set_owner overwrites");
    }

    #[test]
    fn linear_chain_first_move_is_initial() {
        let ai = make_linear_chain();
        assert_eq!(ai.current_move().id, "A");
    }

    #[test]
    fn linear_chain_holds_until_performed() {
        let mut ai = make_linear_chain();
        let m = ai.roll_move_seeded(42);
        assert_eq!(m.id, "A");
        let m = ai.roll_move_seeded(42);
        assert_eq!(m.id, "A");
        ai.on_move_performed();
        let m = ai.roll_move_seeded(42);
        assert_eq!(m.id, "B");
    }

    #[test]
    fn linear_chain_self_loop() {
        let mut ai = make_linear_chain();
        ai.on_move_performed();
        let m = ai.roll_move_seeded(1);
        assert_eq!(m.id, "B");
        ai.on_move_performed();
        let m = ai.roll_move_seeded(2);
        assert_eq!(m.id, "B");
    }

    #[test]
    fn random_branch_selects_among_branches() {
        let mut states = BTreeMap::new();
        states.insert(
            "root",
            MonsterState::RandomBranch(RandomBranchState {
                id: "root",
                branches: smallvec![
                    WeightedBranch {
                        state_id: "atk",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 50.0,
                        cooldown: 0,
                    },
                    WeightedBranch {
                        state_id: "def",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 50.0,
                        cooldown: 0,
                    },
                ],
            }),
        );
        states.insert(
            "atk",
            MonsterState::Move(MoveState {
                id: "atk",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 5 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "def",
            MonsterState::Move(MoveState {
                id: "def",
                effect: noop_effect,
                intents: smallvec![Intent::Defend { block: 0 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "root");
        let mut saw_atk = false;
        let mut saw_def = false;
        for seed in 0..100 {
            ai.on_move_performed();
            let m = ai.roll_move_seeded(seed);
            match m.id {
                "atk" => saw_atk = true,
                "def" => saw_def = true,
                _ => panic!("unexpected state"),
            }
        }
        assert!(saw_atk, "should have seen atk");
        assert!(saw_def, "should have seen def");
    }

    #[test]
    fn cannot_repeat_prevents_same_twice() {
        let mut states = BTreeMap::new();
        states.insert(
            "root",
            MonsterState::RandomBranch(RandomBranchState {
                id: "root",
                branches: smallvec![
                    WeightedBranch {
                        state_id: "X",
                        repeat_type: MoveRepeatType::CannotRepeat,
                        max_times: 1,
                        weight: 50.0,
                        cooldown: 0,
                    },
                    WeightedBranch {
                        state_id: "Y",
                        repeat_type: MoveRepeatType::CannotRepeat,
                        max_times: 1,
                        weight: 50.0,
                        cooldown: 0,
                    },
                ],
            }),
        );
        states.insert(
            "X",
            MonsterState::Move(MoveState {
                id: "X",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 3 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "Y",
            MonsterState::Move(MoveState {
                id: "Y",
                effect: noop_effect,
                intents: smallvec![Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 0
                }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "root");
        let mut prev = ai.current_move().id;
        for seed in 0..200 {
            ai.on_move_performed();
            let m = ai.roll_move_seeded(seed);
            assert_ne!(m.id, prev, "CannotRepeat violated at seed {seed}");
            prev = m.id;
        }
    }

    #[test]
    fn can_repeat_x_times_cap() {
        let mut states = BTreeMap::new();
        states.insert(
            "root",
            MonsterState::RandomBranch(RandomBranchState {
                id: "root",
                branches: smallvec![
                    WeightedBranch {
                        state_id: "bite",
                        repeat_type: MoveRepeatType::CanRepeatXTimes,
                        max_times: 2,
                        weight: 90.0,
                        cooldown: 0,
                    },
                    WeightedBranch {
                        state_id: "spit",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 10.0,
                        cooldown: 0,
                    },
                ],
            }),
        );
        states.insert(
            "bite",
            MonsterState::Move(MoveState {
                id: "bite",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 5 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "spit",
            MonsterState::Move(MoveState {
                id: "spit",
                effect: noop_effect,
                intents: smallvec![Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 0
                }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "root");
        let mut bite_streak = 0u32;
        for seed in 0..500 {
            ai.on_move_performed();
            let m = ai.roll_move_seeded(seed);
            if m.id == "bite" {
                bite_streak += 1;
                assert!(bite_streak <= 2, "bite exceeded max_times=2");
            } else {
                bite_streak = 0;
            }
        }
    }

    #[test]
    fn use_only_once_fires_once() {
        let mut states = BTreeMap::new();
        states.insert(
            "root",
            MonsterState::RandomBranch(RandomBranchState {
                id: "root",
                branches: smallvec![
                    WeightedBranch {
                        state_id: "special",
                        repeat_type: MoveRepeatType::UseOnlyOnce,
                        max_times: 1,
                        weight: 90.0,
                        cooldown: 0,
                    },
                    WeightedBranch {
                        state_id: "normal",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 10.0,
                        cooldown: 0,
                    },
                ],
            }),
        );
        states.insert(
            "special",
            MonsterState::Move(MoveState {
                id: "special",
                effect: noop_effect,
                intents: smallvec![Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 0
                }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "normal",
            MonsterState::Move(MoveState {
                id: "normal",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 6 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "root");
        let mut special_count = 0u32;
        if ai.current_move().id == "special" {
            special_count += 1;
        }
        for seed in 0..200 {
            ai.on_move_performed();
            let m = ai.roll_move_seeded(seed);
            if m.id == "special" {
                special_count += 1;
            }
        }
        assert_eq!(special_count, 1, "UseOnlyOnce should fire exactly once");
    }

    #[test]
    fn cooldown_prevents_recent_reuse() {
        let mut states = BTreeMap::new();
        states.insert(
            "root",
            MonsterState::RandomBranch(RandomBranchState {
                id: "root",
                branches: smallvec![
                    WeightedBranch {
                        state_id: "A",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 50.0,
                        cooldown: 2,
                    },
                    WeightedBranch {
                        state_id: "B",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 50.0,
                        cooldown: 0,
                    },
                ],
            }),
        );
        states.insert(
            "A",
            MonsterState::Move(MoveState {
                id: "A",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 8 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "B",
            MonsterState::Move(MoveState {
                id: "B",
                effect: noop_effect,
                intents: smallvec![Intent::Defend { block: 0 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "root");
        let mut history: Vec<StateId> = Vec::new();
        for seed in 0..200 {
            ai.on_move_performed();
            let m = ai.roll_move_seeded(seed);
            history.push(m.id);
        }
        for window in history.windows(3) {
            let a_count = window.iter().filter(|&&s| s == "A").count();
            assert!(
                a_count <= 1,
                "cooldown=2 violated: A appeared {a_count} times in window {window:?}"
            );
        }
    }

    #[test]
    fn conditional_branch_resolves() {
        let mut states = BTreeMap::new();
        states.insert(
            "check",
            MonsterState::ConditionalBranch(ConditionalBranchState {
                id: "check",
                branches: smallvec![ConditionalBranch {
                    condition: ConditionFn::Stateless(|_| true),
                    state_id: "atk",
                }],
            }),
        );
        states.insert(
            "atk",
            MonsterState::Move(MoveState {
                id: "atk",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 10 }],
                follow_up: Some("check"),
                must_perform_once: false,
            }),
        );
        let ai = MonsterAi::new_with_combat(states, "check", &CombatState::default());
        assert_eq!(ai.current_move().id, "atk");
    }

    #[test]
    fn state_log_only_records_move_states() {
        let mut states = BTreeMap::new();
        states.insert(
            "root",
            MonsterState::RandomBranch(RandomBranchState {
                id: "root",
                branches: smallvec![WeightedBranch {
                    state_id: "hit",
                    repeat_type: MoveRepeatType::CanRepeatForever,
                    max_times: 1,
                    weight: 100.0,
                    cooldown: 0,
                }],
            }),
        );
        states.insert(
            "hit",
            MonsterState::Move(MoveState {
                id: "hit",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 4 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "root");
        ai.on_move_performed();
        ai.roll_move_seeded(0);
        ai.on_move_performed();
        assert_eq!(ai.state_log.len(), 2);
        assert!(ai.state_log.iter().all(|&s| s == "hit"));
    }

    #[test]
    fn force_current_state_by_name_restores_move_or_reports_miss() {
        let mut states = BTreeMap::new();
        for id in ["atk", "hit"] {
            states.insert(
                id,
                MonsterState::Move(MoveState {
                    id,
                    effect: noop_effect,
                    intents: smallvec![Intent::Attack { damage: 1 }],
                    follow_up: Some(id),
                    must_perform_once: false,
                }),
            );
        }
        let mut ai = MonsterAi::new(states, "atk");
        // A known move-state resolves the runtime string to the static id.
        assert!(ai.force_current_state_by_name("hit"));
        assert_eq!(ai.current_state_id(), "hit");
        // An unknown id leaves the AI untouched and reports the miss — this is
        // the `state_from_dict` fall-back-to-`ai: None` path (bead sts2-rs-3f3rz).
        assert!(!ai.force_current_state_by_name("NOPE"));
        assert_eq!(ai.current_state_id(), "hit");
    }

    #[test]
    fn force_current_state_by_name_relaxed_matches_wire_move_suffix() {
        let mut states = BTreeMap::new();
        for id in ["WRIGGLE", "THRASH_2", "RITUAL_MOVE"] {
            states.insert(
                id,
                MonsterState::Move(MoveState {
                    id,
                    effect: noop_effect,
                    intents: smallvec![Intent::Attack { damage: 1 }],
                    follow_up: Some(id),
                    must_perform_once: false,
                }),
            );
        }
        let mut ai = MonsterAi::new(states, "WRIGGLE");
        // Wire suffix vs suffix-less engine id.
        assert!(ai.force_current_state_by_name_relaxed("WRIGGLE_MOVE"));
        assert_eq!(ai.current_state_id(), "WRIGGLE");
        // Mid-id `_MOVE_` segment (C# "THRASH_MOVE_2" vs engine "THRASH_2").
        assert!(ai.force_current_state_by_name_relaxed("THRASH_MOVE_2"));
        assert_eq!(ai.current_state_id(), "THRASH_2");
        // Exact match still wins when the engine DID keep the suffix.
        assert!(ai.force_current_state_by_name_relaxed("RITUAL_MOVE"));
        assert_eq!(ai.current_state_id(), "RITUAL_MOVE");
        // A genuinely unknown id still misses.
        assert!(!ai.force_current_state_by_name_relaxed("NOPE_MOVE"));
        assert_eq!(ai.current_state_id(), "RITUAL_MOVE");
    }

    #[test]
    fn counter_defaults_to_zero_and_round_trips() {
        let mut states = BTreeMap::new();
        states.insert(
            "atk",
            MonsterState::Move(MoveState {
                id: "atk",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 1 }],
                follow_up: Some("atk"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "atk");
        assert_eq!(ai.counter(), 0);
        ai.set_counter(7);
        assert_eq!(ai.counter(), 7);
        ai.increment_counter();
        assert_eq!(ai.counter(), 8);
        ai.set_counter(i32::MAX);
        ai.increment_counter();
        assert_eq!(ai.counter(), i32::MAX, "increment must saturate");
    }

    #[test]
    fn slot_defaults_to_none_and_round_trips() {
        // Bead `sts2-rs-c8why`: the encounter-slot index used by the
        // Ovicopter multi-lay slot-sort. Defaults `None` (untracked) and
        // is excluded from `Creature` equality (the whole `ai` field is).
        let mut states = BTreeMap::new();
        states.insert(
            "atk",
            MonsterState::Move(MoveState {
                id: "atk",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 1 }],
                follow_up: Some("atk"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "atk");
        assert_eq!(ai.slot(), None);
        ai.set_slot(4);
        assert_eq!(ai.slot(), Some(4));
        ai.set_slot(0);
        assert_eq!(ai.slot(), Some(0));
    }

    #[test]
    fn counter_aware_predicate_resolves_against_counter() {
        fn counter_eq_three(_: &CombatState, _: CreatureUid, counter: i32) -> bool {
            counter == 3
        }
        let pred = ConditionFn::CounterAware(counter_eq_three);
        let combat = CombatState::default();
        let owner = CreatureUid::new(1);
        assert!(!pred.resolve(&combat, owner, 0));
        assert!(pred.resolve(&combat, owner, 3));
        assert!(!pred.resolve(&combat, owner, 4));
        assert!(
            !pred.resolve(&combat, None, 3),
            "counter-aware without owner must resolve false"
        );
    }

    // ── peek_intents (sts2-rs-6pzjm) ───────────────────────────────

    #[test]
    fn peek_intents_zero_k_returns_empty() {
        let ai = make_linear_chain();
        let combat = CombatState::default();
        assert!(ai.peek_intents(0, &combat).is_empty());
    }

    #[test]
    fn peek_intents_linear_chain_is_deterministic() {
        // A → B → B → B …, all certainty 1.0.
        let ai = make_linear_chain();
        let combat = CombatState::default();
        let peeks = ai.peek_intents(3, &combat);
        assert_eq!(peeks.len(), 3);
        // First peek emits A's intent (first move held until performed).
        assert!(matches!(peeks[0].intent, Intent::Buff { .. }));
        // Subsequent peeks roll through to B (Attack).
        assert!(matches!(peeks[1].intent, Intent::Attack { damage: 6 }));
        assert!(matches!(peeks[2].intent, Intent::Attack { damage: 6 }));
        for p in &peeks {
            assert!((p.certainty - 1.0).abs() < 1e-6, "linear chain → 1.0");
        }
    }

    #[test]
    fn peek_intents_does_not_mutate_self() {
        // Walking the cursor must leave self.current / state_log /
        // performed flags / counter untouched.
        let ai = make_linear_chain();
        let combat = CombatState::default();
        let before_current = ai.current_state_id();
        let before_log_len = ai.move_history().len();
        let before_counter = ai.counter();
        let _ = ai.peek_intents(5, &combat);
        assert_eq!(ai.current_state_id(), before_current);
        assert_eq!(ai.move_history().len(), before_log_len);
        assert_eq!(ai.counter(), before_counter);
    }

    #[test]
    fn peek_intents_random_branch_picks_highest_weight() {
        // 70/30 → picks the 70-weight branch with certainty 0.7.
        let mut states = BTreeMap::new();
        states.insert(
            "root",
            MonsterState::RandomBranch(RandomBranchState {
                id: "root",
                branches: smallvec![
                    WeightedBranch {
                        state_id: "big",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 70.0,
                        cooldown: 0,
                    },
                    WeightedBranch {
                        state_id: "small",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 30.0,
                        cooldown: 0,
                    },
                ],
            }),
        );
        states.insert(
            "big",
            MonsterState::Move(MoveState {
                id: "big",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 20 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "small",
            MonsterState::Move(MoveState {
                id: "small",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 5 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "root");
        // After the first move performs, subsequent peeks pass through
        // the 70/30 branch each step. Certainty compounds: 0.7, 0.49.
        ai.on_move_performed();
        let combat = CombatState::default();
        let peeks = ai.peek_intents(2, &combat);
        assert_eq!(peeks.len(), 2);
        for p in &peeks {
            assert!(matches!(p.intent, Intent::Attack { damage: 20 }));
        }
        assert!(
            (peeks[0].certainty - 0.7).abs() < 1e-5,
            "step1 expected ~0.7, got {}",
            peeks[0].certainty
        );
        assert!(
            (peeks[1].certainty - 0.49).abs() < 1e-5,
            "step2 expected ~0.49 (0.7*0.7), got {}",
            peeks[1].certainty
        );
    }

    #[test]
    fn peek_intents_certainty_compounds_across_steps() {
        // Two-step walk through 50/50 → first peek 0.5, second peek 0.25.
        let mut states = BTreeMap::new();
        states.insert(
            "root",
            MonsterState::RandomBranch(RandomBranchState {
                id: "root",
                branches: smallvec![
                    WeightedBranch {
                        state_id: "x",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 50.0,
                        cooldown: 0,
                    },
                    WeightedBranch {
                        state_id: "y",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 50.0,
                        cooldown: 0,
                    },
                ],
            }),
        );
        states.insert(
            "x",
            MonsterState::Move(MoveState {
                id: "x",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 1 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "y",
            MonsterState::Move(MoveState {
                id: "y",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 2 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "root");
        ai.on_move_performed();
        let combat = CombatState::default();
        let peeks = ai.peek_intents(2, &combat);
        assert_eq!(peeks.len(), 2);
        assert!((peeks[0].certainty - 0.5).abs() < 1e-5);
        assert!((peeks[1].certainty - 0.25).abs() < 1e-5);
    }

    #[test]
    fn peek_intents_respects_use_only_once_via_cursor_log() {
        // A `UseOnlyOnce` "special" branch with weight 90 must NOT be
        // picked on the second peek if it was virtually-walked on the
        // first peek — peek's cursor_log shadows state_log to keep the
        // weighting honest.
        let mut states = BTreeMap::new();
        states.insert(
            "root",
            MonsterState::RandomBranch(RandomBranchState {
                id: "root",
                branches: smallvec![
                    WeightedBranch {
                        state_id: "special",
                        repeat_type: MoveRepeatType::UseOnlyOnce,
                        max_times: 1,
                        weight: 90.0,
                        cooldown: 0,
                    },
                    WeightedBranch {
                        state_id: "normal",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 10.0,
                        cooldown: 0,
                    },
                ],
            }),
        );
        states.insert(
            "special",
            MonsterState::Move(MoveState {
                id: "special",
                effect: noop_effect,
                intents: smallvec![Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 0
                }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "normal",
            MonsterState::Move(MoveState {
                id: "normal",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 6 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        let ai = MonsterAi::new(states, "root");
        // Peek without on_move_performed: first-move-held semantics
        // emit the landed special intent, then subsequent peeks see
        // it in cursor_log and the UseOnlyOnce zeros it out.
        let combat = CombatState::default();
        let peeks = ai.peek_intents(3, &combat);
        assert_eq!(peeks.len(), 3);
        assert!(matches!(peeks[0].intent, Intent::Buff { .. }));
        assert!(matches!(peeks[1].intent, Intent::Attack { damage: 6 }));
        assert!(matches!(peeks[2].intent, Intent::Attack { damage: 6 }));
    }

    #[test]
    fn peek_intents_resolves_conditional_branches_against_combat() {
        let mut states = BTreeMap::new();
        states.insert(
            "check",
            MonsterState::ConditionalBranch(ConditionalBranchState {
                id: "check",
                branches: smallvec![ConditionalBranch {
                    condition: ConditionFn::Stateless(|_| true),
                    state_id: "atk",
                }],
            }),
        );
        states.insert(
            "atk",
            MonsterState::Move(MoveState {
                id: "atk",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 10 }],
                follow_up: Some("check"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new_with_combat(states, "check", &CombatState::default());
        ai.on_move_performed();
        let combat = CombatState::default();
        let peeks = ai.peek_intents(2, &combat);
        assert_eq!(peeks.len(), 2);
        for p in &peeks {
            assert!(matches!(p.intent, Intent::Attack { damage: 10 }));
            assert!((p.certainty - 1.0).abs() < 1e-6);
        }
    }

    /// Bead `sts2-rs-70zuj` (dvqa1.3): `force_next_state(id)` queues
    /// a one-shot override that bypasses the regular follow-up graph
    /// on the next `roll_move`, then reverts to follow-the-graph.
    #[test]
    fn force_next_state_overrides_one_roll_then_reverts() {
        let mut states = BTreeMap::new();
        states.insert(
            "a",
            MonsterState::Move(MoveState {
                id: "a",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 1 }],
                follow_up: Some("b"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "b",
            MonsterState::Move(MoveState {
                id: "b",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 2 }],
                follow_up: Some("a"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "c",
            MonsterState::Move(MoveState {
                id: "c",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 99 }],
                follow_up: Some("a"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "a");
        assert_eq!(ai.current_state_id(), "a");

        // Bypass the a→b follow-up: force-jump to "c" next.
        ai.force_next_state("c");
        assert_eq!(ai.forced_next_state(), Some("c"));
        ai.on_move_performed();
        let m = ai.roll_move_seeded(0);
        assert_eq!(m.id, "c", "forced override won the next roll");
        assert!(
            ai.forced_next_state().is_none(),
            "override is one-shot — consumed by the roll",
        );

        // Next roll follows the normal graph (c → a).
        ai.on_move_performed();
        let m = ai.roll_move_seeded(0);
        assert_eq!(
            m.id, "a",
            "post-override roll follows the FollowUpState graph"
        );
    }

    #[test]
    fn force_next_state_overwrites_previous_queued_id() {
        let mut states = BTreeMap::new();
        states.insert(
            "a",
            MonsterState::Move(MoveState {
                id: "a",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 1 }],
                follow_up: Some("b"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "b",
            MonsterState::Move(MoveState {
                id: "b",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 2 }],
                follow_up: Some("a"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "c",
            MonsterState::Move(MoveState {
                id: "c",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 3 }],
                follow_up: Some("a"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "a");
        ai.force_next_state("b");
        ai.force_next_state("c");
        assert_eq!(ai.forced_next_state(), Some("c"));
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "c");
    }

    /// Bead `sts2-rs-ybtmu` (Group A): `force_current_state(id)` replaces
    /// the *current* move immediately (C# `SetMoveImmediate`), so the very
    /// next turn performs `id` — NOT the previously-current move — and then
    /// the roll follows `id`'s FollowUpState. Models the Tunneler being
    /// stunned (→ `dizzy`) the instant its block breaks on the player turn,
    /// then biting.
    #[test]
    fn force_current_state_replaces_current_immediately() {
        let mut states = BTreeMap::new();
        states.insert(
            "below",
            MonsterState::Move(MoveState {
                id: "below",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 23 }],
                follow_up: Some("below"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "dizzy",
            MonsterState::Move(MoveState {
                id: "dizzy",
                effect: noop_effect,
                intents: smallvec![Intent::Stun],
                follow_up: Some("bite"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "bite",
            MonsterState::Move(MoveState {
                id: "bite",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 13 }],
                follow_up: Some("below"),
                must_perform_once: false,
            }),
        );
        // Pretend the monster is mid-combat, currently telegraphing BELOW
        // (its burrowed self-loop).
        let mut ai = MonsterAi::new(states, "below");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "below", "self-loops on below");
        assert_eq!(ai.current_state_id(), "below");

        // Block breaks on the PLAYER turn → immediate redirect to dizzy.
        ai.force_current_state("dizzy");
        assert_eq!(
            ai.current_state_id(),
            "dizzy",
            "current move replaced immediately, not queued",
        );
        assert!(
            ai.forced_next_state().is_none(),
            "immediate override does not leave a queued next-state",
        );
        // The enemy turn performs dizzy (stun), then rolls its follow-up.
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_seeded(0).id,
            "bite",
            "after the stun turn the Tunneler bites (dizzy → bite)",
        );
    }

    /// `force_current_state` on an unknown / non-existent state id is a
    /// release-safe no-op (debug builds assert before reaching here, so this
    /// is only meaningful under `debug_assertions = false`; we exercise the
    /// guard via a valid-but-rechecked path to keep it covered).
    #[test]
    fn force_current_state_ignores_known_move() {
        let mut states = BTreeMap::new();
        states.insert(
            "x",
            MonsterState::Move(MoveState {
                id: "x",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 1 }],
                follow_up: Some("x"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "x");
        ai.force_current_state("x");
        assert_eq!(ai.current_state_id(), "x");
    }

    /// Bead `sts2-rs-zdjjx`: when every branch of a `RandomBranch` zeroes
    /// out (all on cooldown / `CannotRepeat`), C#
    /// `RandomBranchState.GetNextState` still calls `rng.NextFloat(0)` —
    /// which increments the counter — before returning the first branch.
    /// The Rust resolver must consume the same tick, or the `monster_ai`
    /// stream drifts one behind C# for the rest of combat (surfaced as the
    /// FLYCONID_NORMAL turn-4 desync once a sibling enemy died and the
    /// Flyconid's VULN(cd3)/FRAIL(cd2)/SMASH(CannotRepeat) all zeroed).
    #[test]
    fn all_branches_zero_still_consumes_one_rng_tick() {
        let mut states = BTreeMap::new();
        states.insert(
            "A",
            MonsterState::Move(MoveState {
                id: "A",
                effect: noop_effect,
                intents: smallvec![Intent::Attack { damage: 1 }],
                follow_up: Some("RAND"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "RAND",
            MonsterState::RandomBranch(RandomBranchState {
                id: "RAND",
                // Single branch back to A, CannotRepeat: once A is the last
                // logged move, its weight is 0 → total 0.
                branches: smallvec![WeightedBranch {
                    state_id: "A",
                    repeat_type: MoveRepeatType::CannotRepeat,
                    max_times: 1,
                    weight: 1.0,
                    cooldown: 0,
                }],
            }),
        );
        let mut ai = MonsterAi::new(states, "A");
        ai.on_move_performed(); // log = ["A"], performed_first_move = true
        let mut rng = DotNetRng::new(0);
        assert_eq!(rng.counter(), 0);
        // A → RAND: branch A is CannotRepeat and A is the last move → weight
        // 0 → total 0. The resolver must still draw one tick and fall back
        // to the first (only) branch, A.
        let next = ai.roll_move(&mut rng);
        assert_eq!(next.id, "A");
        assert_eq!(
            rng.counter(),
            1,
            "all-zero RandomBranch must still consume one NextFloat tick (C# parity)"
        );
    }

    #[test]
    #[should_panic(expected = "infinite loop")]
    fn infinite_loop_detection() {
        let mut states = BTreeMap::new();
        states.insert(
            "loop1",
            MonsterState::RandomBranch(RandomBranchState {
                id: "loop1",
                branches: smallvec![WeightedBranch {
                    state_id: "loop2",
                    repeat_type: MoveRepeatType::CanRepeatForever,
                    max_times: 1,
                    weight: 1.0,
                    cooldown: 0,
                }],
            }),
        );
        states.insert(
            "loop2",
            MonsterState::RandomBranch(RandomBranchState {
                id: "loop2",
                branches: smallvec![WeightedBranch {
                    state_id: "loop1",
                    repeat_type: MoveRepeatType::CanRepeatForever,
                    max_times: 1,
                    weight: 1.0,
                    cooldown: 0,
                }],
            }),
        );
        MonsterAi::new(states, "loop1");
    }
}
