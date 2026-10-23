//! Phase 3 turn-state-machine entry points. Mirrors
//! `sts2-rl-agent@01c094d:sts2_env/core/combat.py` lifecycle functions.
//!
//! P3-1 (this file initial; bead sts2-rs-ypr): `start_combat` —
//! resets combat scalars, fires `before_combat_start` once, rolls the
//! initial intent for each enemy, advances phase `Setup` -> `PlayerTurnStart`.
//!
//! P3-2 (bead sts2-rs-9l9): `start_player_turn` — clears per-turn
//! event lists, fires `before_side_turn_start(PLAYER)`, clears block
//! (round > 1) + fires `after_block_cleared`, resets / accumulates
//! energy via `should_reset_energy` + `modify_max_energy`, draws the
//! base hand (mediated by `modify_hand_draw` + opening-innate sort
//! on round 1), fires `after_energy_reset`, fires
//! `after_side_turn_start(PLAYER)`, then advances
//! `PlayerTurnStart` -> `PlayerPlayPhase`.
//!
//! Subsequent P3-3..P3-8 beads extend this module with the end-turn
//! pipeline, enemy turn execution, and `check_combat_end`.

use crate::cards::{self, PileKind, TargetType, VarKey};
use crate::hooks::{
    self, CardCtx, CombatLifecycleCtx, CreatureRef, DamageCtx, HandDrawCtx, HpLostCtx,
    MaxEnergyCtx, TurnSideCtx, ValueProps,
};
use crate::monsters::Intent;
use crate::relic_effects::channels::{
    dispatch_if_live, fold_if_live, relic_channel_live, relic_guarded_channel_live, GuardedChannel,
    PlannedChannel,
};
use crate::{CombatPhase, CombatState, CreatureUid, DotNetRng, Side, StepResult};

/// Append a play to the combat-scoped UI play-log when recording is enabled.
/// No-op (and no allocation) when the flag is off, so headless / parity /
/// MCTS-rollout paths are unaffected. See [`crate::PlayLogEntry`].
pub(crate) fn record_play_into(
    te: &mut crate::TurnEvents,
    card_id: crate::cards::CardId,
    upgraded: bool,
    energy_spent: i32,
    auto_played: bool,
    target: Option<u8>,
) {
    if !te.record_card_play_log {
        return;
    }
    te.card_play_log.push(crate::PlayLogEntry {
        card_id,
        upgraded,
        energy_spent,
        auto_played,
        target,
    });
}

/// Collect the player's live `VoidForm` power instance uids.
///
/// GH #363 A2. VoidForm's gate reads a tally the engine had been conflating
/// with [`CombatState::count_player_cards_played_this_turn`] — the analogue of
/// C# `History.CardPlaysFinished`, which counts auto-plays and every replay
/// iteration. C# `VoidFormPower` keeps its OWN `Data.cardsPlayedThisTurn`
/// (VoidFormPower.cs:14-17) bumped only on `!IsAutoPlay && IsLastInSeries`
/// (`:66-73`), so a Mayhem auto-play or a Bombardment self-auto-play must not
/// consume a free card. The engine holds that tally in the instance's
/// `Power::counter`, which `state_to_dict` already emits verbatim.
fn player_power_uids_of_kind(
    state: &CombatState,
    kind: crate::powers::PowerKind,
) -> smallvec::SmallVec<[crate::PowerUid; 2]> {
    if !crate::hooks::power_kind_live(state, kind) {
        return smallvec::SmallVec::new();
    }
    state
        .player_power_uids()
        .into_iter()
        .filter(|uid| state.powers.iter().any(|p| p.id == uid.0 && p.kind == kind))
        .collect()
}

fn void_form_uids(state: &CombatState) -> smallvec::SmallVec<[crate::PowerUid; 2]> {
    player_power_uids_of_kind(state, crate::powers::PowerKind::VoidForm)
}

/// Charge one first-in-series card play against every live player EchoForm
/// instance, so the tally rides `Power::counter` and therefore survives a
/// wire round trip.
///
/// `PlayerCombatState::cards_first_in_series_this_turn` does NOT cross
/// `state_to_dict` / `state_from_dict` — both PyO3 constructors default it to
/// 0. A mirror rebuilt from a wire read 0 for the rest of the turn, so
/// EchoForm's `num >= Amount` gate never closed and it echoed EVERY card
/// played after a reconcile: seed 1003's Cold Snap and Strike each landed
/// twice for 6 against TheObscura on the DLL arm (GH #368.1, found with
/// `damage_trace`). Same defect shape as MakeItSo (`skills_played_this_turn`,
/// bead `sts2-rs-j9x8`) and VoidForm (GH #363) — this is the third instance.
///
/// The world producer does not emit a `counter` for EchoForm, so
/// `combat_mirror::_undisclosed_power_counters` lists it and the transplant
/// carries the mirror's accumulation forward. That is what makes the counter
/// durable where the plain field was not.
fn bump_echo_form_series_tally(state: &mut CombatState) {
    for uid in player_power_uids_of_kind(state, crate::powers::PowerKind::EchoForm) {
        if let Some(p) = state.powers.get_mut_by_id(uid.0) {
            p.counter = p.counter.saturating_add(1);
        }
    }
}

/// Zero every live player EchoForm instance's per-turn tally, alongside the
/// plain-field reset at side-turn start.
fn reset_echo_form_series_tally(state: &mut CombatState) {
    for uid in player_power_uids_of_kind(state, crate::powers::PowerKind::EchoForm) {
        if let Some(p) = state.powers.get_mut_by_id(uid.0) {
            p.counter = 0;
        }
    }
}

/// First-in-series card plays this turn, as EchoForm's gate should see them —
/// C# `History.CardPlaysStarted … IsFirstInSeries && HappenedThisTurn`.
///
/// The MAX of the plain field and the per-instance counters, because each is
/// authoritative in a regime the other is not: the field is exact inside a
/// single decision (including for an EchoForm applied mid-turn, which starts
/// its counter at 0), and the counters are the only half that survives a
/// reconstruction. Taking the max means neither regime can under-count, and
/// under-counting is the direction that echoes a card it should not.
///
/// The per-power counter alone is NOT sufficient, and the measured reason is
/// worth keeping: it can only count plays made while an EchoForm instance
/// already existed, and Echo Form is itself a power card — so its own
/// first-in-series play, which C#'s history does count, is invisible to it.
/// The plain field records that play, and `PyCombatState::transplant_hidden`
/// carries the field across a rebuild for exactly this reason. The two halves
/// close different seams: the carry is exact on the normal reconcile path, the
/// counters survive a full re-seed where no transplant runs.
#[must_use]
pub fn echo_form_first_in_series_this_turn(state: &CombatState) -> u32 {
    let carried = player_power_uids_of_kind(state, crate::powers::PowerKind::EchoForm)
        .into_iter()
        .filter_map(|uid| state.powers.get_by_id(uid.0).map(|p| p.counter))
        .max()
        .unwrap_or(0);
    state
        .player
        .cards_first_in_series_this_turn
        .max(u32::try_from(carried.max(0)).unwrap_or(0))
}

/// Charge one card-play series against every live player VoidForm instance.
/// See [`void_form_uids`]; called once per hand play, past the replay loops.
fn bump_void_form_play_tally(state: &mut CombatState) {
    for uid in void_form_uids(state) {
        if let Some(p) = state.powers.get_mut_by_id(uid.0) {
            // `void_form_uids` already filtered to the player's own instances,
            // so the ownership predicate is satisfied by construction.
            p.counter = crate::powers::regent::void_form_increment_on_play(p.counter, true);
        }
    }
}

/// Zero every live player VoidForm instance's per-turn tally. C#
/// `VoidFormPower.BeforeSideTurnStart` (VoidFormPower.cs:75-83).
fn reset_void_form_play_tally(state: &mut CombatState) {
    for uid in void_form_uids(state) {
        if let Some(p) = state.powers.get_mut_by_id(uid.0) {
            p.counter = crate::powers::regent::void_form_reset_on_turn_start(p.counter, true);
        }
    }
}

/// The number of cards VoidForm considers already played this turn — the
/// engine's stand-in for C# `GetInternalData<Data>().cardsPlayedThisTurn`, read
/// by both cost gates ([`crate::hooks`]'s `modify_card_cost` VoidForm arm and
/// [`crate::effective_star_cost_in_combat`]). Multiple instances would each
/// carry their own tally in C#; they are bumped in lockstep here, so the max is
/// the shared value and an empty set reads 0.
#[must_use]
pub fn void_form_cards_played_this_turn(state: &CombatState) -> i32 {
    void_form_uids(state)
        .into_iter()
        .filter_map(|uid| state.powers.get_by_id(uid.0).map(|p| p.counter))
        .max()
        .unwrap_or(0)
}

/// Vanilla starter draw count. Mirrors Python's `BASE_DRAW = 5` at
/// `sts2-rl-agent@01c094d:sts2_env/core/constants.py:5`.
pub const BASE_DRAW: i32 = 5;

/// Vanilla starter max energy. Mirrors Python's `BASE_ENERGY = 3` at
/// `sts2-rl-agent@01c094d:sts2_env/core/constants.py:6`, and C#
/// `Player.MaxEnergy` (the stored BASE). The per-turn energy reset reads
/// the effective max as `BASE_MAX_ENERGY + dispatch_modify_max_energy(...)`
/// fresh each turn — see `start_player_turn` Step 6.
pub const BASE_MAX_ENERGY: i32 = 3;

// Hot-path presence gates (perf): `state.powers.present_kinds()` is a
// documented SUPERSET of every live power's kind, so a `contains` miss
// provably means "no live power of that kind" and the guarded walk can be
// skipped bit-identically (the walk is a pure read that would find nothing).
// Single-kind gates use `PowerKindSet::contains` — a one-word test — rather
// than an `intersects` against a one-kind mask; see the `PowerKindSet` docs
// in lib.rs.

/// Initialize combat scalars + fire `before_combat_start` + roll initial
/// enemy intents + advance phase from `Setup` to `PlayerTurnStart`.
///
/// Mirrors Python's `start_combat` at
/// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:632-662`.
///
/// **Pipeline order** (locked by the Python source-of-truth):
///   1. Combat-end short-circuit. If `state.is_over` is already set,
///      return the matching [`StepResult`] immediately — no scalar
///      reset, no hook fire, no intent roll. P3-8 (sts2-rs-qct) lands
///      the full `check_combat_end` helper that this short-circuit
///      consumes; for now, `is_over` is the only condition.
///   2. Reset combat scalars to start-of-combat defaults
///      (`round=1`, `side=Player`, `energy=0`, `stars=0`,
///      `player.stars=0`).
///   3. Clear per-combat / per-turn event state. Python clears six
///      separate event lists at combat.py:644-650
///      (`_pending_retain_count`, `_damage_events_this_turn`,
///      `_damage_events_combat`, `_draw_events_this_turn`,
///      `_stars_gained_this_turn`, `_generated_cards_combat`,
///      `_played_cards_this_turn`, `_played_cards_combat`,
///      `extra_card_rewards`). At this pin those structures are
///      collapsed into a single placeholder POD `combat_log:
///      TurnEvents`; resetting via `Default::default()` is the
///      structural equivalent. Phase 6 splits these out into per-list
///      fields and this fn updates accordingly.
///   4. Fire `before_combat_start` on every listener (powers-then-relics,
///      iter order matching Python's `combat.all_creatures` walk).
///   5. For each enemy: roll the initial intent via
///      `MonsterKind::roll_intent(rng, creature.intent_history)`.
///      Mirrors Python's `for enemy in self.alive_enemies: ai = ...;
///      ai.roll_move(self.rng)` at combat.py:660. For a freshly-
///      constructed `Creature`, `intent_history` is the
///      `IntentHistory::default()` produced by `Creature`'s field
///      defaulting (no prior moves) — equivalent to a freshly-
///      constructed `MonsterAI`. Bead sts2-rs-o7r (P3-stretch) reads
///      the persisted slot rather than a local `default()` so any
///      pre-seeded history is honored.
///   6. Advance phase `Setup` -> `PlayerTurnStart` and return
///      [`StepResult::Advanced`]. The caller drives
///      [`start_player_turn`] next — the engine is caller-stepped,
///      not tail-called.
///
/// **Out of scope for P3-1:**
///   * Player turn-start hooks (P3-2 territory).
///   * Card draw / shuffle (lifted from existing `cards` primitives by P3-2).
///   * Lagavulin sleeping exemption / `Sleep` intent (Phase 5 monster DSL).
///   * Per-combat-state reset like `combat.combat_player_states[*].creature.combat_state = self`
///     (combat.py:652-654) — at this Rust pin we don't track
///     creature-back-references; the relationship is stored in the
///     `enemy_uids` `SmallVec` only.
pub fn start_combat(state: &mut CombatState) -> StepResult {
    // Step 1: combat-end short-circuit. If a previous step already
    // resolved combat (e.g. all enemies dead before start_combat is
    // called — defensive only; the canonical caller invokes us with
    // is_over=false), return the matching StepResult immediately.
    if state.is_over {
        return if state.player_won {
            StepResult::CombatWon
        } else {
            StepResult::CombatLost
        };
    }

    // Step 2: reset combat scalars to start-of-combat defaults.
    // Python's CombatState.__init__ sets round_number=1; the
    // VTHIN-1 fixture overrides round=0 because that fixture skips
    // start_combat entirely. The start_combat path itself sets
    // round=1 unconditionally.
    state.round = 1;
    state.side = Side::Player;
    state.player.energy = 0;
    state.player.stars = 0;
    // bead sts2-rs-ob7h: reset per-combat energy-wasted counter so
    // [`evaluate_in_progress`]'s lightspeed energy-penalty term reads
    // a clean baseline for each new combat.
    state.energy_wasted = 0;
    // bead sts2-rs-o05r: sister reset for the cards-drawn counter.
    state.cards_drawn = 0;

    // Bead `sts2-rs-wlbg`: initial draw-pile shuffle. Mirrors Python's
    // `self.rng.shuffle(state.draw)` inside `_reset_player_combat_state`
    // at `sts2-rl-agent@01c094d:sts2_env/core/combat.py:615`. Without
    // this call every fixture-replay served its deck in TOML-list order
    // (same opening hand across all seeds). `crate::cards::shuffle`
    // respects `pinned_top`/`pinned_bottom` — irrelevant at this point
    // (those collections start empty) but keeps the canonical entry
    // point. Determinism is preserved: identical seeds yield identical
    // draw orderings.
    crate::cards::shuffle(&mut state.player, &mut state.run_rng.shuffle);

    // Bead `sts2-rs-d03` Phase 1: ensure the player has a unified
    // [`Creature`] record in `state.creatures` mirroring the canonical
    // pin's `Combat.SetupPlayerCreature`. Idempotent — if a prior
    // `start_combat` already populated `creature_uid`, this is a
    // no-op. Phase 2 will make `state.player.{current_hp, max_hp,
    // block, powers}` read through this uid; Phase 1 keeps the legacy
    // fields as the source of truth and uses the Creature only to
    // satisfy the new C#-pin call surface.
    if state.player.creature_uid.is_none() {
        let _ = state.init_player_creature();
    }

    // Step 3: clear per-combat / per-turn event state. At this pin
    // `combat_log: TurnEvents` is a placeholder POD; resetting to
    // default is a structural no-op. Phase 6 splits the eight Python
    // event lists into separate fields and this assignment becomes
    // multi-line.
    state.combat_log = crate::TurnEvents::default();
    // The reset above also clears the event log's opt-in flag, so re-assert it
    // for the ONE caller that cannot re-arm per combat: `STS2_COMBAT_LOG=1`.
    // Without this the env switch would print the first combat of a process
    // and nothing after it. In-process callers (workbench / PyO3) re-arm each
    // combat themselves, exactly as they already do for `card_play_log`.
    if crate::event_log::forced_by_env() {
        state.combat_log.event_log.set_recording(true);
    }
    // Bead `sts2-rs-8h2q`: clear the per-combat generated-cards tally.
    // Mirrors Python's `_generated_cards_combat = []` at
    // `sts2-rl-agent@01c094d:sts2_env/core/combat.py:647`. Consumed by
    // `Supermassive` (`cards/regent.py:578`) via
    // `CombatState::count_player_generated_cards_this_combat`.
    state.player.generated_cards_this_combat = 0;
    // Bead `sts2-rs-masterplanner-sly`: clear the combat-long Sly mark
    // (MasterPlanner). C# `_keywords` lives on the per-combat card model, so a
    // new combat starts with no MasterPlanner-applied Sly keywords.
    state.player.sly_this_combat.clear();

    // gh#176: materialize the Defect orb belt at its base capacity
    // (`Defect.BaseOrbSlotCount => 3`) at combat start, mirroring C#'s
    // belt setup. The port previously left the belt at capacity 0 and only
    // grew it to 3 lazily on the FIRST channel (`orbs::channel_orb`), so a
    // pre-channel capacity mutation was lost: BulkUp's
    // `OrbCmd.RemoveSlots(1)` (guarded `if capacity > 0`) no-op'd when no orb
    // had been channelled yet, leaving Rust at 3 slots while C# dropped to 2
    // — a belt-capacity desync that cascaded into the `ORB_STATE|*` orb-card
    // residuals (ICE_LANCE/GLACIER/GLASSWORK/NULL). Gating on Defect keeps
    // non-Defect channels (e.g. an Ironclad channeling a generated card's
    // Plasma) on the temporary 1-slot path the `channel_orb` lazy init still
    // provides (bead `sts2-rs-dnxjt`).
    if state.player.character == Some(crate::potions::CharacterId::Defect) {
        state.player.orbs = crate::orbs::OrbSlots::with_default_capacity();
    }

    // Step 4: fire before_combat_start on every listener (powers,
    // then relics — matching Python's `combat.all_creatures` walk
    // ordered primary_player → allies → enemies for powers, plus
    // primary_player relics). Phase 2 has no active
    // before_combat_start listeners; the dispatcher walks all
    // listeners but every per-listener fn is a no-op default.
    hooks::dispatch_before_combat_start(state, &CombatLifecycleCtx::default());
    // VitalSpark (Infested Prism) `BeforeCombatStart`: taint every player Skill
    // card (bead `…wb8lk.4`). Dedicated pass because the generic
    // `before_combat_start` channel is read-only (`&CombatState`) and VitalSpark
    // must MUTATE cards. Fired here, after the read-only dispatch, so the deck is
    // shuffled-and-present (Step 2 above) and any other before-combat listeners
    // have run.
    crate::monster_power_effects::vital_spark_before_combat_start(state);
    // Bead `sts2-rs-j87`: parallel relic-effects bus drain. The
    // `hooks::*` registry above carries the legacy power+relic
    // walk; the `relic_effects::*` channel carries the per-relic
    // listener bodies registered by `sts2-content`'s P7 pool beads.
    // Both must fire so future retrofits (bead `sts2-rs-01o`) light up.
    let _ = crate::relic_effects::dispatch_before_combat_start(state);

    // Step 5: roll initial intent for each enemy. We collect the
    // enemy UIDs into a fixed SmallVec first to release the immutable
    // borrow on `state.enemy_uids` before mutating `state.creatures`
    // and `state.rng` inside the loop. Iteration order matches
    // `enemy_uids` (which Python's `_iter_power_listeners` and
    // `combat.all_creatures` also respect).
    let enemy_uids: smallvec::SmallVec<[crate::CreatureUid; 8]> =
        state.enemy_uids.iter().copied().collect();
    for uid in enemy_uids {
        // Look up creature by UID. enemy_uids is canonically a
        // strict subset of state.creatures' ids; if the lookup
        // fails it indicates a caller-side construction bug.
        let creature_idx = state
            .creatures
            .iter()
            .position(|c| c.id == uid.0)
            .expect("enemy_uid must resolve to a Creature in state.creatures");
        // Bead sts2-rs-8ao (BR-VI): when a `MonsterAi` is attached
        // (DSL-driven enemies like Nibbit), the per-turn intent is
        // determined by `ai.current_move().intents.first()` — the
        // hand-rolled `kind.roll_intent` arm is a placeholder that
        // would yield the wrong value for any monster whose intent
        // varies across moves. Prefer the AI's first-move intent
        // whenever it exists; otherwise fall through to the legacy
        // `MonsterKind` rolls (Cultist / JawWorm / AcidSlimeS /
        // GreenLouse / Sentry — none of which carry an `ai` today).
        let intent = if state.creatures[creature_idx].ai.is_some() {
            // Bead sts2-rs-0j24s: resolve the FIRST move against the run
            // `MonsterAi` RNG, mirroring C#'s per-enemy combat-start
            // `RollMove(RunRng.MonsterAi)` (CombatManager.AfterCreatureAdded,
            // in monster-add order == `enemy_uids` order). The constructor
            // pre-resolved the `initial` state with a throwaway RNG so
            // `current_move()` is valid for spawn-time reads, but C#
            // resolves it lazily off the run stream: a `Move` initial
            // (TwigSlimeM STICKY_SHOT) draws 0 ticks, a `RandomBranch`
            // initial (Flyconid INITIAL) draws one `NextFloat`. Skipping
            // this drew the wrong first move AND left the run counter
            // off-by-one for every later monster roll. Same split-borrow
            // dance as the turn-end re-telegraph (sts2-rs-5w76f).
            let owner_uid = CreatureUid(state.creatures[creature_idx].id);
            let mut ai = state.creatures[creature_idx]
                .ai
                .take()
                .expect("just checked is_some");
            ai.set_owner(owner_uid);
            let mut local_rng =
                core::mem::replace(&mut state.run_rng.monster_ai, DotNetRng::placeholder());
            ai.resolve_initial_move_with_combat(&mut local_rng, state);
            state.run_rng.monster_ai = local_rng;
            let intent = ai.current_move().intents.first().copied();
            state.creatures[creature_idx].ai = Some(ai);
            intent
        } else {
            let kind = state.creatures[creature_idx]
                .species
                .expect("enemy creature must carry a species (bead sts2-rs-a19m)");
            // Bead sts2-rs-o7r (P3-stretch): read the persisted
            // `intent_history` slot rather than constructing a fresh
            // default. For the canonical `start_combat` entry this
            // is observably the same (the slot is
            // `IntentHistory::default()` because no prior turn ran),
            // but it preserves any pre-seeded history a recorder /
            // fixture might inject.
            let history = state.creatures[creature_idx].intent_history;
            Some(kind.roll_intent(&mut state.run_rng.monster_ai, history))
        };
        state.creatures[creature_idx].intent = intent;

        // Bead `sts2-rs-ztx5u`: clear enemy block before the player's
        // first actionable turn. C# grants spawn-time block in
        // `AfterAddedToRoom` (only `CubexConstruct.cs:84`
        // `GainBlock(13, ValueProp.Move)` at this pin) via
        // [`CombatState::on_spawn_block`], but the combat-start enemy
        // turn-setup zeroes it, so the player never sees it — verified
        // against the v0.103.2 headless harness, which reports
        // CUBEX_CONSTRUCT `block=0` at turn 0. Enemies always act after
        // the player, so they carry no block into turn 1. Mid-combat
        // summons (e.g. Vantom CONSUME) bypass `start_combat` and keep
        // their `AfterAddedToRoom` block until their own next turn-start.
        state.creatures[creature_idx].block = 0;
    }

    // Step 6: advance phase to PlayerTurnStart. The engine is
    // caller-stepped — [`start_player_turn`] runs on the next
    // call, driven by the StepResult::Advanced returned below.
    state.phase = CombatPhase::PlayerTurnStart;

    StepResult::Advanced
}

/// Run the player turn-start phase. Mirrors Python's
/// `_start_player_turn` + `_continue_player_turn_setup` at
/// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:666-721`.
///
/// **Pipeline order** (locked by the Python source-of-truth, line
/// citations in parentheses):
///   1. Combat-end short-circuit. Same defensive guard as
///      [`start_combat`] — if `state.is_over` is already set, return
///      the matching [`StepResult`] immediately, no hook fire / no
///      mutation.
///   2. Clear per-turn event lists (combat.py:674-678). At this pin
///      `combat_log: TurnEvents` is a placeholder POD; resetting via
///      `Default::default()` is the structural equivalent of Python's
///      four `_*_this_turn` list clears (`_pending_retain_count`,
///      `_damage_events_this_turn`, `_draw_events_this_turn`,
///      `_stars_gained_this_turn`, `_played_cards_this_turn`).
///      Phase 6 splits these out into per-list fields and this
///      assignment becomes multi-line.
///   3. Fire `before_side_turn_start(PLAYER, combat)` (combat.py:680).
///      Powers-then-relics walk via
///      [`hooks::dispatch_before_side_turn_start`].
///   4. Pending-choice deferral path (combat.py:681-683). If a
///      `before_side_turn_start` listener (or any prior code) left
///      `state.pending_choice = Some(_)`, we set
///      `pending_turn_setup_resume = true` and return
///      [`StepResult::PendingChoice`]; the rest of turn-setup
///      defers until `apply(Action::ResolveChoice ...)` clears the
///      choice and re-invokes `continue_player_turn_setup` (bead
///      sts2-rs-l2nz).
///   5. Per `player_state` (primary player only at Phase 3 — allies
///      omitted): clear block when `round > 1` (combat.py:698-699),
///      then fire `after_block_cleared(owner, combat)`
///      (combat.py:700) via [`hooks::dispatch_after_block_cleared`].
///   6. Energy reset (combat.py:702-708). Computes the effective max
///      fresh each turn as
///      `BASE_MAX_ENERGY + dispatch_modify_max_energy(...)` (mirroring
///      C# `PlayerCombatState.MaxEnergy => ModifyMaxEnergy(base)`). If
///      [`hooks::dispatch_should_reset_energy`] returns `true`,
///      `state.player.energy = max`; else
///      `state.player.energy += max` (saturating). The base is read
///      from the immutable [`BASE_MAX_ENERGY`], never from the stored
///      effective `state.player.max_energy` (which would compound a
///      persistent `ModifyMaxEnergy` power +Amount/turn — sts2-rs-gpsvo).
///   7. Hand draw (combat.py:710-712). Computes
///      `draw_count = BASE_DRAW + dispatch_modify_hand_draw(...)`.
///      On round 1, `prepare_opening_draw_for_owner` sorts innate
///      cards to the front of the draw pile and adjusts `draw_count`
///      to be at least the innate-card count, capped at the hand
///      size limit. Then [`crate::cards::draw`] pops cards into the
///      hand.
///   8. Fire `after_energy_reset(combat)` (combat.py:718).
///   9. Fire `after_side_turn_start(PLAYER, combat)`
///      (combat.py:720). Python's dispatcher additionally fires
///      `on_turn_start_own_side` for every power on a creature whose
///      `side == PLAYER` whose listener overrides
///      `on_turn_start_own_side` but NOT `after_side_turn_start`.
///      No power or relic has an `on_turn_start_own_side` arm (C# has
///      no such hook), so Rust no longer walks it.
///  10. Combat-end check (combat.py:721). P3-8 (sts2-rs-qct) lands
///      the full `check_combat_end` helper; until then the cheap
///      `is_over` guard is sufficient.
///  11. Advance phase `PlayerTurnStart` -> `PlayerPlayPhase`.
///
/// **Out of scope for P3-2:**
///   * Ally turn-start (Phase 3 is single-primary-player only;
///     `state.allies` is unused at this pin).
///   * `apply_enchantment_after_player_turn_start` (Phase 7).
///   * Orb queue `trigger_after_turn_start` (Phase 7).
///   * `Action::EndTurn` dispatch (P3-7, sts2-rs-e3k) — that bead
///     wires the call site that follows the play phase.
pub fn start_player_turn(state: &mut CombatState) -> StepResult {
    // Step 1: combat-end short-circuit.
    if state.is_over {
        return if state.player_won {
            StepResult::CombatWon
        } else {
            StepResult::CombatLost
        };
    }

    debug_assert_eq!(state.phase, CombatPhase::PlayerTurnStart);

    // Bead `sts2-rs-8ukqd` (parity-fuzz R2): re-assert the active side to
    // Player at the top of the player's turn, mirroring `start_enemy_turn`
    // which sets `state.side = Side::Enemy` (see below). `state.side` is
    // set to Player once in `start_combat` and flipped to Enemy each enemy
    // turn, but was never restored here — so from round 2 on it stayed
    // `Enemy` for the whole player turn. That silently disabled every
    // turn-start / `after_block_cleared` relic+power whose handler proxies
    // "creature is owner" via `state.side == Side::Player` (HornCleat,
    // SparklingRouge, CaptainsWheel, SturdyClamp). Confirmed by the C#
    // parity-fuzz: e.g. HornCleat's round-2 +14 block (`HornCleat.cs:20-27`)
    // never fired in Rust. Set here so it holds across the fall-through to
    // `continue_player_turn_setup` and the resume-after-choice path.
    state.side = Side::Player;

    // Step 2: clear per-turn event lists. Bead `sts2-rs-xsvp9`:
    // use `reset_per_turn` so the combat-wide
    // `unblocked_damage_received_this_combat` counter (consumed by
    // TearAsunder's `CalculatedHits`) survives the turn boundary.
    state.combat_log.reset_per_turn();
    // Bead `sts2-rs-zlx.3`: reset the per-turn Shiv counter for
    // PhantomBlades. Mirrors the Python `_shiv_played_this_turn = False`
    // reset in `PhantomBladesPower.before_side_turn_start`
    // (`card_play_effects.py:411-413`).
    state.player.shivs_played_this_turn = 0;
    // Bead `sts2-rs-r6l9`: reset the per-turn cards-played bucket
    // counter (Attack / Skill / Power / Curse / Status). Mirrors
    // Python's `_played_cards_this_turn = []` at the top of
    // `_start_player_turn` (`sts2-rl-agent@01c094d:sts2_env/core/
    // combat.py:678`). Read by
    // `CombatState::count_player_cards_played_this_turn` (the analogue
    // of Python's `count_cards_played_this_turn(owner, card_type=...)`).
    state.player.cards_played_this_turn = [0; 5];
    state.player.card_plays_started_this_turn = 0;
    // GH #363 A2: C# `VoidFormPower.BeforeSideTurnStart` (VoidFormPower.cs:75-83)
    // zeroes its own `Data.cardsPlayedThisTurn` when the owner is among the
    // participants. That tally is NOT the same quantity as the bucket array
    // above — see [`reset_void_form_play_tally`].
    reset_void_form_play_tally(state);
    // Round boundary for the HistoryCourse slot: everything stamped during
    // the round that just ended becomes "last round's last Attack/Skill";
    // the new round (incl. its turn-start Mayhem auto-plays) accumulates
    // into the live slot. Mirrors C#'s `RoundNumber == round - 1` scan key.
    state.combat_log.prev_round_last_attack_skill_played =
        state.combat_log.last_attack_skill_played.take();
    // Bead `sts2-rs-stomp`: undo Stomp's per-turn cost discount (C#
    // `EnergyCost.AddThisTurn` resets at the owner's turn start). Restores
    // each Stomp's cost by the amount shaved off last turn.
    state.restore_stomp_costs_for_turn_start();
    // Undo Pinpoint's per-turn Skill-cost discount (same C#
    // `EnergyCost.AddThisTurn` reset shape as Stomp above).
    state.restore_pinpoint_costs_for_turn_start();
    // Bead `sts2-rs-2e9q2` follow-up: undo Snecko Oil's per-turn cost
    // randomization (C# `EnergyCost.SetThisTurnOrUntilPlayed` reverts at the
    // owner's turn start). Restores each randomized card to its base cost.
    state.restore_snecko_costs_for_turn_start();
    // Undo the this-turn discounts a FOREIGN producer had already applied
    // when this state was reconstructed. The three restores above only know
    // about discounts this engine itself banked; a state rebuilt mid-turn
    // from a C# wire inherits the shaved cost with no bank behind it, so
    // without this the discount never expires.
    state.restore_wire_transient_costs_for_turn_start();
    // Bead `sts2-rs-x646`: reset the per-turn stars-gained tally.
    // Mirrors Python's `_stars_gained_this_turn = []` at the top of
    // `_start_player_turn` (`sts2-rl-agent@01c094d:sts2_env/core/
    // combat.py:677`). Read by
    // `CombatState::count_player_stars_gained_this_turn` (the Rust
    // analogue of `combat.count_stars_gained_this_turn(owner)` —
    // `combat.py:1550-1551`); consumed by `Radiate`
    // (hits = stars gained this turn — `cards/regent.py:L500-504`).
    state.player.stars_gained_this_turn = 0;
    // Bead `sts2-rs-jm6e` (ozy.4-rider): reset the per-turn
    // player-as-dealer powered-hits counter consumed by
    // BeatIntoShape (`sts2-rl-agent@01c094d:sts2_env/cards/
    // regent.py:599-607`). Mirrors Python's
    // `_damage_events_this_turn = []` clear at
    // `sts2-rl-agent@01c094d:sts2_env/core/combat.py:678`.
    state.player.powered_hits_this_turn = 0;
    // Reset the per-target powered-hits tally alongside the combat-wide
    // scalar (BeatIntoShape's strictly-per-target forge multiplier,
    // issue #177).
    state.player.powered_hits_by_target_this_turn.clear();
    // Keep the Vigor-consume snapshot (`ph5mx`) in step with the counter it
    // gates on — a stale previous-turn stamp must never survive into the new
    // turn's first play (sweep seed 5816).
    state.player.powered_hits_at_card_play_start = 0;
    state.player.attack_command_suppressed_this_play = false;
    // Bead `sts2-rs-j9x8`: reset the per-turn Skill counter for
    // MakeItSo's late-effect rider. Mirrors Python's implicit
    // `count_cards_played_this_turn` semantics — the counter rolls
    // over at side-turn start.
    state.player.skills_played_this_turn = 0;
    // Bead `sts2-rs-3guy`: clear the VoidForm-style end-turn-after-play
    // rider at the top of each player turn. The flag is set by handlers
    // that call the Rust analogue of Python's
    // `combat.request_end_turn_after_current_play()` (VoidForm at
    // `sts2-rl-agent@01c094d:sts2_env/cards/regent.py:846`); if a turn
    // started with the flag still high (e.g. a save/load round-trip
    // captured mid-play before the bottom-of-`play_card` consumer), this
    // reset prevents it from prematurely ending the fresh turn.
    state.player.request_end_turn_after_play = false;
    // q6bix F-A: player-owned `NoDraw` (BattleTrance) is now removed at the
    // owning turn's end via `hooks::after_turn_end` (mirroring C#
    // `NoDrawPower.AfterTurnEnd`, which has no side guard), so it never
    // survives into the next player turn. The bead-16p0 start-of-turn strip
    // that previously worked around the missing listener has been removed.
    // Bead `sts2-rs-8ett`: reset the per-turn first-in-series play counter
    // that drives EchoForm. C# `EchoFormPower.ModifyCardPlayCount` echoes a
    // card while the number of first-in-series plays this turn is `< Amount`,
    // so the counter starts at 0 each turn and the LIVE EchoForm amount is
    // read per play (see `cards_first_in_series_this_turn` and the echo block
    // in `play_card`). The prior turn-start snapshot of the EchoForm stack sum
    // missed mid-turn gains and let one card drain the whole budget.
    state.player.cards_first_in_series_this_turn = 0;
    // ...and the wire-durable twin of the same tally (GH #368.1). Both are
    // reset here so `echo_form_first_in_series_this_turn`'s max cannot be held
    // up by a stale half.
    reset_echo_form_series_tally(state);
    // Release any stale Vigor command-latch at turn start (bead
    // `sts2-rs-vigor-cmd`): the latch normally clears on Vigor consume, but a
    // turn that ends with un-consumed Vigor (no attack played) must not carry a
    // stale latch into the next turn's first attack.
    state.player.vigor_latch_card = None;

    // Step 3: fire before_side_turn_start(PLAYER). The TurnSideCtx is
    // a placeholder POD at this pin — Phase 6 grows it to carry the
    // CombatSide enum (the Rust `Side` value). For now, the dispatcher
    // walks listeners with the default ctx; no Phase-2 listener arm
    // discriminates on side so the no-op default is correct.
    hooks::dispatch_before_side_turn_start(state, &hooks::TurnSideCtx::default());
    // Bead `sts2-rs-j87`: parallel relic-effects bus drain.
    if relic_channel_live(state, PlannedChannel::BeforeSideTurnStart) {
        let _ = crate::relic_effects::dispatch_before_side_turn_start(state, Side::Player);
    }

    // NOTE (bead `sts2-rs-ybtmu`, retimed): a KnowledgeDemon
    // CURSE_OF_KNOWLEDGE used to defer its player card-select to HERE, on the
    // theory that the four curse powers only act at player-turn boundaries so
    // the timing was observationally identical. It is not: C#
    // `KnowledgeDemon.CurseOfKnowledge` awaits
    // `CardSelectCmd.FromChooseACardScreen(new BlockingPlayerChoiceContext(),
    // …)` INSIDE the move body (`KnowledgeDemon.cs:149-189`), so the live game
    // pauses mid-enemy-turn at the SAME round/phase while the engine ran the
    // whole enemy turn atomically and answered the choice a full turn later.
    // The raise now happens where C# raises it — [`open_monster_curse_select`]
    // called from [`perform_intent`], with the rest of the enemy turn parked on
    // [`CombatState::pending_enemy_exec_resume`]. Deliberately no fallback
    // raise here: a late raise is exactly the divergence being removed, so a
    // stale `pending_curse_choice` must surface as a missing curse rather than
    // be silently re-timed.

    // Step 4: pending-choice deferral. Mirrors Python's
    // `_start_player_turn` lines 681-684 at
    // `sts2-rl-agent@01c094d:sts2_env/core/combat.py` — if a
    // `before_side_turn_start` listener opened a `pending_choice`,
    // we stash a "resume turn-setup" marker and return; the rest of
    // turn-setup (block clear, energy, draw, after-energy hooks)
    // runs in `continue_player_turn_setup` once the choice resolves
    // via `apply(Action::ResolveChoice)` (bead sts2-rs-l2nz).
    if state.pending_choice.is_some() {
        state.pending_turn_setup_resume = true;
        return StepResult::PendingChoice;
    }

    continue_player_turn_setup(state)
}

/// Second half of `_start_player_turn` — runs after any
/// `pending_choice` opened by a `before_side_turn_start` listener
/// has been resolved. Mirrors Python's `_continue_player_turn_setup`
/// at `sts2-rl-agent@01c094d:sts2_env/core/combat.py:686-721`.
///
/// Steps 5-11 of the original `start_player_turn` pipeline: clear
/// block (round > 1) + `after_block_cleared`, energy reset, hand
/// draw, `after_energy_reset`, `after_side_turn_start`, combat-end
/// re-check, advance phase.
///
/// Bead `sts2-rs-l2nz`: extracted so the resume path triggered from
/// `apply(Action::ResolveChoice)` runs the same body.
pub(crate) fn continue_player_turn_setup(state: &mut CombatState) -> StepResult {
    // Step 5: clear block (round > 1) + fire after_block_cleared.
    // Python's `combat_player_states` walk is single-element at this
    // Phase-3 pin (allies omitted); we operate directly on
    // `state.player`.
    if state.round > 1 {
        // Bead `sts2-rs-2e9q2`-follow-up: tag the cleared creature so
        // owner-scoped `after_block_cleared` arms (BlockNextTurn,
        // SelfFormingClay) fire ONLY on the player's own turn-start clear,
        // mirroring C# `AfterBlockCleared(creature)` gating `creature ==
        // Owner`. Without this they also fired on every enemy block-clear
        // (see the enemy-turn-start dispatch below), granting the player's
        // queued block one phase early — the player's own clear then wiped
        // it (Ship in a Bottle read 0 block instead of 10).
        let ctx = hooks::BlockClearedCtx {
            cleared: Some(hooks::CreatureRef::Player),
        };
        // Bead `sts2-rs-d7w6` (1h8q ch.3 wire-up): consult both buses
        // for the `should_clear_block` predicate. Any-false on either
        // bus suppresses the clear (SturdyClamp's relic-bus listener
        // returns false on player-owned). Empty-bus early-out keeps
        // the no-listener fast path lock-free.
        let power_preventer = hooks::block_clear_preventer(state, &ctx);
        let live = relic_guarded_channel_live(state, GuardedChannel::ShouldClearBlock);
        let relic_permit = fold_if_live(state, live, true, |state| {
            crate::relic_effects::dispatch_should_clear_block(state, &ctx)
        });
        if let Some(preventer) = power_preventer {
            // C# `Creature.ClearBlock` else-arm: `Hook.AfterPreventingBlockClear`
            // fires on the PREVENTER alone, at the moment of prevention. On the
            // power bus that is Blur's `Flash()` and nothing else — Barricade
            // and Burrowed veto without overriding the hook (GH #634).
            //
            // A power veto short-circuits before the relic registry is
            // consulted, so SturdyClamp's cap below does NOT run here. That is
            // C#'s `this != preventer` guard: the clamp only caps a clear IT
            // stopped.
            hooks::dispatch_after_preventing_block_clear(state, preventer, &ctx);
        } else if relic_permit {
            state.player.block = 0;
            // Every player-block mutation must back-sync the mirror
            // Creature row (see `sync_creature_block_from_player`);
            // without this the turn-start clear left last turn's block
            // on the mirror, and `teardown_player_creature` would
            // flush the stale value back into the player.
            state.sync_creature_block_from_player();
        } else {
            // SturdyClamp is the preventer: cap the retained block to 10 HERE,
            // before CombatManager's later unconditional `AfterBlockCleared`
            // fires HornCleat's turn-2 grant (world 10+14=24 vs the old
            // mirror's grant-then-cap ≡10).
            let _ = crate::relic_effects::sturdy_clamp::after_preventing_block_clear_sturdy_clamp(
                state, &ctx,
            );
        }
        hooks::dispatch_after_block_cleared(state, &ctx);
        // Bead `sts2-rs-1h8q` ch.2: relic-bus mirror. Empty-bus early-out
        // in the dispatcher keeps the no-listener fast path lock-free.
        // SturdyClamp's listener caps block to MAX_RETAINED_BLOCK here.
        if relic_channel_live(state, PlannedChannel::AfterBlockCleared) {
            let _ = crate::relic_effects::dispatch_after_block_cleared(state, &ctx);
        }
    }

    // Step 6: energy reset. C# splits base vs effective max energy:
    //   * `Player.MaxEnergy` (Player.cs:143, `{get;set;}`) is the stored
    //     BASE — character base + permanent run-level increases. There is
    //     no permanent base-increase effect at this pin, so the base is
    //     always `BASE_MAX_ENERGY`.
    //   * `PlayerCombatState.MaxEnergy` (PlayerCombatState.cs:73) is a
    //     COMPUTED property `Hook.ModifyMaxEnergy(base, ...)`, recomputed
    //     fresh on every read; `ResetEnergy` sets `Energy = MaxEnergy`
    //     (line 125) and `AddMaxEnergyToCurrent` does `Energy += MaxEnergy`.
    //
    // The base MUST be read from the immutable `BASE_MAX_ENERGY` each turn,
    // NOT from the previously-stored `state.player.max_energy` — that field
    // holds the *effective* result we wrote below, so reading it back as the
    // base compounds any persistent `ModifyMaxEnergy` power (PyrePower,
    // DemesnePower) by `+Amount` every turn (sts2-rs-gpsvo: Pyre energy
    // cs4/rs5 @t6, growing +1/turn). We store the effective value into
    // `state.player.max_energy` purely as the reported mirror of
    // `PlayerCombatState.MaxEnergy`.
    let base_max_i32 = BASE_MAX_ENERGY;
    let max_delta = hooks::dispatch_modify_max_energy(state, &MaxEnergyCtx::default());
    let max_energy_i32 = base_max_i32.saturating_add(max_delta);
    let max_energy_i8 = i8::try_from(max_energy_i32.clamp(0, i32::from(i8::MAX)))
        .expect("max_energy fits in i8 after clamp");
    if hooks::dispatch_should_reset_energy(state, &CombatLifecycleCtx::default()) {
        state.player.energy = max_energy_i8;
    } else {
        state.player.energy = state.player.energy.saturating_add(max_energy_i8);
    }
    state.player.max_energy = max_energy_i8;

    // Step 7: hand draw. modify_hand_draw is an additive chain over
    // BASE_DRAW (Python: `modify_hand_draw(BASE_DRAW, self)` returns
    // base + sum(listener deltas)). Round-1 innate handling sorts
    // innate cards to the front of the draw pile and may grow
    // draw_count up to the innate count.
    let hooks_draw_delta = hooks::dispatch_modify_hand_draw(state, &HandDrawCtx::default());
    // C# `MindRotPower.AfterModifyingHandDraw` (`MindRotPower.cs:25`) flashes
    // unconditionally — it carries no target of its own, so (mirroring the
    // established `AfterModifying*` precedent) it fires whenever the
    // `ModifyHandDraw` pipeline runs for the owning player. This is the one
    // Rust call site for that pipeline (`dispatch_modify_hand_draw` has no
    // other non-test caller), so it matches C#'s call frequency exactly.
    if crate::event_log::armed(state) && state.player_power_amount(crate::PowerKind::MindRot) > 0 {
        crate::event_log::flash_player_power(state, crate::PowerKind::MindRot);
    }
    // C# `PaleBlueDotPower.cs:41` (`AfterModifyingHandDraw`) is the same
    // unconditional no-target hook, same single convergence point.
    if crate::event_log::armed(state)
        && state.player_power_amount(crate::PowerKind::PaleBlueDot) > 0
    {
        crate::event_log::flash_player_power(state, crate::PowerKind::PaleBlueDot);
    }
    // Bead `sts2-rs-j87`: parallel relic-effects bus fold. The
    // `relic_effects::dispatch_modify_hand_draw` walker transforms
    // the base draw through every registered listener (left-to-right
    // fold over the BTreeMap iteration order); we feed it the
    // hooks-folded base so both registries compose. Until any relic
    // registers on this channel, `dispatch_modify_hand_draw` returns
    // the input unchanged, so the combined delta equals the
    // hooks-only delta (current behaviour).
    let hooks_folded_draw = BASE_DRAW.saturating_add(hooks_draw_delta);
    let live = relic_guarded_channel_live(state, GuardedChannel::ModifyHandDraw);
    let relic_folded_draw = fold_if_live(state, live, hooks_folded_draw, |state| {
        crate::relic_effects::dispatch_modify_hand_draw(state, hooks_folded_draw)
    });
    // C# `CombatManager.SetupPlayerTurn` runs the round-1 Innate-to-top
    // reorder + draw-count adjustment AFTER `Hook.BeforeHandDraw` and right
    // BEFORE the actual draw (`CombatManager.cs:654` BeforeHandDraw →
    // `:659-674` innate/enchantment reorder → `:675` Draw). So the reorder
    // must come AFTER every `before_hand_draw` effect that mutates the draw
    // pile (FuneraryMask's `CardPilePosition.Random` soul inserts,
    // Bolas/ThrummingHatchet returns, ToastyMittens exhaust). Doing it here —
    // before the dispatch — placed the Innate cards on top BEFORE the souls
    // were inserted, so a soul could land in the opening hand that C# kept in
    // the draw pile (DRAW_ORDER residual, FuneraryMask + DramaticEntrance,
    // bead `sts2-rs-2un` follow-up). The reorder now lives in
    // `resume_after_foregone_select` immediately before the draw.
    let draw_count = relic_folded_draw;
    // `BeforeHandDraw` listener order follows C# `Hook.BeforeHandDraw`
    // (`Hook.cs:423-431`), which walks `combatState.IterateHookListeners()`.
    // For the player creature that iterator emits **Powers FIRST, then
    // Relics, then potions/orbs, then cards-in-piles** (`CombatState.cs:108-162`:
    // `list.AddRange(creature.Powers)` precedes the relic loop). So the
    // power-side channel must dispatch BEFORE the relic-side channel.
    //
    // This ordering is load-bearing: ForegoneConclusionPower (power) pulls
    // cards from the top of the draw pile into hand, and ToastyMittens (relic)
    // exhausts the top of the draw pile + Bombardment auto-plays from exhaust.
    // Powers-first means ForegoneConclusion claims Bombardment off the draw
    // top before ToastyMittens can exhaust-then-auto-play it (Regent:1021 — Rust
    // was killing Chomper a turn early by exhausting Bombardment first; sts2-rs-RP29).
    //
    // Power-side `before_hand_draw` channel. Necrobinder CallOfTheVoidPower
    // generates `Amount` Ethereal cards from the character pool into hand here,
    // before the natural draw (C# `Hook.BeforeHandDraw` at CombatManager.cs:539,
    // ahead of the draw). No-op unless a CallOfTheVoidPower is active.
    hooks::dispatch_before_hand_draw_powers(state);
    // Bead `sts2-rs-fc-handdraw-suspend`: ForegoneConclusionPower's
    // `BeforeHandDraw` surfaces a `ForegoneConclusionFromDraw` card-select when
    // the draw pile exceeds the take (the FC cards are NOT moved to hand yet —
    // the move + `PowerCmd.Remove` are deferred into the resolver). In C# the
    // turn-start sequence AWAITS that select before the natural hand-draw, so
    // the 2 FC cards are in hand BEFORE the draw of 5 runs (hand = 7). We must
    // mirror that: suspend the rest of turn setup (relic before_hand_draw, card
    // returns, DrumOfBattle-late, the natural draw, `finish_player_turn_setup`)
    // until the select resolves — otherwise the natural draw runs while the FC
    // cards still sit in the draw pile, over-draining it (Regent A5P164 step18
    // enemies_hp cs=30 rs=36). Only ForegoneConclusion sets `pending_choice` in
    // `dispatch_before_hand_draw_powers` (Nightmare/CallOfTheVoid/Sentry/
    // Spectrum/InfiniteBlades/CreativeAi all spawn their cards inline), so this
    // guard exclusively catches the FC surface. The deferred tail re-runs from
    // here on resume via `resume_after_foregone_select`.
    if state.pending_choice.is_some() {
        state.pending_hand_draw_resume = Some(draw_count);
        return StepResult::PendingChoice;
    }
    resume_after_foregone_select(state, draw_count)
}

/// The post-`before_hand_draw_powers` tail of [`continue_player_turn_setup`]
/// (relic `before_hand_draw`, Bolas/ThrummingHatchet returns, Howl/Bombardment
/// auto-play, DrumOfBattle-late exhaust, the natural hand draw, then
/// `finish_player_turn_setup`). Extracted (bead `sts2-rs-fc-handdraw-suspend`)
/// so that when `ForegoneConclusionPower.BeforeHandDraw` surfaces its
/// card-select, the natural draw is suspended until the select resolves and
/// then this continuation runs with the FC cards already in hand — matching
/// C#'s awaited ordering (FC 2 cards → hand, THEN draw 5 → hand = 7). Re-entry
/// MUST start AFTER `dispatch_before_hand_draw_powers` so it does not re-spawn
/// other before-hand-draw powers.
pub(crate) fn resume_after_foregone_select(state: &mut CombatState, draw_count: i32) -> StepResult {
    // Relic-side `before_hand_draw` channel. Mirrors Python's
    // `before_hand_draw` invocation site in `_start_player_turn` —
    // fires AFTER `modify_hand_draw` folding and BEFORE the actual
    // draw. Toolbox opens a `request_card_choice` here.
    let live = relic_guarded_channel_live(state, GuardedChannel::BeforeHandDraw);
    let _ = dispatch_if_live(
        state,
        live,
        crate::EffectResult::NotImplemented,
        crate::relic_effects::dispatch_before_hand_draw,
    );
    // GH #369: a relic that opens a select here SUSPENDS the rest of turn
    // setup, exactly as the ForegoneConclusion power leg above does. C#
    // `Toolbox.BeforeHandDraw` (`Toolbox.cs:22-33`) `await`s
    // `CardSelectCmd.FromChooseACardScreen` and only then
    // `AddGeneratedCardToCombat(.., Hand)`, and the whole listener runs inside
    // the awaited `Hook.BeforeHandDraw` at `CombatManager.cs:654` — so the
    // chosen card is in hand BEFORE the natural draw of 5.
    //
    // Not deferring was a KNOWN divergence carried since the original port:
    // the choice sat in `pending_choice`, the draw and the remaining setup ran
    // straight past it, and the player answered during PlayerPlayPhase against
    // an already-drawn hand. Two consequences, both real. Ordering: the grant
    // landed after the draw instead of before, so a hand at HAND_LIMIT
    // silently dropped the Toolbox card that C# had room for. And for the
    // mirror it was worse than an ordering slip — the engine never parked at
    // the moment the world did, so every live Toolbox screen arrived
    // `unparked` (78 of 144 tagged records, the largest family), was
    // classified by a shape heuristic rather than the engine's own derivation,
    // and resolved into a state whose draw had already happened.
    if state.pending_choice.is_some() {
        state.pending_relic_hand_draw_resume = Some(draw_count);
        return StepResult::PendingChoice;
    }
    resume_after_relic_hand_draw_select(state, draw_count)
}

/// The Bolas / ThrummingHatchet copies that return to hand at this turn
/// start: those in a non-hand pile whose `ReturnIfPlayedRound` stamp is
/// `prev_round` (see [`resume_after_relic_hand_draw_select`]).
fn bolas_hatchet_returning(
    state: &CombatState,
    prev_round: i32,
) -> smallvec::SmallVec<[crate::CardUid; 4]> {
    // C# `Bolas`/`ThrummingHatchet.BeforeHandDraw` returns the card from
    // ANY non-Hand pile (`pile == null || pile.Type != PileType.Hand`),
    // not just discard/draw — a copy that was exhausted (DrumOfBattle,
    // Corruption, …) or still resting in the play pile must also come
    // back. Scan all non-hand piles to match (sts2-rs-219ds follow-up).
    // Scan order follows C# `PlayerCombatState.AllPiles` = [Hand,
    // Draw, Discard, Exhaust, Play] (`PlayerCombatState.cs:42-52`) —
    // the hook walk visits the DRAW-pile copy before the discard one,
    // so when both a base and an upgraded copy return the same turn,
    // hand order (and thus which copy a low-index play picks) matches
    // C# (sweep seed 2954: Rust returned the discarded +1 first and
    // played 14 dmg where C# played the draw-pile base 11).
    state
        .player
        .draw
        .iter()
        .chain(state.player.discard.iter())
        .chain(state.player.exhaust.iter())
        .chain(state.player.play.iter())
        .copied()
        .filter(|uid| {
            let idx = uid.0.get() as usize - 1;
            state.cards.get(idx).is_some_and(|card| {
                matches!(
                    card.card_id,
                    cards::CardId::Bolas | cards::CardId::ThrummingHatchet
                ) && card.combat_var(VarKey::ReturnIfPlayedRound) == prev_round
            })
        })
        .collect()
}

/// The post-relic-`before_hand_draw` tail of [`resume_after_foregone_select`]
/// (Bolas/ThrummingHatchet returns, Howl/Bombardment auto-play,
/// DrumOfBattle-late exhaust, the natural hand draw, then
/// `finish_player_turn_setup`). Split out for GH #369 so a relic select opened
/// in the relic channel — Toolbox today — suspends the natural draw the same
/// way `ForegoneConclusionPower`'s does one stage earlier.
///
/// Re-entry MUST start AFTER `dispatch_before_hand_draw` so the relic channel
/// does not re-fire. Toolbox's own per-combat latch (its `Relic.counter`)
/// would absorb a second call, but Bolas/ThrummingHatchet returns and the
/// natural draw would not.
pub(crate) fn resume_after_relic_hand_draw_select(
    state: &mut CombatState,
    mut draw_count: i32,
) -> StepResult {
    // Card-side `BeforeHandDraw`: Bolas / ThrummingHatchet return themselves
    // to hand at turn start iff they were played the PREVIOUS round. C#
    // `Bolas.cs` / `ThrummingHatchet.cs` `BeforeHandDraw`:
    //   if CardPlaysFinished.Any(e => e.RoundNumber == RoundNumber - 1
    //                                 && e.CardPlay.Card == this)
    //       await CardPileCmd.Add(this, PileType.Hand);
    // The colorless play handlers stamp the play round into each card's
    // `ReturnIfPlayedRound` combat_var; a stored value of `round - 1` means
    // "played last round" (0 = never played, never matches). Scan the
    // discard + draw piles (where a played non-exhaust card rests at turn
    // start) and route matches through `move_card_to_hand`. Bead
    // `sts2-rs-219ds`.
    let prev_round = i32::from(state.round).saturating_sub(1);
    // Fast absence gate (perf 2026-07-04, Melancholy pattern): the pile scan
    // below only matches cards whose `card_id` is Bolas / ThrummingHatchet.
    // The card arena is append-only for the whole combat and no in-combat
    // path mutates `card_id` in place, so "neither id anywhere in the arena"
    // proves the four-pile filter walk collects nothing — skipping it is
    // bit-identical (no RNG, no hooks fire from an empty collect). The
    // arena answers it from its presence bits (J02).
    if prev_round >= 1 {
        if state.cards.holds_id(cards::CardId::Bolas)
            || state.cards.holds_id(cards::CardId::ThrummingHatchet)
        {
            for uid in bolas_hatchet_returning(state, prev_round) {
                state.move_card_to_hand(uid);
            }
        } else {
            // Debug shadow: the skipped collect really is empty.
            debug_assert!(
                bolas_hatchet_returning(state, prev_round).is_empty(),
                "J02 presence gate skipped the Bolas / ThrummingHatchet return, \
                 but a copy would return"
            );
        }
    }
    // NOTE: Bombardment's start-of-turn exhaust-pile auto-play is NOT fired here.
    // It is `Bombardment.AfterAutoPrePlayPhaseEnteredEarly`, an AutoPrePlay-phase
    // hook that C# runs AFTER `SetupPlayerTurn`'s hand draw + `AfterPlayerTurnStart`
    // and AFTER the outer-loop `AfterSideTurnStart` (`CombatManager.cs:570` follows
    // `:524`). So it is dispatched in `run_after_player_turn_start_cluster`,
    // alongside Mayhem (also AutoPrePlay), after `fire_after_side_turn_start`.
    // Bead `sts2-rs-bombardment-autoprePlay` (was incorrectly here, pre-hand-draw).
    //
    // C# `CombatManager.SetupPlayerTurn` round-1 opening draw (bead
    // `sts2-rs-2un`): AFTER every `Hook.BeforeHandDraw` effect and right
    // before the draw (`CombatManager.cs:659-674`), move every Innate
    // draw-pile card to the TOP of the pile (so it is drawn first), then
    // raise the draw count to at least the number of Innate cards, capped at
    // `MAX_HAND_SIZE`. The Enchantment `ShouldStartAtBottomOfDrawPile`
    // bottom-move is not modeled at this pin (no enchantment-draw support
    // yet). This MUST run after the relic/card `before_hand_draw` dispatch
    // above so a `CardPilePosition.Random` soul insert (FuneraryMask) lands
    // in the same pile slot C# uses before the Innate cards float to the top
    // (DRAW_ORDER FuneraryMask+DramaticEntrance residual).
    //
    // C# loops `list2.MoveToTopInternal(item)` in pile order, so each
    // successive Innate card lands ABOVE the previous one — the net top order
    // is the reverse of pile order. We reproduce that by removing each Innate
    // uid (in pile order) and re-inserting it at the front.
    if state.round == 1 {
        let innate_uids: smallvec::SmallVec<[crate::CardUid; 8]> = state
            .player
            .draw
            .iter()
            .copied()
            .filter(|uid| {
                let idx = uid.0.get() as usize - 1;
                state
                    .cards
                    .get(idx)
                    .is_some_and(crate::card_effects::card_innate_inst)
            })
            .collect();
        for uid in &innate_uids {
            if let Some(pos) = state.player.draw.iter().position(|u| u == uid) {
                state.player.draw.remove(pos);
                state.player.draw.insert(0, *uid);
                // Pin top-of-draw (bead `sts2-rs-09x5n`): the C# loop moves
                // each Innate card to the physical top in pile order, so the
                // player has definite knowledge of the resulting stack —
                // mirrors Headbutt/ThinkingAhead. Calling `pin_top` in the
                // SAME iteration order as the physical reinsert reproduces
                // the identical top-to-bottom order in `pinned_top` (each
                // call pushes to `pinned_top[0]`, exactly like the physical
                // `draw.insert(0, ..)` above).
                cards::pin_top(&mut state.player, *uid);
            }
        }
        let innate_count = i32::try_from(innate_uids.len()).unwrap_or(i32::MAX);
        draw_count = draw_count
            .max(0)
            .max(innate_count)
            .min(crate::cards::HAND_LIMIT as i32);
    }
    // Bead `sts2-rs-jlc` (n3w-b): route through
    // `CombatState::draw_into_hand` so the `after_shuffle` relic-bus
    // channel fires once per auto-reshuffle the draw triggers.
    let _moved = state.draw_into_hand(draw_count);
    // Bead `sts2-rs-63mew`: the turn-start draw can suspend mid-reshuffle
    // when an `after_shuffle` listener opens a player choice (Stratagem's
    // grid-select — C# awaits it INSIDE the draw loop). Mark the stashed
    // remainder so `apply(Action::ResolveChoice)` resumes the draw and
    // then `finish_player_turn_setup` (steps 8-11). Distinct from the
    // step-4 deferral: re-running `continue_player_turn_setup` here
    // would double the block clear / energy reset / draw.
    if let Some(pdr) = state.pending_draw_resume.as_mut() {
        pdr.resume_turn_setup = true;
        return StepResult::PendingChoice;
    }

    finish_player_turn_setup(state)
}

/// Steps 8-11 of `start_player_turn` — the post-draw tail of
/// [`continue_player_turn_setup`]: `after_energy_reset`,
/// `after_side_turn_start`, the turn-start auto-play powers
/// (Mayhem / RollingBoulder / Entropy), combat-end re-check, phase
/// advance. Extracted (bead `sts2-rs-63mew`) so a turn-start hand draw
/// suspended by an `after_shuffle` choice (Stratagem) can resume into
/// EXACTLY the remaining setup, without re-running the block clear /
/// energy reset / draw of the step-4 resume path.
pub(crate) fn finish_player_turn_setup(state: &mut CombatState) -> StepResult {
    // Step 8: fire after_energy_reset.
    hooks::dispatch_after_energy_reset(state, &CombatLifecycleCtx::default());
    // Bead `sts2-rs-j87`: parallel relic-effects bus drain.
    let live = relic_guarded_channel_live(state, GuardedChannel::AfterEnergyReset);
    let _ = dispatch_if_live(
        state,
        live,
        crate::EffectResult::NotImplemented,
        crate::relic_effects::dispatch_after_energy_reset,
    );
    // NOTE: `StarNextTurnPower` / `GenesisPower` `AfterEnergyReset` star grants
    // are already wired via the `after_energy_reset` hooks bus
    // (`hooks::after_energy_reset` → `Effect::GainStars` → `gain_stars`); that
    // path now fires `BlackHolePower` because `CombatState::gain_stars` calls
    // `apply_black_hole_on_stars_event`. No grant is duplicated here.

    // Step 9 (`after_side_turn_start`) is fired BELOW, AFTER the
    // `AfterPlayerTurnStart` auto-play cluster (enchantment / RollingBoulder /
    // Entropy / Tyranny / Tools / folded powers). This mirrors
    // C# `CombatManager` turn-start ordering (`CombatManager.cs`):
    // `SetupPlayerTurn` runs `AfterPlayerTurnStart` (line 516 — RollingBoulder
    // damage, Entropy transform, …) and only THEN does the outer loop fire
    // `Hook.AfterSideTurnStart` (line 524 — Rampart block, DemonForm Strength,
    // PrepTime Vigor); Mayhem's AutoPrePlay auto-play follows at line 570, after
    // the side-turn-start walk (handled in `run_after_player_turn_start_cluster`).
    // The cluster vs side-turn-start were previously inverted in RS, which let
    // RampartPower's 25 block land on the TurretOperator BEFORE RollingBoulder's
    // Unpowered damage (TURRET_OPERATOR_WEAK hp/block divergence). Relocating the
    // dispatch fixes it natively: the boulder now precedes the shield. Bead
    // `sts2-rs-adsld` (cascade triage).

    // Snapshot the player's power instances NOW, before the turn-start
    // auto-play cluster runs. EntropyPower's `AfterPlayerTurnStart` fires over
    // the powers present at the TOP of the turn (C# dispatch snapshots its
    // listener list), so an Entropy applied DURING this same sequence (e.g. a
    // Mayhem-auto-played ENTROPY card) must NOT transform a card this turn —
    // gate the Entropy block below on this snapshot (bead `sts2-rs-3tder`).
    // DemonForm's `AfterSideTurnStart` fires after the snapshot walk, so it sees
    // freshly-applied powers via the normal dispatch — no re-fire compensation
    // needed.
    let pre_autoplay_power_uids: smallvec::SmallVec<[crate::PowerUid; 8]> =
        state.player_power_uids();

    // Bead `sts2-rs-6ynt.11`: enchantment `after_player_turn_start`
    // channel. Python's `apply_enchantment_after_player_turn_start`
    // fires here (combat.py round-1 Imbued auto-play site). The
    // dispatcher early-outs when the draw pile carries no
    // enchantments; the per-card stub is a no-op at this surface bead.
    crate::enchantment_hooks::dispatch_after_player_turn_start(state);

    // ── `AfterPlayerTurnStart` auto-play cluster (single ordered walk) ──────
    //
    // C# `Hook.AfterPlayerTurnStart` (inside `SetupPlayerTurn`, `CombatManager.cs:516`)
    // dispatches EVERY `AfterPlayerTurnStart` power in one pass over
    // `CombatState.IterateHookListeners()`, which SNAPSHOTS the listener list (in
    // power-application order) when the `foreach` begins — before any auto-play in
    // this pass can apply a new power, and strictly before the outer-loop
    // `Hook.AfterSideTurnStart` (`:524` — Rampart / DemonForm / PrepTime). Walk
    // the pre-auto-play power snapshot in that same order and dispatch each kind's
    // effect inline:
    //
    //   * RollingBoulder (`RollingBoulderPower.cs`, ioty0 / boulder-inst): deal
    //     its OWN `Amount` Unpowered to all hittable enemies, then ramp
    //     `Amount += DamageVar(5)`; IsInstanced, so each instance fires once in
    //     power order.
    //   * Entropy (`EntropyPower.cs`, isooo / 3tder): transform the first
    //     `Amount` hand cards (deterministic select; RNG-faithful to
    //     `CardCmd.TransformToRandom`). Snapshot-gated by construction.
    //   * Folded `AfterPlayerTurnStart` powers — Inferno self-damage, Loop
    //     orb-passive, SummonNextTurn, CrimsonMantle (`is_folded_after_player_turn_start_kind`,
    //     beads r2ixj / ho3qq). The `after_side_turn_start` walk below SKIPS
    //     these; this is their sole dispatch site.
    //
    // Mayhem is NOT in this cluster. In v0.107.1 `MayhemPower` is
    // `AfterAutoPrePlayPhaseEntered` (`MayhemPower.cs:16`), which C#
    // `CombatManager` runs in the AutoPrePlay phase at `:570` — AFTER
    // `Hook.AfterSideTurnStart` (`:524`). It is dispatched in
    // `run_after_player_turn_start_cluster` AFTER `fire_after_side_turn_start`,
    // so a turn-start `PrepTimePower`'s Vigor is granted before Mayhem's
    // auto-played powered attack consumes it (v0.103.2 classed Mayhem as
    // `AfterPlayerTurnStart`, which inverted that ordering — bead `sts2-rs-181`,
    // CEREMONIAL_BEAST_BOSS seed 3515 PREP_TIME + MAYHEM_CARD).
    //
    // Application order within the cluster is load-bearing: folded powers fire in
    // snapshot (power-application) order. Beads `sts2-rs-r2ixj` (seed 1130) /
    // `sts2-rs-ho3qq` (seed 5695) / `sts2-rs-ioty0` / `sts2-rs-isooo` /
    // `sts2-rs-3tder`. The cluster + Plasma + `after_side_turn_start` walk +
    // Mayhem auto-play + phase advance run as a RESUMABLE walk
    // (`run_after_player_turn_start_cluster`, beads `sts2-rs-o0rz3` /
    // `sts2-rs-181`): a `StratagemPower.AfterShuffle` grid-select during Mayhem's
    // auto-play suspends and resumes via `apply(Action::ResolveChoice)` (with
    // `after_side_turn_start` already fired, so it is not re-fired).
    run_after_player_turn_start_cluster(state, pre_autoplay_power_uids, 0, None, false, None)
}

/// Phase-1 collect of a turn-start Mayhem `AutoPlayFromDrawPile`, firing the
/// `after_shuffle` hook chain (Stratagem) on each reshuffle so a Stratagem
/// grid-select can SUSPEND it, then phase-2 play of everything collected.
/// Mirrors C# `CardPileCmd.AutoPlayFromDrawPile`: `ShuffleIfNecessary` (which
/// awaits `Hook.AfterShuffle`) runs before each `drawPile.First()`, and each
/// `CardCmd.AutoPlay` is `await`ed. Bead `sts2-rs-o0rz3`.
///
/// `None` means the batch ran to completion — the caller may proceed to the
/// turn-start tail. `Some(ap)` means it SUSPENDED and the caller must park `ap`
/// on [`crate::CombatState::pending_cluster_resume`]; re-entering here with it
/// resumes exactly where it stopped. Two suspend points:
///
/// * **phase 1**, on a Stratagem reshuffle grid-select — `ap` carries the live
///   collect state (the card it was about to take has NOT been collected);
/// * **phase 2**, on a collected card's surfacing `OnPlay` select (bead
///   `sts2-rs-2itwm`) — that card's tail and its un-played siblings ride
///   `pending_autoplay_resume` instead, so `ap` is a NO-OP `PendingAutoplay`
///   whose only job is to make the caller park and re-enter for the tail.
fn auto_play_collect_suspending(
    state: &mut CombatState,
    mut ap: crate::PendingAutoplay,
) -> Option<crate::PendingAutoplay> {
    while ap.remaining > 0 {
        if state.player.draw.is_empty() {
            let shuffled = state.reshuffle_into_draw_with_after_shuffle();
            if shuffled && state.pending_choice.is_some() {
                // Stratagem opened its grid-select — suspend BEFORE taking the
                // top card (it is collected on resume).
                return Some(ap);
            }
        }
        let Some(&top_uid) = state.player.draw.first() else {
            break;
        };
        state.player.draw.remove(0);
        // Bead `sts2-rs-09x5n`: this is a raw physical-top pop that bypasses
        // `cards::pop_next_card`, so it must clear any stale `pinned_top`
        // entry itself — otherwise a later `pop_next_card` sees a non-empty
        // `pinned_top` and short-circuits to a deterministic `draw.remove(0)`
        // for what may now be a heterogeneous unknown draw, silently
        // suppressing a chance node.
        cards::unpin(&mut state.player, top_uid);
        state.player.play.push(top_uid);
        ap.collected.push(top_uid);
        ap.remaining -= 1;
    }
    // Phase 2: play each collected card in order (inline OnPlay choices are
    // drained inside `resolve_auto_played_card`, matching the legacy path).
    //
    // Bead `sts2-rs-2itwm`: AWAIT each play. C# `CardPileCmd.AutoPlayFromDrawPile`
    // `await`s every `CardCmd.AutoPlay(card)`, so a collected card whose `OnPlay`
    // opens a SURFACING select blocks the rest of the batch until the pick lands.
    // This loop used to run straight through the suspend: the surfaced card's
    // parked `pending_autoplay_resume` was left with an EMPTY `remaining`, the
    // next sibling played on top of it (pushing a `parent_chain` frame that no
    // driver ever attached siblings to), and the enclosing cluster walk ran its
    // turn-start tail — so the turn completed with the select still open and the
    // un-played siblings stranded on the Play pile for the rest of combat
    // (turn-boundary invariant #18; hunt seeds 2786254 / 42613 / 666604, every
    // one of them Mayhem). Stash the not-yet-played remainder on the resume
    // marker (exactly as the Cascade / Havoc driver
    // `auto_play_cards_from_draw_top` does) and report the suspend upward with a
    // NO-OP `PendingAutoplay`: the caller parks it on `pending_cluster_resume`,
    // and re-entering `auto_play_collect_suspending` with it collects nothing and
    // plays nothing, dropping straight into the turn-start tail once the
    // suspended card and its siblings have finished.
    for (n, uid) in ap.collected.iter().copied().enumerate() {
        resolve_auto_played_card(state, uid, ap.force_exhaust);
        if let Some(resume) = state.pending_autoplay_resume.as_mut() {
            let rest: smallvec::SmallVec<[crate::CardUid; 8]> =
                ap.collected[n + 1..].iter().copied().collect();
            // Same frame choice as the Cascade driver: when the card WE resolved
            // auto-played a nested card that surfaced FIRST, our siblings belong
            // to OUR parent-chain frame (they replay after our tail), not to the
            // inner card's top-level slot.
            if let Some(parent) = resume.parent_chain.last_mut() {
                parent.remaining = rest;
                parent.force_exhaust = ap.force_exhaust;
            } else {
                resume.remaining = rest;
                resume.force_exhaust = ap.force_exhaust;
            }
            return Some(crate::PendingAutoplay::default());
        }
    }
    None
}

/// Fire the player-side `after_side_turn_start` walk (power + relic buses).
/// C# outer-loop `Hook.AfterSideTurnStart` (`CombatManager.cs:437` — Rampart /
/// DemonForm / PrepTime / CreativeAi). Skips the folded `AfterPlayerTurnStart`
/// kinds (those are dispatched in the cluster). Bead `sts2-rs-o0rz3`.
fn fire_after_side_turn_start(state: &mut CombatState) {
    hooks::dispatch_after_side_turn_start(state, &hooks::TurnSideCtx::default());
    if relic_channel_live(state, PlannedChannel::AfterSideTurnStart) {
        let _ = crate::relic_effects::dispatch_after_side_turn_start(state, Side::Player);
    }
}

/// The `AfterPlayerTurnStart` auto-play cluster (RollingBoulder / Entropy /
/// Tyranny / Tools / folded Inferno/Loop/SummonNextTurn/CrimsonMantle) + relic
/// `AfterPlayerTurnStart` + Plasma passive + the `after_side_turn_start` walk +
/// the Mayhem AutoPrePlay auto-play + phase advance — as a RESUMABLE walk over
/// the pre-auto-play power `snapshot`.
///
/// C# `Hook.AfterPlayerTurnStart` dispatches every such power in ONE snapshot
/// pass in power-application order; application order among the folded powers is
/// load-bearing (seeds 1130 / 5695).
///
/// **Mayhem (v0.107.1):** Mayhem is `AfterAutoPrePlayPhaseEntered`
/// (`MayhemPower.cs:16`), which C# runs in the AutoPrePlay phase at
/// `CombatManager.cs:570` — AFTER `Hook.AfterSideTurnStart` (`:524`). So its
/// auto-play is dispatched here AFTER `fire_after_side_turn_start`, not in the
/// snapshot walk. A `StratagemPower.AfterShuffle` grid-select during that
/// auto-play suspends with `mayhem_phase == true`; the resume finishes the
/// auto-play and runs the tail WITHOUT re-firing `after_side_turn_start` (it has
/// already fired). A Tyranny / Tools exhaust/discard select during the
/// snapshot walk suspends with `mayhem_phase == false` and resumes the walk.
/// Beads `sts2-rs-o0rz3` / `sts2-rs-181`.
fn run_after_player_turn_start_cluster(
    state: &mut CombatState,
    snapshot: smallvec::SmallVec<[crate::PowerUid; 8]>,
    start_idx: usize,
    resume_autoplay: Option<crate::PendingAutoplay>,
    side_turn_start_fired: bool,
    mayhem_resume: Option<crate::PendingAutoplay>,
) -> StepResult {
    let ctx = hooks::TurnSideCtx::default();
    // Mayhem (AutoPrePlay) resume: in v0.107.1 Mayhem's auto-play fires AFTER
    // `Hook.AfterSideTurnStart` (`MayhemPower.AfterAutoPrePlayPhaseEntered`;
    // `CombatManager.cs:524` AfterSideTurnStart precedes `:570`'s
    // `RunAutoPrePlayPhase`), so `after_side_turn_start` has already fired here.
    // Finish the suspended Mayhem auto-play and run the turn-start tail — no
    // snapshot re-walk and no `after_side_turn_start` re-fire (bead `sts2-rs-181`).
    if let Some(ap) = mayhem_resume {
        if !state.is_over {
            if let Some(susp) = auto_play_collect_suspending(state, ap) {
                state.pending_cluster_resume = Some(crate::PendingClusterResume {
                    snapshot,
                    idx: 0,
                    autoplay: susp,
                    side_turn_start_fired: true,
                    mayhem_phase: true,
                    relic_start: None,
                });
                return StepResult::PendingChoice;
            }
        }
        return finish_turn_start_tail(state);
    }
    let mut resume_autoplay = resume_autoplay;
    let mut i = start_idx;
    while i < snapshot.len() {
        if state.is_over {
            break;
        }
        let uid = snapshot[i];
        // Resuming a suspended Mayhem at this index: finish its collect+play
        // (it may suspend AGAIN on a second reshuffle) before moving on.
        if let Some(ap) = resume_autoplay.take() {
            if let Some(susp) = auto_play_collect_suspending(state, ap) {
                state.pending_cluster_resume = Some(crate::PendingClusterResume {
                    snapshot,
                    idx: i,
                    autoplay: susp,
                    side_turn_start_fired,
                    mayhem_phase: false,
                    relic_start: None,
                });
                return StepResult::PendingChoice;
            }
            i += 1;
            continue;
        }
        let Some(kind) = state.powers.get_by_id(uid.0).map(|p| p.kind) else {
            i += 1;
            continue;
        };
        match kind {
            crate::PowerKind::Mayhem => {
                // Dispatched AFTER `fire_after_side_turn_start` below, NOT in this
                // `AfterPlayerTurnStart` walk: in v0.107.1 Mayhem is
                // `AfterAutoPrePlayPhaseEntered` (`MayhemPower.cs:16`), which C#
                // `CombatManager` runs after `Hook.AfterSideTurnStart`
                // (`CombatManager.cs:524` precedes `:570`'s `RunAutoPrePlayPhase`).
                // v0.103.2 classed it as `AfterPlayerTurnStart` (before
                // `AfterSideTurnStart`), so it used to fire here — that inverted
                // the PrepTime-Vigor/Mayhem ordering (bead `sts2-rs-181`).
            }
            crate::PowerKind::RollingBoulder => {
                let amount = state.powers.get_by_id(uid.0).map_or(0, |p| p.amount);
                // C# `RollingBoulderPower.cs:36` flashes unconditionally inside
                // `player == base.Owner.Player` — before the damage/amount-grow,
                // with no amount check.
                crate::event_log::flash_player_power(state, crate::PowerKind::RollingBoulder);
                if amount > 0 {
                    let _ = crate::card_effects::deal_damage_to_all_alive_enemies(
                        state,
                        amount,
                        ValueProps::UNPOWERED,
                        None,
                    );
                    if let Some(pow) = state.powers.get_mut_by_id(uid.0) {
                        pow.amount = pow.amount.saturating_add(5);
                    }
                }
            }
            crate::PowerKind::Entropy => {
                let amount = state.powers.get_by_id(uid.0).map_or(0, |p| p.amount);
                if amount > 0 {
                    let targets: smallvec::SmallVec<[crate::CardUid; 8]> = state
                        .player
                        .hand
                        .iter()
                        .take(amount as usize)
                        .copied()
                        .collect();
                    for t in targets {
                        let _ = crate::card_effects::transform_card_to_random(state, t);
                    }
                }
            }
            crate::PowerKind::Tyranny => {
                // C# `TyrannyPower.AfterPlayerTurnStart` (TyrannyPower.cs:32-42):
                // `CardSelectCmd.FromHand(prefs(ExhaustSelectionPrompt, Amount))`
                // → `CardCmd.Exhaust` each pick — exhaust EXACTLY `Amount` hand
                // cards (the 2-arg `CardSelectorPrefs(prompt, Amount)` sets
                // `min == max == Amount`, so it is a forced exact-count pick).
                // `FromHand`'s `cards.Count <= MinSelect` fast path
                // (`CardSelectCmd.cs`) auto-takes the WHOLE hand without opening a
                // screen when the hand has `<= Amount` cards; a larger hand opens
                // [`ChoiceKind::ExhaustFromHand`] and suspends the cluster (like
                // Mayhem's Stratagem suspend) so the player pick drains before the
                // walk continues. Options are hand-order (C# `FromHand` passes the
                // hand `Cards` with no OrderBy). Bead `sts2-rs-jxz`.
                let amount = state.powers.get_by_id(uid.0).map_or(0, |p| p.amount);
                if amount > 0 && !state.player.hand.is_empty() && state.pending_choice.is_none() {
                    let take = amount as usize;
                    if state.player.hand.len() <= take {
                        // Auto-take fast path: exhaust the entire hand in hand
                        // order (no screen). Snapshot uids first — `exhaust_card_uid`
                        // mutates `player.hand`.
                        let all: smallvec::SmallVec<[crate::CardUid; 8]> =
                            state.player.hand.iter().copied().collect();
                        for cuid in all {
                            let _ = crate::card_effects::exhaust_card_uid(state, cuid);
                        }
                    } else {
                        let options: Vec<crate::ChoiceOption> = state
                            .player
                            .hand
                            .iter()
                            .map(|&cuid| {
                                let card = state.cards.get(cuid.0.get() as usize - 1);
                                crate::ChoiceOption {
                                    card_id: card
                                        .map_or(crate::cards::CardId::StrikeIronclad, |c| {
                                            c.card_id
                                        }),
                                    upgraded: card.is_some_and(|c| c.upgraded),
                                    card_uid: Some(cuid),
                                    source_pile: Some(crate::cards::PileKind::Hand),
                                }
                            })
                            .collect();
                        let sel = u8::try_from(take).unwrap_or(u8::MAX);
                        state.request_card_choice(
                            crate::ChoiceKind::ExhaustFromHand,
                            options,
                            sel,
                            sel,
                            false,
                        );
                        // Suspend the cluster: Tyranny (snapshot[i]) has done its
                        // work, so stash a no-op autoplay at `idx = i` — the resume
                        // runs `auto_play_collect_suspending(default)` (a clean
                        // no-op: remaining 0, nothing collected) and advances to
                        // `i + 1`. `after_side_turn_start` has NOT fired yet (it is
                        // step 9, after this loop), so `side_turn_start_fired`
                        // stays false and it fires at its normal spot on resume.
                        state.pending_cluster_resume = Some(crate::PendingClusterResume {
                            snapshot,
                            idx: i,
                            autoplay: crate::PendingAutoplay::default(),
                            side_turn_start_fired,
                            mayhem_phase: false,
                            relic_start: None,
                        });
                        return StepResult::PendingChoice;
                    }
                }
            }
            crate::PowerKind::ToolsOfTheTrade => {
                // C# `ToolsOfTheTradePower.AfterPlayerTurnStart`
                // (ToolsOfTheTradePower.cs:27-37): for the owning player, open
                // `CardSelectCmd.FromHandForDiscard(prefs(Amount))` → discard
                // exactly `Amount` hand cards. The +Amount draw half is the
                // `ToolsOfTheTrade` arm in `hooks::modify_hand_draw` (folds at
                // the draw step, before this cluster). Mirrors HiddenDaggers'
                // discard shape: auto-discard when the hand has <= Amount cards
                // (FromHandForDiscard auto-resolve), else open a DiscardFromHand
                // choice and SUSPEND the cluster, exactly as Tyranny does above.
                //
                // The suspend is not optional. C# `await`s the select inside
                // `AfterPlayerTurnStart`, so every later power in this walk, the
                // relic `AfterPlayerTurnStart` bus, Plasma, Step 9's
                // `after_side_turn_start`, and the Bombardment / Mayhem auto-plays
                // all wait behind the pick. Parking the choice and falling through
                // ran that whole tail FIRST and left the discard to resolve in
                // `PlayerPlayPhase` — the wrong order, and a `StepResult::Advanced`
                // returned with `pending_choice` set, which is a state transition
                // the engine did not make. `run_turn_start_relic_tail`'s own
                // resume-safety comment already says "a Tyranny/Tools cluster
                // suspend stashes `idx` BEFORE this point"; this arm never did.
                //
                // Cost of the lie: the advisor's end-of-turn replay branches on
                // that return string, so it declined the screen as "the turn
                // ended but parked no choice (advanced)" and fell through to the
                // heuristic — 9 of the 17 unsearched in-combat decisions in the
                // `logs/f7-mirror-check15` corpus.
                //
                // NOT all 13 that corpus labels `TO_DISCARD`: that label is the
                // prompt key, and `FromHandForDiscard` has two unrelated
                // callers. The other 4 are cards whose OnPlay opens the same
                // screen mid-play (HiddenDaggers, Survivor, DaggerThrow,
                // Prepared) and go down `replay_opening_play` instead. Telling
                // them apart needs the play line before the screen, not the
                // label — the first read of this corpus attributed the whole
                // bucket to this power.
                let amount = state.powers.get_by_id(uid.0).map_or(0, |p| p.amount);
                if amount > 0 && state.pending_choice.is_none() {
                    let discard_count = (amount as usize).min(state.player.hand.len());
                    if discard_count > 0 {
                        let hand: smallvec::SmallVec<[crate::CardUid; 16]> =
                            state.player.hand.iter().copied().collect();
                        if hand.len() <= discard_count {
                            // Route through the shared C# `Discard` path so each
                            // fires `AfterCardDiscarded` and any discarded `Sly`
                            // card auto-plays.
                            let to_discard: smallvec::SmallVec<[crate::CardUid; 16]> =
                                hand.iter().take(discard_count).copied().collect();
                            let _ = crate::card_effects::discard_hand_cards_with_hooks(
                                state,
                                &to_discard,
                            );
                        } else {
                            let options: Vec<crate::ChoiceOption> = hand
                                .iter()
                                .map(|&cuid| {
                                    let idx = cuid.0.get() as usize - 1;
                                    crate::ChoiceOption {
                                        card_id: state.cards[idx].card_id,
                                        upgraded: state.cards[idx].upgraded,
                                        card_uid: Some(cuid),
                                        source_pile: Some(crate::cards::PileKind::Hand),
                                    }
                                })
                                .collect();
                            let n = u8::try_from(discard_count).unwrap_or(u8::MAX);
                            state.request_card_choice(
                                crate::ChoiceKind::DiscardFromHand,
                                options,
                                n,
                                n,
                                false,
                            );
                            // Same stash as Tyranny: this power (snapshot[i]) has
                            // done its work, so `idx = i` resumes with a no-op
                            // autoplay and advances to `i + 1`.
                            // `after_side_turn_start` is step 9, after this loop,
                            // so it has not fired and resumes at its normal spot.
                            state.pending_cluster_resume = Some(crate::PendingClusterResume {
                                snapshot,
                                idx: i,
                                autoplay: crate::PendingAutoplay::default(),
                                side_turn_start_fired,
                                mayhem_phase: false,
                                relic_start: None,
                            });
                            return StepResult::PendingChoice;
                        }
                    }
                }
            }
            k if hooks::is_folded_after_player_turn_start_kind(k) => {
                if let Some(effects) =
                    hooks::collect_folded_after_player_turn_start_one(state, uid, &ctx)
                {
                    hooks::apply_effects(state, &effects);
                }
            }
            _ => {}
        }
        i += 1;
    }

    run_turn_start_relic_tail(state, snapshot, 0, side_turn_start_fired)
}

/// The post-power-walk half of the turn-start cluster: the relic
/// `AfterPlayerTurnStart` bus, the Plasma passive, Step 9's
/// `after_side_turn_start`, the Bombardment + Mayhem AutoPrePlay auto-plays and
/// [`finish_turn_start_tail`].
///
/// Extracted (mirror-fidelity round-4 defect #13) so a relic that opens a
/// BLOCKING card screen from `AfterPlayerTurnStart` — GamblingChip's round-1
/// mulligan, which C# `Hook.AfterPlayerTurnStart` `await`s — can suspend the
/// whole remainder and resume into exactly this point once the pick lands.
/// `relic_start` is the equip index to (re-)enter the relic bus at.
pub(crate) fn run_turn_start_relic_tail(
    state: &mut CombatState,
    snapshot: smallvec::SmallVec<[crate::PowerUid; 8]>,
    relic_start: u8,
    side_turn_start_fired: bool,
) -> StepResult {
    // Bead `sts2-rs-axdgh`: relic `AfterPlayerTurnStart` bus. C# relics with an
    // `AfterPlayerTurnStart` override (MercuryHourglass — 3 Unpowered damage to
    // all hittable enemies) fire HERE, inside `SetupPlayerTurn`'s
    // `AfterPlayerTurnStart`, BEFORE Step 9's `after_side_turn_start` where the
    // enemy LivingShield's RampartPower arms the TurretOperator's block. Drains
    // alongside the RollingBoulder power above so turn-start enemy damage lands
    // on bare HP, not the not-yet-granted shield (`TURRET_OPERATOR_WEAK` arena
    // parity). Was previously on `after_side_turn_start`, firing one step too
    // late (after Rampart).
    //
    // Defect #13: the walk SUSPENDS when a listener opens a card screen
    // (GamblingChip). C# awaits it inside the hook, so the rest of this
    // function — later `AfterPlayerTurnStart` relics, Plasma,
    // `after_side_turn_start` (Akabeko / Lantern / BloodVial), Bombardment,
    // Mayhem, the phase advance — must not run until the pick resolves.
    if !state.is_over && relic_channel_live(state, PlannedChannel::AfterPlayerTurnStart) {
        if let Some(next) =
            crate::relic_effects::dispatch_after_player_turn_start_from(state, relic_start)
        {
            state.pending_cluster_resume = Some(crate::PendingClusterResume {
                snapshot,
                idx: 0,
                autoplay: crate::PendingAutoplay::default(),
                side_turn_start_fired,
                mayhem_phase: false,
                relic_start: Some(next),
            });
            return StepResult::PendingChoice;
        }
    }

    // C# `PlasmaOrb.AfterTurnStartOrbTrigger` → `Passive` → `GainEnergy(1)`.
    // Plasma is the SOLE orb whose passive fires at TURN START — Lightning /
    // Frost / Dark use `BeforeTurnEndOrbTrigger` (routed through Rust's
    // turn-end `orbs::passive_tick`). Each Plasma still in a slot grants +1
    // energy now; `PlasmaOrb.PassiveVal => 1m` is a plain literal (no
    // `ModifyOrbValue`), so Focus does NOT scale it. The turn-end
    // `passive_tick` Plasma arm is intentionally inert — this is the only
    // Plasma passive site. Bead `sts2-rs-plasma-passive`.
    if !state.is_over {
        let plasma_count = state
            .player
            .orbs
            .slots
            .iter()
            .filter(|o| matches!(o, crate::orbs::Orb::Plasma))
            .count();
        if plasma_count > 0 {
            let add = i8::try_from(plasma_count).unwrap_or(i8::MAX);
            state.player.energy = state.player.energy.saturating_add(add);
        }
    }

    // Step 9: fire `after_side_turn_start(PLAYER)` — after the
    // `AfterPlayerTurnStart` cluster above and BEFORE Mayhem's AutoPrePlay
    // auto-play below, mirroring C#
    // `CombatManager` (`AfterPlayerTurnStart` inside `SetupPlayerTurn` at
    // `CombatManager.cs:516`, then `Hook.AfterSideTurnStart` at `:524`, then
    // `RunAutoPrePlayPhase` → `AfterAutoPrePlayPhaseEntered` at `:570`).
    //
    // Powers reacting to this arm at the pin: RampartPower (enemy LivingShield
    // → 25 retained block to each TurretOperator — fires AFTER RollingBoulder's
    // Unpowered damage so the boulder hits bare HP, matching C#), DemonFormPower
    // (player → +Strength), PrepTimePower (player → +Vigor, which the Mayhem
    // auto-play below then consumes via its powered attack), and CreativeAiPower
    // (spawns Power cards to hand).
    //
    // Python's `fire_after_side_turn_start` (hooks.py:413-425) also fires
    // `on_turn_start_own_side` for powers overriding only that arm. No power
    // or relic has such an arm (and C# has no such hook), so the separate
    // no-op walk Rust used to call here was deleted.
    //
    // Guarded on `!is_over`: now that this dispatch runs after the
    // `AfterPlayerTurnStart` cluster, RollingBoulder (or an auto-played card)
    // may already have ended the combat — don't fire turn-start hooks on a
    // resolved combat (Step 10 below catches the terminal state). The
    // `side_turn_start_fired` gate is retained for the `AfterPlayerTurnStart`
    // walk-resume path (always `false` there); Mayhem's own suspend resumes via
    // `mayhem_phase` above and never re-enters this point (bead `sts2-rs-o0rz3`).
    if !side_turn_start_fired && !state.is_over {
        fire_after_side_turn_start(state);
    }

    // ── Bombardment AutoPrePlay(Early) exhaust-pile auto-play ──────────────
    //
    // Card-side START-of-turn auto-play from the EXHAUST pile: Bombardment
    // (v0.107.1 `Bombardment.AfterAutoPrePlayPhaseEnteredEarly`):
    //   `if pile?.Type == Exhaust && player == Owner -> CardCmd.AutoPlay(this)`
    // (`Bombardment.cs`). Bombardment HAS the Exhaust keyword, so the auto-played
    // copy re-exhausts and fires again EVERY subsequent turn start — its design (an
    // 18-damage `AnyEnemy` attack that recurs from the exhaust pile once Exhaust
    // puts it there). `resolve_auto_played_card` rolls the random enemy target off
    // the `combat_targets` stream and routes the card back to Exhaust via its
    // keyword.
    //
    // PHASE (bead `sts2-rs-bombardment-autoprePlay`, #177): this is an
    // AutoPrePlay-phase hook, which C# `CombatManager` runs AFTER `SetupPlayerTurn`
    // (hand draw + `Hook.AfterPlayerTurnStart`, `:677`) AND AFTER the outer-loop
    // `Hook.AfterSideTurnStart` (`:524`), in `RunAutoPrePlayPhase` (`:570`). So it
    // fires HERE — after `fire_after_side_turn_start` above — NOT pre-hand-draw in
    // `continue_player_turn_setup` where it used to live. Two parity facets the old
    // placement broke:
    //   * BLOCK: an enemy LivingShield's RampartPower arms its block during
    //     `after_side_turn_start`; firing Bombardment before that let its 18 land on
    //     bare HP instead of the shield (Regent TURRET_OPERATOR seed 3940).
    //   * TIMING: a `TyrannyPower` exhaust during the `AfterPlayerTurnStart` cluster
    //     walk (above) puts a Bombardment into the exhaust pile THIS turn; the old
    //     pre-walk scan missed it and fired it a turn late (Regent seed 2871). The
    //     scan now runs after the walk, so a Tyranny-exhausted Bombardment fires the
    //     same turn as C#.
    //
    // The `Early` variant fires before Mayhem's normal `AfterAutoPrePlayPhaseEntered`
    // (so the `combat_targets` read precedes any Mayhem auto-play) and guards against
    // a Mayhem-driven double-trigger. v0.107.1 NOTE: HowlFromBeyond used to share
    // this scan (both were C# `BeforeHandDraw` in v0.103.2) but now fires at turn END
    // (`HowlFromBeyond.AfterAutoPostPlayPhaseEntered`).
    //
    // Resume-safety: this scan sits AFTER the snapshot walk + `after_side_turn_start`
    // and BEFORE the Mayhem block, so it runs exactly once. A Tyranny/Tools cluster
    // suspend stashes `idx` BEFORE this point and re-enters from the top — the scan
    // runs once the resumed walk reaches here. A Mayhem suspend resumes via the
    // `mayhem_resume` fast path (top of fn) straight into `finish_turn_start_tail`,
    // which is AFTER this scan, so it is never re-run. force_exhaust=false (the
    // result pile follows the card's Exhaust keyword); the exhaust pile is
    // snapshotted BEFORE the loop so a re-exhausted Bombardment is not replayed twice
    // this turn — it fires anew next turn.
    if !state.is_over {
        let auto_exhaust: smallvec::SmallVec<[crate::CardUid; 2]> = state
            .player
            .exhaust
            .iter()
            .copied()
            .filter(|uid| {
                let idx = uid.0.get() as usize - 1;
                state
                    .cards
                    .get(idx)
                    .is_some_and(|c| matches!(c.card_id, cards::CardId::Bombardment))
            })
            .collect();
        for uid in auto_exhaust {
            if state.is_over {
                break;
            }
            if let Some(pos) = state.player.exhaust.iter().position(|&u| u == uid) {
                state.player.exhaust.remove(pos);
                state.player.play.push(uid);
                resolve_auto_played_card(state, uid, /*force_exhaust=*/ false);
            }
        }
    }

    // ── Mayhem AutoPrePlay auto-play (bead `sts2-rs-181`) ──────────────────
    //
    // C# `MayhemPower.AfterAutoPrePlayPhaseEntered` (`MayhemPower.cs:16`):
    // `CardPileCmd.AutoPlayFromDrawPile(Amount, Top)`. v0.107.1 runs this in the
    // AutoPrePlay phase, which `CombatManager` enters at `:570` — AFTER
    // `Hook.AfterSideTurnStart` (`:524`). So the Vigor a turn-start
    // `PrepTimePower` granted in Step 9 is LIVE when Mayhem's auto-played powered
    // attack resolves and is consumed by it; and a `PrepTimePower` Mayhem
    // auto-plays this turn does NOT grant Vigor (its `AfterSideTurnStart` already
    // passed). v0.103.2 classed Mayhem as `AfterPlayerTurnStart` (before
    // `AfterSideTurnStart`), so the prior code fired it in the cluster walk —
    // which left the freshly-granted Vigor dangling (CEREMONIAL_BEAST_BOSS seed
    // 3515, PREP_TIME + MAYHEM_CARD). Mayhem is a `Counter` power (a single
    // instance), so dispatch the lone instance from the snapshot here. A
    // `StratagemPower.AfterShuffle` grid-select during the auto-play suspends via
    // `mayhem_phase` (after_side_turn_start has already fired, so it is not
    // re-fired on resume).
    if !state.is_over {
        let mayhem_uid = snapshot.iter().copied().find(|u| {
            state
                .powers
                .get_by_id(u.0)
                .is_some_and(|p| p.kind == crate::PowerKind::Mayhem)
        });
        if let Some(mayhem_uid) = mayhem_uid {
            let amount = state.powers.get_by_id(mayhem_uid.0).map_or(0, |p| p.amount);
            if amount > 0 {
                let ap = crate::PendingAutoplay {
                    remaining: amount,
                    collected: smallvec::SmallVec::new(),
                    force_exhaust: false,
                };
                if let Some(susp) = auto_play_collect_suspending(state, ap) {
                    state.pending_cluster_resume = Some(crate::PendingClusterResume {
                        snapshot,
                        idx: 0,
                        autoplay: susp,
                        side_turn_start_fired: true,
                        mayhem_phase: true,
                        relic_start: None,
                    });
                    return StepResult::PendingChoice;
                }
            }
        }
    }

    finish_turn_start_tail(state)
}

/// Turn-start tail (steps 10-11) shared by the normal cluster path and the
/// Mayhem `mayhem_phase` resume: combat-end re-check, phase advance to
/// `PlayerPlayPhase`, the `before_play_phase_start` relic dispatch, and a final
/// combat-end re-check. Bead `sts2-rs-181`.
fn finish_turn_start_tail(state: &mut CombatState) -> StepResult {
    // Step 10: combat-end check. Hooks / turn-start auto-plays fired
    // between steps 5-9 may have downed the last enemy. Use the FULL
    // `check_combat_end` (not a bare `is_over` read): a turn-start
    // auto-play (Mayhem) can kill the last PRIMARY enemy while a
    // SECONDARY minion is still alive — e.g. FOGMOG's EyeWithTeeth,
    // revived to full on the prior enemy turn (`IllusionPower`), lingering
    // after a Mayhem-auto-played attack kills FOGMOG. Only
    // `check_combat_end`'s minion-collapse (`sts2-rs-jjwam`) resolves that
    // as victory; C# fires `CreatureCmd`'s `Kill(teammates)` on the
    // primary's death during the auto-play (`CreatureCmd.cs:392`). The old
    // cheap `is_over` read missed it, so combat wrongly continued with the
    // revived minion (gh #6, FOGMOG seeds 2351/5302). `check_combat_end`
    // is idempotent when `is_over` is already set (it short-circuits on the
    // recorded outcome without re-firing victory/defeat hooks).
    //
    // Guarded on a non-empty enemy roster: a real combat always carries at
    // least the dying primary's corpse here (corpses are retained, never
    // pruned from `enemy_uids`), so the guard is transparent in production.
    // It only suppresses the degenerate zero-enemy unit fixtures
    // (`state_for_turn_start` builds no creatures), for which an empty
    // roster would otherwise trivially resolve as victory and skip the
    // phase advance those tests assert.
    if state.is_over || !state.enemy_uids.is_empty() {
        match check_combat_end(state) {
            StepResult::Advanced => {}
            end => return end,
        }
    }

    // Step 11: advance phase.
    state.phase = CombatPhase::PlayerPlayPhase;

    // Bead `sts2-rs-ummci`: relic-side `before_play_phase_start` dispatch.
    // Fires once per player turn at the play-phase boundary (after the
    // phase flip so listeners observe `PlayerPlayPhase` and may auto-play
    // cards). `HistoryCourse` reads
    // `combat_log.prev_round_last_attack_skill_played` here — the slot the
    // turn-start shift populated from the just-ended round, so this round's
    // own turn-start auto-plays (already stamped into the live slot by the
    // Mayhem auto-play above) are correctly excluded, matching C#'s
    // `RoundNumber == round - 1` scan. Empty-bus early-out makes this free
    // when no play-phase-start relic is registered.
    let live = relic_guarded_channel_live(state, GuardedChannel::BeforePlayPhaseStart);
    let _ = dispatch_if_live(
        state,
        live,
        crate::EffectResult::NotImplemented,
        crate::relic_effects::dispatch_before_play_phase_start,
    );

    // Bead `sts2-rs-fogmog-playphase-collapse`: the `before_play_phase_start`
    // dispatch above can AUTO-PLAY a card — HISTORY_COURSE re-plays last round's
    // last Attack/Skill (CardCmd.AutoPlay) — which may kill the last PRIMARY
    // enemy while a SECONDARY minion lingers. That play resolves AFTER the
    // Step-10 `check_combat_end` (which saw the primary alive), so without a
    // re-check the minion-collapse never fires and combat wrongly continues
    // with the orphaned minion. C# `IsEnding` (`!Enemies.Any(alive &&
    // IsPrimary)`) resolves it the instant the primary dies. Re-run the FULL
    // `check_combat_end` (idempotent once `is_over` is set). FOGMOG's
    // EyeWithTeeth lingered a full extra turn after a HISTORY_COURSE
    // PerfectedStrike dupe killed FOGMOG at turn start (mid-tier
    // MONSTER_STATE|end_turn). Guarded on a non-empty roster like Step 10.
    if state.is_over || !state.enemy_uids.is_empty() {
        match check_combat_end(state) {
            StepResult::Advanced => {}
            end => return end,
        }
    }

    StepResult::Advanced
}

/// Resume the turn-start `AfterPlayerTurnStart` cluster after a
/// `StratagemPower.AfterShuffle` grid-select (stashed in
/// [`crate::PendingClusterResume`]) resolves: finish the suspended Mayhem
/// auto-play and continue the snapshot walk, without re-firing
/// `after_side_turn_start`. Consumed by `apply(Action::ResolveChoice)`.
/// Bead `sts2-rs-o0rz3`.
pub(crate) fn resume_player_turn_start_cluster(
    state: &mut CombatState,
    r: crate::PendingClusterResume,
) -> StepResult {
    if let Some(relic_start) = r.relic_start {
        // Case 3 (defect #13): the suspend happened INSIDE the relic
        // `AfterPlayerTurnStart` bus (GamblingChip's mulligan). The power walk
        // and everything before it already ran; re-enter the relic bus at the
        // stashed equip index and run the rest of the turn-start tail —
        // MercuryHourglass, Plasma, `after_side_turn_start` (Akabeko's Vigor,
        // Lantern's energy, BloodVial's heal), Bombardment, Mayhem, the phase
        // advance — exactly the batch C# resolves after the awaited pick.
        return run_turn_start_relic_tail(state, r.snapshot, relic_start, r.side_turn_start_fired);
    }
    if r.mayhem_phase {
        // Mayhem's AutoPrePlay auto-play suspended (Stratagem grid-select);
        // `after_side_turn_start` already fired before it, so resume the Mayhem
        // auto-play + tail directly (bead `sts2-rs-181`).
        run_after_player_turn_start_cluster(
            state,
            r.snapshot,
            r.idx,
            None,
            r.side_turn_start_fired,
            Some(r.autoplay),
        )
    } else {
        run_after_player_turn_start_cluster(
            state,
            r.snapshot,
            r.idx,
            Some(r.autoplay),
            r.side_turn_start_fired,
            None,
        )
    }
}

/// Run the player end-turn phase 1: fire `before_turn_end(PLAYER)` and
/// re-check combat-end before handing off to phase 2.
///
/// Mirrors the head of Python's `end_player_turn` at
/// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:937-948` (the part
/// BEFORE the orb-queue / `_resolve_end_of_turn_hand` body that
/// [`end_player_turn_phase_two`] handles, and BEFORE the
/// `_execute_enemy_turn` tail call that P3-7 (sts2-rs-e3k) wires).
///
/// **Pipeline order** (locked by the Python source-of-truth):
///   1. Combat-end short-circuit. If `state.is_over` is already set,
///      return the matching [`StepResult`] immediately — no hook fire,
///      no phase advance. Mirrors `combat.py:941-942`.
///   2. Fire `before_turn_end(PLAYER, combat)` (combat.py:944) via
///      [`hooks::dispatch_before_turn_end`] with `TurnSideCtx { side:
///      Side::Player }` (P2-2b sts2-rs-3to grew the ctx to carry side).
///   3. Combat-end re-check (combat.py:945-947). A `before_turn_end`
///      listener may have flipped `is_over` (e.g. a future Phase-7
///      thorns-on-end-turn-listener); we re-check before advancing
///      phase so the play loop doesn't observe a resolved combat in
///      `PlayerEndTurnPhaseOne`.
///   4. Advance phase `PlayerPlayPhase` -> `PlayerEndTurnPhaseOne`.
///
/// **Out of scope for P3-3 (deferred):**
///   * Orb queue `trigger_before_turn_end` (combat.py:949-955) — Phase 7.
///     No orb queue field exists on `PlayerCombatState` at this pin; the
///     Python loop body is a no-op against a `None` `orb_queue` and the
///     Rust port simply skips the loop.
///   * `_execute_enemy_turn` tail call (combat.py:975) — P3-7
///     (sts2-rs-e3k) wires `Action::EndTurn` through the full
///     phase-one -> phase-two -> enemy-turn pipeline.
/// Fire Stampede's `AfterAutoPostPlayPhaseEntered` auto-play (C#
/// `StampedePower.cs:18-34`, v0.107.1): auto-play `Amount` random playable
/// Attacks from hand off the Shuffle stream. Returns `true` when it fired
/// (the caller re-checks combat end — a lethal Stampede attack ends combat
/// inside AutoPostPlay). Extracted from `end_player_turn_phase_one` so the
/// Stampede stream/targeting tests can drive the exact production body.
pub(crate) fn run_stampede_auto_post_play(state: &mut CombatState) -> bool {
    let stampede = state
        .player
        .powers
        .iter()
        .find_map(|puid| {
            state
                .powers
                .get_by_id(puid.0)
                .filter(|p| p.kind == crate::PowerKind::Stampede)
                .map(|p| p.amount)
        })
        .or_else(|| {
            state.player.creature_uid.and_then(|cu| {
                crate::creature_by_id(&state.creatures, cu.0).and_then(|c| {
                    c.powers.iter().find_map(|pu| {
                        state
                            .powers
                            .get_by_id(pu.0)
                            .filter(|p| p.kind == crate::PowerKind::Stampede)
                            .map(|p| p.amount)
                    })
                })
            })
        });
    match stampede {
        Some(count) if count > 0 && !state.is_over => {
            crate::hooks::apply_effects(
                state,
                &[crate::hooks::Effect::AutoPlayRandomAttacksFromHand { count }],
            );
            true
        }
        _ => false,
    }
}

pub fn end_player_turn_phase_one(state: &mut CombatState) -> StepResult {
    // Step 1: combat-end short-circuit.
    if state.is_over {
        return if state.player_won {
            StepResult::CombatWon
        } else {
            StepResult::CombatLost
        };
    }

    debug_assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);

    // C# `StampedePower.AfterAutoPostPlayPhaseEntered` (StampedePower.cs:18-34,
    // v0.107.1): Stampede's random-Attack auto-play fires when the turn enters
    // `AutoPostPlay` — BEFORE `BeforeTurnEnd` and BEFORE the card-side
    // `AfterAutoPostPlayPhaseEntered` listeners (powers precede cards in
    // `IterateHookListeners`), i.e. before the Howl loop below. It previously
    // rode the `before_turn_end` bus (a Python-sim inheritance citing a
    // `BeforeTurnEndEarly` hook that does not exist in v0.107.1), which
    // re-ordered it against Plating/Doom/Hailstorm and the Howl replay —
    // 8 v28wr enemy-hp events with the mirror one `combat_targets` roll
    // AHEAD (the mirror's Stampede attack landed where C#'s bailed).
    if run_stampede_auto_post_play(state) {
        // A lethal Stampede attack ends combat inside AutoPostPlay.
        match check_combat_end(state) {
            StepResult::Advanced => {}
            end => {
                // Combat is over; nothing is owed. Don't leak the marker a
                // suspended batch may have set (`auto_play_choice_should_surface`
                // already refuses to surface once `is_over` is set, but the flip
                // can happen here rather than mid-damage).
                state.pending_end_turn_resume = None;
                return end;
            }
        }
    }

    // Bead `sts2-rs-eirvo`: the Stampede batch suspended mid-way on a surfaced
    // card-select, so C# is still inside its `await CardCmd.AutoPlay` — block
    // the whole turn transition HERE, where it blocks, not after
    // `BeforeTurnEnd`. Everything below (the rest of the batch, the Howl
    // replay, the unspent-energy / zero-block snapshots, `BeforeTurnEnd`, the
    // flush, the enemy turn) is owed to
    // [`resume_end_turn_after_auto_post_play`], which the choice resolver calls
    // once the pick lands.
    if state.pending_end_turn_resume.is_some() {
        return StepResult::PendingChoice;
    }

    end_player_turn_phase_one_after_auto_post_play_stampede(state)
}

/// The rest of [`end_player_turn_phase_one`] once `StampedePower`'s
/// `AfterAutoPostPlayPhaseEntered` batch has fully resolved: the Howl replay,
/// the turn-end snapshots, the `BeforeTurnEnd` dispatch, and the phase-one
/// tail.
///
/// Extracted (bead `sts2-rs-eirvo`) so the resume path re-enters exactly where
/// C#'s `await` returns — the batch previously ran to completion regardless of
/// a surfaced select, and everything here ran ahead of the player's answer.
fn end_player_turn_phase_one_after_auto_post_play_stampede(state: &mut CombatState) -> StepResult {
    // v0.107.1 `HowlFromBeyond.AfterAutoPostPlayPhaseEntered`: a Howl resting in
    // the EXHAUST pile auto-plays at the START of end-of-turn processing — C#
    // enters `PlayerTurnPhase.AutoPostPlay` and fires this hook BEFORE
    // `BeforeTurnEnd` (`CombatManager.cs:1168-1182`). Howl carries NO Exhaust
    // keyword (its "Exhaust" is a cosmetic hover-tip), so the auto-played copy
    // lands in DISCARD — it recurs exactly once per visit to the exhaust pile,
    // never looping. A Howl reaches the exhaust pile only via an external effect
    // (e.g. GlowwaterPotion exhausting the hand). In v0.103.2 this fired at turn
    // START alongside Bombardment; v0.107.1 retimed it to turn END. The exhaust
    // pile is snapshotted before the loop; force_exhaust=false routes the copy
    // to discard per the card's (empty) keyword set.
    {
        let howl_exhaust: smallvec::SmallVec<[crate::CardUid; 2]> = state
            .player
            .exhaust
            .iter()
            .copied()
            .filter(|uid| {
                let idx = uid.0.get() as usize - 1;
                state
                    .cards
                    .get(idx)
                    .is_some_and(|c| matches!(c.card_id, cards::CardId::HowlFromBeyond))
            })
            .collect();
        for uid in howl_exhaust {
            if state.is_over {
                break;
            }
            if let Some(pos) = state.player.exhaust.iter().position(|&u| u == uid) {
                state.player.exhaust.remove(pos);
                state.player.play.push(uid);
                resolve_auto_played_card(state, uid, /*force_exhaust=*/ false);
            }
        }
        // A lethal Howl ends combat during AutoPostPlay, before BeforeTurnEnd.
        match check_combat_end(state) {
            StepResult::Advanced => {}
            end => return end,
        }
    }

    // bead sts2-rs-ob7h: accumulate unspent energy before the end-turn
    // dispatch chain runs. Captured BEFORE `before_turn_end` so the
    // counter reflects the player's actual unspent budget — listeners
    // that mutate `player.energy` during the chain (e.g. zero-it-out
    // power hooks) don't retroactively reduce the wasted total.
    state.energy_wasted = state
        .energy_wasted
        .saturating_add(i32::from(state.player.energy.max(0)));

    // Bead `sts2-rs-xk9yo`: snapshot "player had 0 block at turn-end start"
    // BEFORE any `before_turn_end` block grant runs. C# Orichalcum /
    // FakeOrichalcum arm their grant by reading `Block == 0` in the FIRST
    // (VeryEarly) of three ordered turn-end passes, then grant in the Normal
    // pass — so an earlier-ordered grant (Plating's power block, RippleBasin's
    // relic block) does NOT cancel them. Rust runs the relic handlers AFTER
    // the power block grants in a single pass, so they must read this snapshot
    // instead of live `player.block`.
    state.combat_log.block_zero_at_turn_end = state.player.block == 0;

    // Step 2: fire before_turn_end(PLAYER). Side is explicit here (not
    // the default) because `TurnSideCtx::default()` = Player matches
    // already, but we lock the intent: this is the player-side end of
    // turn, and Phase 7 enemy-side analogues will pass `Side::Enemy`.
    hooks::dispatch_before_turn_end(state, &TurnSideCtx { side: Side::Player });
    // Bead `sts2-rs-j87`: parallel relic-effects bus drain.
    if relic_channel_live(state, PlannedChannel::BeforeTurnEnd) {
        let _ = crate::relic_effects::dispatch_before_turn_end(state, Side::Player);
    }

    // Step 3: combat-end re-check after listener walk. C# `EndPlayerTurn`
    // runs `if (await CheckWinCondition()) return;` here — the FULL
    // all-enemies-dead / player-dead evaluation, not a flag read. A
    // `before_turn_end` listener kill (TheBomb detonating on the last
    // enemy) only zeroes the creature's HP; nothing on that path flips
    // `is_over`, so the old flag-read sailed through the entire enemy
    // phase + `start_player_turn` — Rust drew a phantom next-turn hand
    // (+4 `shuffle` reads, turn/energy skew) on a combat C# had already
    // ended (coverage-sweep `card:THE_BOMB_CARD` follow-on). Bead
    // `sts2-rs-01ygn`.
    match check_combat_end(state) {
        StepResult::Advanced => {}
        end => return end,
    }

    // Parity bug `combat_select_ironclad` (Ironclad seed 3859): a
    // `before_turn_end` listener auto-play can surface an interactive
    // card-select — C# `StampedePower.AfterAutoPostPlayPhaseEntered`
    // `await CardCmd.AutoPlay`s a random hand Attack, and an auto-played
    // `SeekerStrike` then `await`s `CardSelectCmd.FromCombatPile` over the
    // draw pile INSIDE its `OnPlay` (`SeekerStrike.cs`). C#'s awaited call
    // BLOCKS the whole turn transition on that selection: the hand discard,
    // enemy turn, and next turn-start draw do NOT run until the player picks.
    // The auto-play resolver above parked its post-dispatch tail on
    // `pending_autoplay_resume` and surfaced the choice; mirror C#'s block by
    // suspending the rest of the `end_turn_dispatch` chain here. On resolution
    // `apply(Action::ResolveChoice)` finishes the suspended auto-play (moving
    // the card to discard) and runs `resume_end_turn_after_before_turn_end`,
    // which re-enters the phase-one tail (Chains clear, orb passive, advance)
    // then continues the chain. Without the suspend the chain ran to completion
    // while the choice sat pending: the auto-played card was stranded in the
    // Play pile (never reaching discard) and the next hand was drawn before the
    // select resolved — non-conservation (12→11, `SEEKER_STRIKE` vanished).
    if state.pending_choice.is_some() {
        state.pending_end_turn_resume = Some(crate::EndTurnResume::AfterBeforeTurnEnd);
        return StepResult::PendingChoice;
    }

    finish_end_player_turn_phase_one(state)
}

/// Post-`before_turn_end` tail of [`end_player_turn_phase_one`]: the
/// ChainsOfBinding `Bound`-clear sweep, the end-of-turn orb passives, and the
/// `PlayerPlayPhase -> PlayerEndTurnPhaseOne` advance. Extracted (parity bug
/// `combat_select_ironclad`) so the resume path
/// ([`resume_end_turn_after_before_turn_end`]) can run it AFTER a
/// `before_turn_end` auto-play's surfaced card-select resolves — matching C#'s
/// awaited ordering, where these steps run only once the selection completes.
fn finish_end_player_turn_phase_one(state: &mut CombatState) -> StepResult {
    // Bead `sts2-rs-qjw41` (f933s follow-up): ChainsOfBindingPower
    // `BeforeTurnEnd` — clear `Bound` from every player card. C#
    // `ChainsOfBindingPower.cs` (v0.103.2) walks
    // `player.PlayerCombatState.AllCards` and calls
    // `CardCmd.ClearAffliction(item)` on any `Bound`-afflicted card.
    // We only fire the sweep if the player currently has the power
    // (matches the C# self-listener contract — a removed power runs
    // no BeforeTurnEnd hook). Cheap early-out keeps the per-turn
    // cost flat for combats without Chains.
    // Perf 2026-07-04: `present_kinds` is a conservative superset of every
    // live power kind, so a miss proves the uid walk below could never match
    // — bit-identical skip on the (overwhelmingly common) no-Chains turn.
    let player_has_chains = state
        .powers
        .present_kinds()
        .contains(crate::powers::PowerKind::ChainsOfBinding)
        && state.player.powers.iter().any(|puid| {
            state
                .powers
                .get_by_id(puid.0)
                .is_some_and(|p| p.kind == crate::powers::PowerKind::ChainsOfBinding)
        });
    if player_has_chains {
        for card in &mut state.cards {
            if matches!(
                card.affliction,
                Some(crate::card_afflictions::CardAffliction {
                    kind: crate::card_afflictions::CardAfflictionKind::Bound,
                    ..
                })
            ) {
                let _ = card.clear_affliction();
            }
        }
    }

    // Step 3.5: end-of-turn orb passives. C# `CombatManager.EndPlayerTurn`
    // → `DoTurnEnd` fires `OrbQueue.BeforeTurnEnd` (each orb's
    // `BeforeTurnEndOrbTrigger` → `Passive`) FIRST — after the
    // `Hook.BeforeTurnEnd` walk + win check above, before the ethereal /
    // turn-end-in-hand / flush pipeline in phase two. The orb runtime
    // (`state.player.orbs`) landed with the Defect port but this trigger
    // was never wired into the loop (only EmotionChip called
    // `passive_tick`), so a CrackedCore-channeled Lightning orb sat inert
    // forever: the coverage sweep's `relic:CRACKED_CORE` showed C#'s
    // 3-damage random-enemy zap (one `combat_targets` read) every turn
    // with Rust doing nothing (monsters[1].hp cs=46 rs=49 + a
    // combat_targets cursor desync). Bead `sts2-rs-01ygn`.
    crate::orbs::passive_tick(state);

    // NOTE: there is intentionally NO combat-end re-check here. C#'s single
    // `await CheckWinCondition();` (`CombatManager.cs:963`) runs only AFTER the
    // whole `DoTurnEnd` batch (orb passive `OrbQueue.BeforeTurnEnd` at 968,
    // ethereal exhaust at 984-987, `OnTurnEndInHand` at 995) and the
    // `Hook.BeforeFlush` walk (959). Re-checking here would short-circuit
    // phase two (`end_player_turn_phase_two` Step 1, line ~1589) BEFORE the
    // ethereal-expel + turn-end-in-hand loop runs — dropping every
    // `OnTurnEndInHand` effect (DEBT gold loss, BURN/DECAY self-damage, …) on
    // any turn where an end-of-turn orb passive (e.g. a Lightning orb's zap)
    // downs the last enemy. C# always resolves those before its win check, so
    // the win must be detected LATER, by the existing check at the head of
    // `end_player_turn_phase_two_flush_tail` (line ~1996) — which lands after
    // the flush, mirroring C#'s single post-DoTurnEnd/post-BeforeFlush
    // `CheckWinCondition`. (The orb passive itself is safe to run when it
    // kills the last enemy — it just zaps and lowers HP.) Removed the Step 3.6
    // re-check that bead `sts2-rs-01ygn` added; combat still ends correctly at
    // the flush_tail check, and this is strictly more faithful to C#.

    // Step 4: advance phase.
    state.phase = CombatPhase::PlayerEndTurnPhaseOne;

    StepResult::Advanced
}

/// Resume the `end_turn_dispatch` chain suspended by
/// [`end_player_turn_phase_one`] when a `before_turn_end` listener auto-play
/// surfaced a card-select (parity bug `combat_select_ironclad`, Ironclad seed
/// 3859). Called from `apply(Action::ResolveChoice)` AFTER the surfaced select
/// resolves and the parked auto-play tail (`pending_autoplay_resume`) has
/// finished — i.e. the auto-played card has moved to its result pile. Runs the
/// remaining phase-one tail (Chains clear, orb passive, phase advance) then the
/// rest of the chain (phase two flush, the enemy turn, and the next player
/// turn-start), exactly the steps `end_turn_dispatch` would have run inline had
/// the select not suspended it. Each sub-step guards its own combat-end
/// short-circuit + phase advance, mirroring [`resume_wlp_retain_flush`].
pub(crate) fn resume_end_turn_after_before_turn_end(state: &mut CombatState) -> StepResult {
    // Finish phase one (the post-`before_turn_end` tail skipped at the suspend).
    let tail = finish_end_player_turn_phase_one(state);
    if !matches!(tail, StepResult::Advanced) {
        return tail;
    }
    run_rest_of_end_turn_chain(state)
}

/// Resume the end-turn chain suspended INSIDE `AutoPostPlay` by a Stampede
/// auto-play's surfaced card-select (bead `sts2-rs-eirvo`).
///
/// This is where C#'s `await CardCmd.AutoPlay` returns: the remaining
/// `stampede_remaining` iterations of `StampedePower`'s loop run first — each
/// re-reading a hand the resolved selection may have changed, exactly as C#
/// does — and only then does the rest of phase one (Howl replay, turn-end
/// snapshots, `BeforeTurnEnd`, the tail) and the rest of the chain follow.
///
/// A remaining iteration can surface its own select, in which case the batch
/// re-parks and we suspend again with the new remainder; C# blocks the same
/// way, once per awaited selection.
pub(crate) fn resume_end_turn_after_auto_post_play(
    state: &mut CombatState,
    stampede_remaining: u32,
) -> StepResult {
    if stampede_remaining > 0 {
        crate::hooks::auto_play_random_attacks_from_hand(state, stampede_remaining);
        // Same post-batch combat-end check the inline path runs.
        match check_combat_end(state) {
            StepResult::Advanced => {}
            end => {
                state.pending_end_turn_resume = None;
                return end;
            }
        }
        // Another iteration surfaced: block again, exactly as above.
        if state.pending_end_turn_resume.is_some() {
            return StepResult::PendingChoice;
        }
    }
    let phase_one = end_player_turn_phase_one_after_auto_post_play_stampede(state);
    if !matches!(phase_one, StepResult::Advanced) {
        return phase_one;
    }
    run_rest_of_end_turn_chain(state)
}

/// The `end_turn_dispatch` steps that follow phase one. Shared by both
/// end-turn resume entry points so a suspended chain lands in exactly the
/// state an uninterrupted one does.
fn run_rest_of_end_turn_chain(state: &mut CombatState) -> StepResult {
    type PhaseFn = fn(&mut CombatState) -> StepResult;
    const REST: &[PhaseFn] = &[
        end_player_turn_phase_two,
        enemy_turn_start,
        enemy_turn_exec,
        enemy_turn_end,
        start_player_turn,
    ];
    for step in REST {
        let result = step(state);
        if !matches!(result, StepResult::Advanced) {
            return result;
        }
    }
    StepResult::Advanced
}

/// Run the player end-turn phase 2: resolve end-of-turn hand
/// (ethereal expel + retain + flush) + per-pile `end_of_turn_cleanup`
/// + fire `after_turn_end(PLAYER)` + final combat-end check.
///
/// Mirrors Python's `_resolve_end_of_turn_hand` at
/// `combat.py:982-1047` + `_cleanup_cards_end_of_turn` at
/// `combat.py:2332-2336` + `fire_after_turn_end(PLAYER)` at
/// `combat.py:963` + the `_check_combat_end` short-circuit at
/// `combat.py:964-966`.
///
/// **Pipeline order** (locked by the Python source-of-truth, line
/// citations in parentheses):
///   1. Combat-end short-circuit (defensive — `phase_one` already
///      checked, but a re-entrant caller could land here with
///      `is_over=true`).
///   2. Ethereal expel (combat.py:992-999). Iterate hand, identify
///      every ethereal card, move it to exhaust, fire
///      `after_card_exhausted`. The predicate is sourced from the
///      engine-side ETHEREAL keyword registry
///      (`card_effects::card_ethereal`), populated by sts2-content's
///      `install_default_registry` from each `CardSpec.keywords` slice
///      (bead `sts2-rs-6ynt.3.3`). At pin `01c094d` the canonical
///      ETHEREAL cards are Dazed, Void, AscendersBane, Clumsy.
///   3. Turn-end-in-hand effects (combat.py:1001-1014). Cards like
///      BURN / BECKON execute `_execute_turn_end_in_hand_effect`
///      mid-pipeline. **Deferred to Phase 6** (sts2-rs-2un); no Phase-3
///      card has `_has_turn_end_in_hand_effect == true`, so the loop
///      body is unreachable. Stub block preserves the call-site for
///      Phase 6.
///   4. Compute retained cards (combat.py:1016-1034). Walks
///      `should_retain_this_turn` per card, then adds up to
///      `_pending_retain_count[player]` extra cards. **At this Rust
///      pin** `cards::Card` lacks the per-card `should_retain_this_turn`
///      flag and `PlayerCombatState` lacks `_pending_retain_count`;
///      the retain set is empty. Phase 6 grows both. The structural
///      call site is preserved.
///   5. Compute flush (combat.py:1036-1045). [`hooks::dispatch_should_flush`]
///      defaults true (relic-only chain — first false short-circuits).
///      When true, every non-retained non-ethereal hand card moves to
///      discard and fires `after_card_discarded`; the hand becomes the
///      retain set. When false, hand = retained ++ remaining.
///   6. End-of-turn cleanup (combat.py:2332-2336). Iterates every pile
///      and calls `card.end_of_turn_cleanup` on each card. **At this
///      Rust pin** `cards::Card` does not expose a callable
///      `end_of_turn_cleanup`; per-card per-turn flag resets
///      (`current_cost`, `Rage` extra-damage tracker, ...) live in
///      Phase 6. The structural call site is preserved as a
///      documented no-op.
///   7. Fire `after_turn_end(PLAYER, combat)` (combat.py:963).
///   8. Combat-end re-check (combat.py:964-966).
///   9. Advance phase `PlayerEndTurnPhaseOne` -> `PlayerEndTurnPhaseTwo`,
///      then -> `EnemyTurnStart` (the Python `end_player_turn` body
///      tail-calls `_execute_enemy_turn` — Rust splits this into a
///      separate phase so the play loop can observe the boundary).
///
/// **Out of scope for P3-3 (deferred):**
///   * `apply_enchantment_before_flush` (combat.py:1024) — Phase 7.
///   * `should_take_extra_turn` short-circuit (combat.py:968-973) —
///     Phase 7. No power in the current 11-power set returns true
///     for `dispatch_should_take_extra_turn`.
///   * `_execute_enemy_turn` tail call (combat.py:975) — P3-7
///     (sts2-rs-e3k).
pub fn end_player_turn_phase_two(state: &mut CombatState) -> StepResult {
    // Step 1: combat-end short-circuit.
    if state.is_over {
        return if state.player_won {
            StepResult::CombatWon
        } else {
            StepResult::CombatLost
        };
    }

    debug_assert_eq!(state.phase, CombatPhase::PlayerEndTurnPhaseOne);

    // Step 2: ethereal expel. Bead `sts2-rs-6ynt.3.3` wires the predicate
    // through the engine-side ETHEREAL keyword registry
    // (`card_effects::card_ethereal`), populated by
    // `sts2-content::cards::dispatch::install_default_registry` from each
    // per-character `CardSpec.keywords` slice. Cards never marked default
    // to `false`. Mirrors Python's `card.is_ethereal` filter at
    // `sts2-rl-agent@01c094d:sts2_env/core/combat.py:992-995`.
    //
    // Ordering: matches Python — the ethereal sweep runs BEFORE the
    // turn-end-in-hand effect loop (Step 3 below). Python excludes cards
    // that ALSO have a turn-end-in-hand effect from this initial sweep
    // (`id(card) not in turn_end_ids` at combat.py:994); at this pin
    // none of the four canonical ETHEREAL cards (Dazed, Void,
    // AscendersBane, Clumsy) carry a turn-end-in-hand effect, so the
    // exclusion is structurally empty here. Add the cross-check at the
    // same time as registering an ETHEREAL+turn-end-in-hand card.
    //
    // Bead `sts2-rs-l7k3d`: HexPower (C# `HexPower.AfterApplied`) afflicts
    // every player card with `Hexed`, which makes the card Ethereal. We
    // model the NET OBSERVABLE rather than per-card afflictions (per the
    // Tangled/Entangled precedent in `hooks.rs` — per-card afflictions are
    // not parity-tracked): while the player has an active `Hex` stack,
    // EVERY hand card is treated as Ethereal and expelled here at turn end.
    // `HexPower` is a Single-stack Debuff that persists until the applier
    // dies, so the deck progressively exhausts each turn the player ends
    // with cards in hand. Source-of-truth: `HexPower.cs`.
    // Perf 2026-07-04: `present_kinds` superset gate — no live Hex anywhere
    // proves the uid walk below could never match (bit-identical skip).
    let player_has_hex = state.powers.present_kinds().contains(crate::PowerKind::Hex)
        && state.player.powers.iter().any(|puid| {
            state
                .powers
                .get_by_id(puid.0)
                .is_some_and(|p| p.kind == crate::PowerKind::Hex && p.amount > 0)
        });

    // Collect-then-mutate avoids the borrow conflict between iterating
    // `state.player.hand` and mutating `state.player` via `move_card`.
    let ethereal: smallvec::SmallVec<[crate::CardUid; 8]> = state
        .player
        .hand
        .iter()
        .copied()
        .filter(|uid| {
            let idx = uid.0.get() as usize - 1;
            state.cards.get(idx).is_some_and(|c| {
                // Bead `sts2-rs-ad53n`: per-instance `EtherealThisCombat`
                // slot (set by MusicBox-style clone spawners) takes
                // priority over the static CardId-keyed registry so a
                // single Strike clone can be Ethereal without flipping
                // the global registry for every Strike.
                //
                // Bead `sts2-rs-59atd`: `card_ethereal_inst` also honors an
                // upgrade-aware `keyword_override` on the instance before
                // falling back to the CardId registry.
                crate::card_effects::card_ethereal_inst(c)
                    || c.combat_var(crate::cards::VarKey::EtherealThisCombat) != 0
                    || player_has_hex
            })
        })
        .collect();
    for uid in ethereal {
        // Move from hand -> exhaust. The unwrap is safe because we
        // collected the uid from the hand pile in this same scope
        // and no intervening mutation has occurred.
        crate::cards::move_card(
            &mut state.player,
            uid,
            crate::cards::PileKind::Hand,
            crate::cards::PileKind::Exhaust,
        )
        .expect("uid was just observed in hand pile");
        // Bead `sts2-rs-zlx.1`: construct the typed `CardCtx` payload up
        // front so the hooks bus and the relic-effects bus see the same
        // exhausted card identity. Both buses are drained from here.
        let exhaust_idx = uid.0.get() as usize - 1;
        let exhausted_card = &state.cards[exhaust_idx];
        let card_id = exhausted_card.card_id;
        let card_tags = exhausted_card.tags;
        let card_type = exhausted_card.card_type;
        let hook_ctx = CardCtx {
            card_id: Some(card_id),
            card_uid: Some(uid),
            card_tags,
            card_type: Some(card_type),
        };
        // Bead `sts2-rs-umouk`: this is the end-of-turn ethereal sweep, so
        // `caused_by_ethereal = true` — DarkEmbrace defers its draw to
        // AfterTurnEnd instead of drawing per-card here.
        hooks::dispatch_after_card_exhausted(state, &hook_ctx, true);
        // Bead `sts2-rs-j87`: parallel relic-effects bus drain.
        // Bead `sts2-rs-6ynt.3.3` activated the predicate above so this
        // path now fires for real ETHEREAL hand cards (Dazed, Void,
        // AscendersBane, Clumsy).
        let exhaust_ctx = crate::relic_effects::CardEventCtx {
            card_id,
            card_uid: Some(uid),
            target: None,
            card_tags,
            energy_spent: 0,
        };
        // Relic-side `caused_by_ethereal` marker (JossPaper defers its
        // exhaust credit to AfterTurnEnd; see the field's doc-comment).
        state.combat_log.exhaust_sweep_ethereal = true;
        let _ = crate::relic_effects::dispatch_after_card_exhausted(state, &exhaust_ctx);
        state.combat_log.exhaust_sweep_ethereal = false;
    }

    // Step 3: turn-end-in-hand effects (BURN / BECKON / BAD_LUCK / DEBT
    // / DECAY / DOUBT / REGRET / SHAME). Bead `sts2-rs-6ynt.3.1` ports
    // `_execute_turn_end_in_hand_effect` and the predicate
    // `_has_turn_end_in_hand_effect` from
    // `sts2-rl-agent@01c094d:sts2_env/core/combat.py:2338-2443`.
    //
    // Pipeline (mirrors Python `combat.py:988-1014`):
    //   (a) Snapshot `cards_in_hand_at_turn_end` BEFORE any other
    //       discard/exhaust step touches the hand. REGRET reads this
    //       count as its unblockable HP-loss amount; the snapshot must
    //       capture the post-ethereal-expel hand size (matches Python
    //       `len(self.hand)` evaluated after Step 2).
    //   (b) Collect every hand card whose `card_id` matches the
    //       turn-end-in-hand set into `turn_end_cards` in pile order.
    //   (c) For each card: move Hand → Play, execute the effect, then
    //       move Play → Discard and fire `after_card_discarded`. If the
    //       effect flipped `is_over` (BECKON / BAD_LUCK / REGRET can
    //       lethal the player via unblockable HP loss), short-circuit
    //       the rest of the pipeline by returning the appropriate
    //       `StepResult`. Mirrors Python's `_check_combat_end()` calls
    //       inside the BECKON / BAD_LUCK / REGRET arms.
    //
    // Documented divergences (recorded in tests/parity/known_divergences.md):
    //   * DEBT: now mutates the live combat gold mirror `state.gold`
    //     directly in `execute_turn_end_in_hand_effect` (see the
    //     `CardId::Debt` arm), matching C# `Debt.cs:26-30 OnTurnEndInHand`
    //     (`num = Min(GoldVar(10), Owner.Gold); PlayerCmd.LoseGold(num)`).
    //     The historical no-op note (gold living only on `RunState`) is
    //     obsolete: `state.gold` is the live combat mirror that propagates
    //     to `rs.gold` via the `exit_combat` writeback. Because this fires
    //     inside DoTurnEnd — BEFORE C#'s single post-DoTurnEnd
    //     `CheckWinCondition` — it must still resolve even when an
    //     end-of-turn orb passive downs the last enemy on the same
    //     turn-end (the phase-one Step 3.6 re-check that used to drop it
    //     was removed; see the note after `orbs::passive_tick`).
    //   * DOUBT / SHAME `skip_next_tick`: the `Power` POD has no
    //     `skip_next_tick` field at this pin (see `hooks.rs:2302`).
    //     The first Weak/Frail tick at the very next enemy turn-end
    //     therefore decrements the freshly-applied power by 1 — one
    //     extra tick than Python. Tracked alongside the existing
    //     `skip_next_tick` divergence note.
    let turn_end_cards: smallvec::SmallVec<[crate::CardUid; 8]> = state
        .player
        .hand
        .iter()
        .copied()
        .filter(|uid| {
            // `state.cards` is a 1-based arena (uid `1` -> index `0`). Some
            // legacy unit-test fixtures push hand UIDs without a parallel
            // `state.cards` entry; `.get()` keeps those callers panic-free.
            let idx = uid.0.get() as usize - 1;
            state.cards.get(idx).is_some_and(|card| {
                matches!(
                    card.card_id,
                    cards::CardId::Beckon
                        | cards::CardId::Burn
                        | cards::CardId::BadLuck
                        | cards::CardId::Debt
                        | cards::CardId::Decay
                        | cards::CardId::Doubt
                        | cards::CardId::Regret
                        | cards::CardId::Shame
                        | cards::CardId::Toxic
                        | cards::CardId::Infection
                        | cards::CardId::Wither
                )
            })
        })
        .collect();
    let cards_in_hand_at_turn_end: i32 = i32::try_from(state.player.hand.len()).unwrap_or(i32::MAX);
    for uid in turn_end_cards {
        // Defensive: card may have been removed from hand by a
        // listener that fired during a previous iteration's effect
        // (e.g., BECKON's after_damage_received -> some hypothetical
        // discard reaction). Mirrors Python's `if card not in self.hand`
        // guard at `combat.py:1002-1003`.
        if !state.player.hand.contains(&uid) {
            continue;
        }
        cards::move_card(
            &mut state.player,
            uid,
            cards::PileKind::Hand,
            cards::PileKind::Play,
        )
        .expect("uid was just observed in hand pile");

        // The pre-loop filter already restricted to UIDs that resolve in
        // `state.cards`; the index lookup is safe.
        let card_id = state.cards[uid.0.get() as usize - 1].card_id;
        let lost = execute_turn_end_in_hand_effect(state, uid, card_id, cards_in_hand_at_turn_end);

        // Apply any Rupture Strength the self-damage above DEFERRED. The
        // turn-end-in-hand card was moved Hand->Play before its effect, so the
        // `play.is_empty()` proxy in `dispatch_after_damage_received` mistakes
        // it for a currently-PLAYED card and defers the grant. C# never adds a
        // turn-end-in-hand card to `RupturePower.playedCards` (only
        // `BeforeCardPlayed` does), so it takes the IMMEDIATE branch while
        // CurrentSide==Player. Flush per-event to match (HEMOKINESIS — a truly
        // played card — already flushed on the play path, so nothing is owed
        // here for it). Fuzz: 4xBURN+Rupture2 gave C#10/RS2 Strength.
        flush_rupture_deferred_strength(state);

        // Combat-end short-circuit. BECKON / BAD_LUCK / REGRET each
        // call `_check_combat_end()` immediately after their HP loss
        // in Python (`combat.py:2378 / 2400 / 2434`). When the player
        // is dead we must surface that to the caller before further
        // pipeline stages (retain compute, flush, after_turn_end) run.
        if state.is_over {
            return if state.player_won {
                StepResult::CombatWon
            } else {
                StepResult::CombatLost
            };
        }
        let _ = lost;

        // Move Play -> Discard as a RAW pile move — this disposal does
        // NOT fire `after_card_discarded`. C# `CombatManager.DoTurnEnd`
        // (`MegaCrit.Sts2.Core.Combat/CombatManager.cs:1002`) disposes a
        // resolved turn-end-in-hand card via `CardPileCmd.Add(card,
        // discardPile)`, a raw pile add that invokes neither
        // `Hook.AfterCardDiscarded` nor the Sly auto-play loop (both live
        // only on the active discard path `CardCmd.DiscardAndDraw`,
        // `CardCmd.cs:144-159`). The prior `dispatch_after_card_discarded`
        // here mirrored the Python pin (`combat.py:1009-1011`) but was a
        // fabrication vs C# — exactly the same Python-pin-vs-C# mismatch
        // the end-of-turn FLUSH path was corrected for (q6bix, 532f1446).
        // It made Tingsha / Reflex (Sly) / ToughBandages wrongly fire when
        // a turn-end-in-hand curse (Infection/Burn/Decay/…) sat in hand:
        // fuzz seed 1121 (PhrogParasite INFECT → 2 Infection in hand +
        // TINGSHA) dealt 6 phantom damage (monsters[0].hp cs=27 rs=21,
        // combat_targets cursor cs=0 rs=2). No turn-end-in-hand curse is
        // ethereal at this pin, so the C# `if Ethereal { Exhaust }` branch
        // is structurally dead — the raw discard add is the only reachable
        // path. Bead `sts2-rs-p9sda` follow-up.
        if state.player.play.contains(&uid) {
            cards::move_card(
                &mut state.player,
                uid,
                cards::PileKind::Play,
                cards::PileKind::Discard,
            )
            .expect("uid was just placed in play pile");
        }
    }

    // Step 4: compute retained cards. Bead `sts2-rs-sah` (G3 plumbing)
    // wires the per-player retain side-table (`retain_this_turn`)
    // plus the stack-based budget (`pending_retain_count`).
    //
    // Pipeline (mirrors Python `combat.py:1016-1034`):
    //   (a) Drain stack-power budgets into `pending_retain_count`.
    //       `WellLaidPlans` (Silent) and `RetainHand` (shared) both
    //       call Python's `combat.request_retain(owner, N)` from their
    //       `before_turn_end` listener. Rust collapses that listener
    //       walk into a direct power scan here because per-listener
    //       hooks take `&CombatState` (read-only) and cannot mutate
    //       the budget; mirroring the C# `BeforeFlushLate` shape.
    //   (b) Build the retain set from `retain_this_turn` (explicit
    //       per-card flag set by handlers like WellLaidPlans' card
    //       body — STS2 Tactician does NOT retain itself; the bead
    //       description was conflated with STS1).
    //   (c) Add up to `pending_retain_count` extra hand cards not
    //       already in the set, in hand order. Python's cap of
    //       `len(is_retain cards) + budget` collapses to just
    //       `budget` here because the keyword-`retain` flag is not
    //       yet plumbed (deferred to a future bead alongside the
    //       enchantment/keyword retrofit).
    //   (d) The retain set is consumed in Step 5 (flush). Cleanup of
    //       both `retain_this_turn` and `pending_retain_count`
    //       happens after the flush.
    if let Some(player_uid) = state.player.creature_uid {
        let mut budget: u32 = state.player.pending_retain_count;
        // Bead `sts2-rs-4zxc0`: post-au4l2 (commit 62d8c3ff)
        // `apply_power_to_player` routes through
        // `state.creatures[player_uid].powers`; `init_player_creature`
        // also clones the legacy `state.player.powers` into
        // `creature.powers`, so the creature's list is the canonical
        // union once the d03 mirror is up. Scan creature.powers only
        // when the mirror is set — chaining both surfaces would
        // double-count any pre-init power present in both lists.
        let scan_uids: smallvec::SmallVec<[crate::PowerUid; 8]> =
            crate::creature_by_id(&state.creatures, player_uid.0)
                .map(|c| c.powers.clone())
                .unwrap_or_default();
        for power_uid in scan_uids {
            if let Some(power) = state.powers.get_by_id(power_uid.0) {
                // q6bix F-G: only WellLaidPlans is a count-budget retain
                // (C# `WellLaidPlansPower.BeforeFlushLate` selects up to
                // `Amount` cards). RetainHandPower is whole-hand
                // (`ShouldFlush == false`) and is handled by the flush gate
                // below, NOT this budget drain.
                let is_retain = matches!(power.kind, crate::PowerKind::WellLaidPlans);
                if is_retain && power.amount > 0 {
                    budget = budget.saturating_add(u32::try_from(power.amount).unwrap_or(0));
                }
            }
        }
        state.player.pending_retain_count = budget;
    }
    // (c) Well-Laid Plans interactive end-of-turn retain.
    //
    // Bead `sts2-rs-wlp-retain-select`: C#
    // `WellLaidPlansPower.BeforeFlushLate` (WellLaidPlansPower.cs:19-39)
    // opens an INTERACTIVE `CardSelectCmd.FromHand(prefs(prompt, 0, Amount),
    // filter: !card.ShouldRetainThisTurn)` and gives single-turn retain to
    // each card the player picks (`min=0` — the player may retain nothing).
    // The prior Rust port auto-retained the first `budget` hand cards in
    // hand order (a non-interactive budget walk), which desynced the next
    // round's hand vs the C# headless agent's pick (RP80 Silent
    // `combat/state/enemies_hp`; auto-clears the RP8/RP77 oracle-artifacts
    // of the same mechanism).
    //
    // `pending_retain_count` is the summed `WellLaidPlansPower` counter
    // drained in step (a) above (base 1, +1 per upgrade; it is the ONLY
    // writer of that field — every other retain source flows through the
    // `retain_this_turn` set / keyword path). Build the eligible-card
    // options (hand cards NOT already retaining this turn — mirroring
    // `RetainFilter`), and if any exist, surface a `RetainAtTurnEnd`
    // card-select and SUSPEND the flush. The chosen cards are stamped into
    // `retain_this_turn` on resolution, then `resume_wlp_retain_flush`
    // re-enters the flush tail (which rebuilds the retain set from
    // `retain_this_turn` + keyword) and the remaining `end_turn_dispatch`
    // chain. When no eligible card exists C#'s `FromHand` returns an empty
    // list and `BeforeFlushLate` early-returns, so we fall straight through
    // to the flush tail (no prompt).
    let wlp_amount = state.player.pending_retain_count;
    if wlp_amount > 0 {
        // Eligible = hand cards not already retaining this turn (explicit
        // `retain_this_turn` flag OR keyword-`Retain` instance), in hand
        // order. Matches `WellLaidPlansPower.RetainFilter`
        // (`!card.ShouldRetainThisTurn`).
        let mut options: Vec<crate::ChoiceOption> = Vec::new();
        let hand_snapshot: smallvec::SmallVec<[crate::CardUid; 16]> =
            state.player.hand.iter().copied().collect();
        for uid in hand_snapshot {
            if state.player.retain_this_turn.contains(&uid) {
                continue;
            }
            // O(1) uid-indexed arena access (uid = index + 1; the arena is
            // append-only). The `c.uid == uid` filter keeps this equivalent
            // to the previous linear `find` for any hand-built fixture.
            let idx = uid.0.get() as usize - 1;
            if let Some(card) = state.cards.get(idx).filter(|c| c.uid == uid) {
                if crate::card_effects::card_retain_inst(card) {
                    continue;
                }
                options.push(crate::ChoiceOption {
                    card_id: card.card_id,
                    upgraded: card.upgraded,
                    card_uid: Some(uid),
                    source_pile: Some(crate::cards::PileKind::Hand),
                });
            }
        }
        if !options.is_empty() {
            // Surface the interactive select and park the flush + the rest
            // of the end-turn chain. `pending_retain_count` is now fully
            // represented by the open choice, so clear it (the flush tail
            // never consults it again — retain flows through
            // `retain_this_turn`). `max_select` is clamped to the eligible
            // count by `request_card_choice` (C# `FromHand` caps Amount at
            // the candidate count too).
            #[allow(clippy::cast_possible_truncation)]
            let max_sel = wlp_amount.min(u32::from(u8::MAX)) as u8;
            state.player.pending_retain_count = 0;
            state.pending_wlp_retain_resume = true;
            state.request_card_choice(
                crate::ChoiceKind::RetainAtTurnEnd,
                options,
                /*min_select=*/ 0,
                max_sel,
                /*can_skip=*/ true,
            );
            // `request_card_choice` only opens the choice (it does NOT
            // auto-resolve a `min_select == 0` select), so a `pending_choice`
            // is now set — suspend.
            debug_assert!(state.pending_choice.is_some());
            return StepResult::PendingChoice;
        }
    }

    // No WLP select (no power, or no eligible card): run the flush tail
    // directly.
    end_player_turn_phase_two_flush_tail(state)
}

/// Flush-onward tail of [`end_player_turn_phase_two`], factored out so the
/// suspended Well-Laid Plans retain select (bead `sts2-rs-wlp-retain-select`)
/// can RESUME here after the player's pick lands. Rebuilds the per-turn retain
/// set from the explicit `retain_this_turn` flag (which now also carries any
/// WLP-chosen cards) plus the keyword-`Retain` instances, fires the
/// enchantment `before_flush` channel, performs the hand flush, the per-turn
/// card-var cleanup, the `after_turn_end` hooks/orb riders, the extra-turn
/// short-circuit, and finally advances the phase. Returns the same
/// [`StepResult`] the inlined tail used to.
fn end_player_turn_phase_two_flush_tail(state: &mut CombatState) -> StepResult {
    // Combat may have ended between the suspend point and the resume (e.g.
    // a listener that fired during a chained choice). Short-circuit.
    if state.is_over {
        return if state.player_won {
            StepResult::CombatWon
        } else {
            StepResult::CombatLost
        };
    }

    // Rebuild the retain set. (b) Explicitly-flagged cards still in hand —
    // this set now also holds any cards the player picked in the WLP retain
    // select (stamped by `resolve_retain_at_turn_end`).
    let mut retained: smallvec::SmallVec<[crate::CardUid; 16]> = smallvec::SmallVec::new();
    for uid in &state.player.retain_this_turn {
        if state.player.hand.contains(uid) && !retained.contains(uid) {
            retained.push(*uid);
        }
    }
    // (b2) Keyword-`Retain` cards still in hand. Bead `sts2-rs-o8ixl`
    // (q6bix RESTLESSNESS). C# `CardModel.ShouldRetainThisTurn` is true
    // whenever `Keywords.Contains(CardKeyword.Retain)` (CardModel.cs:476),
    // so the phase-two partition in `EndPlayerTurnPhaseTwoInternal`
    // (CombatManager.cs:1045-1055) keeps these cards IN hand while the
    // rest flush to discard — the flat turn-start draw then lands on top.
    // Resolved per-instance via `card_retain_inst` so an upgrade-applied
    // RETAIN keyword (carried on `Card::keyword_override`) is honored.
    {
        let hand_snapshot: smallvec::SmallVec<[crate::CardUid; 16]> =
            state.player.hand.iter().copied().collect();
        for uid in hand_snapshot {
            if retained.contains(&uid) {
                continue;
            }
            // O(1) uid-indexed arena access (uid = index + 1, append-only
            // arena); `c.uid == uid` filter preserves exact `find` semantics.
            let idx = uid.0.get() as usize - 1;
            if let Some(card) = state.cards.get(idx).filter(|c| c.uid == uid) {
                if crate::card_effects::card_retain_inst(card) {
                    retained.push(uid);
                }
            }
        }
    }

    // Bead `sts2-rs-6ynt.11`: enchantment `before_flush` channel.
    // Mirrors Python's `apply_enchantment_before_flush(self, owner)`
    // call at `combat.py:1024`, fired immediately before the flush
    // decision and per-card discard walk so SlumberingEssence cards
    // can tick down their cost while still in hand. Empty-bus
    // early-out is internal to the dispatcher.
    crate::enchantment_hooks::dispatch_before_flush(state);

    // Step 5: compute flush. `dispatch_should_flush` is relic-only
    // and defaults true; the first `false` from any relic listener
    // short-circuits to keep-hand semantics. At this pin no relic
    // overrides; the predicate is true.
    let mut flush_hand = hooks::dispatch_should_flush(state, &CombatLifecycleCtx::default());
    // q6bix F-G: C# `RetainHandPower.ShouldFlush(owner)` returns `false`
    // while the power is active — the WHOLE hand is retained at end of
    // turn (the power then decrements by 1 in `after_turn_end`). Use the
    // mirror+legacy union so a card/potion-applied RetainHand is seen.
    if state.player_power_amount(crate::PowerKind::RetainHand) > 0 {
        flush_hand = false;
    }
    if flush_hand {
        // Move every non-retained card from hand to discard. The
        // end-of-turn flush does NOT fire `after_card_discarded`.
        //
        // C# routes this flush through
        // `CardPileCmd.Add(list2, discardPile)` in
        // `CombatManager.EndPlayerTurnPhaseTwoInternal`, which invokes
        // **neither** `Hook.AfterCardDiscarded` nor the Sly auto-play
        // loop — both of those live only on the active discard path
        // `CardCmd.DiscardAndDraw` (`CardCmd.cs:144-159`). So Reflex
        // (Sly), Tingsha, and ToughBandages stay silent on the flush
        // and fire only on real card-effect-driven discards. The prior
        // per-card `dispatch_after_card_discarded` here (bead
        // `sts2-rs-ymlf`, mirroring the Python pin `combat.py:1039-1043`)
        // was a fabrication vs C#; removed in q6bix coverage-sweep fix
        // (TINGSHA dealt 3/enemy on a bare `end_turn`, seed 249).
        let to_discard: smallvec::SmallVec<[crate::CardUid; 16]> = state
            .player
            .hand
            .iter()
            .copied()
            .filter(|uid| !retained.contains(uid))
            .collect();
        for uid in to_discard {
            crate::cards::move_card(
                &mut state.player,
                uid,
                crate::cards::PileKind::Hand,
                crate::cards::PileKind::Discard,
            )
            .expect("uid was just observed in hand pile");
        }
        // After flush: hand is exactly the retain set. Until Phase 6
        // grows the retain flags `retained` is empty, so hand becomes
        // empty — matching Python's `self.hand = list(retained)`.
    } else {
        // No-flush path (e.g. Runic Pyramid relic in Phase 7): hand
        // reorders to `retained ++ remaining`. With `retained` empty
        // at this pin the hand stays as-is.
    }

    // Bead `sts2-rs-sah`: clear the per-turn retain state after the
    // flush has consumed it. Mirrors Python's
    // `combat.py:1047` (`self._pending_retain_count[player_id] = 0`)
    // and the per-card `single_turn_retain = False` reset in
    // `CardInstance.end_of_turn_cleanup` (`base.py:166`). The two are
    // collapsed into one block here because the side-table model
    // keeps both pieces of state on `PlayerCombatState`.
    state.player.retain_this_turn.clear();
    state.player.pending_retain_count = 0;
    // Single-turn Sly (HandTrick) expires with the turn (C#
    // `CardModel.HasSingleTurnSly = false` at turn reset).
    state.player.sly_this_turn.clear();

    // Step 6: end_of_turn_cleanup. Python iterates every pile (hand /
    // draw / discard / exhaust / play) and calls `card.end_of_turn_cleanup`
    // on each card — typically resetting `current_cost` to `cost`,
    // clearing per-turn `Rage` extra damage, etc. Bead `sts2-rs-c68o`
    // wires the first concrete clearing: every card with
    // `VarKey::FreeThisTurn != 0` (set by InfernalBlade's random Attack
    // spawn) has the flag reset here. Remaining per-turn vars (Rage
    // extra damage etc.) follow when their cards land.
    clear_per_turn_card_vars(state);

    // C# parity: EndPlayerTurnPhaseTwoInternal's Hook.AfterTurnEnd (RegenPower,
    // Ritual->Strength, ConsumingShadow orb evoke, IAmInvincible) only runs when
    // combat is still IN PROGRESS after phase one. C# gates phase two on
    // `IsInProgress` (CombatManager.cs:904) right after phase one's post-DoTurnEnd
    // CheckWinCondition (line 963). The flush/cleanup above is C#'s phase-one
    // DoTurnEnd+BeforeFlush (which always runs, incl. OnTurnEndInHand DEBT/BURN);
    // the after_turn_end powers below are phase TWO and must be skipped when an
    // end-of-turn orb passive already downed the last enemy this turn. Without
    // this, a Defect Regen power heals +N on a turn C# ended in phase one
    // (parity: Defect card_reward hp rs=cs+RegenAmount, seeds A9P72 / A5P115).
    // This is the same `is_over || !enemy_uids.is_empty()` guard + check_combat_end
    // as Step 8 below, moved earlier so the phase-two AfterTurnEnd riders never
    // fire on an orb-kill turn — exactly C#'s skipped phase-two Hook.AfterTurnEnd.
    if state.is_over || !state.enemy_uids.is_empty() {
        match check_combat_end(state) {
            StepResult::Advanced => {}
            end => return end,
        }
    }

    // Step 7: fire after_turn_end(PLAYER). Bead sts2-rs-p1n grew the
    // dispatcher's signature to `&mut state` so the synthetic
    // `on_turn_end_enemy_side` walk (which only fires on `ctx.side ==
    // Side::Enemy`) can drain its deferred-effect queue. Player-side
    // turn-end takes the `Side::Player` branch and does NOT walk the
    // synthetic guard, so this call is byte-equivalent to the prior
    // read-only dispatch.
    // Snapshot player Block BEFORE the late (`AfterSideTurnEndLate`) power
    // hooks below consume it (e.g. DisintegrationPower deals UNPOWERED damage
    // that block absorbs). C# runs the early `AfterSideTurnEnd` relic hooks
    // (Parrying Shield) before these late powers; Rust runs powers-then-relics
    // in one pass, so Parrying Shield must read this pre-late snapshot, not the
    // already-decremented live value. (sweep seed 5003.)
    state.combat_log.block_before_late_turn_end = state.player.block;
    hooks::dispatch_after_turn_end(state, &TurnSideCtx { side: Side::Player });
    // Bead `sts2-rs-j87`: parallel relic-effects bus drain.
    let live = relic_guarded_channel_live(state, GuardedChannel::AfterTurnEnd);
    let _ = dispatch_if_live(state, live, crate::EffectResult::NotImplemented, |state| {
        crate::relic_effects::dispatch_after_turn_end(state, Side::Player)
    });

    // C# `SmoggyPower.AfterSideTurnEnd` (`SmoggyPower.cs:47-62`): the owner
    // (always the player at this pin) participates in this side turn, so every
    // `Smog` card-affliction laid down by this turn's Skill play is cleared —
    // next turn's first Skill is playable again. Wired directly here rather
    // than through the declarative `after_turn_end` Effect channel because
    // clearing per-card afflictions is not expressible as an `Effect`
    // (same rationale as the ConsumingShadow orb evoke below).
    crate::monster_power_effects::smoggy_after_side_turn_end(state);

    // `ConsumingShadowPower.AfterTurnEnd` (`ConsumingShadowPower.cs`): on the
    // owner's (player's) side turn end, if the orb queue is non-empty, evoke
    // the LAST orb once per power stack (`for i in 0..Amount: OrbCmd.EvokeLast`).
    // Wired directly here (like the Plasma turn-start / IAmInvincible riders
    // above) rather than via the `after_turn_end` Effect channel, because an orb
    // evoke is a multi-step state mutation (deal/heal/block/energy + slot
    // removal) that the declarative `Effect` surface cannot express. Fires
    // AFTER `passive_tick` (phase one) has already grown each Dark orb's stored
    // evoke value, matching C#'s BeforeTurnEnd-passives-then-AfterTurnEnd-powers
    // ordering. Bead `sts2-rs-defect-consuming-shadow`.
    if !state.is_over {
        let stacks = state.player_power_amount(crate::PowerKind::ConsumingShadow);
        if stacks > 0 && state.player.orbs.slots.iter().any(|o| !o.is_empty()) {
            for _ in 0..stacks {
                if state.is_over || !state.player.orbs.slots.iter().any(|o| !o.is_empty()) {
                    break;
                }
                crate::orbs::evoke_last(state);
            }
        }
    }

    // `IAmInvincible.AfterTurnEnd` (Regent rare, `IAmInvincible.cs`): when
    // THIS card sits on top of the owner's draw pile at the player's
    // turn-end, it auto-plays the top of the draw pile (itself), gaining its
    // block. This is a per-card-instance `AfterTurnEnd` rider gated on
    // draw-pile-top; the net observable is a `while draw-top is IAmInvincible:
    // auto-play it` loop (each play moves the card to discard, exposing the
    // next copy if one happens to be beneath it). Fires here in phase two
    // AFTER the hand flush, matching C# `EndPlayerTurnPhaseTwoInternal`'s
    // `Hook.AfterTurnEnd` at `CombatManager.cs:1066`. Bead
    // `sts2-rs-8z4u4` previously DESCOPED this rider (no per-card turn-end
    // hook surface existed); the draw-pile-top gate is engine-resolvable
    // without one, so it is wired directly here.
    auto_play_i_am_invincible_from_draw_top(state);

    // Step 8: combat-end re-check. This is the Rust analogue of C#'s single
    // `await CheckWinCondition();` (`CombatManager.cs:963`) — fired ONCE,
    // AFTER the whole `DoTurnEnd` batch (orb passive + ethereal + every
    // `OnTurnEndInHand`) and the `BeforeFlush` walk (959). Use the FULL
    // `check_combat_end` (not a bare `is_over` read), because the lethal blow
    // can land deep inside this turn-end pipeline with nothing flipping
    // `is_over` on the way: an end-of-turn ORB passive (e.g. a Lightning zap)
    // downing the last enemy back in `phase_one`'s Step 3.5 only zeroes the
    // enemy's HP. The premature phase-one Step-3.6 re-check that used to flip
    // `is_over` right after the orb passive was removed (it short-circuited
    // phase two BEFORE the turn-end-in-hand loop, dropping DEBT's gold loss /
    // BURN's self-damage / … on the orb-kill turn — parity bug
    // `card_reward_gold`). Detecting the win HERE, after the flush, restores
    // the C# ordering exactly. Guarded on a non-empty roster like the
    // enemy-side checks so zero-enemy unit fixtures keep their phase-advance
    // behavior.
    if state.is_over || !state.enemy_uids.is_empty() {
        match check_combat_end(state) {
            StepResult::Advanced => {}
            end => return end,
        }
    }

    // Bead `sts2-rs-vfzgs`: extra-turn short-circuit. If any listener
    // (currently `PaelsEye`) returns true, fire `after_taking_extra_turn`
    // (which lets the relic latch its `used_this_combat` flag) and
    // re-enter the player-turn-start phase. We deliberately do NOT
    // bump `state.round` here — Python's `combat.take_extra_turn`
    // (`core/combat.py`) re-runs the player turn within the same
    // round-number; the relic's `_used_this_combat` latch + the
    // `_any_cards_played` clear at side-turn-start are sufficient to
    // prevent infinite loops (cards played on the extra turn flip the
    // bit). The exhaust-hand body lives in the relic's `before_turn_end`
    // listener (PaelsEye); this short-circuit only handles re-entry.
    if hooks::dispatch_should_take_extra_turn(state, &CombatLifecycleCtx::default()) {
        // Latch the once-per-combat flag on any relic that opted in.
        // At this pin only PaelsEye (`shop_event.py`) consumes this
        // channel; its `_used_this_combat` flag is encoded as bit 0 of
        // `Relic.counter`. The matching `should_take_extra_turn`
        // predicate (hooks.rs) checks the same bit.
        for relic in &mut state.relics {
            if relic.is_melted {
                continue;
            }
            if relic.kind == crate::RelicKind::PaelsEye {
                relic.counter |= 0b01;
            }
        }
        hooks::dispatch_after_taking_extra_turn(state, &CombatLifecycleCtx::default());
        state.phase = CombatPhase::PlayerTurnStart;
        return StepResult::Advanced;
    }

    // Step 9: advance phase to EnemyTurnStart. P3-4 (sts2-rs-x6i)
    // implements the enemy turn-start handler that consumes this
    // phase; P3-7 (sts2-rs-e3k) wires the play-loop call site that
    // dispatches `Action::EndTurn` through phase_one -> phase_two ->
    // enemy turn.
    state.phase = CombatPhase::EnemyTurnStart;

    StepResult::Advanced
}

/// Resume the suspended end-of-turn flush after a Well-Laid Plans retain
/// select resolves (bead `sts2-rs-wlp-retain-select`). Runs the flush tail
/// (which the suspend in [`end_player_turn_phase_two`] parked) and then the
/// REMAINDER of the [`end_turn_dispatch`] chain — enemy turn-start / exec /
/// end and the next player turn-start — so a `Action::EndTurn` that paused for
/// the retain pick settles back to the resting `PlayerPlayPhase` exactly like a
/// non-suspended end turn. Mirrors `end_turn_dispatch`'s tail-call order and
/// its first-non-`Advanced` short-circuit (a mid-chain combat-end or a fresh
/// turn-start `PendingChoice` propagates up). Called from
/// `resolve_choice_action` once the `RetainAtTurnEnd` choice clears.
pub(crate) fn resume_wlp_retain_flush(state: &mut CombatState) -> StepResult {
    // Finish phase two (flush, cleanup, after_turn_end, extra-turn check,
    // phase advance). May return a non-Advanced result (combat ended, or an
    // extra-turn re-entry left the phase at PlayerTurnStart).
    let tail = end_player_turn_phase_two_flush_tail(state);
    if !matches!(tail, StepResult::Advanced) {
        return tail;
    }
    // Continue the rest of the chain. `end_player_turn_phase_two` already ran;
    // `end_player_turn_phase_one` ran before the suspend. Each step guards its
    // own combat-end short-circuit and phase advance.
    type PhaseFn = fn(&mut CombatState) -> StepResult;
    const REST: &[PhaseFn] = &[
        enemy_turn_start,
        enemy_turn_exec,
        enemy_turn_end,
        start_player_turn,
    ];
    for step in REST {
        let result = step(state);
        if !matches!(result, StepResult::Advanced) {
            return result;
        }
    }
    StepResult::Advanced
}

/// Execute one card's `_execute_turn_end_in_hand_effect` body. Mirrors
/// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:2352-2443`.
///
/// Returns the HP loss applied (0 for non-damaging effects). The caller
/// uses the result for diagnostics; combat-end is observed via
/// `state.is_over` after this returns (set by `apply_damage_to_player`
/// when the player drops to 0 HP).
///
/// **Owner**: at this Rust pin the only owner is the primary player.
/// Python's `getattr(card, "owner", None) or self.primary_player`
/// degrades to `primary_player` because cards do not carry an `owner`
/// attribute in the simulator's data model — every curse in this list
/// resolves to the player. The Rust port follows suit; ally / co-op
/// curses (if they ever land) will need a per-card owner field threaded
/// through `cards::Card`.
///
/// Bead `sts2-rs-6ynt.3.1`.
fn execute_turn_end_in_hand_effect(
    state: &mut CombatState,
    card_uid: crate::CardUid,
    card_id: cards::CardId,
    cards_in_hand_at_turn_end: i32,
) -> i32 {
    use crate::card_effects::apply_power_to_creature;
    match card_id {
        // 6 unblockable HP loss (Python `combat.py:2369-2378`).
        cards::CardId::Beckon => {
            let _ = apply_damage_to_player(
                state,
                CreatureRef::Player,
                6,
                ValueProps::UNBLOCKABLE
                    .union(ValueProps::UNPOWERED)
                    .union(ValueProps::MOVE),
                Some(card_id),
            );
            6
        }
        // 2 self-damage (no UNBLOCKABLE — Python uses
        // `deal_damage(props=UNPOWERED|MOVE)` at `combat.py:2381-2389`,
        // so the player's own block can absorb).
        cards::CardId::Burn => {
            let _ = apply_damage_to_player(
                state,
                CreatureRef::Player,
                2,
                ValueProps::UNPOWERED.union(ValueProps::MOVE),
                Some(card_id),
            );
            2
        }
        // 5 self-damage, blockable. C# `Toxic.OnTurnEndInHand`
        // (Models.Cards/Toxic.cs) -> `CreatureCmd.Damage(owner,
        // DamageVar(5m, Unpowered|Move))`. No UNBLOCKABLE, so the
        // player's own block absorbs — Burn-class.
        cards::CardId::Toxic => {
            let _ = apply_damage_to_player(
                state,
                CreatureRef::Player,
                5,
                ValueProps::UNPOWERED.union(ValueProps::MOVE),
                Some(card_id),
            );
            5
        }
        // 3 self-damage, blockable. C# `Infection.OnTurnEndInHand`
        // (Models.Cards/Infection.cs) -> `CreatureCmd.Damage(owner,
        // DamageVar(3m, Unpowered|Move))`. Burn-class (no UNBLOCKABLE).
        cards::CardId::Infection => {
            let _ = apply_damage_to_player(
                state,
                CreatureRef::Player,
                3,
                ValueProps::UNPOWERED.union(ValueProps::MOVE),
                Some(card_id),
            );
            3
        }
        // 13 unblockable HP loss (Python `combat.py:2391-2400`).
        cards::CardId::BadLuck => {
            let _ = apply_damage_to_player(
                state,
                CreatureRef::Player,
                13,
                ValueProps::UNBLOCKABLE
                    .union(ValueProps::UNPOWERED)
                    .union(ValueProps::MOVE),
                Some(card_id),
            );
            13
        }
        // Lose up to 10 gold, mutating the live combat gold immediately
        // (C# `Debt.cs:26-30 OnTurnEndInHand`:
        // `int num = Mathf.Min(DynamicVars.Gold.IntValue, Owner.Gold);
        //  await PlayerCmd.LoseGold(num, Owner);`, where `GoldVar(10)`
        // gives `DynamicVars.Gold = 10` and `PlayerCmd.LoseGold`
        // (`PlayerCmd.cs:153`) sets `player.Gold = Max(0, Gold - amount)`).
        // C# fires this during turn-end resolution, so the loss is visible
        // live mid-combat; `state.gold` is the live combat mirror that
        // propagates to `rs.gold` through the `exit_combat` writeback
        // (`run_bridge.rs:416`). Mirrors the direct-mutation pattern of
        // SealOfGold (`relics/shop_event.rs`).
        cards::CardId::Debt => {
            let lost = 10.min(state.gold.max(0)); // C# Mathf.Min(GoldVar(10), Owner.Gold)
            state.gold -= lost; // C# PlayerCmd.LoseGold -> Max(0, Gold - amount)
            0
        }
        // 2 self-damage, identical shape to BURN (Python
        // `combat.py:2407-2415`).
        cards::CardId::Decay => {
            let _ = apply_damage_to_player(
                state,
                CreatureRef::Player,
                2,
                ValueProps::UNPOWERED.union(ValueProps::MOVE),
                Some(card_id),
            );
            2
        }
        // Apply WEAK 1 to owner (Python `combat.py:2417-2423`).
        // **Divergence**: Python sets `skip_next_tick = True` when the
        // owner did not previously have WEAK; the Rust `Power` POD has
        // no `skip_next_tick` field at this pin (see hooks.rs:2302).
        // Net: the freshly-applied WEAK ticks at the very next enemy
        // turn-end (one extra tick than Python). Recorded in
        // `tests/parity/known_divergences.md`.
        cards::CardId::Doubt => {
            if let Some(player_uid) = state.player.creature_uid {
                apply_power_to_creature(state, player_uid, crate::PowerKind::Weak, 1);
            }
            0
        }
        // Apply unblockable HP loss equal to the snapshot of cards in
        // hand at turn-end (Python `combat.py:2425-2434`). The snapshot
        // is taken BEFORE any other end-of-turn discard touches the
        // hand, so each REGRET in the same hand sees the same count.
        cards::CardId::Regret => {
            let _ = apply_damage_to_player(
                state,
                CreatureRef::Player,
                cards_in_hand_at_turn_end,
                ValueProps::UNBLOCKABLE
                    .union(ValueProps::UNPOWERED)
                    .union(ValueProps::MOVE),
                Some(card_id),
            );
            cards_in_hand_at_turn_end
        }
        // Apply FRAIL 1 to owner (Python `combat.py:2437-2442`). Same
        // skip_next_tick divergence as DOUBT.
        cards::CardId::Shame => {
            if let Some(player_uid) = state.player.creature_uid {
                apply_power_to_creature(state, player_uid, crate::PowerKind::Frail, 1);
            }
            0
        }
        // v0.107.1 Wither: deal `3 + 3 * FakeUpgradeLevel` self-damage,
        // blockable (Burn-class: no UNBLOCKABLE). C# `Wither.OnTurnEndInHand`
        // → `CreatureCmd.Damage(owner, DynamicVars.Damage)` where
        // `DamageVar(3, Unpowered|Move)` grows `+3` per `FakeUpgrade()`. The
        // per-instance level lives in `VarKey::WitherFakeUpgradeLevel` (set by
        // the Aeonglass boss each turn; 0 ⇒ base 3).
        cards::CardId::Wither => {
            let level = state
                .cards
                .get(card_uid.0.get() as usize - 1)
                .map_or(0, |c| c.combat_var(cards::VarKey::WitherFakeUpgradeLevel));
            let dmg = 3 + 3 * level.max(0);
            let _ = apply_damage_to_player(
                state,
                CreatureRef::Player,
                dmg,
                ValueProps::UNPOWERED.union(ValueProps::MOVE),
                Some(card_id),
            );
            dmg
        }
        // Predicate filter at the call site already restricts to the
        // turn-end-in-hand set; defensive `_ => 0` keeps the helper total.
        _ => 0,
    }
}

/// Run the enemy turn-start phase. Mirrors the head of Python's
/// `_execute_enemy_turn` at
/// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1051-1072` (the
/// portion BEFORE the per-enemy intent-perform loop that P3-5
/// (sts2-rs-hrz) implements).
///
/// **Pipeline order** (locked by the Python source-of-truth, line
/// citations in parentheses):
///   1. Combat-end short-circuit. Same defensive guard as
///      [`start_combat`] / [`start_player_turn`] — if `state.is_over`
///      is already set, return the matching [`StepResult`] immediately,
///      no hook fire / no mutation. (combat.py does not have an
///      explicit guard at the head of `_execute_enemy_turn` because the
///      caller already checked, but P3-7 will route here and a defensive
///      guard preserves the play-loop invariant.)
///   2. Set `state.side = Side::Enemy` (combat.py:1061,
///      `self.current_side = CombatSide.ENEMY`).
///   3. Fire `before_side_turn_start(ENEMY, combat)` (combat.py:1063)
///      via [`hooks::dispatch_before_side_turn_start`] with explicit
///      `TurnSideCtx { side: Side::Enemy }`.
///   4. Per alive enemy (`current_hp > 0`, mirroring Python's
///      `self.alive_enemies` property): clear block to 0
///      (combat.py:1066, `enemy.clear_block(self)`), then fire
///      `after_block_cleared(enemy, combat)` (combat.py:1067) via
///      [`hooks::dispatch_after_block_cleared`]. Index-based iteration
///      is used because the dispatcher takes `&CombatState` and would
///      otherwise borrow-conflict with the in-place block mutation.
///   5. Fire `after_side_turn_start(ENEMY, combat)` (combat.py:1069)
///      via [`hooks::dispatch_after_side_turn_start`] with explicit
///      `TurnSideCtx { side: Side::Enemy }`. This is the hook that
///      ticks ENEMY-owned Poison: `PoisonPower.after_side_turn_start`
///      (powers/common.py:485-492) fires when `owner.side == ctx.side`,
///      meaning enemy-side Poison fires on the enemy turn boundary.
///      The Effect queue inside the dispatcher (see bead sts2-rs-3to)
///      handles the actual `current_hp -= amount` + `amount -= 1` ticks.
///   6. (Python's synthetic `on_turn_start_own_side` walk: combat.py's
///      `fire_after_side_turn_start` introspects every power's method
///      table to ALSO fire `on_turn_start_own_side` for any power that
///      overrides only that arm. No power or relic has that arm, and
///      C# has no such hook, so Rust no longer walks it.)
///   7. Combat-end check (combat.py:1070-1072). A hook fired in steps
///      3-6 may have flipped `is_over` (e.g., a thorns-on-block-cleared
///      listener inflicting damage that downs the player). Re-check
///      before advancing phase.
///   8. Advance phase `EnemyTurnStart` -> `EnemyTurnExec`. P3-5
///      (sts2-rs-hrz) implements the per-enemy intent-perform loop
///      that consumes this phase.
///
/// **Out of scope for P3-4:**
///   * Per-enemy intent execution (P3-5, sts2-rs-hrz).
///   * Enemy turn-end (P3-6, sts2-rs-p1n).
///   * Ally turn-start (Phase 3 is single-primary-player only;
///     `state.allies` is unused at this pin).
pub fn enemy_turn_start(state: &mut CombatState) -> StepResult {
    // Step 1: combat-end short-circuit.
    if state.is_over {
        return if state.player_won {
            StepResult::CombatWon
        } else {
            StepResult::CombatLost
        };
    }

    debug_assert_eq!(state.phase, CombatPhase::EnemyTurnStart);

    // Step 2: set side to Enemy.
    state.side = Side::Enemy;

    // Step 3: fire before_side_turn_start(ENEMY). Side is explicit here
    // because `TurnSideCtx::default()` is `Side::Player`; we must pass
    // `Side::Enemy` to keep listener guards (e.g. Poison's
    // `owner.side == ctx.side` check) firing for enemy-side powers.
    hooks::dispatch_before_side_turn_start(state, &TurnSideCtx { side: Side::Enemy });
    // Bead `sts2-rs-j87`: parallel relic-effects bus drain.
    if relic_channel_live(state, PlannedChannel::BeforeSideTurnStart) {
        let _ = crate::relic_effects::dispatch_before_side_turn_start(state, Side::Enemy);
    }

    // Step 4: per alive enemy, clear block + fire after_block_cleared.
    // Python's `self.alive_enemies` filter is `current_hp > 0`. We
    // collect the alive-enemy indices first to avoid borrow conflicts
    // when calling the dispatcher (which holds `&CombatState`) mid-walk.
    // Index-based to match the iteration order of `enemy_uids`
    // (canonical insertion order, also used by `iter_power_listeners`).
    let alive_enemy_indices: smallvec::SmallVec<[usize; 8]> = state
        .enemy_uids
        .iter()
        .filter_map(|uid| {
            crate::creature_pos(&state.creatures, uid.0)
                .filter(|&i| state.creatures[i].current_hp > 0)
        })
        .collect();
    for idx in alive_enemy_indices {
        // Bead `sts2-rs-2e9q2`-follow-up: tag the cleared creature as this
        // enemy so the player's owner-scoped `after_block_cleared` arms
        // (BlockNextTurn / SelfFormingClay) do NOT fire on an enemy's
        // turn-start block clear (C# gates `AfterBlockCleared(creature ==
        // Owner)`). Enemy-owned block-clear listeners still fire on their
        // own clear via `owner_of_listener`.
        let ctx = hooks::BlockClearedCtx {
            cleared: Some(hooks::CreatureRef::Enemy(crate::CreatureUid(
                state.creatures[idx].id,
            ))),
        };
        // C# `Creature.ClearBlock` (`Creature.cs:728-738`) is shared by every
        // creature and asks `Hook.ShouldClearBlock` regardless of side, so
        // this takes the same path the player's clear does. Bead
        // `sts2-rs-ybtmu` hardcoded a Burrowed lookup here instead, because
        // `BlockClearedCtx` was then a placeholder that could not say WHICH
        // creature was clearing; the ctx above has carried that since, and
        // GH #646 moved this loop onto the shared walk. An enemy-owned
        // Barricade or Blur is honoured now, and the #634 Blur flash — always
        // owner-generic — is reachable from this side.
        //
        // Power bus only, unlike the player site: the relic registry's one
        // `should_clear_block` listener is SturdyClamp, which returns "clear"
        // for any `state.side != Player` (`sturdy_clamp.rs`), so consulting
        // it here could only ever permit, and its `AfterPreventingBlockClear`
        // cap would be dead code.
        match hooks::block_clear_preventer(state, &ctx) {
            // Fires on the PREVENTER alone (`Hook.cs:1176-1183`). On the power
            // bus only Blur overrides it, so Barricade and Burrowed veto
            // silently.
            Some(preventer) => hooks::dispatch_after_preventing_block_clear(state, preventer, &ctx),
            None => state.creatures[idx].block = 0,
        }
        hooks::dispatch_after_block_cleared(state, &ctx);
        // Bead `sts2-rs-1h8q` ch.2: relic-bus mirror.
        if relic_channel_live(state, PlannedChannel::AfterBlockCleared) {
            let _ = crate::relic_effects::dispatch_after_block_cleared(state, &ctx);
        }
    }

    // Step 5: fire after_side_turn_start(ENEMY). (The synthetic
    // `on_turn_start_own_side` walk is gone — see the matching comment in
    // [`start_player_turn`].)
    hooks::dispatch_after_side_turn_start(state, &TurnSideCtx { side: Side::Enemy });
    // Bead `sts2-rs-j87`: parallel relic-effects bus drain.
    if relic_channel_live(state, PlannedChannel::AfterSideTurnStart) {
        let _ = crate::relic_effects::dispatch_after_side_turn_start(state, Side::Enemy);
    }

    // Step 6: combat-end check after listener walk.
    if state.is_over {
        return if state.player_won {
            StepResult::CombatWon
        } else {
            StepResult::CombatLost
        };
    }

    // Step 7: advance phase to EnemyTurnExec. P3-5 (sts2-rs-hrz)
    // implements the per-enemy intent-perform loop that consumes
    // this phase.
    state.phase = CombatPhase::EnemyTurnExec;

    StepResult::Advanced
}

/// Run the enemy turn-exec phase: per alive enemy, perform that enemy's
/// current intent, fire `on_move_performed` (history record), check
/// combat-end. After all enemies have acted, roll the next intent for
/// each surviving enemy and advance phase `EnemyTurnExec` ->
/// `EnemyTurnEnd`.
///
/// Mirrors the per-enemy perform loop body of Python's
/// `_execute_enemy_turn` at
/// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1074-1086`:
///
/// ```python
/// for enemy in self.alive_enemies:
///     ai = self.enemy_ais[enemy]
///     move = ai.current_move
///     move.perform(self)               # MoveState.perform: state_machine.py:67-79
///     ai.on_move_performed()           # records the move into history
///     self._check_combat_end()
///     if self.is_over:
///         return
/// for enemy in self.alive_enemies:
///     self.enemy_ais[enemy].roll_move(self.rng)
/// ```
///
/// **Pipeline order** (locked by the Python source-of-truth, line
/// citations in parentheses):
///   1. Combat-end short-circuit (defensive — re-entrant guard mirroring
///      the other phase fns; combat.py's caller already guards but we
///      re-check so the play loop invariant holds).
///   2. Snapshot per-alive-enemy `(uid, intent)` pairs in `enemy_uids`
///      iteration order. We snapshot up front rather than re-querying
///      mid-loop because (a) the dispatcher walk inside
///      `apply_damage_to_player` takes `&mut CombatState`, conflicting
///      with an outer `&` walk over `state.creatures`, and (b) Python's
///      `self.alive_enemies` materializes a list at call site
///      (`combat.py:108`-style property), so snapshotting matches the
///      Python semantics (a mid-loop death does not silently reorder
///      the perform-walk).
///   3. For each `(uid, intent)`:
///       a. Perform the intent via [`perform_intent`]:
///          - `Intent::Attack { damage }`: invoke
///            [`apply_damage_to_player`] once with `dealer =
///            CreatureRef::Enemy(uid)`, `props = ValueProps::MOVE`
///            (Python's `is_powered=True`, mirroring
///            `record_fixture.py:322` default). Vulnerable on the
///            player triples the post-modify pipeline at 3/2 multiplier
///            via the existing 5if/v7d listener arms.
///          - `Intent::MultiAttack { damage, hits }`: same as Attack,
///            looped `hits` times. Mirrors Python's
///            `multi_attack_intent` which fires one
///            `combat.deal_damage` call per hit (each running the full
///            `modify_damage` / `before_damage` / block /
///            `modify_hp_lost` / `after_damage` cycle independently —
///            verified at
///            `sts2_env/monsters/intents.py::multi_attack_intent`).
///          - `Intent::Defend { block: 0 }`/`Buff`/`Debuff`/`DebuffStrong`/`Sleep`/
///            `Stun`/`StatusCard`/`Unknown`: stub no-op with a TODO
///            citing Phase-5 territory (the per-monster `effect_fn` DSL
///            at `sts2_env/monsters/act1.py` etc lands in P5).
///       b. After each hit (or each non-attack intent stub), check
///          `state.player.current_hp <= 0`. If so, mark `is_over=true`
///          + `player_won=false` and return [`StepResult::CombatLost`]
///          immediately. Mirrors Python's `_check_combat_end` at
///          `combat.py:1080-1082`.
///       c. The `ai.on_move_performed()` analogue
///          (`IntentHistory::record`) fires after every successful
///          perform — bead sts2-rs-o7r (P3-stretch) added the
///          `intent_history: IntentHistory` slot on `Creature` so the
///          per-monster move state survives across turns. Pre-o7r,
///          the next-intent roll always saw a default history and
///          Cultist collapsed to a permanent Incantation loop, `AcidSlimeS`
///          alternation broke, `JawWorm` caps were ignored, and the second
///          sentry's bolt/beam offset was lost. The record fires only
///          when the enemy survived the perform — a mid-perform
///          Phase-7 Thorns death short-circuits the record so the
///          slot stays consistent with `current_hp > 0`.
///   4. After the perform-walk: for each alive enemy, roll the next
///      intent via [`crate::monsters::MonsterKind::roll_intent`],
///      reading the persisted `creature.intent_history` (just updated
///      by step 3c). Stores the result on `creature.intent`. Mirrors
///      `combat.py:1085-1086`. Uses index-based mutation to avoid
///      borrow conflicts with the in-place `state.creatures[i].intent
///      = ...` write.
///   5. Advance phase `EnemyTurnExec` -> `EnemyTurnEnd`. P3-6
///      (sts2-rs-p1n) consumes the `EnemyTurnEnd` phase.
///
/// **Out of scope for P3-5 (deferred):**
///   * Enemy `gain_block` / `Defend` intent block field — Phase 5
///     (`sts2_env/monsters/intents.py::defend_intent`).
///   * Buff/Debuff/Status/Sleep/Stun intent effects — Phase 5
///     `effect_fn` DSL.
///   * `flush_pending_attack_context` (state_machine.py:69-71, 74-76)
///     — Phase 7.
///   * Enemy ability sequences (`follow_up_id` chains in `MoveState`)
///     — Phase 5 monster DSL.
/// C# retains a dead Decimillipede segment inside the enemy walk:
/// `ReattachPower.ShouldCreatureBeRemovedFromCombatAfterDeath` returns
/// `false` (`ReattachPower.cs:84-91`) and neither
/// `CombatManager.ExecuteEnemyTurn` (`CombatManager.cs:857-889`) nor
/// `Creature.TakeTurn` (`Creature.cs:597-607`) gates on `IsDead`, so the
/// corpse performs its DEAD no-op and later the REATTACH heal **at its own
/// roster slot**, and `StartTurn`'s `PrepareForNextTurn` rolls its move
/// machine like any other enemy. This predicate admits exactly that corpse —
/// armed onto DEAD/REATTACH by `arm_reattach_dead_state` — into the acting
/// snapshot, the on-move-performed walk, and the next-intent roll. Every
/// other corpse (the `c8why` retention class) keeps the `hp > 0` behavior.
/// Bead `sts2-rs-8lwnq`.
pub(crate) fn is_acting_reattach_corpse(state: &CombatState, c: &crate::Creature) -> bool {
    c.current_hp <= 0
        && c.ai
            .as_ref()
            .is_some_and(|ai| matches!(ai.current_state_id(), "DEAD" | "REATTACH"))
        && state
            .powers
            .present_kinds()
            .contains(crate::PowerKind::Reattach)
        && c.powers.iter().any(|puid| {
            state
                .powers
                .get_by_id(puid.0)
                .is_some_and(|p| p.kind == crate::PowerKind::Reattach)
        })
}

pub fn enemy_turn_exec(state: &mut CombatState) -> StepResult {
    // Step 1: combat-end short-circuit.
    if state.is_over {
        return if state.player_won {
            StepResult::CombatWon
        } else {
            StepResult::CombatLost
        };
    }

    debug_assert_eq!(state.phase, CombatPhase::EnemyTurnExec);

    // Step 2: snapshot alive-enemy (uid, intent) pairs. We collect into a
    // SmallVec to release the immutable borrow on `state.creatures` and
    // `state.enemy_uids` before the mutable per-enemy perform loop.
    // Iteration order matches `enemy_uids` (canonical insertion order,
    // also used by `iter_power_listeners`).
    let acting_enemies: smallvec::SmallVec<[(CreatureUid, Option<Intent>); 8]> = state
        .enemy_uids
        .iter()
        .filter_map(|uid| {
            crate::creature_by_id(&state.creatures, uid.0)
                .filter(|c| c.current_hp > 0 || is_acting_reattach_corpse(state, c))
                .and_then(|c| match c.intent {
                    Some(i) => Some((*uid, Some(i))),
                    // A move can legitimately telegraph nothing: C# `MoveState`
                    // takes `params AbstractIntent[]` and `DEAD_MOVE` passes
                    // none, yet `Creature.TakeTurn` performs it anyway — that
                    // no-op turn is what carries the corpse from DEAD to
                    // REATTACH. Admit it on the strength of
                    // `is_acting_reattach_corpse`, not on a fabricated
                    // placeholder intent (which would then leak onto the wire).
                    // An ordinary intent-less creature has simply not rolled a
                    // move yet and is correctly skipped.
                    None if is_acting_reattach_corpse(state, c) => Some((*uid, None)),
                    None => None,
                })
        })
        .collect();

    // Step 2.5: IllusionPower revive (sts2-rs-vvi6g). A creature that
    // died while carrying `IllusionPower` does NOT leave combat — C#
    // `IllusionPower.ShouldCreatureBeRemovedFromCombatAfterDeath`
    // returns false for the owner, and `AfterDeath` arms a `REVIVE_MOVE`
    // (HealIntent) that the illusion performs on its next turn, healing
    // to full (`ReviveMove → Heal(MaxHp - CurrentHp)`). See
    // ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Powers/IllusionPower.cs.
    //
    // We detect the reviving state as `has IllusionPower && current_hp
    // <= 0`: IllusionPower is a Buff (not stripped on death), so a
    // 0-HP illusion is exactly the reviving window; healing to MaxHp
    // closes it. The revive consumes the illusion's turn — it was at 0
    // HP when `acting_enemies` was snapshotted above, so it is excluded
    // from the normal perform loop and does NOT also perform its DISTRACT
    // intent this turn (matching `REVIVE_MOVE`'s `MustPerformOnce-
    // BeforeTransitioning`). The revived illusion re-rolls its follow-up
    // intent in step 4 below (now `current_hp > 0`).
    //
    // Slot order: reviving illusions are front-slot (FOGMOG's
    // EyeWithTeeth is slot 0), so reviving before the perform loop keeps
    // the illusion acting ahead of its summoner. The MinionPower
    // `ShouldOwnerDeathTriggerFatal=false` combat-end nuance (a primary
    // death ending combat while a revived minion lives) IS now modeled:
    // when a later turn-start auto-play (Mayhem) kills FOGMOG, the
    // player-turn-start `check_combat_end` at Step 10 of
    // `run_after_player_turn_start_cluster` runs the minion-collapse
    // (`sts2-rs-jjwam`), killing the lone surviving illusion and resolving
    // victory — matching C# `Kill(teammates)` (gh #6, seeds 2351/5302).
    // Perf gate: skip the corpse scan entirely when no Illusion power is live
    // (bit-identical — the filter below could never match).
    if state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Illusion)
    {
        let reviving_indices: smallvec::SmallVec<[usize; 4]> = state
            .enemy_uids
            .iter()
            .filter_map(|uid| {
                state.creatures.iter().position(|c| {
                    c.id == uid.0
                        && c.current_hp <= 0
                        && c.powers.iter().any(|puid| {
                            state
                                .powers
                                .get_by_id(puid.0)
                                .is_some_and(|p| p.kind == crate::PowerKind::Illusion)
                        })
                })
            })
            .collect();
        for idx in reviving_indices {
            state.creatures[idx].current_hp = state.creatures[idx].max_hp;
        }
    }

    // Step 2.6: AdaptablePower revive (TestSubject — bead `sts2-rs-jnf4a`).
    // The same shape as the IllusionPower revive above, but the heal is to
    // the NEXT FORM's HP (not max) and adds the form-transition powers. C#
    // does NOT revive at death — `AdaptablePower` keeps the corpse (hp 0)
    // and `TriggerDeadState` arms `RESPAWN_MOVE`, which performs on the
    // creature's next enemy turn (this pass). We detect the pending state as
    // `has Adaptable && current_hp <= 0` (Adaptable is a Buff, removed only
    // on the 2nd revive, so a 0-HP Adaptable carrier is exactly the
    // awaiting-RESPAWN window). The handler (content
    // `act3_glory::adaptable_after_death`) sets the form HP / powers and
    // advances the move machine onto RESPAWN → REVIVE_BRANCH so the
    // post-revive moveset telegraphs on the next intent roll. Like the
    // illusion, the corpse was excluded from `acting_enemies` (hp 0 at the
    // snapshot), so reviving here consumes its turn — it does NOT also attack
    // — and `enemy_turn_end::roll_next_enemy_intents` rolls its next intent
    // (now `current_hp > 0`).
    // Perf gate: no live Adaptable power ⇒ the filter can never match; skip
    // the corpse scan (bit-identical, collect degenerates to empty).
    let adaptable_corpse_uids: smallvec::SmallVec<[CreatureUid; 4]> = if state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Adaptable)
    {
        state
            .enemy_uids
            .iter()
            .filter(|uid| {
                state.creatures.iter().any(|c| {
                    c.id == uid.0
                        && c.current_hp <= 0
                        && c.powers.iter().any(|puid| {
                            state
                                .powers
                                .get_by_id(puid.0)
                                .is_some_and(|p| p.kind == crate::PowerKind::Adaptable)
                        })
                })
            })
            .copied()
            .collect()
    } else {
        smallvec::SmallVec::new()
    };
    for uid in adaptable_corpse_uids {
        if let Some(handler) = crate::enemy_death_effects::lookup(crate::PowerKind::Adaptable) {
            handler(state, uid);
        }
    }

    // Step 3+: the perform walk, the `on_move_performed` walk and the phase
    // advance live in [`run_enemy_perform_walk`] so a mid-turn blocking player
    // choice (KnowledgeDemon CURSE_OF_KNOWLEDGE) can suspend and resume the
    // walk at the monster it stopped on.
    run_enemy_perform_walk(state, acting_enemies, 0)
}

/// The per-enemy perform walk of [`enemy_turn_exec`], resumable at
/// `start_index`.
///
/// `acting` is the `(uid, intent)` roster snapshot taken at `enemy_turn_exec`
/// entry (see its Step-2 rationale for why it is snapshotted rather than
/// re-queried). Monsters `[0, start_index)` have already performed; the walk
/// resumes at `start_index` and, once every monster has acted, runs the
/// `on_move_performed` walk over the WHOLE snapshot and advances the phase to
/// `EnemyTurnEnd`.
///
/// **Mid-walk suspension.** C#'s monster move bodies are `async Task`s that can
/// `await` a blocking player choice: `KnowledgeDemon.CurseOfKnowledge`
/// (`KnowledgeDemon.cs:149-189`) awaits
/// `CardSelectCmd.FromChooseACardScreen(new BlockingPlayerChoiceContext(), …)`
/// mid-move, which halts the entire enemy turn until the player picks. When a
/// performed move leaves such a choice open, we park the remainder of the walk
/// on [`CombatState::pending_enemy_exec_resume`] and return
/// [`StepResult::PendingChoice`]; `apply(Action::ResolveChoice)` re-enters via
/// [`resume_enemy_turn_exec`]. The suspend is deliberately keyed on the ONE
/// choice kind a monster move can raise today rather than a blanket
/// `pending_choice.is_some()`, so no existing (non-monster) mid-enemy-turn
/// choice changes timing.
///
/// The suspend check sits after the per-monster tail (intent-history record +
/// `after_death` dispatch) rather than immediately after `perform_intent`:
/// both are inert bookkeeping for the curse move (it deals no damage and the
/// history is not observable on the wire), and the AI state machine — which IS
/// observable, via the telegraphed intent — is only advanced by the
/// `on_move_performed` walk BELOW the loop, so a suspended state still reports
/// the curse move as the monster's current move exactly like C#.
fn run_enemy_perform_walk(
    state: &mut CombatState,
    acting_enemies: smallvec::SmallVec<[(CreatureUid, Option<Intent>); 8]>,
    start_index: usize,
) -> StepResult {
    // Step 3: per alive enemy, perform their current intent; check
    // combat-end after each perform. The `perform_intent` helper
    // returns `CombatLost` if the player dropped to 0 HP mid-perform;
    // the outer loop short-circuits in that case (matching Python's
    // `if self.is_over: return` at combat.py:1081-1082).
    //
    // Bead sts2-rs-o7r (P3-stretch): after a successful perform, record
    // the executed intent into `creature.intent_history` BEFORE the
    // step-4 next-intent roll. This is the analogue of Python's
    // `ai.on_move_performed()` (state_machine.py:67-71) which advances
    // the per-monster move state machine. Without this record, the
    // step-4 roll sees a `last_kind=None` history every turn and
    // multi-state monsters collapse to their turn-0 intent forever.
    let mut slot = start_index;
    while slot < acting_enemies.len() {
        let (enemy_uid, intent) = acting_enemies[slot];
        slot += 1;
        let result = perform_intent(state, enemy_uid, intent);
        if matches!(result, StepResult::CombatWon | StepResult::CombatLost) {
            // Even on combat-end, the just-performed intent's history
            // bookkeeping is moot — combat won't roll another intent.
            return result;
        }
        // Defensive: a future before/after_damage_received listener
        // (e.g. Phase-7 reflective Thorns from the player back at the
        // enemy) could flip is_over via a non-player-death path. The
        // explicit re-check matches `combat.py:1080-1082`.
        if state.is_over {
            return if state.player_won {
                StepResult::CombatWon
            } else {
                StepResult::CombatLost
            };
        }
        // Record on the enemy that just acted (if it survived — a
        // mid-perform Thorns/poison death is a Phase-7 corner case but
        // we guard defensively). Looking up by uid mirrors how step 4
        // re-resolves; the enemy's perform_intent call already used
        // `&mut state` so we re-find here without a borrow conflict.
        if let Some(idx) = crate::creature_pos(&state.creatures, enemy_uid.0)
            .filter(|&i| state.creatures[i].current_hp > 0)
        {
            if let Some(i) = intent {
                state.creatures[idx].intent_history.record(i);
            }
        }

        // Bead sts2-rs-79tv9 (dvqa1.2): cross-creature AfterDeath
        // dispatch. See [`dispatch_after_death_listeners`] doc-comment
        // for the per-`(dead, witness)` iteration semantics.
        dispatch_after_death_listeners(state);

        // The move just performed opened a BLOCKING player card screen
        // (KnowledgeDemon CURSE_OF_KNOWLEDGE — the only monster move that
        // does at this pin). C# `await`s it inside the move body, so park the
        // rest of the enemy turn and surface the choice; `apply(
        // Action::ResolveChoice)` re-enters at [`resume_enemy_turn_exec`].
        if state
            .pending_choice
            .as_ref()
            .is_some_and(|pc| pc.kind == crate::ChoiceKind::MonsterCurseSelect)
        {
            // Cloned rather than moved: the walk is inside a loop, so the
            // snapshot must stay live for the (possible) next iteration and
            // for the `on_move_performed` walk below. Only paid on suspend.
            state.pending_enemy_exec_resume = Some(crate::PendingEnemyExecResume {
                acting: acting_enemies.clone(),
                next_index: slot,
            });
            return StepResult::PendingChoice;
        }
    }

    // Bead sts2-rs-iod: For DSL-driven enemies (those with a
    // `MonsterAi`), record the just-performed move into `ai.state_log`
    // so the upcoming step-4 roll sees the correct history. Mirrors
    // Python's `state_machine.on_move_performed()` at
    // `sts2-rl-agent@01c094d:sts2_env/monsters/state_machine.py:67-71`,
    // called from `combat.py` between the perform loop and the next
    // `roll_move`. Without this, `ai.current` and `ai.state_log` stay
    // frozen after the first turn and Rust diverges from Python from
    // turn 1 onwards. Collect indices first to avoid the same borrow
    // conflict the step-4 loop guards against.
    //
    // Bead `sts2-rs-z4jsg`: walk ONLY the enemies that actually
    // performed (the `acting_enemies` snapshot), not every alive
    // ai-driven enemy. A monster summoned mid-turn (Ovicopter's
    // ToughEggs, Fabricator bots, Fogmog's illusion) has performed
    // nothing, and a phantom `on_move_performed` both poisons its
    // `state_log` and sets `performed_first_move` — which makes the
    // step-4 roll TRANSITION AWAY from its never-performed initial
    // move (the eggs skipped HATCH_MOVE straight to NIBBLE). C# only
    // advances the machine from `PerformMove` → `OnMovePerformed`.
    let ai_indices: smallvec::SmallVec<[usize; 8]> = acting_enemies
        .iter()
        .filter_map(|(uid, _)| {
            crate::creature_pos(&state.creatures, uid.0).filter(|&i| {
                let c = &state.creatures[i];
                (c.current_hp > 0 || is_acting_reattach_corpse(state, c)) && c.ai.is_some()
            })
        })
        .collect();
    for idx in ai_indices {
        if let Some(ai) = state.creatures[idx].ai.as_mut() {
            ai.on_move_performed();
        }
    }

    // Step 4: roll the next intent for each surviving enemy. We collect
    // the alive-enemy creature indices first to avoid borrow conflicts
    // when calling `roll_intent(&mut state.rng, ...)` mid-walk
    // (`MonsterKind::roll_intent` mutably borrows the RNG which lives
    // on `state.rng`, conflicting with an outer `&state.creatures`
    // borrow).
    //
    // Bead sts2-rs-o7r (P3-stretch): the legacy-roll path reads from
    // `creature.intent_history` (just updated by the step-3 record).
    // Pre-fix this was `IntentHistory::default()`, which collapsed
    // Cultist Incantation→Attack to a permanent Incantation loop and
    // broke AcidSlimeS Tackle/Lick alternation, JawWorm repeat caps,
    // GreenLouse bite-cap, and Sentry phase tracking.
    //
    // Bead sts2-rs-iod: for DSL-driven enemies (`creature.ai.is_some()`)
    // route the next-intent roll through `MonsterAi::roll_move_with_combat`
    // — matching the existing `start_combat`/`monsters.rs` callsites
    // (combat-aware variant for parity with Python's transition rules,
    // e.g. JawWorm enrages on player block). The legacy
    // `MonsterKind::roll_intent` arm remains the default for
    // `ai.is_none()` enemies (Cultist / JawWorm / AcidSlimeS /
    // GreenLouse / Sentry — the Phase-3 hand-rolled set). The seed is
    // drawn from `state.rng` *before* the AI is taken so the
    // `&mut state.rng` borrow ends before the immutable `&state` borrow
    // required by `roll_move_with_combat`.
    // Step 4 (was here): the next-intent roll has MOVED to `enemy_turn_end`
    // (after `after_turn_end`). C# resolves AfterTurnEnd self-damage/deaths
    // (e.g. DemisePower) BEFORE telegraphing the next move, so a creature that
    // dies at turn-end is NOT rolled — RS previously rolled it here (before
    // after_turn_end), over-reading the `monster_ai` stream by 1 per such death
    // and desyncing the rest of combat (RNG_DESYNC cluster). Bead
    // `sts2-rs-c8why` follow-up: see `roll_next_enemy_intents`.

    // Step 5: advance phase. P3-6 (sts2-rs-p1n) consumes EnemyTurnEnd.
    state.phase = CombatPhase::EnemyTurnEnd;

    StepResult::Advanced
}

/// Resume the enemy turn suspended by a monster move's blocking player choice
/// (KnowledgeDemon CURSE_OF_KNOWLEDGE). Called from
/// `apply(Action::ResolveChoice)` once the curse pick has been applied.
///
/// Finishes the parked perform walk (the monsters that had not yet acted, then
/// the `on_move_performed` walk + the `EnemyTurnExec -> EnemyTurnEnd` advance),
/// then runs the rest of what `end_turn_dispatch` still owes: `enemy_turn_end`
/// (turn-end hooks, next-intent roll, round increment) and `start_player_turn`.
/// Same shape as [`resume_end_turn_after_before_turn_end`] — each sub-step
/// guards its own combat-end short-circuit and phase advance.
///
/// A no-op returning `Advanced` if nothing is parked (defensive: the caller
/// already checks).
pub(crate) fn resume_enemy_turn_exec(state: &mut CombatState) -> StepResult {
    let Some(parked) = state.pending_enemy_exec_resume.take() else {
        return StepResult::Advanced;
    };
    let walk = run_enemy_perform_walk(state, parked.acting, parked.next_index);
    if !matches!(walk, StepResult::Advanced) {
        // Either combat ended mid-walk, or a LATER monster raised its own
        // blocking choice (a second KnowledgeDemon in one roster) and re-parked.
        return walk;
    }
    type PhaseFn = fn(&mut CombatState) -> StepResult;
    const REST: &[PhaseFn] = &[enemy_turn_end, start_player_turn];
    for step in REST {
        let result = step(state);
        if !matches!(result, StepResult::Advanced) {
            return result;
        }
    }
    StepResult::Advanced
}

/// Raise the KnowledgeDemon CURSE_OF_KNOWLEDGE card screen, if the move that
/// just resolved stamped a curse counter onto
/// [`CombatState::pending_curse_choice`].
///
/// C# `KnowledgeDemon.ChooseCurse` (`KnowledgeDemon.cs:167-189`) builds a
/// 2-card list — option 0 is always `Disintegration` (its `DisintegrationPower`
/// var set to `_disintegrationDamageValues[counter]` = 6/7/8), option 1 is the
/// counter-keyed alt `MindRot`/`Sloth`/`WasteAway` — and `await`s
/// `CardSelectCmd.FromChooseACardScreen(new BlockingPlayerChoiceContext(), …)`,
/// which blocks the whole enemy turn. The chosen token's `OnChosen()` applies
/// its power to the player; [`crate::resolve_monster_curse_select`] is the Rust
/// analogue and reads the magnitude off the same `pending_curse_choice` stamp.
///
/// Called at the tail of [`perform_intent`] so the screen opens inside the move
/// body, exactly where C# awaits it. Inert (and cheap) for every other move:
/// only the curse effect_fn ever stamps the counter.
fn open_monster_curse_select(state: &mut CombatState) {
    let Some(counter) = state.pending_curse_choice else {
        return;
    };
    if state.pending_choice.is_some() {
        return;
    }
    let alt = match counter {
        0 => crate::cards::CardId::MindRot,
        1 => crate::cards::CardId::SlothStatus,
        _ => crate::cards::CardId::WasteAway,
    };
    let mk = |card_id| crate::ChoiceOption {
        card_id,
        upgraded: false,
        card_uid: None,
        source_pile: None,
    };
    state.pending_choice = Some(crate::PendingChoice {
        kind: crate::ChoiceKind::MonsterCurseSelect,
        options: vec![mk(crate::cards::CardId::Disintegration), mk(alt)],
        min_select: 1,
        max_select: 1,
        can_skip: false,
        post_effect: None,
        // Single-pick: no sequential accumulation. Bead `sts2-rs-sopgf`.
        picked: smallvec::SmallVec::new(),
    });
}

/// Per-intent dispatcher: route `intent` to the appropriate execution
/// path. Returns `StepResult::CombatLost` if the player dropped to 0 HP
/// mid-perform; otherwise `StepResult::Advanced`.
///
/// Phase-3 scope: only `Attack` and `MultiAttack` execute real effects
/// (via [`apply_damage_to_player`]). All other variants are stubbed
/// no-ops with TODO citations to the Phase-5 `effect_fn` DSL — the
/// structural call site is preserved so Phase 5 only needs to swap the
/// stub bodies for real implementations.
/// Cross-creature AfterDeath dispatch — bead `sts2-rs-79tv9` (dvqa1.2).
///
/// Drains `state.combat_log.newly_dead_this_turn` and, for every
/// `dead` uid recorded there, walks every other alive creature and
/// fires its [`crate::monster_ai::MonsterAi::after_death`] listener
/// (if registered) once per `(dead, witness)` pair. The
/// `newly_dead_this_turn` carrier is cleared eagerly so a subsequent
/// `perform_intent` does not re-fire on the same death; the
/// `TurnEvents::reset_per_turn` clear remains as defense-in-depth.
///
/// Iteration: the witnesses are snapshotted before the first listener
/// fires, so each `(dead, witness)` pair is observed exactly once
/// even if a listener mutates the surviving-creatures set.
///
/// Use sites: called from the per-enemy `perform_intent` loop above
/// (`combat_loop.rs:1683-ish`). The player's death does not flow
/// through this carrier (player is excluded at the
/// `apply_damage_full` Step-5 recording site, bead `sts2-rs-ar9ll`).
pub(crate) fn dispatch_after_death_listeners(state: &mut CombatState) {
    if state.combat_log.newly_dead_this_turn.is_empty() {
        return;
    }
    let dead_list: smallvec::SmallVec<[CreatureUid; 4]> = state
        .combat_log
        .newly_dead_this_turn
        .iter()
        .copied()
        .collect();
    state.combat_log.newly_dead_this_turn.clear();
    for dead in dead_list {
        let listeners: smallvec::SmallVec<
            [(CreatureUid, fn(&mut CombatState, CreatureUid, CreatureUid)); 4],
        > = state
            .creatures
            .iter()
            .filter_map(|c| {
                if c.current_hp <= 0 || c.id == dead.0 {
                    return None;
                }
                let ai = c.ai.as_ref()?;
                let f = ai.after_death?;
                Some((CreatureUid(c.id), f))
            })
            .collect();
        for (owner, f) in listeners {
            f(state, dead, owner);
        }
    }
}

fn perform_intent(
    state: &mut CombatState,
    enemy_uid: CreatureUid,
    intent: Option<Intent>,
) -> StepResult {
    let dealer = CreatureRef::Enemy(enemy_uid);
    // Forensics frame (`damage_trace`): names the acting monster and its
    // telegraphed intent, so a trace row separates "the enemy hit me" from
    // "a reactive power on the enemy hit me during its move".
    let _trace = if crate::damage_trace::armed() {
        let who = state
            .creatures
            .iter()
            .find(|c| c.id == enemy_uid.0)
            .map_or_else(|| "?".to_string(), |c| c.monster_id.0.to_string());
        let kind = intent.map_or_else(|| "None".to_string(), |i| format!("{:?}", i.kind()));
        Some(crate::damage_trace::push_source(format!(
            "intent:{who}:{kind}"
        )))
    } else {
        None
    };
    // Event log (C# `MonsterPerformedMoveEntry`). Recorded BEFORE the move
    // resolves, so every hit it deals attributes to it — and, because a move
    // row resets the open-source stack, so does anything a reactive power
    // fires during the enemy's turn.
    if crate::event_log::armed(state) {
        let who = crate::event_log::LogCreature::of_uid(state, enemy_uid);
        let move_id = state
            .creatures
            .iter()
            .find(|c| c.id == enemy_uid.0)
            .and_then(|c| {
                c.ai.as_ref()
                    .map(crate::monster_ai::MonsterAi::current_state_id)
            })
            .unwrap_or("?");
        crate::event_log::record(
            state,
            crate::event_log::EventLogEntry::MonsterMove {
                monster: who,
                move_id,
            },
        );
    }
    // Python `record_fixture.py:322` defaults to `ValueProp.MOVE` for
    // attack-from-a-move damage; this is the canonical "powered attack"
    // bitflag that triggers `is_powered() == true` in the listener arms
    // (Strength / Vulnerable / Weak / Frail / Dexterity).
    let props = ValueProps::MOVE;

    // Bead `sts2-rs-k27e`: per-move effect_fn extracted up-front and
    // fired regardless of intent variant. Python's `MoveState.effect`
    // body runs once per performed move whether it's an attack
    // (FrogKnight BEETLE_CHARGE flips `_state["beetle_charged"]`
    // alongside damage delivery) or a pure side-effect (Entomancer
    // PHEROMONE_SPIT). The pnv5 wiring originally fired effect_fn only
    // for non-attack intents — that path is preserved below for the
    // intents it shipped against, but Attack / MultiAttack now also
    // dispatch effect_fn after damage so the FrogKnight CounterAware
    // gate can flip its counter.
    let (attack_effect_fn, attack_move_id) = if matches!(
        intent,
        Some(Intent::Attack { .. } | Intent::MultiAttack { .. } | Intent::DeathBlow { .. })
    ) {
        crate::creature_by_id(&state.creatures, enemy_uid.0)
            .filter(|c| c.current_hp > 0)
            .and_then(|c| {
                c.ai.as_ref()
                    .map(|ai| (Some(ai.current_move().effect), Some(ai.current_state_id())))
            })
            .unwrap_or((None, None))
    } else {
        (None, None)
    };

    // ThievingHopper THIEVERY: C# `ThieveryMove` (`ThievingHopper.cs:180-228`)
    // performs the card-STEAL (`CardPileCmd.RemoveFromCombat`) and the
    // `SwipePower` apply BEFORE `DamageCmd.Attack(TheftDamage)` at the END of
    // the move. The default Attack pipeline below fires the per-move
    // `effect_fn` AFTER damage; that ordering is wrong for the steal: the
    // TheftDamage hit is the player's first HP loss, which triggers
    // CentennialPuzzle's "draw 3 cards" — pulling Draw-pile cards into hand
    // and SHRINKING the steal candidate pool before the steal reads it (seed
    // 1390: C# steals a Draw-pile OMNISLICE from the full 7-card pool; Rust
    // drew 3 cards out first, so its pool — and thus the
    // CombatCardGeneration NextItem — diverged). Run the steal effect FIRST
    // here, then fall through to deal the damage, matching the C# order.
    let effect_before_damage =
        matches!(intent, Some(Intent::Attack { .. })) && attack_move_id == Some("THIEVERY_MOVE");
    if effect_before_damage {
        if let Some(effect) = attack_effect_fn {
            effect(state, enemy_uid);
        }
    }
    let attack_effect_fn = if effect_before_damage {
        None
    } else {
        attack_effect_fn
    };

    match intent {
        Some(Intent::Attack { damage }) => {
            let suck = attacker_suck_amount(state, enemy_uid);
            let hp_before = (suck > 0).then(|| player_side_hp(state));
            let result = apply_damage_to_player(state, dealer, damage, props, None);
            if let Some(end) = result {
                return end;
            }
            // C# `VigorPower.AfterAttack` fires when the attack command
            // completes (before the move body's next line); consume the
            // attacker's one-shot Vigor here, ahead of `effect_fn`.
            consume_attacker_vigor(state, enemy_uid);
            // C# `SuckPower.AfterAttack` fires in the same post-command
            // window; a single-hit attack has one `Results` hit-list.
            let sucked_hits =
                i32::from(hp_before.is_some_and(|before| player_side_hp(state) < before));
            apply_suck_strength(state, enemy_uid, suck, sucked_hits);
            if let Some(effect) = attack_effect_fn {
                effect(state, enemy_uid);
            }
        }
        // Bead `sts2-rs-f99e7`: `DeathBlowIntent` mirrors C#
        // `DeathBlowIntent : SingleAttackIntent` (a damage intent whose
        // move body kills the dealer after the hit lands). C#
        // GasBomb.ExplodeMove: `DamageCmd.Attack(ExplodeDamage)` then
        // `CreatureCmd.Kill(base.Creature)`. We deal damage first
        // (matching `Intent::Attack`'s pipeline so Thorns/FlameBarrier
        // reflection still fires), then mark the dealer dead with
        // `current_hp = 0` — the canonical "dead" marker at this pin
        // (Rust keeps corpses; the combat loop's alive-filter handles
        // the rest, per `feedback_dead_enemy_array_removal_divergence`).
        // Fires the per-move `effect_fn` last so any AI bookkeeping
        // (HasExploded flag in C#) runs in the same order.
        Some(Intent::DeathBlow { damage }) => {
            let suck = attacker_suck_amount(state, enemy_uid);
            let hp_before = (suck > 0).then(|| player_side_hp(state));
            let result = apply_damage_to_player(state, dealer, damage, props, None);
            if let Some(end) = result {
                return end;
            }
            consume_attacker_vigor(state, enemy_uid);
            // `SuckPower.AfterAttack` is owner-generic in C# — it would fire
            // for a DeathBlow attacker too (the dealer dies AFTER the attack
            // command completes). No current DeathBlow monster carries Suck;
            // kept for hook-parity symmetry with the `Attack` arm.
            let sucked_hits =
                i32::from(hp_before.is_some_and(|before| player_side_hp(state) < before));
            apply_suck_strength(state, enemy_uid, suck, sucked_hits);
            if let Some(effect) = attack_effect_fn {
                effect(state, enemy_uid);
            }
            if let Some(c) = state
                .creatures
                .iter_mut()
                .find(|c| c.id == enemy_uid.0 && c.current_hp > 0)
            {
                c.current_hp = 0;
            }
        }
        // Bead `sts2-rs-8f336`: `Intent::Escape` mirrors C#
        // `EscapeIntent` — the monster flees combat. C#
        // `FatGremlin.FleeMove` plays animation/SFX then
        // `CreatureCmd.Escape(base.Creature)`, which removes the
        // creature from the active enemies list. At this Rust pin
        // we keep corpses in `state.creatures` (see
        // `project_dead_enemy_array_removal_divergence` /
        // `sts2-rs-c8why`), so removal manifests as
        // `current_hp = 0` — same shape as `Intent::DeathBlow` minus
        // the damage step. Dispatches the per-move `effect_fn` first
        // (FatGremlin has none, but other future Escape-using
        // monsters might) so any AI bookkeeping runs before removal.
        Some(Intent::Escape) => {
            let effect_fn = state
                .creatures
                .iter()
                .find(|c| c.id == enemy_uid.0 && c.current_hp > 0)
                .and_then(|c| c.ai.as_ref().map(|ai| ai.current_move().effect));
            if let Some(effect) = effect_fn {
                effect(state, enemy_uid);
            }
            if let Some(c) = state
                .creatures
                .iter_mut()
                .find(|c| c.id == enemy_uid.0 && c.current_hp > 0)
            {
                c.current_hp = 0;
                // Bead `sts2-rs-RP15`: count the flee for the combat gold-reward
                // proportion (C# `CombatState.EscapedCreatures`; the reward is
                // scaled by `1 - escaped/total` at `exit_combat`). Guarded on the
                // `current_hp > 0` find so a creature can't be double-counted.
                state.escaped_creatures += 1;
            }
        }
        Some(Intent::MultiAttack { damage, hits }) => {
            // Mirrors Python's `multi_attack_intent` at
            // `sts2_env/monsters/intents.py`: one `deal_damage` call per
            // hit, each running the full `modify_damage` / block /
            // `modify_hp_lost` / `after_damage` cycle. The `hits` count
            // is a `u8` (intent.rs); we cast to a loop counter directly.
            //
            // Bead sts2-rs-v7isp: between hits, re-check that the dealer
            // is still alive. Reflective Thorns / FlameBarrier on the
            // player can kill the dealer mid-chain (2nd hit reflects
            // lethal damage back); without this guard the remaining
            // hits would land from a 0-HP dealer and the post-loop
            // `effect_fn` would fire on a dead enemy. Python guards
            // each hit on `dealer.is_alive` for the same reason.
            let suck = attacker_suck_amount(state, enemy_uid);
            let mut sucked_hits = 0_i32;
            for _ in 0..hits {
                let dealer_alive = state
                    .creatures
                    .iter()
                    .any(|c| c.id == enemy_uid.0 && c.current_hp > 0);
                if !dealer_alive {
                    break;
                }
                // C# `SuckPower.AfterAttack` walks `command.Results` — one
                // `List<DamageResult>` PER HIT — and counts the hits with any
                // `UnblockedDamage > 0`. Tally the realised per-hit HP delta
                // here; the Strength is applied ONCE after the chain, so
                // later hits of this same attack are NOT inflated.
                let hp_before = (suck > 0).then(|| player_side_hp(state));
                let result = apply_damage_to_player(state, dealer, damage, props, None);
                if let Some(end) = result {
                    return end;
                }
                if hp_before.is_some_and(|before| player_side_hp(state) < before) {
                    sucked_hits += 1;
                }
            }
            // A multi-hit attack is a single C# `AttackCommand`, so Vigor's
            // `AfterAttack` fires ONCE after the whole chain — consume here,
            // after the hit loop, before the post-attack `effect_fn`.
            consume_attacker_vigor(state, enemy_uid);
            // Same single-command timing for `SuckPower.AfterAttack`
            // (SuckPower.cs:22-47): Strength = Amount × unblocked-hit count,
            // applied once post-chain (`apply_power_to_creature`'s alive gate
            // drops it if reflective Thorns killed the dealer mid-chain, the
            // same window C#'s power-removal-on-death closes).
            apply_suck_strength(state, enemy_uid, suck, sucked_hits);
            // Re-resolve `effect_fn` against the live state: the pre-loop
            // snapshot is stale if the dealer died mid-chain. Only fire
            // when the dealer is still alive at chain end (Python
            // semantics: post-attack effects don't run from a corpse).
            let dealer_alive_at_end = state
                .creatures
                .iter()
                .any(|c| c.id == enemy_uid.0 && c.current_hp > 0);
            if dealer_alive_at_end {
                if let Some(effect) = attack_effect_fn {
                    effect(state, enemy_uid);
                }
            }
        }
        // ── Non-attack intents. ─────────────────────────────────────────
        // Defend: enemy gains `block` (applied below via `GainBlockCreature`
        //   for the primary intent — bead `sts2-rs-qk2du`).
        Some(
            Intent::Defend { .. }
            // Buff: apply self-buff (Strength, Ritual, Curl Up, etc).
            | Intent::Buff { .. }
            // Debuff: apply weak/strength-down debuff to the player.
            | Intent::Debuff { .. }
            // DebuffStrong: apply Vulnerable + Weak (e.g. SlimeBoss spawn).
            | Intent::DebuffStrong
            // Sleep: skip turn (Lagavulin pre-wake).
            | Intent::Sleep
            // Stun: skip turn (stunned via Knife / Sword Boomerang).
            | Intent::Stun
            // StatusCard: add a status card to player's discard (e.g. Slime
            //   spit, Burn from a future Heart-style enemy).
            | Intent::StatusCard
            // Unknown: telegraphed when stunned and the next move is not
            //   yet chosen. Should never actually fire as the active
            //   intent; defensive no-op.
            | Intent::Unknown
            // Summon: telegraphed when the monster spawns new enemies
            //   into the fight (Vantom's CONSUME). Effect body lives in
            //   the per-monster AI closure (Python `combat.add_enemy(...)`);
            //   the intent itself is metadata-only at this dispatch layer.
            //   Bead `sts2-rs-47p5` / dispatcher `sts2-rs-vn0m`.
            | Intent::Summon
            // Heal: telegraphed when the monster restores HP to itself
            //   or an ally (SoulNexus SOUL_DRAIN slot, DecimillipedeSegment
            //   REATTACH). Effect body lives in the per-move `effect_fn`
            //   same as Summon. Bead `sts2-rs-va5r`.
            | Intent::Heal,
        )
        // No intent at all: C# `MoveState` takes `params AbstractIntent[]`
        //   and plenty of moves pass none (`DEAD_MOVE`, the pet
        //   `NOTHING_MOVE`s). `Creature.TakeTurn` performs the body
        //   regardless, which is exactly this arm minus every
        //   intent-specific branch — the Decimillipede corpse's DEAD no-op
        //   rides here on its way to REATTACH.
        | None => {
            // Bead `sts2-rs-qk2du`: monster block-gain. `Intent::Defend
            // { block }` telegraphs a block the monster gains (C#
            // `CreatureCmd.GainBlock(creature, amount, …)` in each `*Move`);
            // previously this was a Phase-5 stub that dropped the block
            // entirely. Apply it to the dealer via the shared
            // `GainBlockCreature` effect (which runs `modify_block` and fires
            // both `after_block_gained` buses) before the per-move
            // `effect_fn`. Only a move's PRIMARY intent reaches
            // `perform_intent` (`intents.first()`), so this covers dedicated
            // Defend moves (SewerClam SHELL); a secondary Defend in a
            // multi-intent move (e.g. `[Attack, Defend]`, or Seapunk
            // BUBBLE_BURP's `[Buff, Defend]`) is still dropped by the
            // `first()`-only dispatch — a separate, pre-existing multi-intent
            // limitation. A move whose C# intent order puts Defend second
            // must therefore grant its block in the move's own `effect_fn`,
            // the way C# does (`Seapunk.BubbleBurpMove`, `Seapunk.cs:84-90`).
            if let Some(Intent::Defend { block }) = intent {
                if block > 0 {
                    hooks::apply_effects(
                        state,
                        &[hooks::Effect::GainBlockCreature {
                            target: dealer,
                            amount: block,
                        }],
                    );
                }
            }
            // Bead `sts2-rs-pnv5`: dispatch the per-move `effect_fn` for
            // non-attack intents. Python's `MoveState.effect` body applies
            // self-buffs / debuffs / summons via `creature.apply_power(...)`;
            // the Rust analogue routes through `MonsterAi::current_move().effect`
            // with the acting creature's uid threaded so the effect can
            // target itself (e.g. Entomancer PHEROMONE_SPIT applying
            // PERSONAL_HIVE or STRENGTH to self). A 0-HP Reattach corpse
            // still resolves its move body — its REATTACH heal is exactly
            // the effect that brings it back (bead `sts2-rs-8lwnq`).
            let effect_fn = crate::creature_by_id(&state.creatures, enemy_uid.0)
                .filter(|c| c.current_hp > 0 || is_acting_reattach_corpse(state, c))
                .and_then(|c| c.ai.as_ref().map(|ai| ai.current_move().effect));
            if let Some(effect) = effect_fn {
                effect(state, enemy_uid);
            }
        }
    }

    // A move body that opened a blocking player card screen raises it HERE —
    // inside the move, where C# `await`s it. Today that is only KnowledgeDemon
    // CURSE_OF_KNOWLEDGE; the helper is a cheap `Option` read otherwise. The
    // caller ([`run_enemy_perform_walk`]) then parks the rest of the enemy turn.
    open_monster_curse_select(state);

    StepResult::Advanced
}

/// Consume the attacker's `Vigor` after a powered attack resolves —
/// faithful to C# `VigorPower.AfterAttack`
/// (`MegaCrit.Sts2.Core.Models.Powers/VigorPower.cs:78-85`): once the
/// first powered attack command completes, the power removes its
/// ENTIRE amount (`ModifyAmount(-amountWhenAttackStarted)`), so Vigor
/// is a one-shot "+Amount to the next attack, then gone".
///
/// The PLAYER half is modelled as an `after_card_played` surrogate
/// (`hooks::dispatch_after_card_played`, bead `sts2-rs-6049l`), since
/// player attacks are card plays. Enemy attacks are not cards, so the
/// enemy half is consumed here after [`perform_intent`]'s
/// `Attack` / `MultiAttack` / `DeathBlow` damage resolves. Without it
/// an enemy's Vigor (e.g. the `act1_underdocks` `Vigor(6)` self-buff)
/// kept adding its `modify_damage_additive` bonus to every subsequent
/// attack instead of decaying after the first — the
/// `modify_damage_additive` arm is owner-agnostic, so the bonus
/// applied but never decayed (bead `sts2-rs-lam0b`, follow-up to
/// `sts2-rs-6049l`).
///
/// Removes the power at amount 0 (C# `ShouldRemoveDueToAmount`),
/// unlinking it from `state.powers` and every creature's `powers`
/// list — the same multi-surface prune the player arm uses. A
/// multi-hit attack is a single C# `AttackCommand`, so the caller
/// invokes this ONCE after the whole hit loop, not per hit.
fn consume_attacker_vigor(state: &mut CombatState, attacker: CreatureUid) {
    use crate::PowerKind;
    // Perf gate: no live Vigor power anywhere ⇒ nothing to consume. Skips the
    // creature find + power-uid walk on every enemy attack (bit-identical —
    // the find_map below could never match).
    if !state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Vigor)
    {
        return;
    }
    // Shared borrows only — no need to clone the attacker's power-uid list.
    let vigor_pid = state
        .creatures
        .iter()
        .find(|c| c.id == attacker.0)
        .and_then(|c| {
            c.powers.iter().copied().find_map(|uid| {
                state
                    .powers
                    .get_by_id(uid.0)
                    .filter(|p| p.kind == PowerKind::Vigor && p.amount > 0)
                    .map(|_| uid.0)
            })
        });
    let Some(pid) = vigor_pid else {
        return;
    };
    state.powers.retain(|p| p.id != pid);
    state.player.powers.retain(|u| u.0 != pid);
    for c in &mut state.creatures {
        c.powers.retain(|u| u.0 != pid);
    }
}

/// Live `Suck` amount on the attacking monster, or 0.
///
/// C# `SuckPower` (`MegaCrit.Sts2.Core.Models.Powers/SuckPower.cs:14-47`) is
/// FossilStalker's spawn buff (`FossilStalker.cs:45-49`,
/// `PowerCmd.Apply<SuckPower>(…, 3m, …)` in `AfterAddedToRoom`): a permanent
/// Counter-stack power whose `AfterAttack` grants the owner
/// `Strength = Amount × unblocked-hit-count` after the owner's own powered
/// attack against the enemy side. The three C# gates (`SuckPower.cs:24`) —
/// `command.Attacker == Owner`, `TargetSide != Owner.Side`,
/// `DamageProps.IsPoweredAttack()` — are structurally satisfied at the
/// [`perform_intent`] call sites: the attack is always the acting monster's
/// own move (`props = ValueProps::MOVE`, the powered-attack flag) against the
/// player side.
///
/// Perf-gated on `present_kinds` like [`consume_attacker_vigor`] so the
/// per-attack cost is a bitset test when no Suck power is in play.
fn attacker_suck_amount(state: &CombatState, attacker: CreatureUid) -> i32 {
    use crate::PowerKind;
    if !state.powers.present_kinds().contains(PowerKind::Suck) {
        return 0;
    }
    state
        .creatures
        .iter()
        .find(|c| c.id == attacker.0)
        .map_or(0, |c| {
            c.powers
                .iter()
                .filter_map(|uid| state.powers.get_by_id(uid.0))
                .filter(|p| p.kind == PowerKind::Suck)
                .map(|p| p.amount)
                .sum()
        })
}

/// Player-side realised HP for `SuckPower`'s per-hit "unblocked" test:
/// the player's HP plus the Osty pet's (when summoned).
///
/// C# counts a hit when any `DamageResult` in that hit's list has
/// `UnblockedDamage > 0`, where `UnblockedDamage = currentHp - CurrentHp`
/// (`Creature.LoseHpInternal`, Creature.cs:382) — the ACTUAL HP lost,
/// post-block AND post-`modify_hp_lost`, so a fully-blocked or
/// Buffer-absorbed hit does NOT count (same realised-delta rule as bead
/// `sts2-rs-xsvp9`'s TearAsunder/Spite counters). A before/after snapshot of
/// this sum reproduces that test exactly. Including the pet mirrors
/// `SuckPower.cs:30-36`: when a hit lands on a pet, C# drops only the
/// pet-OWNER's duplicate results, so the hit still counts through the pet's
/// own `UnblockedDamage` — i.e. an Osty-redirected hit that costs the pet HP
/// is a counted hit.
fn player_side_hp(state: &CombatState) -> i32 {
    let mut hp = state.player.current_hp;
    if let Some(pet) = state.player.pet_uid {
        if let Some(c) = state.creatures.iter().find(|c| c.id == pet.0) {
            hp += c.current_hp;
        }
    }
    hp
}

/// Post-attack half of C# `SuckPower.AfterAttack` (`SuckPower.cs:42-46`):
/// `if (num > 0) { … PowerCmd.Apply<StrengthPower>(…, Amount * num, …) }` —
/// one Strength apply per ATTACK COMMAND, after the whole hit chain, never
/// per hit (so a multi-hit attack cannot inflate its own later hits).
/// [`crate::card_effects::apply_power_to_creature`]'s alive gate mirrors the
/// owner-death window (a dealer killed mid-chain by reflective damage has its
/// powers removed in C# before the apply could land).
fn apply_suck_strength(
    state: &mut CombatState,
    attacker: CreatureUid,
    suck_amount: i32,
    unblocked_hits: i32,
) {
    if suck_amount > 0 && unblocked_hits > 0 {
        // C# `SuckPower.cs:44` flashes inside this same `num > 0` guard,
        // before the Strength grant.
        crate::event_log::flash_power(state, attacker, crate::PowerKind::Suck);
        crate::card_effects::apply_power_to_creature(
            state,
            attacker,
            crate::PowerKind::Strength,
            suck_amount * unblocked_hits,
        );
    }
}

/// Clear the player's powers on death, mirroring C# `CreatureCmd`'s generic
/// post-death teardown: `creature.RemoveAllPowersAfterDeath()`
/// (`CreatureCmd.cs:385`) runs for the dying creature including the player
/// (`creature.IsPlayer`, `:397`), dropping every power whose
/// `ShouldPowerBeRemovedAfterOwnerDeath()` is true. That predicate defaults to
/// `true` (`PowerModel.cs:515`) and NO *player*-applicable power overrides it —
/// the only survivors (Minion / Reattach / DieForYou / PainfulStabs /
/// Adaptable / SteamEruption) are monster powers — so on player defeat C#
/// leaves `player.powers == []`. Rust previously retained them.
///
/// Root cause of the coverage-sweep `player.powers.len 1 vs 0` drift
/// (bead `sts2-rs-2e9q2`): in the SlitheringStrangler arena the player dies to
/// Constrict's end-of-turn self-damage and C# clears the lingering powers. This
/// is a player-death teardown gap, NOT a Constrict-lifetime / applier-death bug
/// (the applier is alive at the time).
///
/// Clears all three power surfaces — the central [`crate::Power`] rows, the
/// legacy `state.player.powers` list, and the player mirror creature's `powers`
/// — mirroring [`crate::damage::remove_constrict_from_dead_applier`]'s
/// multi-surface unlink. Monster corpses' own powers are intentionally NOT
/// touched here: Rust retains them under `removed_after_death` for stable NN
/// feature slots (bead `sts2-rs-c8why`).
///
/// Called from every combat player-defeat site: [`apply_damage_to_player`] and
/// [`check_combat_end`] here, plus the reflective / self-damage HP-loss branch
/// in [`crate::hooks::apply_effect`] (`apply_dealhploss`) which is where the
/// Constrict/Poison/Thorns end-of-turn self-damage actually zeroes the player.
pub(crate) fn clear_player_powers_on_death(state: &mut CombatState) {
    // Union of both player power surfaces (legacy list + mirror creature);
    // they normally coincide, but unioning is robust to either apply path.
    let mut ids: smallvec::SmallVec<[std::num::NonZeroU32; 8]> =
        state.player.powers.iter().map(|u| u.0).collect();
    if let Some(uid) = state.player.creature_uid {
        if let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) {
            for u in &c.powers {
                if !ids.contains(&u.0) {
                    ids.push(u.0);
                }
            }
        }
    }
    if ids.is_empty() {
        return;
    }
    state.powers.retain(|p| !ids.contains(&p.id));
    state.player.powers.clear();
    if let Some(uid) = state.player.creature_uid {
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.powers.clear();
        }
    }
}

/// C# `FairyInABottle` automatic death-prevention.
///
/// `FairyInABottle` (`PotionUsage.Automatic`, `FairyInABottle.cs:23-37`)
/// votes to prevent its owner's death in `Hook.ShouldDie` and then runs
/// `AfterPreventingDeath -> OnUse`, healing `max(MaxHp * 0.3, 1)`. Because
/// the creature sits at/below 0 HP at prevention time, the net effect is
/// to SET current HP to `floor(0.3 * MaxHp)` (empirically exact: 69 -> 20,
/// 68 -> 20). C#'s `ShouldDie` loop runs BEFORE `ShouldDieLate`
/// (LizardTail), so callers must check this *before*
/// [`crate::relic_effects::dispatch_should_die_late`].
///
/// Consumes the belt's Fairy and returns `true` if one fired (the caller
/// re-checks `current_hp > 0` before flipping to defeat). Bead
/// `sts2-rs-19hm7` (Automatic-potion death-prevention).
pub(crate) fn try_fairy_in_a_bottle_prevent_death(state: &mut CombatState) -> bool {
    let Some(slot) = state
        .player
        .potions
        .iter()
        .position(|p| *p == crate::potions::PotionKind::FairyInABottle)
    else {
        return false;
    };
    let Some(uid) = state.player.creature_uid else {
        return false;
    };
    let Some(idx) = state.creatures.iter().position(|c| c.id == uid.0) else {
        return false;
    };
    let _ = state.player.remove_potion_at(slot);
    let max_hp = state.creatures[idx].max_hp;
    // `max(MaxHp * 0.3, 1)` from a <=0 base == `floor(0.3 * MaxHp)`, clamped.
    let revive = (max_hp.saturating_mul(30) / 100).max(1).min(max_hp);
    state.creatures[idx].current_hp = revive;
    state.player.current_hp = revive;
    // Bead `sts2-rs-4iy6f`: a `PotionUsage.Automatic` potion is still a potion
    // USE — C# routes FairyInABottle's auto-fire through the same path that
    // raises `Hook.AfterPotionUsed`, so AfterPotionUsed relics react. The Rust
    // auto-consume skipped this dispatch, so e.g. REPTILE_TRINKET (+3 temporary
    // Strength on potion use) never fired when the Fairy saved the player
    // (seed 3139: player.powers cs=[REPTILE_TRINKET,STRENGTH] rs=[]). Fire it
    // now that the heal has applied (listeners see the post-revive state).
    let _ = crate::relic_effects::dispatch_after_potion_used(
        state,
        crate::potions::PotionKind::FairyInABottle,
        None,
    );
    true
}

/// DieForYou redirect target — C# `DieForYouPower.ModifyUnblockedDamageTarget`
/// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Powers/DieForYouPower.cs`)
/// folded through `Hook.ModifyUnblockedDamageTarget` (`Hook.cs:1564`). When the
/// player has a living Osty carrying [`PowerKind::DieForYou`], the UNBLOCKED
/// portion of a **powered** attack aimed at the player is redirected to the
/// Osty (the pet "dies for you"). Returns the Osty's [`CreatureUid`] when the
/// redirect applies, else `None`.
///
/// Gates mirror the C# overrides: the attack is powered (`props.is_powered()`),
/// the Osty is alive (`current_hp > 0`), and it carries `DieForYou`. The
/// target-is-pet-owner check is implicit — this is only called from
/// [`apply_damage_to_player`], whose target is always the player (the pet
/// owner). Non-powered hits (block-grant riders, Poison/Constrict self-damage)
/// pass straight through to the player.
///
/// Single consumer at this pin, so this is a focused helper rather than a full
/// `modify_unblocked_damage_target` dispatch channel; promote it if a second
/// redirect power ever lands.
fn osty_redirect_target(state: &CombatState, props: ValueProps) -> Option<CreatureUid> {
    if !props.is_powered() {
        return None;
    }
    let pet_uid = state.player.pet_uid?;
    let osty = state.creatures.iter().find(|c| c.id == pet_uid.0)?;
    if osty.current_hp <= 0 {
        return None;
    }
    let has_die_for_you = osty.powers.iter().any(|u| {
        state
            .powers
            .get_by_id(u.0)
            .is_some_and(|p| p.kind == crate::PowerKind::DieForYou)
    });
    has_die_for_you.then_some(pet_uid)
}

/// Player-targeted damage application. Mirrors
/// `damage::apply_damage` (which targets enemies only at this Rust pin)
/// but writes HP / block to `state.player` instead of a creature in
/// `state.creatures`. Returns `Some(StepResult::CombatLost)` if the
/// player dropped to 0 HP after the hit; `None` otherwise.
///
/// **Pipeline** matches Python `damage.py:106-172` step-for-step:
///   1. `modify_damage` (additive → multiplicative → cap → single floor).
///   2. `dispatch_before_damage_received` (Thorns / `FlameBarrier` —
///      which may flip `is_over` via reflective HP loss back to the
///      dealer).
///   3. Block absorption: `min(final_damage, player.block)` consumed,
///      remainder is `unblocked` (skipped if `props.UNBLOCKABLE` is set
///      — a defensive guard mirroring `damage.py:135-137` even though
///      no Phase-3 enemy intent currently sets UNBLOCKABLE).
///   4. `modify_hp_lost` chain on `unblocked` (no-op default at this
///      pin — Phase-7 Buffer / `HardenedShell` etc fire here).
///   5. Apply HP loss to `state.player.current_hp` (clamped at 0).
///   6. `dispatch_after_damage_received` + `dispatch_after_damage_given`.
///
/// **Why inline rather than calling `damage::apply_damage`?** The
/// shipped `damage::apply_damage(state, dealer, target: CreatureUid,
/// raw_damage, props)` constructs `target_ref = CreatureRef::Enemy(target)`
/// internally and operates on `state.creatures[i]` exclusively. A
/// signature change extending it to take `target: CreatureRef` is a
/// separate refactor (documented as a Phase-3 followup in CASS
/// `b-mou5xfbf-km9fam` finding (4)); P3-5's bead-spec partition pins
/// `damage.rs` as out-of-bounds, so we route the player target through
/// the shared hook dispatchers directly. The result is byte-identical
/// to what an extended `apply_damage` would produce — same dispatcher
/// walks, same single-floor invariant, same Phase-2 listener arms.
///
/// Exposed publicly so the parity harness (and future Phase-5 card
/// effects that target the player — e.g., Reaper, Bite) can route
/// through this helper without re-implementing the pipeline.
/// The player-bound damage a hit would land for *right now*, stopping before
/// block absorption and `modify_hp_lost` — i.e. exactly the number the game
/// paints on a monster's intent icon.
///
/// C# `AttackIntent.GetSingleDamage` runs
/// `Hook.ModifyDamage(…, ValueProp.Move, ModifyDamageHookType.All,
/// CardPreviewMode.None)` and returns `Math.Max(0, (int)num)`: the same
/// pipeline a real hit takes, truncated once at the very end. `Intent::Attack
/// { damage }` stores the **base** number from the monster's move table, so
/// anything that reads a telegraph the way the player sees it — the wire, the
/// mirror comparator — must run it through here first. The two numbers differ
/// on every board where the dealer holds Strength / Weak / Shrink or the
/// player is Vulnerable.
///
/// This is [`apply_damage_to_player`]'s step 1, extracted verbatim and shared
/// with it, so a previewed number and a delivered number cannot drift apart.
/// `raw_damage` with the `relic_effects`-family additive folded in — the base
/// the player-side `hooks::modify_damage` call actually receives.
///
/// Extracted from [`preview_damage_to_player`] so that
/// `apply_damage_to_player`'s `AfterModifyingDamageAmount` flashes can be
/// selected against the SAME number the pipeline used. The cap stage's listener
/// selection is defined against the post-multiplicative amount, so a flash pass
/// fed the un-folded raw damage would light the wrong powers on any board
/// carrying a damage relic.
///
/// The `None` arm is not a fallback for a failure: mid-combat the player always
/// has a `creature_uid`, and before Phase-1 assigns one there is no slot for
/// the relic registry to target, so the fold is genuinely empty.
#[must_use]
fn player_damage_base_after_relic_additive(
    state: &CombatState,
    dealer: CreatureRef,
    raw_damage: i32,
    props: ValueProps,
    card: Option<crate::cards::CardId>,
) -> i32 {
    let Some(target_uid) = CreatureRef::Player.to_creature_uid(state) else {
        return raw_damage;
    };
    // J01b: a dead channel adds the additive identity `0`.
    if !relic_channel_live(state, PlannedChannel::ModifyDamageAdditive) {
        return raw_damage;
    }
    let md_ctx = crate::relic_effects::ModifyDamageCtx {
        dealer: dealer.to_creature_uid(state),
        target: target_uid,
        base_damage: raw_damage,
        props,
        card,
        card_uid: card.and_then(|cid| {
            crate::card_effects::resolving_card_uid(state).filter(|u| {
                state
                    .cards
                    .get(u.0.get() as usize - 1)
                    .is_some_and(|c| c.card_id == cid)
            })
        }),
    };
    raw_damage.saturating_add(crate::relic_effects::dispatch_modify_damage_additive(
        state, &md_ctx,
    ))
}

#[must_use]
pub fn preview_damage_to_player(
    state: &CombatState,
    dealer: CreatureRef,
    raw_damage: i32,
    props: ValueProps,
    card: Option<crate::cards::CardId>,
) -> i32 {
    let target_ref = CreatureRef::Player;
    let ctx = DamageCtx {
        dealer,
        target: target_ref,
        props,
    };
    let previewed = hooks::modify_damage(
        state,
        player_damage_base_after_relic_additive(state, dealer, raw_damage, props, card),
        &ctx,
    );
    // C# clamps the telegraph with `Math.Max(0, …)`; a fully-Weakened 1-damage
    // multi-hit really does display 0 (Exoskeleton, TrackerRubyRaider).
    previewed.max(0)
}

/// The modifiers behind a [`preview_damage_to_player`] number, for DISPLAY.
///
/// Same `(state, dealer, props, card)` the preview itself takes, so a caller
/// that already has the number can ask "and which listeners produced it?"
/// without rebuilding the context. `base_damage` is deliberately absent: the
/// rows are base-independent (the pipeline's additive/multiplicative/cap
/// channels do not read it), and pairing them with a base is the consumer's
/// job.
///
/// See [`hooks::damage_modifier_breakdown`] for why the rows are read off the
/// engine's own per-listener entry points rather than re-derived.
#[must_use]
pub fn preview_damage_to_player_modifiers(
    state: &CombatState,
    dealer: CreatureRef,
    props: ValueProps,
) -> smallvec::SmallVec<[hooks::DamageModifier; 4]> {
    hooks::damage_modifier_breakdown(
        state,
        &DamageCtx {
            dealer,
            target: CreatureRef::Player,
            props,
        },
    )
}

pub fn apply_damage_to_player(
    state: &mut CombatState,
    dealer: CreatureRef,
    raw_damage: i32,
    props: ValueProps,
    card: Option<crate::cards::CardId>,
) -> Option<StepResult> {
    let target_ref = CreatureRef::Player;
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

    // Step 1: full damage pipeline. Single floor at the end via
    // `DamageMul::apply` (rational `div_euclid`) — see bead 58w / the
    // multiplier-rounding audit doc.
    //
    // Bead `sts2-rs-6jk` (j87-a): fold the relic_effects-family additive
    // contribution into `raw_damage` before the hooks-family pipeline runs so
    // multiplicative + cap + floor apply to the combined sum. Target slot
    // resolves via `state.player.creature_uid` (Some(uid) mid-combat after
    // d03 Phase 1); if it's None we skip the relic-side fold entirely.
    // Shared verbatim with `preview_damage_to_player` so the number a
    // telegraph advertises and the number a hit delivers cannot drift apart.
    // Its trailing `.max(0)` is behaviourally inert here: the block-absorption
    // step below already floors a negative `final_damage` to `absorbed = 0`
    // and `unblocked = 0`.
    //
    // Bead sts2-rs-4nh5 threads `card` from the caller (monster intents pass
    // `None`); bead sts2-rs-nwz1n resolves the played instance uid from
    // top-of-`Play`. Both now live in the shared helper.
    let final_damage = preview_damage_to_player(state, dealer, raw_damage, props, card);

    // Step 1b: `Hook.AfterModifyingDamageAmount` (`CreatureCmd.cs:262`), which
    // C# dispatches between `ModifyDamage` and `BeforeDamageReceived`. It lands
    // HERE and not inside `preview_damage_to_player`, which this shares with
    // the intent telegraph, the wire and the mirror comparator: the game
    // dispatches this hook only from `CreatureCmd.Damage`, never from a
    // preview (`AttackIntent.GetSingleDamage` passes `CardPreviewMode.None` to
    // `ModifyDamage` and dispatches nothing after it).
    crate::damage::flash_damage_amount_modifiers(
        state,
        dealer,
        target_ref,
        player_damage_base_after_relic_additive(state, dealer, raw_damage, props, card),
        props,
    );

    // Step 2: before_damage_received event hook. Bead 3to (P2-2b)
    // promoted the dispatcher to `(state: &mut CombatState, ...)` so
    // listeners can produce deferred-effect queue entries (Thorns:
    // reflective `Effect::DealHpLoss` to dealer) which the dispatcher
    // drains internally via `apply_effects`.
    hooks::dispatch_before_damage_received(state, &ctx);

    // Step 3: block absorption against `state.player.block`.
    // Phase-2 unblockable note: `props.UNBLOCKABLE` skips block
    // absorption entirely (mirrors `damage.py:135-137`). No Phase-3
    // enemy intent sets UNBLOCKABLE today; the guard is structural for
    // Phase-5 expansion (e.g., Hexaghost's UNBLOCKABLE Inferno tail).
    let unblockable = props.contains(ValueProps::UNBLOCKABLE);
    let absorbed = if unblockable {
        0
    } else {
        final_damage.min(state.player.block).max(0)
    };
    state.player.block -= absorbed;
    let unblocked = (final_damage - absorbed).max(0);

    // Bead `sts2-rs-xsvp9`: track DamageReceivedEntry analogues for the
    // player. C# `Spite.LostHpThisTurn` and `TearAsunder`'s `CalculatedHits`
    // both gate on `Result.UnblockedDamage > 0`, where C# computes
    // `UnblockedDamage = currentHp - CurrentHp` (`Creature.LoseHpInternal`,
    // Creature.cs:382) — the ACTUAL HP the creature lost, post-block AND
    // post-`modify_hp_lost` (so a hit fully absorbed by Buffer / a hp-loss
    // reducer does NOT count). The increment therefore moves below the
    // Step-5 HP write, gated on the realised delta — see bead
    // `sts2-rs-tearcount`. (The prior gate on the post-block `unblocked`
    // over-counted Buffer-absorbed hits: a LUCKY_TONIC Buffer hit with
    // `unblocked=4` but `modified_hp_loss=0` was counted, swinging
    // TearAsunder one hit high — coverage seed 1402, PHROG_PARASITE.)

    // Step 4: modify_hp_lost pipeline. Mirrors Python's guarded call at
    // `damage.py:140-141` (`if combat is not None and remaining > 0`);
    // the `remaining > 0` short-circuit is preserved here so a fully-
    // blocked hit doesn't walk the dispatcher and contaminate Phase-7
    // listener state (e.g. Buffer's stack-decrement).
    // C# stage split (`CreatureCmd.cs:140-170`): every hp-lost listener
    // EXCEPT Buffer runs before the Osty redirect; Buffer is
    // `ModifyHpLostAfterOstyLate` and only sees the player-bound overkill.
    // Pre-osty stage: only the power hooks (`ModifyHpLost`) run here. The
    // relic-side `modify_hp_lost` fold (TungstenRod -1, BeatingRemnant cap) is
    // C# `ModifyHpLostAfterOsty` and MUST run on the player-bound residual
    // AFTER the Osty redirect (Step 4.7) — running it pre-redirect wrongly
    // reduced the damage the Osty absorbs, leaving the Osty 1 HP too high
    // (RP109 Necrobinder).
    let mut modified_hp_loss = if unblocked > 0 {
        let hp_loss = hooks::modify_hp_lost_pre_osty(state, unblocked, &hp_lost_ctx);
        // C# `IntangiblePower.AfterModifyingHpLostAfterOsty` (`IntangiblePower.cs:37`)
        // flashes here — see `damage::flash_intangible_on_target`'s doc for why this
        // is the player-side counterpart of the enemy-path call in
        // `damage::apply_damage_with_final_impl`.
        crate::damage::flash_intangible_on_target(state, hooks::CreatureRef::Player);
        crate::damage::flash_hardened_shell_on_target(state, hooks::CreatureRef::Player);
        hp_loss
    } else {
        0
    };

    // Step 4.6: DieForYou redirect (Necrobinder Osty "die for you" tank).
    // C# `CreatureCmd.cs:148-169` via `DieForYouPower.ModifyUnblockedDamageTarget`:
    // the unblocked remainder of a POWERED hit aimed at the player is dealt to a
    // living Osty as DIRECT HP loss (the Osty's own block does NOT apply —
    // `LoseHpInternal`). Any OVERKILL beyond the Osty's HP flows back to the
    // player. Player block was already consumed above, so the Osty (and the
    // overkill) see the post-block, pre-Buffer residual — Buffer applies only
    // to the player-bound overkill afterwards (see below).
    let mut osty_absorbed_hp_loss = 0i32;
    let mut osty_reflect_uid: Option<CreatureUid> = None;
    // Tracks an alive→0 Osty transition driven by this redirect, so the
    // Necrobinder `Melancholy.AfterDeath` cost-reduction fires for the Osty's
    // death (this inline fast-path bypasses the power-side after-death witness
    // dispatch in `damage.rs`).
    let mut osty_just_died = false;
    if modified_hp_loss > 0 {
        if let Some(osty_uid) = osty_redirect_target(state, props) {
            if let Some(idx) = state.creatures.iter().position(|c| c.id == osty_uid.0) {
                let osty_hp = state.creatures[idx].current_hp;
                let osty_loss = modified_hp_loss.min(osty_hp);
                state.creatures[idx].current_hp -= osty_loss;
                if state.creatures[idx].current_hp < 0 {
                    state.creatures[idx].current_hp = 0;
                }
                // `osty_redirect_target` only returns a LIVING Osty (`current_hp
                // > 0`), so a post-loss HP of 0 is an alive→dead transition. This
                // inline fast-path bypasses the power-side after-death witness
                // dispatch, so the Melancholy rider is fired separately below.
                osty_just_died = state.creatures[idx].current_hp == 0;
                // Overkill beyond the Osty's HP is what the player actually
                // loses; the Step 5 write + Step 6 after-damage hooks below then
                // operate on the player's true residual.
                modified_hp_loss -= osty_loss;
                osty_absorbed_hp_loss = osty_loss;
                // Necrobinder `NecroMastery`: the Osty losing HP reflects
                // `osty_loss * Amount` Unblockable|Unpowered damage to all
                // hittable enemies (fired below, after the player's overkill HP
                // write). C# DEFERS every `Hook.AfterCurrentHpChanged` to a
                // second pass (`CreatureCmd.cs:253-263`) that runs AFTER all the
                // batch's `LoseHpInternal` calls — including the player's overkill
                // loss (`CreatureCmd.cs:163-169`). The reflect is dealt BY the
                // player (`base.Owner`), and `CreatureCmd.cs:122` no-ops a Damage
                // whose dealer `IsDead`. So a redirected hit that kills the Osty
                // AND overkills the player to death reflects nothing. Capture the
                // Osty here; fire after Step 5, gated on the player surviving.
                if osty_loss > 0 {
                    osty_reflect_uid = Some(osty_uid);
                }
            }
        }
    }

    // Step 4.7: AfterOsty relic reducers (TungstenRod -1, BeatingRemnant
    // per-turn cap) — C# `ModifyHpLostAfterOsty` is gated
    // `target != Owner.Creature -> return amount`, so these apply ONLY to the
    // player-bound residual that survived the Osty redirect, NEVER to the loss
    // the Osty absorbed (`CreatureCmd.cs:149/163`). Skipped when the player
    // target slot can't resolve to a `CreatureUid` (pre-d03 fixtures). TheBoot,
    // also on this channel, is a no-op on the player path (target==player).
    if modified_hp_loss > 0 && relic_channel_live(state, PlannedChannel::ModifyHpLost) {
        if let Some(target_uid) = target_ref.to_creature_uid(state) {
            let ml_ctx = crate::relic_effects::ModifyHpLostCtx {
                target: target_uid,
                dealer: dealer.to_creature_uid(state),
                props,
            };
            modified_hp_loss =
                crate::relic_effects::dispatch_modify_hp_lost(state, &ml_ctx, modified_hp_loss);
        }
    }

    // Step 4.8: Buffer (`ModifyHpLostAfterOstyLate`, `BufferPower.cs:10-17`)
    // absorbs whatever player-bound HP loss survived the Osty redirect —
    // C# Buffer runs AFTER the redirect, so a living Osty always eats the
    // hit first (and usually dies); Buffer spends a stack only when a
    // positive loss actually reached the player. With no living Osty this
    // degenerates to the previous behavior (Buffer zeroes the full
    // residual). Bead `sts2-rs-q6bix.6`, reordered for sweep seeds
    // 3553/3943.
    if modified_hp_loss > 0 && state.player_power_amount(crate::PowerKind::Buffer) > 0 {
        modified_hp_loss = 0;
        crate::damage::decrement_buffer_on_target(state, hooks::CreatureRef::Player);
    }

    // Step 5: write HP loss to the player.
    let hp_before_loss = state.player.current_hp;
    state.player.current_hp -= modified_hp_loss;
    if state.player.current_hp < 0 {
        state.player.current_hp = 0;
    }
    // Bead `sts2-rs-tearcount`: count one `DamageReceivedEntry`-analogue
    // event per hit that actually reduced the player's HP, mirroring C#'s
    // `UnblockedDamage = currentHp - CurrentHp` (Creature.cs:382). Gated on
    // the realised post-write delta so a Buffer-/reducer-absorbed hit
    // (`modified_hp_loss == 0`) does NOT count — see the moved-from note at
    // the post-block step above. Consumed by TearAsunder `CalculatedHits`
    // and Spite `LostHpThisTurn`.
    let actual_hp_lost = hp_before_loss - state.player.current_hp;
    // Forensics only — see `damage_trace`. The enemy-side twin lives at the
    // `damage::apply_damage_with_final_impl` chokepoint; the player has its
    // own pipeline, so it needs its own row or a player-HP desync traces
    // empty.
    if crate::damage_trace::armed() {
        crate::damage_trace::record(
            crate::damage::describe_dealer_for_trace(state, dealer),
            state.player.creature_uid.map_or(0, |u| u.0.get()),
            "player".to_string(),
            final_damage,
            absorbed,
            actual_hp_lost,
            hp_before_loss,
            state.player.current_hp,
            i32::from(state.round),
        );
    }
    // Game-style event log (C# `DamageReceivedEntry`), player half. Same
    // reason the trace needs its own row here: the player's HP write does not
    // pass through the enemy-side chokepoint.
    if crate::event_log::armed(state) {
        let dealer_who = crate::event_log::LogCreature::of_dealer(state, dealer);
        let receiver = crate::event_log::LogCreature::Player(state.player.character);
        let hp_after = state.player.current_hp;
        crate::event_log::record(
            state,
            crate::event_log::EventLogEntry::DamageReceived {
                dealer: Some(dealer_who),
                receiver,
                unblocked: modified_hp_loss,
                blocked: absorbed,
                hp_before: hp_before_loss,
                hp_after,
            },
        );
    }
    if actual_hp_lost > 0 {
        state.combat_log.hp_lost_this_turn = state.combat_log.hp_lost_this_turn.saturating_add(1);
        state.combat_log.unblocked_damage_received_this_combat = state
            .combat_log
            .unblocked_damage_received_this_combat
            .saturating_add(1);
    }
    // Bead `sts2-rs-73up8` (d03 Phase-2 prereq): back-sync the legacy
    // player block (decremented at line 1880) and the current HP
    // (just clamped) into the mirror Creature row. Per-field micro-
    // syncs so test fixtures that pre-populate Creature rows with
    // independent values aren't stomped. Skipped silently when the
    // player has no creature_uid (pre-d03 fixtures).
    state.sync_creature_block_from_player();
    state.sync_creature_current_hp_from_player();

    // Necrobinder NecroMastery reflect, deferred from the Step 4.6 Osty
    // redirect to mirror C#'s post-batch results loop (`CreatureCmd.cs:253-263`),
    // which fires the Osty's `AfterCurrentHpChanged` AFTER the player's overkill
    // `LoseHpInternal`. The reflect is dealt by the player and no-ops if the
    // player-dealer is dead (`CreatureCmd.cs:122`). The overkill was just written
    // in Step 5, so a redirected hit that kills the Osty and overkills the player
    // to death reflects nothing — Necrobinder fuzz seed 208: OwlMagistrate
    // VERDICT (33) vs an 8-HP Osty at player 3 HP → Osty dies, the 25 overkill
    // kills the player, no reflect (the player is then revived).
    if let Some(osty_uid) = osty_reflect_uid {
        if state.player.current_hp > 0 {
            hooks::dispatch_after_current_hp_changed(state, osty_uid, -osty_absorbed_hp_loss);
        }
    }

    // Necrobinder `Melancholy.AfterDeath`: a DieForYou redirect that kills the
    // Osty is a creature death — C# fires `Hook.AfterDeath` for it in the
    // post-batch death sweep (CreatureCmd.cs:371), so reduce each in-pile
    // Melancholy's cost by 1. This inline Osty fast-path keeps the corpse and
    // does NOT route through the power-side after-death witness dispatch in
    // `damage.rs`, so fire the Melancholy rider here.
    if osty_just_died {
        state.reduce_melancholy_cost_on_death();
    }

    // Step 6: after-damage event hooks. FlameBarrier / Poison after-
    // damage arms route here; bead 3to wires the deferred-effect queue.
    // Bead `sts2-rs-6ynt.6.3` threads `modified_hp_loss` so Inferno's
    // post-walk reaction can gate on `damage > 0` (player-side
    // received-damage path).
    hooks::dispatch_after_damage_received(state, &ctx, modified_hp_loss);
    // Regent `ReflectPower.AfterDamageReceived` (C# ReflectPower.cs:24): when
    // the player's block absorbs part of a POWERED attack, reflect the BLOCKED
    // amount back at the dealer as Unpowered damage, dealt by the player. The
    // generic `after_damage_received` listener arms can't host this — they have
    // no blocked-amount channel — so it fires here at the player-received
    // chokepoint with `absorbed` (= C# `result.BlockedDamage`) in scope, the
    // same pattern as the NecroMastery reflect above. No-ops once the player is
    // dead (C# `CreatureCmd.Damage` skips a dead dealer, CreatureCmd.cs:122).
    if state.player.current_hp > 0 && state.player_power_amount(crate::PowerKind::Reflect) > 0 {
        let reflect_amt = crate::powers::regent::reflect_damage_to_dealer(
            absorbed,
            !matches!(dealer, CreatureRef::Player),
            props.is_powered(),
        );
        if reflect_amt > 0 {
            hooks::apply_effect(
                state,
                hooks::Effect::DealHpLoss {
                    dealer: CreatureRef::Player,
                    target: dealer,
                    amount: reflect_amt,
                    props: ValueProps::UNPOWERED,
                },
            );
        }
    }
    // Bead `sts2-rs-ybtmu`: pass the post-block `modified_hp_loss` residual
    // (not pre-block `final_damage`) so the PaperCuts / Envenom arms gate on
    // C#'s `UnblockedDamage > 0`. This is the player-received path, so a
    // monster's fully-blocked powered hit no longer costs the player max HP.
    // `final_damage` is the post-modifier pre-block total (= C# TotalDamage)
    // for the ReaperForm gate added in bead `sts2-rs-4wppb`. ReaperForm only
    // fires for a player/Osty dealer, so it never triggers on this
    // enemy-dealt-to-player path; the total is threaded for consistency.
    // C# `DamageResult.WasFullyBlocked` (CreatureCmd.cs:154): `!Unblockable &&
    // (blockedDamage > 0 || target.Block > 0) && unblockedDamage == 0`. Here
    // `absorbed` = C# blockedDamage, `state.player.block` is the post-absorption
    // block, and `modified_hp_loss` the realised HP loss. Drives BowlbugRock's
    // off-balance arm (bead `sts2-rs-0l7dd`); the strength-zeroed headbutt vs a
    // still-blocked player has `absorbed == 0` but `block > 0`, so it counts as
    // fully blocked.
    //
    // `osty_absorbed_hp_loss == 0` guard: when the DieForYou Osty soaks the
    // unblocked remainder, C# `ModifyUnblockedDamageTarget` redirects the hit so
    // the Osty is the damage TARGET — and the Osty (no block) takes real HP
    // loss, so its `DamageResult.WasFullyBlocked` is false. The player's block
    // partially absorbing first (`absorbed > 0`) plus the Osty eating the rest
    // (`modified_hp_loss` driven to 0) must NOT read as "player fully blocked",
    // or BowlbugRock wrongly goes off-balance and skips a headbutt (Osty-surface
    // sweep seed 222: rs DIZZY vs cs HEADBUTT at turn 3, osty survives in rs).
    let was_fully_blocked = !unblockable
        && (absorbed > 0 || state.player.block > 0)
        && modified_hp_loss == 0
        && osty_absorbed_hp_loss == 0;
    hooks::dispatch_after_damage_given(
        state,
        &ctx,
        modified_hp_loss,
        final_damage,
        was_fully_blocked,
    );

    // Bead `sts2-rs-6jk` (j87-a): parallel relic_effects channel. The
    // `DamageGivenCtx` payload requires non-optional `CreatureUid` for both
    // slots; we skip when either dealer or player target can't resolve, and
    // (J01b) when the channel is dead (no owned handler).
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
                // Player-received path: HandDrill must never fire when the
                // target is the player (C# guards on `!target.IsPlayer`).
                was_block_broken: false,
            };
            let _ = crate::relic_effects::dispatch_after_damage_given(state, &dg_ctx);
        }
    }

    // Combat-end check: player at 0 HP → CombatLost. Mirrors Python's
    // `_check_combat_end` at `combat.py:1080-1082` (called after each
    // intent perform). We set both `is_over` and `player_won` here so
    // the outer loop's defensive `state.is_over` re-check catches the
    // same condition uniformly with reflective-Thorns-against-the-
    // player's flip path.
    //
    // Bead `sts2-rs-p62n0`: before flipping, walk the
    // `should_die_late` channel — listeners (LizardTail, …) can heal
    // the player mid-call and vote prevent. If at least one votes
    // prevent AND the heal pushed `current_hp` back above 0, we skip
    // the death flip entirely. Mirrors Python's
    // `core/combat.py:3122-3123` ordering.
    if state.player.current_hp <= 0 {
        // C# `Hook.ShouldDie` (Automatic potions: FairyInABottle) runs
        // BEFORE `ShouldDieLate` (LizardTail). Check the Fairy first; only
        // fall through to the relic channel if it didn't restore HP.
        let mut prevented = try_fairy_in_a_bottle_prevent_death(state);
        if !prevented || state.player.current_hp <= 0 {
            prevented = crate::relic_effects::dispatch_should_die_late(state) || prevented;
        }
        if !prevented || state.player.current_hp <= 0 {
            state.is_over = true;
            state.player_won = false;
            // C# `RemoveAllPowersAfterDeath` on player death (bead `sts2-rs-2e9q2`).
            clear_player_powers_on_death(state);
            return Some(StepResult::CombatLost);
        }
    }

    None
}

/// Run the enemy turn-end phase: fire `before_turn_end(ENEMY)`, run the
/// per-pile end-of-turn cleanup (Phase-6 stub at this pin), fire
/// `after_turn_end(ENEMY)` (which fires the synthetic
/// `on_turn_end_enemy_side` walk that ticks Vulnerable / Weak / Frail by
/// `-1` via the deferred-effect queue), then increment `state.round` and
/// transition `EnemyTurnEnd` -> `PlayerTurnStart` for the next round.
///
/// Mirrors the tail of Python's `_execute_enemy_turn` at
/// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1088-1095`:
///
/// ```python
/// fire_before_turn_end(CombatSide.ENEMY, self)
/// self._check_combat_end()
/// if self.is_over:
///     return
///
/// self._cleanup_cards_end_of_turn()
/// fire_after_turn_end(CombatSide.ENEMY, self)
/// self._check_combat_end()
/// ```
///
/// plus the post-`_execute_enemy_turn` round increment in
/// `end_player_turn` at `combat.py:976-980`:
///
/// ```python
/// self._execute_enemy_turn()
/// if self.is_over:
///     return
///
/// self.round_number += 1
/// self._start_player_turn()
/// ```
///
/// The Rust port splits the round-bump out of the enemy-turn body so the
/// play loop can observe the `EnemyTurnEnd -> PlayerTurnStart` boundary
/// (Python tail-calls `_start_player_turn` directly inside
/// `end_player_turn`; the Rust phase machine returns control to the
/// caller after each phase advances so MCTS can pause anywhere).
///
/// **Pipeline order** (locked by the Python source-of-truth, line
/// citations in parentheses):
///   1. Combat-end short-circuit. Same defensive guard as the other
///      phase fns — if `state.is_over` is already set, return the matching
///      [`StepResult`] without firing hooks or advancing phase.
///   2. Fire `before_turn_end(ENEMY)` (combat.py:1088) via
///      [`hooks::dispatch_before_turn_end`] with `TurnSideCtx { side:
///      Side::Enemy }`.
///   3. Combat-end re-check (combat.py:1089-1091). A `before_turn_end`
///      listener may have flipped `is_over`; we re-check before running
///      cleanup.
///   4. End-of-turn cleanup (combat.py:1093) — same Phase-6 stub as
///      [`end_player_turn_phase_two`]. At this Rust pin `cards::Card` does
///      not expose a callable `end_of_turn_cleanup`; per-card per-turn
///      flag resets land in Phase 6. The structural call site is
///      preserved as a documented no-op.
///   5. Fire `after_turn_end(ENEMY)` (combat.py:1094). The dispatcher
///      first walks the standard `after_turn_end` listeners (no-op
///      defaults at this pin), then because `ctx.side == Side::Enemy`,
///      walks the synthetic `on_turn_end_enemy_side` listeners and
///      drains the resulting `Effect::DecrementPowerAmount` batch via
///      [`hooks::apply_effects`]. Vulnerable / Weak / Frail on every
///      creature (player- AND enemy-owned) tick down by 1; the parity
///      exporter filters `amount == 0` instances at serialization, so
///      no explicit removal is required.
///   6. Combat-end re-check (combat.py:1095). Defensive — none of
///      Vulnerable / Weak / Frail's `on_turn_end_enemy_side` arms set
///      `is_over` at this pin, but a future Phase-7 `after_turn_end`
///      listener (e.g. `BurningBlood`-style heal-on-victory) may.
///   7. Round increment (combat.py:979). `state.round = state.round + 1`,
///      saturating at `u16::MAX`.
///   8. Advance phase `EnemyTurnEnd` -> `PlayerTurnStart`. The next
///      [`apply`] call routes into [`start_player_turn`] for the new
///      round. Mirrors the Python tail-call to `_start_player_turn` at
///      `combat.py:980` — Rust splits this to a phase boundary so MCTS
///      can pause between turns.
///
/// **Out of scope for P3-6 (deferred):**
///   * Per-pile `card.end_of_turn_cleanup` (Phase 6, sts2-rs-2un) — the
///     structural slot is preserved.
///   * `should_take_extra_turn` short-circuit on the enemy side
///     (Phase 7) — no Phase-2/3 power returns true.
///   * Player-owned Vulnerable / Weak / Frail tick — these tick at the
///     SAME enemy-turn-end boundary per Python's synthetic-guard
///     semantics (the listener body fires for every power-listener, not
///     just enemy-owned), so the same dispatcher call here handles
///     player-owned debuff tick-down implicitly. No separate
///     player-side tick-down call is needed at the player-turn-end
///     boundary.
/// Roll the next-turn intent for each SURVIVING enemy. Called from
/// `enemy_turn_end` AFTER `after_turn_end(Enemy)` so that creatures killed by
/// end-of-turn self-damage (DemisePower) are already dead (`current_hp <= 0`)
/// and excluded — matching C#, which telegraphs the next move only after
/// AfterTurnEnd resolves. Moving this out of the perform phase fixes the
/// `monster_ai` +1 over-read (bead `sts2-rs-c8why`).
pub(crate) fn roll_next_enemy_intents(state: &mut CombatState) {
    // A dead Reattach segment keeps rolling its DEAD→REATTACH cycle — C#
    // `StartTurn` calls `PrepareForNextTurn` for EVERY retained enemy with
    // no `IsDead` gate (`CombatManager.cs:395-398`). Bead `sts2-rs-8lwnq`.
    let alive_indices: smallvec::SmallVec<[usize; 8]> = state
        .enemy_uids
        .iter()
        .filter_map(|uid| {
            crate::creature_pos(&state.creatures, uid.0).filter(|&i| {
                let c = &state.creatures[i];
                c.current_hp > 0 || is_acting_reattach_corpse(state, c)
            })
        })
        .collect();
    for idx in alive_indices {
        let next_intent = if state.creatures[idx].ai.is_some() {
            // Bead `sts2-rs-5w76f`: do NOT pre-draw a seed. C#
            // RandomBranchState.GetNextState consumes one
            // `rng.NextFloat(total)` tick ONLY when the AI traverses a
            // RandomBranch transition; Move/ConditionalBranch consume
            // zero. Take the AI out so we can mutate it while passing
            // `&mut state.run_rng.monster_ai` through the roll path;
            // the AI is restored on the same line as the intent read.
            let owner_uid = CreatureUid(state.creatures[idx].id);
            let mut ai = state.creatures[idx]
                .ai
                .take()
                .expect("just checked is_some");
            // Bead sts2-rs-msi1: thread the owning creature uid into
            // the AI so `ConditionFn::OwnerAware` predicates (e.g.
            // CeremonialBeast PLOW_CHECK) can read per-creature
            // Power amounts during the upcoming branch resolution.
            ai.set_owner(owner_uid);
            // Split borrow: temporarily move MonsterAi RNG out of the
            // run RNG bundle so we can pass `&mut DotNetRng` alongside
            // `&CombatState`. `mem::replace` with a sentinel keeps the
            // `RunRng` shape valid for the brief window; the real RNG
            // is restored before the borrow ends.
            let mut local_rng =
                core::mem::replace(&mut state.run_rng.monster_ai, DotNetRng::placeholder());
            if !ai.initial_resolved() && !ai.performed_first_move() {
                // A creature SUMMONED this turn has never had its opening
                // telegraph resolved against the run stream — `start_combat`
                // only does that for the encounter's own roster, and the
                // normal roll below early-returns while the AI has not
                // performed a move yet. C# has no such gap: `CreatureCmd.Add`
                // runs the new machine from its initial state, so a branch
                // initial costs one `Rng.MonsterAi` read HERE, in enemy order,
                // like every other telegraph. Skipping it left the throwaway
                // constructor rng's answer standing and the run cursor one
                // read behind C# for the rest of the combat — every later
                // intent roll, for every monster, then diverged (GH #544,
                // TwoTailedRat CALL_FOR_BACKUP).
                ai.resolve_initial_move_with_combat(&mut local_rng, state);
            } else {
                ai.roll_move_with_combat(&mut local_rng, state);
            }
            state.run_rng.monster_ai = local_rng;
            let mut intent = ai.current_move().intents.first().copied();
            // TestSubject MULTI_CLAW escalation: C# `MultiClawTotalCount =>
            // BaseMultiClawCount + ExtraMultiClawCount` (TestSubject.cs:163)
            // with `ExtraMultiClawCount++` after each perform
            // (TestSubject.cs:310), so the telegraph grows 3→4→5→… per
            // repeat. The Rust port keeps the extra count in the owner's
            // PainfulStabs power counter — the power exists exactly for the
            // 2nd form's lifetime (applied on the 1st revive, removed on the
            // 2nd; see `adaptable_after_death`), which matches when
            // MULTI_CLAW is reachable. 2026-07-17 matched-seed gap audit:
            // the static 3-hit telegraph under-predicted late-2nd-form
            // incoming damage by up to 2× (real 10×6 vs modeled 10×3).
            if ai.current_move().id == "MULTI_CLAW" {
                if let Some(Intent::MultiAttack { damage, hits }) = intent {
                    let extra = state.creatures[idx]
                        .powers
                        .iter()
                        .filter_map(|pu| state.powers.iter().find(|p| p.id == pu.0))
                        .find(|p| p.kind == crate::PowerKind::PainfulStabs)
                        .map_or(0, |p| p.counter);
                    if extra > 0 {
                        intent = Some(Intent::MultiAttack {
                            damage,
                            hits: hits.saturating_add(extra.clamp(0, i32::from(u8::MAX)) as u8),
                        });
                    }
                }
            }
            state.creatures[idx].ai = Some(ai);
            intent
        } else {
            let kind = state.creatures[idx]
                .species
                .expect("enemy creature must carry a species (bead sts2-rs-a19m)");
            let history = state.creatures[idx].intent_history;
            Some(kind.roll_intent(&mut state.run_rng.monster_ai, history))
        };
        state.creatures[idx].intent = next_intent;
    }
}

/// Tick every enemy-owned `BattlewornDummyTimeLimit` timer at enemy-side
/// turn end. Mirrors C# `BattlewornDummyTimeLimitPower.AfterTurnEnd`
/// (`MegaCrit.Sts2.Core.Models.Powers/BattlewornDummyTimeLimitPower.cs`):
/// fired on the owner's (enemy) turn end — while `Amount > 1` the power
/// decrements; at `Amount == 1` the dummy's time is up, so it sets the
/// encounter's `RanOutOfTime` flag and `CreatureCmd.Escape`s. The Rust
/// escape mirror zeroes the creature's HP (corpse retained per
/// `sts2-rs-c8why`) WITHOUT counting it as a kill or a gold-scaling flee;
/// the combat then ends via the caller's `check_combat_end`, and the
/// event→combat resume path reads [`CombatState::event_combat_timed_out`]
/// back (in `exit_combat`) to suppress the victory reward. Bead
/// `sts2-rs-ck9z6` (GH #166).
fn tick_battleworn_dummy_time_limit(state: &mut CombatState) {
    const TIMER_MASK: crate::PowerKindSet =
        crate::PowerKindSet::from_kinds(&[crate::PowerKind::BattlewornDummyTimeLimit]);
    if !state.powers.present_kinds().intersects(&TIMER_MASK) {
        return;
    }
    // Collect each live enemy's timer power id first (immutable borrow),
    // then apply the decrement / expiry pass (mutable) — avoids aliasing
    // the creature arena and the power store at once.
    let timers: smallvec::SmallVec<[(std::num::NonZeroU32, std::num::NonZeroU32); 2]> = state
        .creatures
        .iter()
        .filter(|c| c.current_hp > 0 && c.species.is_some())
        .filter_map(|c| {
            c.powers
                .iter()
                .find(|puid| {
                    state
                        .powers
                        .get_by_id(puid.0)
                        .is_some_and(|p| p.kind == crate::PowerKind::BattlewornDummyTimeLimit)
                })
                .map(|puid| (c.id, puid.0))
        })
        .collect();
    for (creature_id, power_id) in timers {
        let Some(power) = state.powers.get_mut_by_id(power_id) else {
            continue;
        };
        if power.amount > 1 {
            power.amount -= 1;
            continue;
        }
        // Out of time: the dummy flees, no reward.
        state.event_combat_timed_out = true;
        if let Some(c) = state
            .creatures
            .iter_mut()
            .find(|c| c.id == creature_id && c.current_hp > 0)
        {
            c.current_hp = 0;
        }
    }
}

pub fn enemy_turn_end(state: &mut CombatState) -> StepResult {
    // Step 1: combat-end short-circuit.
    if state.is_over {
        return if state.player_won {
            StepResult::CombatWon
        } else {
            StepResult::CombatLost
        };
    }

    debug_assert_eq!(state.phase, CombatPhase::EnemyTurnEnd);

    // Step 2: fire before_turn_end(ENEMY).
    hooks::dispatch_before_turn_end(state, &TurnSideCtx { side: Side::Enemy });

    // Step 3: combat-end re-check after the before_turn_end(ENEMY) walk. Use
    // the FULL `check_combat_end` (not a bare `is_over` read) for the same
    // reason as Step 6 below — a listener kill only zeroes HP and nothing on
    // that path flips `is_over`. Mirrors the player-side
    // `end_player_turn_phase_one` after its before_turn_end(PLAYER) walk.
    // Guarded on a non-empty roster like the turn-start cluster's Step-10
    // check so zero-enemy unit fixtures keep their phase-advance behavior.
    if state.is_over || !state.enemy_uids.is_empty() {
        match check_combat_end(state) {
            StepResult::Advanced => {}
            end => return end,
        }
    }

    // Step 4: end_of_turn_cleanup. Python iterates every pile (hand /
    // draw / discard / exhaust / play) on every player_state and calls
    // `card.end_of_turn_cleanup` on each card — typically resetting
    // `current_cost` / clearing per-turn `Rage` extra damage / etc.
    // Bead `sts2-rs-c68o` clears `VarKey::FreeThisTurn` here too so
    // that the InfernalBlade rider drops if the player skips playing
    // the spawned card and the enemy turn ends with it still in hand.
    clear_per_turn_card_vars(state);

    // Step 5: fire after_turn_end(ENEMY). Dispatcher fires the standard
    // after_turn_end walk (no-op default at this pin) THEN — because
    // ctx.side == Side::Enemy — the synthetic on_turn_end_enemy_side
    // walk that emits Effect::DecrementPowerAmount(-1) for every
    // Vulnerable / Weak / Frail listener. The dispatcher drains the
    // effect batch via apply_effects post-walk.
    hooks::dispatch_after_turn_end(state, &TurnSideCtx { side: Side::Enemy });

    // Step 5b: tick BattlewornDummyTimeLimit (event→combat training-dummy
    // timer). Fired here at enemy-side turn end, after the standard
    // after_turn_end walk, mirroring C# `BattlewornDummyTimeLimitPower.
    // AfterTurnEnd`. An expiry zeroes the dummy's HP, so the Step 6
    // `check_combat_end` below resolves the (no-reward) win. Bead
    // `sts2-rs-ck9z6` (GH #166).
    tick_battleworn_dummy_time_limit(state);

    // Step 6: combat-end re-check after the after_turn_end(ENEMY) walk. Use
    // the FULL `check_combat_end` (not a bare `is_over` read): an
    // `after_turn_end` end-of-turn death (CountdownPower/Doom, self-damage,
    // …) only zeroes the last enemy's HP — nothing on that path flips
    // `is_over`. A bare flag read sails through to the `PlayerTurnStart`
    // advance below, and `start_player_turn` then draws a PHANTOM next-turn
    // hand whose reshuffle over-reads the run-level `shuffle` RNG stream,
    // desyncing EVERY subsequent combat's opening shuffle (whole-run parity
    // PARITY116: combat-2 opening hand diverged on identical decks — Rust did
    // an extra 10-card end-of-combat-1 reshuffle that C# never performs).
    // Mirrors the player-side fix in `end_player_turn_phase_one` (bead
    // `sts2-rs-01ygn`). Short-circuiting here ALSO skips the
    // `roll_next_enemy_intents` below, matching C# (it does not telegraph a
    // next move once the combat has ended). Guarded on a non-empty roster
    // like the turn-start cluster's Step-10 check.
    if state.is_over || !state.enemy_uids.is_empty() {
        match check_combat_end(state) {
            StepResult::Advanced => {}
            end => return end,
        }
    }

    // Bead `sts2-rs-c8why` follow-up: NOW (after after_turn_end resolved any
    // end-of-turn self-damage deaths, and the combat-end re-check above) roll
    // each surviving enemy's next-turn intent. This is the relocated Step-4
    // roll from the enemy perform phase.
    roll_next_enemy_intents(state);

    // Step 7: increment round counter. Saturating add prevents wrap on
    // pathological 65k-round combats; Python's round_number is a
    // Python int (unbounded) so the saturation is purely defensive at
    // this pin.
    state.round = state.round.saturating_add(1);

    // Step 8: advance phase to PlayerTurnStart for the next round.
    state.phase = CombatPhase::PlayerTurnStart;

    StepResult::Advanced
}

/// Detect combat end based on alive-enemy count + primary-player HP.
///
/// Mirrors Python's `_check_combat_end` + `_end_combat` at
/// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:2446-2470`.
///
/// **Outcomes (locked by the Python source-of-truth):**
///   * `state.is_over` already true on entry: idempotent — return the
///     recorded `CombatWon` / `CombatLost` outcome with no hook fires
///     (Phase-7 listeners would otherwise observe duplicate fires).
///   * `state.player.current_hp <= 0`: `is_over=true`, `player_won=false`,
///     fire `after_combat_end` only. Returns [`StepResult::CombatLost`].
///     Mirrors `combat.py:2458` (`_end_combat(player_won=False)`).
///   * All enemies in `state.enemy_uids` have `current_hp <= 0`
///     **AND** the Phase-7 `should_stop_combat_ending` predicate is
///     false (stubbed `false` at this Rust pin — no Phase-2/3/4 power
///     overrides the listener arm): `is_over=true`, `player_won=true`,
///     fire `after_combat_victory` THEN `after_combat_end` (in that
///     order, matching `combat.py:2456-2457`). Returns
///     [`StepResult::CombatWon`].
///   * Otherwise: combat ongoing. Returns [`StepResult::Advanced`]
///     without mutation.
///
/// **Caller contract:** `check_combat_end` does NOT advance the phase.
/// The caller (typically the [`end_turn_dispatch`] orchestrator) uses
/// the returned `StepResult` to decide whether to short-circuit the
/// remaining pipeline. Each P3-1..P3-6 phase fn already guards on
/// `state.is_over` at entry and post-listener-walk; this helper is a
/// canonical entry point that the [`apply`] dispatcher can invoke
/// when no specific phase fn is in scope.
///
/// **Out of scope:**
///   * `should_stop_combat_ending` predicate (Phase 7) — stubbed as
///     `false` so the all-enemies-dead branch always fires the victory
///     hooks. No power in the current 11-power Phase-2 set overrides
///     the listener arm; bead `sts2-rs-h9u` (Phase-7 umbrella) lifts
///     this stub.
///   * Special "primary player died but ally survived" / multi-player
///     parties — Python supports this via `_get_first_dead_primary_player`
///     but the Rust pin is single-primary-player.
///
/// Pure, read-only mirror of C# `CombatManager.IsOverOrEnding`
/// (`CombatManager.cs:229`) — combat has resolved OR is *ending* but the
/// formal teardown has not run yet. C# `IsEnding` (`:205-226`) is true when
/// there is a pending loss (player dead) OR no alive **primary** enemy remains
/// and nothing votes `ShouldStopCombatFromEnding`.
///
/// Unlike [`check_combat_end`], this has NO side effects (no `is_over` flip, no
/// victory/end hooks, no minion collapse) — it answers "would C# treat combat
/// as over/ending right now?" so callers can mirror C#'s `IsOverOrEnding`
/// short-circuits without prematurely resolving the combat. The canonical
/// resolution still happens at the next [`check_combat_end`] call site.
///
/// Primary use: gating the mid-draw reshuffle in
/// [`crate::CombatState::draw_suspending`]. C# `CardPileCmd.Shuffle`
/// (`CardPileCmd.cs:806`) early-returns on `IsOverOrEnding`, so when an on-draw
/// auto-play (Hellraiser) downs the last enemy *mid-draw*, C# never reshuffles
/// the discard pile for the remaining requested cards — while Rust defers the
/// combat-end flip to the turn-start tail (`finish_player_turn_setup` Step 10),
/// leaving `is_over` false during the draw. Without this gate Rust performs a
/// phantom reshuffle (over-reading the run-level `shuffle` stream), which
/// desyncs every later combat's opening shuffle (PARITY86 Ironclad: +12 RNG in
/// combat 1 round 5 → combat 2 opening-hand divergence).
///
/// The `ShouldStopCombatFromEnding` vote is modeled here only for `Adaptable`
/// corpses (TestSubject mid-revive — the same exception [`check_combat_end`]
/// honors); pending on-death reinforcement spawns (Infested) are NOT consulted
/// (they have side effects and cannot co-occur with a mid-draw auto-play kill
/// in any real fixture).
#[must_use]
pub fn combat_is_ending(state: &CombatState) -> bool {
    if state.is_over {
        return true;
    }
    // C# IsEnding: a pending loss (player dead) ends combat regardless of the
    // enemy roster.
    if state.player.current_hp <= 0 {
        return true;
    }
    // C# IsEnding: ending once no alive PRIMARY (non-minion) enemy remains,
    // unless an Adaptable corpse keeps it open (ShouldStopCombatFromEnding).
    let mut any_primary_present = false;
    let mut alive_primary = 0usize;
    let mut adaptable_corpse = false;
    // Presence-summary gates hoisted out of the loop (bit-identical: with no
    // live Minion / Adaptable power the per-creature scans always miss).
    let minion_possible = state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Minion);
    let adaptable_possible = state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Adaptable);
    for uid in &state.enemy_uids {
        let Some(c) = crate::creature_by_id(&state.creatures, uid.0) else {
            continue;
        };
        let is_minion = minion_possible
            && c.powers.iter().any(|puid| {
                state
                    .powers
                    .get_by_id(puid.0)
                    .is_some_and(|p| p.kind == crate::PowerKind::Minion)
            });
        if is_minion {
            continue;
        }
        any_primary_present = true;
        if c.current_hp > 0 {
            alive_primary += 1;
        } else if adaptable_possible
            && c.powers.iter().any(|puid| {
                state
                    .powers
                    .get_by_id(puid.0)
                    .is_some_and(|p| p.kind == crate::PowerKind::Adaptable)
            })
        {
            adaptable_corpse = true;
        }
    }
    any_primary_present && alive_primary == 0 && !adaptable_corpse
}

/// [`apply`]: crate::apply
pub fn check_combat_end(state: &mut CombatState) -> StepResult {
    // Idempotency: a second invocation on an already-resolved combat
    // returns the recorded outcome without re-firing the hooks.
    // Critical for Phase-7 listener bodies that increment counters
    // (e.g., a future "wins-this-floor" tracker).
    if state.is_over {
        return if state.player_won {
            StepResult::CombatWon
        } else {
            StepResult::CombatLost
        };
    }

    // Player-death branch first (mirrors `combat.py:2455`'s elif chain
    // — Python's `_get_first_dead_primary_player` short-circuits before
    // the alive-enemy check). At this pin the primary player is
    // `state.player`; allies are not yet wired into the death check.
    //
    // Bead `sts2-rs-p62n0`: walk `should_die_late` before the flip;
    // LizardTail-class listeners can heal the player and vote prevent.
    if state.player.current_hp <= 0 {
        // C# `Hook.ShouldDie` (FairyInABottle) runs before `ShouldDieLate`
        // (LizardTail); check the Fairy first.
        let mut prevented = try_fairy_in_a_bottle_prevent_death(state);
        if !prevented || state.player.current_hp <= 0 {
            prevented = crate::relic_effects::dispatch_should_die_late(state) || prevented;
        }
        if !prevented || state.player.current_hp <= 0 {
            state.is_over = true;
            state.player_won = false;
            // C# `RemoveAllPowersAfterDeath` on player death, before the
            // combat-end teardown (bead `sts2-rs-2e9q2`).
            clear_player_powers_on_death(state);
            // Fire after_combat_end ONLY on player death — no victory hook.
            hooks::dispatch_after_combat_end(state, &CombatLifecycleCtx::default());
            // Bead `sts2-rs-j87`: parallel relic-effects bus drain.
            let _ = crate::relic_effects::dispatch_after_combat_end(state);
            return StepResult::CombatLost;
        }
    }

    // Bead `sts2-rs-jjwam`: minion-only collapse. C# `CreatureCmd` kills the
    // surviving teammates when a primary enemy dies and every teammate is a
    // SECONDARY enemy (`isPrimaryEnemy && teammates.All(IsSecondaryEnemy)
    // → Kill(teammates)`, CreatureCmd.cs:392). A creature is secondary when
    // it carries a power with `OwnerIsSecondaryEnemy` — at this pin only
    // `PowerKind::Minion` (Creature.cs:228, MinionPower.cs). So once every
    // non-minion (primary) enemy is dead, the still-live minions are killed
    // and combat resolves — e.g. FOGMOG's EyeWithTeeth lingering after FOGMOG
    // dies (parity re-audit 2026-05-29, seeds 15/140). Guarded on a primary
    // having been present (a minion never fights alone) so a degenerate
    // minion-only fixture is untouched. C# fires the dying creature's
    // `AfterDeath` (on-death spawns) BEFORE this check, so we drain
    // `fire_pending` first and abort the collapse if a spawn re-introduced a
    // live primary (e.g. an Infested split into non-minion reinforcements).
    // Perf: `is_minion(c)` can only be true when a live Minion power exists;
    // gate the whole collapse scan on the presence summary (bit-identical —
    // with no Minion live, `minion_only_survivors` always returns `None` and
    // this block was a pure no-op walk).
    let minion_possible = state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Minion);
    let is_minion = |state: &CombatState, c: &crate::Creature| -> bool {
        c.powers.iter().any(|puid| {
            state
                .powers
                .get_by_id(puid.0)
                .is_some_and(|p| p.kind == crate::PowerKind::Minion)
        })
    };
    let minion_only_survivors =
        |state: &CombatState| -> Option<smallvec::SmallVec<[CreatureUid; 4]>> {
            let mut any_primary_present = false;
            let mut alive_primary = 0usize;
            let mut alive_minions: smallvec::SmallVec<[CreatureUid; 4]> = smallvec::SmallVec::new();
            for uid in &state.enemy_uids {
                let Some(c) = crate::creature_by_id(&state.creatures, uid.0) else {
                    continue;
                };
                if is_minion(state, c) {
                    if c.current_hp > 0 {
                        alive_minions.push(*uid);
                    }
                } else {
                    any_primary_present = true;
                    if c.current_hp > 0 {
                        alive_primary += 1;
                    }
                }
            }
            if any_primary_present && alive_primary == 0 && !alive_minions.is_empty() {
                Some(alive_minions)
            } else {
                None
            }
        };
    if minion_possible && minion_only_survivors(state).is_some() {
        // C# AfterDeath (on-death spawns) precede the teammate-kill check.
        let _ = crate::enemy_death_effects::fire_pending(state);
        // Re-check: only collapse if a spawn did not re-introduce a primary.
        if let Some(minions) = minion_only_survivors(state) {
            for uid in minions {
                if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
                    c.current_hp = 0;
                }
            }
        }
    }

    // All-enemies-dead branch. Python's `self.alive_enemies` filter is
    // `current_hp > 0`; we walk `enemy_uids` and check whether any matched
    // `Creature::current_hp` is positive (short-circuiting `any` — only the
    // zero/non-zero distinction is consumed). An empty `enemy_uids`
    // (degenerate / fixture state) trivially satisfies "none alive".
    // Creature ids are unique, so the dense lookup answers what the old
    // `creatures.iter().any(id == uid && alive)` scan did (J07).
    let any_enemy_alive = state.enemy_uids.iter().any(|uid| {
        crate::creature_by_id(&state.creatures, uid.0).is_some_and(|c| c.current_hp > 0)
    });
    if !any_enemy_alive {
        // C# `AdaptablePower.ShouldStopCombatFromEnding()` → true: a
        // TestSubject corpse (hp 0, still carrying `Adaptable`) has not
        // truly died — it revives to its next form on its NEXT enemy turn
        // (`enemy_turn_exec` revive pass). Keep the combat OPEN so the
        // player's killing blow does not resolve as a premature victory;
        // the deferred RESPAWN heal happens once the enemy turn runs. The
        // `Adaptable` power is removed on the 2nd revive, so the 3rd
        // (final-form) death falls through to the normal victory resolution
        // below. Bead `sts2-rs-jnf4a`.
        let adaptable_corpse_pending = state
            .powers
            .present_kinds()
            .contains(crate::PowerKind::Adaptable)
            && state.enemy_uids.iter().any(|uid| {
                state.creatures.iter().any(|c| {
                    c.id == uid.0
                        && c.current_hp <= 0
                        && c.powers.iter().any(|puid| {
                            state
                                .powers
                                .get_by_id(puid.0)
                                .is_some_and(|p| p.kind == crate::PowerKind::Adaptable)
                        })
                })
            });
        if adaptable_corpse_pending {
            return StepResult::Advanced;
        }
        // Phase-7 `should_stop_combat_ending` sliver (bead `sts2-rs-h9u`).
        // Before resolving victory, fire any pending enemy on-death
        // "second phase" effects — e.g. `InfestedPower.AfterDeath`
        // spawning 4 stunned Wrigglers when the PhrogParasite elite dies.
        // If reinforcements were summoned and any are alive, the combat
        // continues (C# `ShouldStopCombatFromEnding() == true`) instead of
        // resolving as a win.
        if crate::enemy_death_effects::fire_pending(state) {
            let any_alive_after = state.enemy_uids.iter().any(|uid| {
                crate::creature_by_id(&state.creatures, uid.0).is_some_and(|c| c.current_hp > 0)
            });
            if any_alive_after {
                return StepResult::Advanced;
            }
        }
        state.is_over = true;
        state.player_won = true;
        // Order matters: Python fires `after_combat_victory` BEFORE
        // `after_combat_end` (combat.py:2456-2457). The two dispatchers
        // walk the same powers-then-relics listener set; victory-only
        // listeners (e.g., BurningBlood's HEAL=6) fire here, then the
        // generic combat-end listeners fire below.
        hooks::dispatch_after_combat_victory(state, &CombatLifecycleCtx::default());
        hooks::dispatch_after_combat_end(state, &CombatLifecycleCtx::default());
        // Bead `sts2-rs-j87`: parallel relic-effects bus drain. Order
        // matches the legacy hooks call: victory THEN end.
        let _ = crate::relic_effects::dispatch_after_combat_victory(state);
        let _ = crate::relic_effects::dispatch_after_combat_end(state);
        return StepResult::CombatWon;
    }

    // Combat ongoing.
    StepResult::Advanced
}

/// Walk the [`Action::EndTurn`] dispatch chain. Returns the first
/// non-`Advanced` [`StepResult`] from any sub-step; otherwise
/// [`StepResult::Advanced`] after the full chain completes (i.e.,
/// `state.phase` is back at [`CombatPhase::PlayerTurnStart`] for the
/// next round).
///
/// Mirrors the tail-call structure of Python's `end_player_turn` at
/// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:937-980`:
///
/// ```python
/// def end_player_turn(self):
///     fire_before_turn_end(CombatSide.PLAYER, self)             # phase_one
///     ...
///     self._resolve_end_of_turn_hand()                          # phase_two
///     ...
///     self._cleanup_cards_end_of_turn()
///     fire_after_turn_end(CombatSide.PLAYER, self)
///     self._check_combat_end()
///     ...
///     self._execute_enemy_turn()                                # turn_start + exec + turn_end
///     ...
///     self.round_number += 1
///     self._start_player_turn()                                 # next round
/// ```
///
/// **Chain (locked by the Python source-of-truth):**
///   1. [`end_player_turn_phase_one`] — fire `before_turn_end(PLAYER)`,
///      advance `PlayerPlayPhase -> PlayerEndTurnPhaseOne`.
///   2. [`end_player_turn_phase_two`] — ethereal expel + retain + flush,
///      `end_of_turn_cleanup`, fire `after_turn_end(PLAYER)`, advance
///      `PlayerEndTurnPhaseOne -> EnemyTurnStart`.
///   3. [`enemy_turn_start`] — set `side=Enemy`, fire
///      `before_side_turn_start(ENEMY)`, clear enemy block + fire
///      `after_block_cleared`, fire `after_side_turn_start(ENEMY)`,
///      advance `EnemyTurnStart -> EnemyTurnExec`.
///   4. [`enemy_turn_exec`] — per alive enemy, perform intent (`Attack` /
///      `MultiAttack` hit the player); roll next intent; advance
///      `EnemyTurnExec -> EnemyTurnEnd`.
///   5. [`enemy_turn_end`] — fire `before_turn_end(ENEMY)`, fire
///      `after_turn_end(ENEMY)` (which ticks Vulnerable/Weak/Frail via
///      `on_turn_end_enemy_side`), increment `round`, advance
///      `EnemyTurnEnd -> PlayerTurnStart`.
///   6. [`start_player_turn`] — clear block (round > 1), reset energy,
///      draw [`BASE_DRAW`] cards, fire `after_side_turn_start(PLAYER)`,
///      advance `PlayerTurnStart -> PlayerPlayPhase`.
///
/// The first sub-step that returns [`StepResult::CombatWon`] /
/// [`StepResult::CombatLost`] / [`StepResult::PendingChoice`]
/// short-circuits the chain. Each sub-step ALREADY guards on
/// `state.is_over` at entry, so calling them in sequence is safe even
/// when a mid-chain hook flips `is_over` — the next sub-step's guard
/// catches it and returns the matching outcome.
///
/// **Why include `start_player_turn` (step 6)?** Python tail-calls
/// `_start_player_turn` directly inside `end_player_turn` after the
/// round bump. Without it, `Action::EndTurn` would leave the state in
/// `CombatPhase::PlayerTurnStart` (an intermediate) instead of
/// [`CombatPhase::PlayerPlayPhase`] (the resting state where the next
/// `Action::PlayCard` is valid). Including step 6 here matches Python's
/// observable post-condition.
///
/// **Out of scope (deferred):**
///   * `should_take_extra_turn` short-circuit (Phase 7) — no
///     Phase-2/3/4 power returns true, so the stub default is
///     correct. When Phase 7 lands the predicate, the chain above
///     conditionally skips steps 3-5 in favor of going directly to
///     step 6.
///   * `pending_choice` mid-chain return (Phase 6) — at this pin
///     no sub-step returns [`StepResult::PendingChoice`], but the
///     `matches!(result, StepResult::Advanced)` short-circuit handles
///     it correctly when Phase 6 lands the choice-yielding card
///     effects.
pub fn end_turn_dispatch(state: &mut CombatState) -> StepResult {
    end_turn_dispatch_from(state, 0)
}

/// Number of phases in the [`end_turn_dispatch`] chain.
pub const END_TURN_PHASE_COUNT: usize = END_TURN_CHAIN.len();

// Each fn handles its own combat-end short-circuit + phase advance, so
// `end_turn_dispatch` just chains them in Python's tail-call order and
// propagates the first non-Advanced return up.
const END_TURN_CHAIN: [fn(&mut CombatState) -> StepResult; 6] = [
    end_player_turn_phase_one,
    end_player_turn_phase_two,
    enemy_turn_start,
    enemy_turn_exec,
    enemy_turn_end,
    // start_player_turn closes the loop — the next Action::PlayCard
    // expects state.phase == PlayerPlayPhase.
    start_player_turn,
];

/// Run phase `phase` (`0..END_TURN_PHASE_COUNT`) of the
/// [`end_turn_dispatch`] chain alone. Running phases `0..k` one at a time,
/// stopping at the first non-`Advanced` return, and then
/// [`end_turn_dispatch_from`]`(state, k)` is exactly `end_turn_dispatch`.
///
/// MCTS uses the split to cache the RNG-free prefix of an `EndTurn`
/// re-application across chance-node widens (perf item M12).
pub fn end_turn_phase(state: &mut CombatState, phase: usize) -> StepResult {
    END_TURN_CHAIN[phase](state)
}

/// The [`end_turn_dispatch`] chain from phase `first` on (`first ==
/// END_TURN_PHASE_COUNT` runs nothing), with the same first-non-`Advanced`
/// short-circuit. `first == 0` is `end_turn_dispatch`.
pub fn end_turn_dispatch_from(state: &mut CombatState, first: usize) -> StepResult {
    for step in &END_TURN_CHAIN[first..] {
        let result = step(state);
        if !matches!(result, StepResult::Advanced) {
            return result;
        }
    }
    StepResult::Advanced
}

/// Is a diagnostic armed that records OUTSIDE `state` while a combat step
/// runs: the thread-local [`crate::damage_trace`], the `STS2_TRACE_RNG` /
/// `STS2_TRACE_PILES` stderr traces, or `state`'s event log (whose rows ride
/// the state but whose `STS2_COMBAT_LOG` echo goes to stderr)?
///
/// A caller that replays a cached slice of a step instead of re-running it
/// (MCTS's `EndTurn` prefix cache, perf item M12) produces the same state but
/// fires these recorders once instead of once per replay, so it must take
/// the plain path while this is `true`.
#[must_use]
pub fn side_channel_recorders_armed(state: &CombatState) -> bool {
    crate::damage_trace::armed()
        || crate::event_log::armed(state)
        || cards::pile_trace_enabled()
        || crate::dotnet_rng::rng_trace_enabled()
}

// ---------------------------------------------------------------------------
// P6 card-play (bead sts2-rs-y97 — vertical-slice expansion)
// ---------------------------------------------------------------------------
//
// Minimal `play_card` dispatcher covering Ironclad starter cards: Strike
// (deal 6) and Defend (gain 5 block). Bash is wired in the sibling bead
// `sts2-rs-06w` (requires apply_power machinery for Vulnerable).
//
// Out-of-scope vs Python's `combat.play_card` (combat.py:786):
//   * `pending_choice` short-circuit (Phase 7+ choice-yielding cards).
//   * `_pending_play` state machine + replay counts (Echo Form etc.).
//   * Enchantment hooks (`modify_card_play_count`, `apply_enchantment_on_card_played`).
//   * `before_card_played` / `after_card_played` hooks (P6+ — listener bodies
//     are stubs at this pin per `crates/sts2-core/src/hooks.rs::dispatch_after_play_card`).
//   * Card targeting modes beyond Enemy / Self_ / None (AllEnemies and
//     Random are now wired — `sts2-rs-xuk` / `sts2-rs-yva`).
//
// Energy / target / hand-index validation is illegal-input-tolerant: an
// invalid PlayCard is a no-op `StepResult::Advanced`, matching Python's
// `play_card` early-return-False behavior at combat.py:802-810 (we don't
// surface the bool; the bridge JSON shows no change).
//
// Card-effect dispatch is an inline `match card.card_id`. When the second
// content card (Bash) lands, this stays inline; once the third+ card needs
// per-card vars or pending-choice plumbing, this should be lifted to a
// `play_card_effect` registry analogous to Python's
// `sts2_env/cards/registry.py::register_effect`.

/// Play the card at `hand_index` in the player's hand, optionally targeted
/// at enemy slot `target` (index into `state.enemy_uids`).
///
/// Pre-conditions for a meaningful play (else returns `Advanced` no-op):
///   * `state.phase == PlayerPlayPhase` (the resting state).
///   * `state.pending_choice.is_none()`.
///   * `hand_index < state.player.hand.len()`.
///   * `state.player.energy >= card.cost`.
///   * Target satisfies the card's `target_type`:
///     `Enemy` → `target = Some(i)` with `i < enemy_uids.len()` AND that enemy is alive;
///     `Self_` / `None` → `target` is ignored;
///     `AllEnemies` → dispatched with `target_uid = None`; the handler
///     iterates `state.enemy_uids` internally to fan damage / debuffs
///     across all alive enemies (`sts2-rs-yva`);
///     `Random` → dispatched with `target_uid = None`; the handler is
///     expected to call [`CombatState::get_random_enemy`] per-hit to
///     mirror Python's `combat.rng.choice(combat.alive_enemies)` inside
///     the card body (`sts2-rs-xuk`).
///
/// On success:
///   1. Card moves Hand → Play pile.
///   2. Energy decremented by `card.cost`.
///   3. Card effect runs (Strike → `apply_damage`, Defend → `modify_block` + write).
///   4. Card moves Play → Discard (Attack/Skill that don't exhaust).
///   5. `check_combat_end` is invoked; may return `CombatWon` / `CombatLost`.
///
/// Returns `StepResult::Advanced` for successful plays AND illegal no-ops.
/// Terminal results (`CombatWon` / `CombatLost`) only when an effect actually
/// ends combat (e.g. Strike kills the last enemy, or a future Pyramid-style
/// self-damage card kills the player).
/// Consume one stack of the matching `FreeAttack` / `FreeSkill` /
/// `FreePower` power, if any, when a card of `card_type` was just
/// played for 0 energy. Mirrors Python's
/// `card_play_effects.py:FreeXPower.before_card_played` decrement +
/// auto-pop at amount==0.
///
/// Bead `sts2-rs-zlx.5`. Inlined here (rather than going through the
/// `before_card_played` channel) because that channel is not yet
/// wired to mutating dispatch — follow-up beads can lift this into a
/// proper listener body without changing the cost surface.
///
/// Returns the kind that was consumed, if any (useful for tests; the
/// caller in `play_card` discards the result).
pub fn consume_free_card_power(
    state: &mut CombatState,
    card_type: crate::cards::CardType,
) -> Option<crate::PowerKind> {
    use crate::cards::CardType;
    use crate::PowerKind;

    let want = match card_type {
        CardType::Attack => PowerKind::FreeAttack,
        CardType::Skill => PowerKind::FreeSkill,
        CardType::Power => PowerKind::FreePower,
        CardType::Curse | CardType::Status => return None,
    };
    // J07: this runs on every card play and almost never finds a Free*
    // power. `present_kinds` is a superset of every live kind, so with no
    // `want` in the arena the walk below matches nothing and changes nothing.
    if !state.powers.present_kinds().contains(want) {
        debug_assert!(
            !state
                .player_power_uids()
                .iter()
                .any(|u| state.powers.get_by_id(u.0).is_some_and(|p| p.kind == want)),
            "consume_free_card_power gated out a live {want:?}"
        );
        return None;
    }

    // Walk the player's power UIDs; find the first matching kind
    // with amount > 0, decrement, and drop the stack if it hits 0.
    //
    // q6bix F-Q: card-applied Free* powers (Unrelenting → FreeAttack,
    // etc.) land on the player's mirror creature, NOT legacy
    // `state.player.powers`. Walk the mirror+legacy UNION (same source
    // as `iter_power_listeners`) so the consume isn't mirror-blind.
    let power_uids = state.player_power_uids();
    for uid in power_uids {
        let pid = uid.0;
        // Find the power's current amount + kind without holding the
        // borrow long.
        let (kind, new_amount) = {
            let Some(power) = state.powers.get_by_id(pid) else {
                continue;
            };
            (power.kind, power.amount - 1)
        };
        if kind != want {
            continue;
        }
        if new_amount <= 0 {
            // Drop the stack: remove from arena AND from BOTH the mirror
            // creature's and the legacy player's power-uid lists.
            // Mirrors Python's `owner.powers.pop(self.power_id, None)`.
            state.powers.retain(|p| p.id != pid);
            state.player.powers.retain(|u| u.0 != pid);
            if let Some(player_uid) = state.player.creature_uid {
                if let Some(c) = state.creatures.iter_mut().find(|c| c.id == player_uid.0) {
                    c.powers.retain(|u| u.0 != pid);
                }
            }
        } else if let Some(power) = state.powers.get_mut_by_id(pid) {
            power.amount = new_amount;
        }
        return Some(kind);
    }
    None
}

/// Drain and apply `RupturePower`'s deferred Strength after a card play has
/// fully resolved. Mirrors C# `RupturePower.AfterCardPlayed`
/// (`MegaCrit.Sts2.Core.Models.Powers/RupturePower.cs:60-66`): the Strength
/// owed for HP-loss inflicted by the just-resolved card lands now, *after*
/// its own hits, so it never buffs the card that caused the loss (e.g.
/// HEMOKINESIS). The Rupture reaction in
/// [`crate::hooks::dispatch_after_damage_received`] stashes the owed amount on
/// the Rupture instance's per-instance `counter` while the card sits on the
/// Play pile; this drains it to `0` and applies the Strength. No-op when
/// nothing was deferred (drains 0) — the common case for cards that do no
/// self-HP-loss or for a player without Rupture. Bead `sts2-rs-22ttw`.
pub fn flush_rupture_deferred_strength(state: &mut CombatState) {
    // Perf gate: this runs once per card play + once per replay. With no
    // live Rupture power the drain below provably finds nothing, returns 0
    // and mutates nothing — skip its power-uid union walk entirely.
    if !state
        .powers
        .present_kinds()
        .contains(crate::PowerKind::Rupture)
    {
        return;
    }
    let owed = state.drain_rupture_deferred_strength();
    if owed > 0 {
        if let Some(player_uid) = state.player.creature_uid {
            crate::card_effects::apply_power_to_creature(
                state,
                player_uid,
                crate::PowerKind::Strength,
                owed,
            );
        }
    }
}

/// Fire `after_card_exhausted` on both event buses — relic-effects first,
/// then hooks — for a play-time / Corruption / pulled-from-Play exhaust.
/// Mirrors Python's `fire_after_card_exhausted` at `combat.py:904`.
///
/// Not the end-of-turn ethereal sweep, so `caused_by_ethereal = false`
/// (DarkEmbrace draws immediately; bead `sts2-rs-umouk`). The sweep site in
/// [`end_player_turn_phase_two`] stays hand-written: it fires the buses in
/// the opposite order and brackets the relic drain with
/// `combat_log.exhaust_sweep_ethereal`.
fn fire_after_card_exhausted(
    state: &mut CombatState,
    card_id: cards::CardId,
    card_uid: crate::CardUid,
    target: Option<CreatureUid>,
    card_tags: cards::CardTags,
    card_type: cards::CardType,
) {
    let exhaust_ctx = crate::relic_effects::CardEventCtx {
        card_id,
        card_uid: Some(card_uid),
        target,
        card_tags,
        energy_spent: 0,
    };
    let _ = crate::relic_effects::dispatch_after_card_exhausted(state, &exhaust_ctx);
    let hook_exhaust_ctx = CardCtx {
        card_id: Some(card_id),
        card_uid: Some(card_uid),
        card_tags,
        card_type: Some(card_type),
    };
    hooks::dispatch_after_card_exhausted(state, &hook_exhaust_ctx, false);
}

/// Bead `sts2-rs-ummci`: stamp `combat_log.last_attack_skill_played` for a
/// just-FINISHED Attack/Skill play — the slot `HistoryCourse` replays next
/// round. C# scans `History.CardPlaysFinished` (recorded post-`OnPlay`,
/// CardModel.cs:1524) with an `!e.Card.IsDupe` filter, so call sites must
/// run AFTER the effect dispatch (finish order: children, then parent) and
/// dupe-flagged clones never seed the slot.
fn stamp_last_attack_skill_played(
    state: &mut CombatState,
    card: &crate::cards::Card,
    card_uid: crate::CardUid,
) {
    if matches!(
        card.card_type,
        crate::cards::CardType::Attack | crate::cards::CardType::Skill
    ) && card.combat_var(VarKey::IsDupe) == 0
    {
        state.combat_log.last_attack_skill_played =
            Some((card.card_id, card.upgraded, Some(card_uid)));
    }
}

/// Captured locals needed to run the post-OnPlay-dispatch *tail* of a card
/// play ([`finish_card_play`]). Bead `sts2-rs-mcts-surface-onplay-select`:
/// when a top-level player card's `OnPlay` opens an interactive
/// `pending_choice` (Survivor/DaggerThrow discard, Acrobatics, Warcry
/// put-back, WLP retain), we no longer drain it eagerly to first-legal —
/// we surface it so an MCTS agent can actually pick. C#'s `OnPlayWrapper`
/// `await`s the selector INSIDE `OnPlay`, so everything after the selector
/// (the discard + its Tingsha `AfterCardDiscarded` reaction) AND the whole
/// `AfterCardPlayed`/replay/disposal tail must run AFTER the pick. Holding
/// these locals lets the resolve path re-enter the tail post-choice with the
/// exact same ordering the old inline drain produced (driving first-legal is
/// byte-identical to the prior behavior; only the agent-visible decision is
/// new).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardPlayFinish {
    card: crate::cards::Card,
    card_idx: usize,
    card_uid: crate::CardUid,
    target_uid: Option<CreatureUid>,
    resolved_cost: i32,
    weighted_drain: i8,
    extra_plays: i32,
    card_is_ethereal: bool,
    corruption_active_pre_dispatch: bool,
    /// EchoForm inputs captured at play-start, before the card's OnPlay
    /// (bead `sts2-rs-8ett`): the LIVE EchoForm amount and the count of
    /// first-in-series plays made this turn BEFORE this card. The card is
    /// echoed once iff `first_in_series_before < echo_form_amount_at_play_start`.
    echo_form_amount_at_play_start: i32,
    first_in_series_before: u32,
    /// Rebound stacks captured at play-start, before the card's OnPlay
    /// (GH #637). C# resolves the result pile at the TOP of `OnPlayWrapper`
    /// (`CardModel.cs:1891`), so a card that APPLIES Rebound during its own
    /// `OnPlay` — the Rebound card itself does exactly that — cannot redirect
    /// its own play. Same shape, and the same reason, as
    /// `echo_form_amount_at_play_start` above.
    rebound_amount_at_play_start: i32,
}

pub fn play_card(state: &mut CombatState, hand_index: u8, target: Option<u8>) -> StepResult {
    // ── Pre-condition guards (illegal play → Advanced no-op) ─────────────
    if state.phase != CombatPhase::PlayerPlayPhase {
        return StepResult::Advanced;
    }
    if state.pending_choice.is_some() {
        return StepResult::Advanced;
    }
    let hi = hand_index as usize;
    if hi >= state.player.hand.len() {
        return StepResult::Advanced;
    }
    let card_uid = state.player.hand[hi];
    let card_idx = card_uid.0.get() as usize - 1;
    let card = state.cards[card_idx];

    // Forensics frame (`damage_trace`) covering the whole play, replays and
    // AfterCardPlayed included — anything the card sets in motion is
    // attributed to it unless an inner dispatcher (an orb evoke, an enemy
    // reaction) pushes its own frame. Disarmed by default; see the module.
    let _trace = if crate::damage_trace::armed() {
        Some(crate::damage_trace::push_source(format!(
            "card:{:?}",
            card.card_id
        )))
    } else {
        None
    };

    // C# `Hook.ShouldPlay` union (Chains/Sloth/Ringing) — shared with the
    // auto-play resolver (`CardCmd.AutoPlay` checks the same hook,
    // `CardCmd.cs:38-46`).
    if player_should_play_blocked(state, &card) {
        return StepResult::Advanced;
    }

    // Bead `sts2-rs-zlx.5`: cost check goes through the
    // `modify_card_cost` channel so Corruption / Free* / etc. can
    // override the card's base cost before energy is spent. The
    // helper returns the resolved cost saturating-clamped to 0; an
    // empty-bus combat short-circuits to `card.cost.max(0)` so the
    // hot path stays branch-light.
    let effective_cost = hooks::effective_card_cost(state, card_uid);
    if i32::from(state.player.energy) < effective_cost {
        return StepResult::Advanced;
    }

    // Target validation. For `Enemy` cards we resolve the enemy CreatureUid
    // up front so the effect dispatch can borrow `state` mutably without
    // re-resolving (and so a dead-enemy target rejects cleanly).
    //
    // Bead sts2-rs-vaq: out-of-range / dead-enemy targets surface as
    // `StepResult::InvalidTarget` (silent no-op fix). The state is NOT
    // mutated — energy and hand are unchanged — so the caller can
    // re-prompt without losing the play. The missing-target case (no
    // `target` at all for an Enemy card) is still `InvalidTarget` for
    // the same reason: the CLI / bridge wants to surface a typed reject
    // rather than a confusing "Advanced".
    // Shiv's C# `TargetType` is `AllEnemies` (not `AnyEnemy`) while the player
    // has FanOfKnives (Shiv.cs:23-32), so it is played untargeted and auto-hits
    // every enemy. The spec stores the static `Enemy`; fold in the dynamic
    // override so the manual-play path agrees with `legal_actions` (which emits
    // the untargeted action) instead of rejecting the `target: None` play as an
    // InvalidTarget. `play_shiv` resolves the matching AoE. Shared predicate —
    // GH #335 (see `cards::effective_target_type`).
    let effective_target_type = cards::effective_target_type(state, &card);
    let target_uid: Option<CreatureUid> = match effective_target_type {
        TargetType::Enemy => {
            let Some(ti) = target else {
                return StepResult::InvalidTarget;
            };
            let ti_usize = ti as usize;
            if ti_usize >= state.enemy_uids.len() {
                return StepResult::InvalidTarget;
            }
            let uid = state.enemy_uids[ti_usize];
            // Reject targeting a dead enemy. Python's `_resolve_target`
            // returns None for dead/missing creatures and `play_card`
            // returns False (combat.py:812-813).
            let alive =
                crate::creature_by_id(&state.creatures, uid.0).is_some_and(|c| c.current_hp > 0);
            if !alive {
                return StepResult::InvalidTarget;
            }
            Some(uid)
        }
        TargetType::Self_ | TargetType::None => None,
        TargetType::Random => {
            // Bead `sts2-rs-xuk` (G3 plumbing): `Random` cards dispatch
            // with `target_uid = None`. The handler internally calls
            // `state.get_random_enemy()` per-hit (mirroring Python's
            // `combat.rng.choice(combat.alive_enemies)` inside the
            // factory body — see e.g. `sts2_env/cards/ironclad.py:540`
            // `sword_boomerang`, `silent.py:229` `ricochet`,
            // `silent.py:297` `bouncing_flask`, `defect.py:677`
            // `flak_cannon`). Cards whose Python factory takes a
            // `target` parameter despite a `RANDOM_ENEMY` declaration
            // (BubbleBubble is the only one in the audit) also use
            // `None` here; their bodies handle the missing target by
            // routing through `get_random_enemy`.
            //
            // Empty-board guard: if no enemies are alive the handler
            // sees `get_random_enemy() == None` and short-circuits.
            // Energy + hand → discard still run, matching Python's
            // `play_card` behaviour for a played-but-no-target card.
            None
        }
        TargetType::AllEnemies => {
            // Bead `sts2-rs-yva` (G3 plumbing): `AllEnemies` cards
            // dispatch with `target_uid = None`. The handler iterates
            // `state.enemy_uids` internally (typically via the
            // `deal_damage_player_to_all` / `apply_power_to_all_enemies`
            // helpers in sts2-content) so damage and debuff fan-out
            // across every alive enemy in `state.enemy_uids` insertion
            // order — mirroring Python's `for enemy in combat.alive_enemies`
            // loops at e.g. `sts2_env/cards/ironclad.py` (Thunderclap,
            // Stomp, Conflagration, PactsEnd, HowlFromBeyond, Breakthrough),
            // `sts2_env/cards/silent.py` (DaggerSpray, FlickFlack, Haze,
            // PiercingWail, CorrosiveWave, GrandFinale), and
            // `sts2_env/cards/defect.py` (SweepingBeam, Uproar, Hyperbeam,
            // Shatter).
            //
            // Empty-board guard: an empty `state.enemy_uids` makes the
            // handler's iteration a no-op. Energy + hand → discard still
            // run, matching Python's `play_card` behaviour for a
            // played-but-no-target AoE.
            None
        }
    };

    // ── Commit play: spend energy, move Hand → Play ──────────────────────
    // Bead `sts2-rs-zlx.5`: spend the modifier-resolved cost (not the
    // raw `card.cost`) so a Corrupted Skill / Free* card actually
    // deducts zero. `effective_cost` is i32 ≥ 0; clamp to i8 before
    // subtraction because `state.player.energy` is i8.
    let cost_i8: i8 = i8::try_from(effective_cost.clamp(0, i32::from(i8::MAX)))
        .expect("clamp keeps value in i8 range");
    state.player.energy = state.player.energy.saturating_sub(cost_i8);

    // ── EchoForm capture (bead `sts2-rs-8ett`) ──────────────────────────
    // C# `EchoFormPower.ModifyCardPlayCount` (`EchoFormPower.cs:18-27`,
    // v0.107.1) gives this card one extra play iff the number of
    // first-in-series plays already made this turn is `< Amount`. Both inputs
    // must be read NOW, before the card's OnPlay runs: the EchoForm amount so
    // that an EchoForm power applied by THIS very card (Echo Form is itself a
    // power card) does not retroactively echo its own play, and the
    // first-in-series count so it reflects only prior cards. The counter is
    // bumped here (once per original play — replays go through the loops below,
    // which do NOT re-enter `play_card`), so the next card sees this one.
    let echo_form_amount_at_play_start =
        state.player_power_amount(crate::PowerKind::EchoForm).max(0);
    // Read through the max (GH #368.1) — the plain field alone reads 0 on any
    // state rebuilt from a wire, which made EchoForm echo every card.
    let first_in_series_before = echo_form_first_in_series_this_turn(state);
    // GH #637: same play-start read, for the same reason. `ReboundPower` is
    // granted by the Rebound CARD, so reading the stack after `OnPlay` would
    // let the card redirect itself to draw-top and spend the stack it just
    // created. C# resolves the pile before `OnPlay`, so the Rebound card goes
    // to Discard and the stack survives for the NEXT card.
    let rebound_amount_at_play_start = state.player_power_amount(crate::PowerKind::Rebound).max(0);
    state.player.cards_first_in_series_this_turn = state
        .player
        .cards_first_in_series_this_turn
        .saturating_add(1);
    bump_echo_form_series_tally(state);

    // Bead `sts2-rs-n4wd`: X-cost surface. For X-cost cards
    // (`card.cost == -1` and not an UNPLAYABLE Curse/Status sentinel),
    // record the *currently-available* energy on the played
    // `CardInstance.energy_spent`, then drain `state.player.energy`
    // to 0. Mirrors Python's `_execute_card_play` X-cost branch
    // (sets `card.energy_spent` before dispatching the on-play
    // effect — see `cards/regent.py:735` `heavenly_drill`,
    // `cards/silent.py` `skewer`, `cards/defect.py` `tempest`,
    // `cards/ironclad.py` `whirlwind`). Downstream X-cost handlers
    // read this value off the played card (via the matching index in
    // `state.cards`) instead of re-reading `state.player.energy`
    // themselves. The cost deducted above is `max(0, -1) = 0`, so
    // the order (deduct → drain) is correct and `state.player.energy`
    // at this point is the pre-X-spend amount.
    // Energy actually paid for this card (C# `CardModel.SpendEnergy`'s
    // `amount`): the effective cost for a normal card, or all available
    // energy for an X-cost card (which then drains to 0).
    let energy_spent_for_hook: i32 = if card.is_x_cost() {
        let raw_x = i32::from(state.player.energy).max(0);
        // C# `SpendEnergy` sets `CapturedXValue = amount` — the RAW energy
        // spent, which is also the play's `EnergyValue`. The relic
        // `modify_x_value` fold (ChemicalX +2) is NOT stamped here: C# applies
        // it at read time (`CardModel.ResolveEnergyXValue`), so X-cost handlers
        // read `card_effects::resolving_card_x_value` while RightHandHand /
        // IvoryTile see the raw energy (GH #741 follow-up).
        state.cards[card_idx].energy_spent = raw_x;
        state.player.energy = 0;
        raw_x
    } else {
        // GH #741: C# `PlayCardAction.ExecuteAction` sets `EnergyValue =
        // EnergySpent = item` (the energy `SpendResources` paid) for EVERY
        // play, so a non-X card stamps its paid cost too. RightHandHand's late
        // effect and IvoryTile's `after_card_played` read it off the card.
        let paid = i32::from(cost_i8);
        state.cards[card_idx].energy_spent = paid;
        paid
    };

    // C# `CardModel.SpendEnergy` fires `Hook.AfterEnergySpent` immediately
    // after the energy is deducted and BEFORE the card's `OnPlay`
    // (CardModel.cs:1414-1424). The sole player-side listener at the pin is
    // OrbitPower's per-4-energy-spent grant. Regent parity coverage (Orbit).
    crate::hooks::dispatch_after_energy_spent(state, energy_spent_for_hook);
    // Tally this card's energy into the per-turn total (C# writes an
    // `EnergySpentEntry` here). HelixDrill's hit count reads this sum.
    state.combat_log.energy_spent_this_turn = state
        .combat_log
        .energy_spent_this_turn
        .saturating_add(energy_spent_for_hook);
    // Event log (C# `EnergySpentEntry`), at the pin the comment above names.
    // Zero-cost plays record nothing — C# only writes an entry for a spend.
    if energy_spent_for_hook > 0 {
        crate::event_log::record(
            state,
            crate::event_log::EventLogEntry::EnergySpent {
                amount: energy_spent_for_hook,
            },
        );
    }

    // UI play-log: record this MANUAL play BEFORE the card body dispatches, so
    // a trigger that auto-plays other cards (Cascade / Havoc / Catastrophe)
    // appears in the log AHEAD of the children it spawns (parent-first order
    // for the workbench event log). This is a display side-channel only — it is
    // NOT the C# `History.CardPlayFinished` parity surface (which records in
    // play-FINISH order), so ordering it for readability here is intentional.
    // Energy is already spent and the X-cost `energy_spent` stamped above, so an
    // X-cost trigger logs its real X. Recorded exactly once per `play_card`;
    // OneTwoPunch / EchoForm dupe re-resolutions are OUT OF SCOPE for the v1
    // play-log and are not separately recorded. No-op unless recording is on.
    {
        let pl = &state.cards[card_idx];
        // The play logs carry the X value for an X-cost card and 0 for any
        // other, as they did before GH #741 started stamping non-X cards'
        // `energy_spent` (the workbench goldens pin this shape).
        let pl_energy = crate::cards::observational_energy_spent(pl);
        let (pl_id, pl_up) = (pl.card_id, pl.upgraded);
        record_play_into(
            &mut state.combat_log,
            pl_id,
            pl_up,
            pl_energy,
            false,
            target,
        );
        // Same moment on the full event log (C# `CardPlayStartedEntry`), which
        // ALSO opens the source frame every hit / block / power dispatched by
        // this card attributes to — see `event_log::EventLog::track_source`.
        crate::event_log::record(
            state,
            crate::event_log::EventLogEntry::CardPlayStarted {
                card: pl_id,
                upgraded: pl_up,
                energy_spent: pl_energy,
                auto_played: false,
                target,
            },
        );
    }

    // Spend the card's STAR cost (Regent). C# `CardModel.SpendResources`
    // (`CardModel.cs:1398-1411`) pays energy then stars in one step, and
    // `SpendStars` (`CardModel.cs:1427-1435`) does `LoseStars(amount)` +
    // `Hook.AfterStarsSpent` — fired AFTER energy, BEFORE the card's `OnPlay`.
    // Routing through `CombatState::spend_stars` fires the `on_stars_spent`
    // bus so `BlackHolePower.AfterStarsSpent` (deal 3 to all enemies),
    // `ChildOfTheStarsPower` (block), MiniRegent, and GalacticDust all see the
    // spend. This was previously NEVER called on the play path — star-cost
    // cards (FallingStar, CrescentSpear, …) drew their cost down to 0 only via
    // the `legal_actions` gate, leaving `player.stars` untouched and every
    // on-stars-spent listener silent. Parity fix RP34: FallingStar's BlackHole
    // trigger (its +3) went missing, so a Shrunk FallingStar dealt 5 not 8.
    // `effective_star_cost` (not the static base) so a card made free this
    // turn / combat (TouchOfInsanity's `SetToFreeThisCombat`) spends 0 stars,
    // matching C# `SpendResources`'s `GetStarCostWithModifiers` after the
    // SetStarCostThisCombat(0).
    let star_cost = crate::effective_star_cost_in_combat(state, &card);
    if star_cost > 0 {
        state.spend_stars(star_cost);
        if state.is_over {
            return if state.player_won {
                StepResult::CombatWon
            } else {
                StepResult::CombatLost
            };
        }
    }

    cards::move_card(&mut state.player, card_uid, PileKind::Hand, PileKind::Play)
        .expect("hand_index validated above");

    // Bead `sts2-rs-f0own` (f933s follow-up): C# ChainsOfBindingPower
    // `BeforeCardPlayed` — if the just-committed card is Bound, set
    // the per-turn flag so subsequent Bound plays this turn are
    // rejected by the `ShouldPlay` guard above. Set unconditionally
    // (not gated on `player_has_chains`) so a Bound card played in
    // the same turn the power was removed still leaves the flag
    // consistent — `reset_per_turn` clears it next turn.
    if matches!(
        card.affliction,
        Some(crate::card_afflictions::CardAffliction {
            kind: crate::card_afflictions::CardAfflictionKind::Bound,
            ..
        })
    ) {
        state.combat_log.chains_bound_card_played_this_turn = true;
    }

    // Bead `sts2-rs-z4jsg` / round-12 reorder: C# `Weighted.OnPlay`
    // (Afflictions/Weighted.cs:8-11) drains `Amount` EXTRA energy
    // (`PlayerCmd.LoseEnergy`, clamped at 0). Per C#
    // `CardModel.OnPlayWrapper` (CardModel.cs:1498 then 1516), the
    // affliction `OnPlay` fires AFTER the card's own `OnPlay` effect —
    // not before. We capture the drain here but APPLY it after the
    // `card_effects::dispatch` below, so a card that gains energy in its
    // own effect (the Doormaker/GraspPower case) reaches the same
    // clamp-at-0 result as C#. Bead `sts2-rs-l65km`.
    let weighted_drain: i8 = if let Some(crate::card_afflictions::CardAffliction {
        kind: crate::card_afflictions::CardAfflictionKind::Weighted,
        stacks,
        ..
    }) = card.affliction
    {
        i8::try_from(stacks.clamp(0, i32::from(i8::MAX))).expect("clamp keeps value in i8 range")
    } else {
        0
    };

    // Bead `sts2-rs-rnyme` / play-finish fidelity fix: the combat-wide
    // card-play counter (feeding GoldAxe's C# `CalculatedDamageVar`) is
    // bumped AFTER the effect dispatch below, NOT here at play-start.
    // C# records `History.CardPlayFinished` post-`OnPlay`
    // (CardModel.cs:1524, before `AfterCardPlayed`), so a card never counts
    // its own still-in-flight play — and, crucially, a parent auto-play
    // (Havoc / Cascade / Catastrophe) whose `OnPlay` auto-plays a GoldAxe is
    // ALSO still in-flight at that moment, so the child must not see the
    // parent counted. Bumping at play-start leaked the in-flight parent into
    // the child's read (off-by-one GoldAxe damage). See the bump after
    // `dispatch` below.

    // Bead `sts2-rs-ummci`: the `HistoryCourse` last-Attack/Skill slot is
    // stamped AFTER the effect dispatch below (next to the
    // `player_card_plays_this_combat` bump), NOT here at play-start. C#'s
    // scan walks `History.CardPlaysFinished` in FINISH order
    // (CardModel.cs:1524), so a parent auto-play (Havoc / Cascade /
    // Catastrophe) finishes after its children and holds the slot.
    // Stamping at play-start let the auto-played child overwrite the
    // parent (seeds 5169/5957/4550/3681).

    // Bead `sts2-rs-ff3hr`: relic-side `before_card_played` dispatch.
    // Fires after the energy spend + Hand→Play move so listeners
    // observe the play state (energy already deducted by the
    // modifier-resolved cost, which includes SpikedGauntlets' +1 on Power
    // cards — GH #742). The hooks-bus `before_card_played` channel is read-only
    // (`noop_hook!`), so the relic-side `&mut state` channel is the
    // canonical site for state-mutating pre-play hooks.
    let before_played_ctx = crate::relic_effects::CardEventCtx {
        card_id: card.card_id,
        card_uid: Some(card_uid),
        target: target_uid,
        card_tags: card.tags,
        // C# `cardPlay.Resources.EnergyValue` — the energy actually paid (X-cost
        // cards: the drained pool; normal cards: the effective post-modifier
        // cost). IntimidatingHelmet's `before_card_played` gate reads this.
        energy_spent: energy_spent_for_hook,
    };
    if relic_channel_live(state, PlannedChannel::BeforeCardPlayed) {
        let _ = crate::relic_effects::dispatch_before_card_played(state, &before_played_ctx);
    }

    // Necrobinder non-Osty powers: power-side `before_card_played` reactive
    // (DanseMacabrePower). The hooks-bus channel is read-only, so this mutating
    // dispatch mirrors the relic-side one above. `resolved_cost` is the card's
    // modifier-resolved energy cost — for X-cost cards the just-stamped
    // `energy_spent` (Dirge etc.), otherwise the `effective_cost` computed at
    // play-start. Matches C# `EnergyCost.GetResolved()`.
    let resolved_cost = if card.is_x_cost() {
        state.cards[card_idx].energy_spent
    } else {
        effective_cost
    };
    // SpiritOfAsh keys off the played card's Ethereal-ness (keyword in spec OR
    // the per-instance EtherealThisCombat flag OR the player being Hexed — C#
    // `HexPower.TryModifyKeywordsInCombat` adds Ethereal to every card while Hex
    // is active). Uses the same OR-chain as the end-of-turn ethereal expel so a
    // card played under Hex correctly triggers SpiritOfAsh's block grant
    // (necro-player-misc C, seeds 3587/5473). Bead `sts2-rs-4wppb`.
    let card_is_ethereal = card_is_ethereal_at_play(state, &card);
    hooks::dispatch_before_card_played_powers(state, resolved_cost, card_is_ethereal);

    // Bead `sts2-rs-ybtmu`: `SurroundedPower.BeforeCardPlayed` — a targeted
    // card re-faces the surrounded player toward its target (exposing the
    // OTHER KaiserCrab arm at the player's back). Fires before the card
    // effect, matching C#'s `BeforeCardPlayed` slot. No-op without Surrounded.
    if let Some(t) = target_uid {
        hooks::update_surrounded_facing(state, t);
    }

    // Bead `sts2-rs-zlx.5`: Free* powers are "next N", not "this
    // turn" — decrement the consumed power's stack after a successful
    // free play (mirrors Python's `before_card_played` arm in
    // `card_play_effects.py:545/580/615`). The full hook channel
    // wiring of `before_card_played` is deferred; for now we inline
    // the decrement here for the three Free* PowerKinds.
    let _ = consume_free_card_power(state, card.card_type);

    // Bead `sts2-rs-stomp`: Stomp costs 1 less per Attack played this turn —
    // and C# shaves it in `Stomp.BeforeCardPlayed` (`CardModel.cs` fires
    // `Hook.BeforeCardPlayed` BEFORE `OnPlay`), so the shave must land before
    // the card body. Shaving after the body (the old site) meant that while
    // an Attack's body was suspended on a mid-play selection, C#'s Stomp was
    // already 1 cheaper than ours — a cosmetic 1-energy cost diff that the
    // mirror's parked-verify path escalated into a reseed, destroying the
    // suspended play (all 41 mid-play desyncs on the v27wr sweep, 33 of its
    // 112 unanswerable injections). Pinpoint's twin stays post-body: its C#
    // hook is `AfterCardPlayed`.
    if card.card_type == crate::cards::CardType::Attack {
        state.discount_stomp_on_attack_played();
    }

    // NOTE (bead `sts2-rs-ench-onplay-order`): the enchantment `on_card_played`
    // channel used to fire HERE, before the card-effect dispatch, because the
    // port followed Python's `enchantments.on_card_played` site. C#
    // `CardModel.OnPlayWrapper` (`CardModel.cs:1930-1943`) awaits the card's own
    // `OnPlay` FIRST and only then runs `Enchantment.OnPlay`, so the dispatch
    // now lives at the top of [`finish_card_play`] — see the comment there.

    // ── Card-play-count modifier chain (bead `sts2-rs-76t5`) ─────────────
    // Mirrors Python's `_execute_card_play` at `sts2-rl-agent@01c094d:
    // sts2_env/core/combat.py:856-858`:
    //
    // ```py
    // play_count = 1 + getattr(card, "base_replay_count", 0)
    //              + enchant_play_count_bonus(card)
    // play_count = modify_card_play_count(play_count, card, self)
    // fire_after_modifying_card_play_count(card, self)
    // ```
    //
    // The Rust dispatcher returns the *delta* (sum of per-listener
    // contributions) and we add it to a base of 1; OneTwoPunch returns
    // `+1` per attack, so a single OneTwoPunch(1) stack gives
    // `extra_plays = 1` and a total of two attack hits.
    //
    // The `after_modifying_card_play_count` dispatch then fires once per
    // play and routes any deferred `Effect`s (OneTwoPunch decrement /
    // remove) through `apply_effects`. Per Python semantics it fires
    // ONCE per card play, not once per replay.
    //
    // Note: `EchoForm` lives on a separate `cards_first_in_series_this_turn`
    // counter (bead `sts2-rs-8ett`) and runs as an additional replay
    // loop below — its modifier is intentionally NOT wired into this
    // hook channel at this pin to preserve the per-instance reset
    // semantics. OneTwoPunch's extra plays compose with EchoForm's
    // (a OneTwoPunch+EchoForm-active card produces:
    //   1 initial + 1 OneTwoPunch + 1 EchoForm = 3 hits).
    let count_ctx = hooks::CardPlayCountCtx {
        owner: hooks::CreatureRef::Player,
        card_id: Some(card.card_id),
        card_uid: Some(card_uid),
        card_type: Some(card.card_type),
        card_tags: card.tags,
        target: target_uid,
    };
    // Per-card-instance `base_replay_count` (bead `sts2-rs-6ynt.8.4`):
    // mirrors Python's `play_count = 1 + getattr(card, "base_replay_count",
    // 0) + ...`. The field defaults to 0 on every Card POD so existing
    // cards see no behaviour change; HiddenGem mutates it via
    // `cards::colorless::play_hidden_gem`, and SoldiersStew's TODO no-op
    // (`potions::dispatch::drink_soldiers_stew`) becomes a real `+= 1`
    // increment in a follow-up.
    // C# adds the enchantment's replays (Spiral, unused Glam) before the
    // modifiers: `GetEnchantedReplayCount() + 1`.
    let base_replay = card.base_replay_count + crate::enchantment_hooks::enchant_play_count_bonus(&card);
    let power_extra = hooks::dispatch_modify_card_play_count(state, &count_ctx);
    let live = relic_guarded_channel_live(state, GuardedChannel::RelicModifyCardPlayCount);
    let relic_extra = dispatch_if_live(state, live, 0, |state| {
        crate::relic_effects::dispatch_relic_modify_card_play_count(state, &count_ctx)
    });
    let extra_plays = power_extra + base_replay + relic_extra;
    let after_count_ctx = hooks::CardCtx {
        card_id: Some(card.card_id),
        card_uid: Some(card_uid),
        card_tags: card.tags,
        card_type: Some(card.card_type),
    };
    hooks::dispatch_after_modifying_card_play_count(state, &after_count_ctx);

    // Bead `sts2-rs-replay-choice` (subsumed by the unconditional inline-choice
    // resolution below): a card whose `OnPlay` opens a player choice must have
    // that choice resolved before `after_card_played` AND before any replay
    // re-dispatches. C# `OnPlayWrapper` (CardModel.cs:1472-1532) `await`s each
    // loop iteration's `OnPlay`, so the grid is fully resolved before the next
    // `i` (and before `Hook.AfterCardPlayed`). RS exposes a SINGLE
    // `pending_choice` slot and dispatches synchronously; resolving it inline
    // right after the effect dispatch (see below) keeps every replay's selection
    // (SeekerStrike under HIDDEN_GEM+1: 4 `Take(3)` grids → 4 cards, not 1) and
    // fixes the discard-ordering vs `after_card_played` (Silent seed 442).

    // C# `CardModel.OnPlayWrapper` computes the card's result pile via
    // `ModifyCardPlayResultPileTypeAndPosition` BEFORE running `OnPlay`
    // (CardModel.cs:1459 vs :1498), then routes the card to that
    // pre-snapshotted pile afterwards (:1541-1550). So Corruption's
    // Skill→Exhaust redirect is decided from the power state as it stood at
    // play-START, not play-end. This matters for the in-flight auto-play
    // cards (Cascade / Catastrophe / Havoc): if their `OnPlay` auto-plays a
    // CorruptionCard (a Power that applies Corruption then vanishes from
    // combat), the parent Skill must STILL route to Discard — C# already
    // captured "Corruption inactive" before the auto-play applied it. Read
    // the mirror+legacy union (`player_power_amount`) here, pre-dispatch.
    // Bead `sts2-rs-q6bix` (CASCADE/CATASTROPHE PILE_COUNT cluster).
    let corruption_active_pre_dispatch =
        state.player_power_amount(crate::PowerKind::Corruption) > 0;

    // Bead `sts2-rs-ph5mx`: snapshot the powered-hit counter before the card's
    // effect runs so the Vigor/Gigantification consume (which fires in
    // `after_card_played`) can gate on whether a powered attack ACTUALLY
    // executed — C# `VigorPower`/`GigantificationPower` latch on `BeforeAttack`,
    // so an Attack-type card that deals zero hits (X-cost Whirlwind at X=0) must
    // not consume the buff.
    state.player.powered_hits_at_card_play_start = state.player.powered_hits_this_turn;
    state.player.attack_command_suppressed_this_play = false;

    // C# `History.CardPlayStartedEntry` — recorded before `OnPlay` runs, so
    // ShouldPlay listeners (Ringing/Sloth) see this play while its effect's
    // auto-plays resolve.
    state.player.card_plays_started_this_turn =
        state.player.card_plays_started_this_turn.saturating_add(1);

    // ── Card effect dispatch ─────────────────────────────────────────────
    // Bead sts2-rs-ov4 (P6-1): replaced the per-CardId `match` arm with a
    // function-pointer registry lookup. Concrete handlers are installed by
    // `sts2-content::cards::dispatch::install_default_registry` at engine
    // boot (the Python bridge calls this; tests in this crate that exercise
    // `play_card` must `ensure_installed()` before dispatching).
    //
    // Unregistered CardIds return `EffectResult::NotImplemented`, which the
    // engine treats identically to `Resolved` for state-machine purposes —
    // the energy/hand/discard plumbing still runs, matching Python's
    // behaviour for cards whose factories yield empty `EffectsList`s.
    let _result = crate::card_effects::dispatch_for_card(state, card_uid, card.card_id, target_uid);

    // NOTE (bead `sts2-rs-ench-onplay-order`): the captured Weighted energy
    // drain (the affliction `OnPlay` leg, bead `sts2-rs-l65km`) used to be
    // applied HERE. C# `OnPlayWrapper` awaits the card's `OnPlay` — including
    // any mid-play `CardSelectCmd` — and only THEN runs `Enchantment.OnPlay`
    // followed by `Affliction.OnPlay` (`CardModel.cs:1930-1950`). Both now run
    // at the top of [`finish_card_play`], which is the single point both the
    // inline path and the surfaced-choice resume path pass through, so the
    // enchantment leg keeps its C# position ahead of the affliction leg.

    // Resolve any choice the card's `OnPlay` opened (e.g. DAGGER_THROW's
    // `CardSelectCmd.FromHandForDiscard`) INLINE now, before `after_card_played`
    // and any replay loop. C# `OnPlayWrapper` `await`s the selector INSIDE
    // `OnPlay` (CardModel.cs:1498), so every consequence of the selection —
    // CardCmd.Discard, and the relic reactions it fires (Tingsha's
    // `AfterCardDiscarded` 3-damage tick) — lands BEFORE `Hook.AfterCardPlayed`.
    // Leaving the choice pending past `after_card_played` (the old
    // `will_be_replayed`-only gate) inverted that order: an enemy's CurlUp block
    // (granted in `after_card_played`) was already up when the drained discard's
    // Tingsha damage landed, so it hit block instead of HP (Silent seed 442:
    // monster block cs=14 rs=11, hp cs=114 rs=117). The inline resolver replays
    // the harness's exact first-legal pick, so the SELECTION is byte-identical to
    // the post-apply drain — only the ordering moves earlier. `flush_played` stays
    // false so the in-flight card is not swept off the Play pile before its own
    // cleanup / replay loop runs (bead `sts2-rs-replay-choice` — replay loops
    // below still rely on the pending_choice being cleared here).
    let fin = CardPlayFinish {
        card,
        card_idx,
        card_uid,
        target_uid,
        resolved_cost,
        weighted_drain,
        extra_plays,
        card_is_ethereal,
        corruption_active_pre_dispatch,
        echo_form_amount_at_play_start,
        first_in_series_before,
        rebound_amount_at_play_start,
    };

    // Bead `sts2-rs-mcts-surface-onplay-select`: a TOP-LEVEL player play whose
    // `OnPlay` opened an interactive `pending_choice` (Survivor/DaggerThrow
    // discard, Acrobatics, Warcry/Headbutt put-back, WLP retain, …) is no
    // longer drained eagerly to first-legal — we SURFACE it so an MCTS agent
    // can actually pick, stashing the play tail in `pending_play_finish`. The
    // choice resolver (`resolve_choice_action`) re-enters [`finish_card_play`]
    // once the pick lands, so the discard + its `AfterCardDiscarded` reaction
    // run BEFORE `AfterCardPlayed` — exactly the ordering C#'s `OnPlayWrapper`
    // produces by awaiting the selector inside `OnPlay`, and byte-identical to
    // the former inline drain when driven first-legal.
    //
    // Combat-over short-circuit: if the play already ended combat (lethal
    // damage before the select opened), there is no live decision to surface —
    // drain inline and finish, matching the prior behavior for that edge.
    if state.pending_choice.is_some() {
        if state.is_over {
            resolve_inline_auto_play_choices(state);
        } else {
            state.pending_play = Some(crate::PendingPlay { finish: fin });
            return StepResult::PendingChoice;
        }
    }

    finish_card_play(state, fin)
}

/// Run the post-`OnPlay`-dispatch tail of a card play: the combat-wide play
/// counter, `AfterCardPlayed` buses, per-turn tallies, the OneTwoPunch /
/// EchoForm replay loops, the Play→Discard/Exhaust disposal, late-effects,
/// and the combat-end check. Extracted from [`play_card`] (bead
/// `sts2-rs-mcts-surface-onplay-select`) so the same tail can run either
/// inline (no choice opened) or deferred (re-entered from the choice
/// resolver after a surfaced `OnPlay` select is answered). The statement
/// order is unchanged from the former inline body, so driving a surfaced
/// choice first-legal reproduces the prior behavior byte-for-byte.
/// Play-time Ethereal predicate for a card being played: its static/override
/// keyword, the per-instance `EtherealThisCombat` flag, OR the player being
/// Hexed (C# `HexPower.TryModifyKeywordsInCombat` grants Ethereal to every card
/// while Hex is active). Shared by the manual (`play_card`) and auto-play
/// (`finish_auto_played_card`) SpiritOfAsh gate so both agree with the
/// end-of-turn ethereal expel. Reads the Hex stack off the mirror+legacy union.
pub fn card_is_ethereal_at_play(state: &CombatState, card: &crate::cards::Card) -> bool {
    crate::card_effects::card_ethereal_inst(card)
        || card.combat_var(crate::cards::VarKey::EtherealThisCombat) != 0
        || state.player_power_amount(crate::PowerKind::Hex) > 0
}

/// Does a live `ReboundPower` redirect this play's Discard-bound result to the
/// top of the draw pile? C# `ReboundPower.ModifyCardPlayResultPileTypeAndPosition`
/// (`ReboundPower.cs:19-30`): for a card the owner played whose running result
/// pile is still `Discard`, return `(PileType.Draw, CardPilePosition.Top)`.
///
/// Two guards, both load-bearing:
///
/// * **owner** — single-player port, so "the owner played it" is every player
///   card play.
/// * **`pileType == Discard`** — a card headed for Exhaust, or a Power card
///   headed for `PileType.None`, is untouched. Because the base pile comes from
///   `CardModel.GetResultPileTypeForCardPlay()` BEFORE any listener runs
///   (`CardModel.cs:1891`), the caller must only reach this predicate on the
///   Discard-bound branch of its routing chain.
///
/// `stacks_at_play_start` is the Rebound amount read BEFORE the card's `OnPlay`,
/// not the live one. C# resolves the whole `(pileType, position)` at the top of
/// `CardModel.OnPlayWrapper` (`CardModel.cs:1891`), before `OnPlay` runs, so a
/// card that APPLIES Rebound during its own play cannot redirect itself. The
/// Rebound card does exactly that (`play_rebound` grants `Rebound(1)`), and
/// reading the live amount here sent the Rebound card to draw-top while burning
/// the stack it had just created — caught by the C#-oracle coverage sweep
/// (`card:REBOUND` -> `PILE_COUNT :: discard_count`, C# discard 1 / Rust 0,
/// `player.powers.len` 2 vs 1). Same hazard and same fix as
/// `echo_form_amount_at_play_start`.
///
/// Callers must pair a `true` with [`consume_rebound_stack`] — see its doc for
/// why the two cannot be collapsed into one call.
fn rebound_redirects_to_draw_top(stacks_at_play_start: i32) -> bool {
    stacks_at_play_start > 0
}

/// Which power sends this play's Discard-bound result to the TOP of the draw
/// pile, or `None` when it goes to Discard as usual.
///
/// C# `Hook.ModifyCardPlayResultPileTypeAndPosition` (`Hook.cs:1535-1552`)
/// FOLDS: it starts from `CardModel.GetResultPileTypeForCardPlay()` and lets
/// each listener in `IterateCombatHookListeners` order — the owner's
/// POWER-INSERTION order — transform the running `(pileType, position)`,
/// collecting the ones that changed it. `OnPlayWrapper`
/// (`CardModel.cs:1892-1895`) then fires the after-hook over that list alone.
///
/// Nostalgia and Rebound both map Discard -> `(Draw, Top)`, so the first one
/// to run wins and the other's `pileType != Discard` early-out fires. The
/// card's DESTINATION is the same either way; what the order decides is which
/// power raises its `PowerFlashed` row and — because Rebound's after-hook
/// decrements — whether a Rebound stack is spent. Rust used to decide that
/// with a fixed `if / else if` chain that tested Nostalgia first, so a Rebound
/// applied BEFORE Nostalgia kept a stack the game would have spent (GH #650).
///
/// The insertion walk runs only when BOTH powers would redirect; with one
/// live the answer is the same as the old chain's, which keeps this off the
/// hot path for every ordinary play.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DrawTopRedirect {
    Nostalgia,
    Rebound,
}

fn resolve_draw_top_redirect(
    state: &CombatState,
    is_attack_or_skill: bool,
    started_attacks_skills: i64,
    rebound_stacks: i32,
) -> Option<DrawTopRedirect> {
    // Read the stack off the mirror+legacy union, as every call site did.
    let nostalgia = state.player_power_amount(crate::PowerKind::Nostalgia);
    let nostalgia_applies =
        is_attack_or_skill && nostalgia > 0 && started_attacks_skills <= i64::from(nostalgia);
    let rebound_applies = rebound_redirects_to_draw_top(rebound_stacks);
    match (nostalgia_applies, rebound_applies) {
        (false, false) => return None,
        (true, false) => return Some(DrawTopRedirect::Nostalgia),
        (false, true) => return Some(DrawTopRedirect::Rebound),
        (true, true) => {}
    }
    // Both would redirect: whichever the player picked up first runs first.
    for uid in state.player_power_uids() {
        match state.powers.get_by_id(uid.0).map(|p| p.kind) {
            Some(crate::PowerKind::Nostalgia) => return Some(DrawTopRedirect::Nostalgia),
            Some(crate::PowerKind::Rebound) => return Some(DrawTopRedirect::Rebound),
            _ => {}
        }
    }
    // Neither instance is reachable through the uid walk (a test fixture that
    // pushed an amount without a power row). Keep the pre-GH-#650 answer.
    Some(DrawTopRedirect::Nostalgia)
}

/// `Hook.AfterModifyingCardPlayResultPileOrPosition` for the winner of
/// [`resolve_draw_top_redirect`], and only for it — C# dispatches it over the
/// listeners that actually changed the pile. Nostalgia flashes
/// (`NostalgiaPower.cs:56`); Rebound flashes and decrements (see
/// [`consume_rebound_stack`]).
fn fire_draw_top_redirect(state: &mut CombatState, which: DrawTopRedirect) {
    match which {
        DrawTopRedirect::Nostalgia => {
            crate::event_log::flash_player_power(state, crate::PowerKind::Nostalgia);
        }
        DrawTopRedirect::Rebound => consume_rebound_stack(state),
    }
}

/// Fire `ReboundPower.AfterModifyingCardPlayResultPileOrPosition`
/// (`ReboundPower.cs:32-38`): `Flash()` then `PowerCmd.Decrement(this)`.
///
/// **Only call this when the redirect actually fired.** C#
/// `Hook.ModifyCardPlayResultPileTypeAndPosition` (`Hook.cs:1535-1552`) collects
/// a `modifiers` list holding only the listeners that CHANGED the pile or
/// position, and `CardModel.OnPlayWrapper` (`CardModel.cs:1892-1895`) dispatches
/// the after-hook over that list alone. Rebound's own
/// `card.Owner.Creature == base.Owner` line is belt-and-braces on a dispatcher
/// that already only reaches it — the same shape as `BlurPower`'s
/// `this != preventer` (GH #634). So an Exhaust-bound or Power card does NOT
/// spend a Rebound stack, and the Feral arm above already documents this
/// `modifiers` semantics for the same hook.
///
/// GH #637's issue text asserted the opposite ("the decrement is not guarded on
/// `pileType` … port that asymmetry verbatim"). It is not an asymmetry; there is
/// no such guard to drop because the dispatcher supplies it.
///
/// `Effect::DecrementPowerAmount` removes the instance at stack 0, matching
/// `PowerCmd.Decrement`.
fn consume_rebound_stack(state: &mut CombatState) {
    crate::event_log::flash_player_power(state, crate::PowerKind::Rebound);
    let Some(uid) = state.player_power_uids().into_iter().find(|u| {
        state
            .powers
            .get_by_id(u.0)
            .is_some_and(|p| p.kind == crate::PowerKind::Rebound)
    }) else {
        return;
    };
    hooks::apply_effects(
        state,
        &[hooks::Effect::DecrementPowerAmount {
            listener: hooks::ListenerRef::Power(uid),
            delta: -1,
        }],
    );
}

pub(crate) fn finish_card_play(state: &mut CombatState, fin: CardPlayFinish) -> StepResult {
    let CardPlayFinish {
        card,
        card_idx,
        card_uid,
        target_uid,
        resolved_cost,
        weighted_drain,
        extra_plays,
        card_is_ethereal,
        corruption_active_pre_dispatch,
        echo_form_amount_at_play_start,
        first_in_series_before,
        rebound_amount_at_play_start,
    } = fin;

    // ── Enchantment `OnPlay` (bead `sts2-rs-ench-onplay-order`) ──────────
    // C# `CardModel.OnPlayWrapper` (`CardModel.cs:1930-1943`):
    //
    //   await OnPlay(choiceContext, cardPlay);   // the card's own effect
    //   InvokeExecutionFinished();
    //   if (Enchantment != null)
    //       await Enchantment.OnPlay(choiceContext, cardPlay);
    //   if (Affliction != null)
    //       await Affliction.OnPlay(choiceContext, target);
    //   History.CardPlayFinished(...);
    //   await Hook.AfterCardPlayed(...);
    //
    // i.e. the enchantment leg runs AFTER the card body has fully resolved —
    // and, because `OnPlay` is awaited, after any mid-play `CardSelectCmd`
    // the body opened. The port previously fired this channel BEFORE the
    // effect dispatch (following Python's `enchantments.on_card_played`
    // site), which let Swift's draw land ahead of the card's own
    // `CardSelectCmd.FromHand`: `BurningPact.OnPlay`
    // (`MegaCrit.Sts2.Core.Models.Cards/BurningPact.cs:23-32`) is
    // `CardSelectCmd.FromHand` THEN `CardPileCmd.Draw`, so a Swift(3)
    // BURNING_PACT offered the 3 real hand cards in C# but 6 here (the hand
    // PLUS the 3 Swift had already drawn). An injected pick index then
    // resolved to a DIFFERENT card than the player actually chose and every
    // subsequent board diverged (mirror corpus records 269/270).
    //
    // `finish_card_play` is the single point both the inline path (no choice
    // opened) and the surfaced-choice resume path (`resolve_choice_action` →
    // `pending_play`) funnel through, so placing the dispatch here reproduces
    // the C# await ordering on both. It sits ahead of the
    // `History.CardPlayFinished` bump and both `after_card_played` buses,
    // matching the wrapper's statement order.
    //
    // First-play `*_USED_BIT` gates are unchanged: they are still flipped by
    // `enchantment_hooks::dispatch_after_card_played` below (the Rust analogue
    // of C# `Swift`/`Sown` setting `Status = EnchantmentStatus.Disabled`
    // inside their own `OnPlay`), which still runs before the replay loops.
    crate::enchantment_hooks::dispatch_on_card_played(state, card_uid);

    // Affliction `OnPlay` — the captured Weighted energy drain (bead
    // `sts2-rs-l65km`), which C# runs immediately AFTER the enchantment leg
    // (`CardModel.cs:1944-1950`). See the capture site in [`play_card`].
    if weighted_drain != 0 {
        state.player.energy = state.player.energy.saturating_sub(weighted_drain).max(0);
    }

    // Bead `sts2-rs-rnyme` / play-finish fidelity fix: record this play in
    // the combat-wide counter NOW — after the effect dispatch, before
    // `AfterCardPlayed` — mirroring C# `History.CardPlayFinished`
    // (CardModel.cs:1524). Bumping here (not at play-start) means a GoldAxe
    // dispatched above read the count WITHOUT its own in-flight play, and a
    // GoldAxe auto-played by THIS card's `OnPlay` (Havoc / Cascade /
    // Catastrophe) read it WITHOUT this still-in-flight parent. The replay
    // loop below bumps once more per replay (each replay is its own C#
    // `CardPlayFinished`). GoldAxe is the sole reader of this counter.
    state.combat_log.player_card_plays_this_combat = state
        .combat_log
        .player_card_plays_this_combat
        .saturating_add(1);
    state.combat_log.card_plays_finished_this_turn = state
        .combat_log
        .card_plays_finished_this_turn
        .saturating_add(1);
    // Event log (C# `CardPlayFinishedEntry`), which also closes this play's
    // source frame — see the auto-play twin in `finish_auto_played_card`.
    crate::event_log::record(
        state,
        crate::event_log::EventLogEntry::CardPlayFinished {
            card: card.card_id,
            upgraded: card.upgraded,
        },
    );

    // Bead `sts2-rs-ummci`: record the last Attack/Skill the player played
    // this round for `HistoryCourse`'s next-round replay, in C# FINISH order
    // (`History.CardPlayFinished`, CardModel.cs:1524 — see the bump above).
    // A parent whose `OnPlay` auto-played children re-stamps here AFTER the
    // children's post-dispatch stamps, so the slot holds the parent (Havoc /
    // Cascade / Catastrophe; seeds 5169/5957/4550/3681). HistoryCourse's own
    // dupe carries `IsDupe` and is filtered, matching C#'s `!e.Card.IsDupe`
    // scan (seed 5192).
    stamp_last_attack_skill_played(state, &card, card_uid);

    // C# `CardEnergyCost.AfterCardPlayedCleanup()`: a discount flagged
    // `WhenPlayed` is retired the moment the card is played, not at the turn
    // boundary — and the "costs N this turn" helpers carry BOTH flags, so this
    // is the edge that fires first far more often than not. Only a
    // reconstruction can carry such a bank (the engine's own effects bank
    // per-effect), so this is a no-op for a natively-played card.
    state.restore_wire_cost_expiring_on_play(card_uid);

    // Bead `sts2-rs-j87`: fire `after_card_played` on the
    // relic-effects bus. This is a fresh runtime site — the legacy
    // hooks-family equivalent of after-card-played is not wired in
    // `play_card` today (the channel was defined by bead `sts2-rs-2ax`
    // but never wired). DaughterOfTheWind / LostWisp / FakeStrikeDummy
    // listeners (filed under bead `sts2-rs-01o`) will register
    // on this channel.
    let played_ctx = crate::relic_effects::CardEventCtx {
        card_id: card.card_id,
        card_uid: Some(card_uid),
        target: target_uid,
        card_tags: card.tags,
        energy_spent: 0,
    };
    if relic_channel_live(state, PlannedChannel::AfterCardPlayed) {
        let _ = crate::relic_effects::dispatch_after_card_played(state, &played_ctx);
    }

    // Bead `sts2-rs-zlx.3`: also fire the hooks-family
    // `after_card_played` bus so player-side power listeners
    // (RagePower / AfterimagePower / PhantomBladesPower) get their
    // bodies applied after the read-only walk. The `relic_effects`
    // bus above handles relic listeners; the two buses are siblings,
    // not alternatives.
    let hook_played_ctx = hooks::CardCtx {
        card_id: Some(card.card_id),
        card_uid: Some(card_uid),
        card_tags: card.tags,
        card_type: Some(card.card_type),
    };
    hooks::dispatch_after_card_played(state, &hook_played_ctx);

    // Bead `sts2-rs-22ttw`: apply RupturePower's deferred Strength now that
    // the card has fully resolved. See [`flush_rupture_deferred_strength`].
    flush_rupture_deferred_strength(state);

    // Bead `sts2-rs-r6l9`: tally this play in the per-turn
    // cards-played bucket (Python: `_played_cards_this_turn.append(card)`
    // at `sts2-rl-agent@01c094d:sts2_env/core/combat.py:931`, after the
    // `fire_after_card_played` call). Read by
    // `CombatState::count_player_cards_played_this_turn`. EchoForm
    // replays bump the same bucket once per replay (mirrors the Python
    // pin where each `remaining_plays` iteration appends — see the loop
    // at `combat.py:872-935`).
    {
        let idx = card.card_type.index();
        state.player.cards_played_this_turn[idx] =
            state.player.cards_played_this_turn[idx].saturating_add(1);
    }

    // Combat-long ETHEREAL-played tally (feeds Necrobinder PullFromBelow's
    // CalculatedVar hit count). Bumped here, after the body resolves, so a
    // card never counts itself (C# records the `WasEthereal` entry post-OnPlay).
    if crate::card_effects::card_ethereal_inst(&card) {
        state.combat_log.ethereal_cards_played_this_combat = state
            .combat_log
            .ethereal_cards_played_this_combat
            .saturating_add(1);
    }

    // Pinpoint costs 1 less per Skill played this turn (C#
    // `Pinpoint.AfterCardPlayed` → `EnergyCost.AddThisTurn(-1)`). Post-body
    // is CORRECT for Pinpoint — unlike Stomp (whose C# hook is
    // `BeforeCardPlayed` and which is shaved pre-body above), Pinpoint's C#
    // hook genuinely fires after the play resolves.
    if card.card_type == crate::cards::CardType::Skill {
        state.discount_pinpoint_on_skill_played();
    }

    // Bead `sts2-rs-6ynt.11`: enchantment `after_card_played` channel.
    // Python's `enchantments.after_card_played(card)` runs after the
    // card body fully resolves so first-play markers (Swift / Sown /
    // Vigorous / Glam) can flip and Goopy can tick its exhaust
    // counter. Fires after the power-bus + relic-bus
    // `after_card_played` walks so enchantment side-effects observe
    // any state changes those listeners produced.
    crate::enchantment_hooks::dispatch_after_card_played(state, card_uid);

    // ── OneTwoPunch replay loop (bead `sts2-rs-76t5`) ────────────────────
    // Replay the card `extra_plays` additional times (mirrors the
    // `for _ in range(play_count - 1)` arm of Python's
    // `_resume_pending_play`). Each replay re-runs the card effect and
    // re-fires both `after_card_played` buses (so Rage / Afterimage /
    // event-reward relics tick once per replay), matching the EchoForm
    // replay shape below. The card stays on the Play pile across
    // replays; the move-to-Discard step at the end of the function
    // runs once.
    //
    // Combat-end short-circuit: if a replay killed the last enemy,
    // stop replaying to avoid `apply_damage` on an empty board.
    for _ in 0..extra_plays.max(0) {
        // C# `CardModel.OnPlayWrapper` fires `Hook.BeforeCardPlayed` once per
        // play iteration (CardModel.cs:1496, inside the `i < playCount2` loop),
        // so a duplicated/replayed card re-runs the FULL before-play dispatch:
        // it re-snapshots the Storm/Subroutine stack count AND re-arms the
        // Afterimage/SerpentForm/Oblivion/Strangle/Monologue amount snapshots
        // and re-fires the DanseMacabre/SpiritOfAsh block + Veilpiercer
        // decrement. Without the full re-arm, a HiddenGem-replayed (BaseReplay)
        // / OneTwoPunch'd card granted Afterimage block only on its first play
        // (the after-play grant consumes the armed counter, and the bare
        // `arm_storm_snapshot` never re-armed Afterimage) — Silent fuzz seed
        // 262 player.block cs=4 rs=2 (HiddenGem→WraithForm ×3 under Afterimage).
        crate::hooks::dispatch_before_card_played_powers(state, resolved_cost, card_is_ethereal);
        // C# `Free{Attack,Skill,Power}Power.BeforeCardPlayed` decrements on
        // EVERY play iteration (no IsDupe guard), so a card replayed under
        // OneTwoPunch/EchoForm consumes the matching Free* stack once per
        // replay — mirror the original play's decrement at line ~5336. Without
        // this a replayed Power card left a surviving FreePower zeroing the
        // next Power card's cost (gh#180 PANACHE HAND_COST residual).
        let _ = consume_free_card_power(state, card.card_type);
        // Stomp's shave is a `BeforeCardPlayed` hook — pre-body, once per
        // replay iteration (see the initial-play site above).
        if card.card_type == crate::cards::CardType::Attack {
            state.discount_stomp_on_attack_played();
        }
        let _ = crate::card_effects::dispatch_for_card(state, card_uid, card.card_id, target_uid);
        // C# `CardModel.OnPlayWrapper` runs the affliction `OnPlay` INSIDE the
        // per-iteration `for i < playCount2` loop (CardModel.cs:1513-1522), so a
        // Weighted card's `LoseEnergy` drain (Afflictions/Weighted.cs:8-11) fires
        // once PER play — each OneTwoPunch replay drains another `Amount`, not
        // just the initial play. Mirror the initial-play drain captured above.
        // Bead `sts2-rs-7ert6`.
        if weighted_drain != 0 {
            state.player.energy = state.player.energy.saturating_sub(weighted_drain).max(0);
        }
        // Bead `sts2-rs-replay-choice`: resolve THIS replay's choice inline
        // before `after_card_played` (C# resolves each iteration's `OnPlay`
        // grid before its `AfterCardPlayed`). Without this, the next replay
        // overwrites the slot and the selection is lost (SeekerStrike ×N).
        if state.pending_choice.is_some() {
            resolve_inline_auto_play_choices(state);
        }
        if relic_channel_live(state, PlannedChannel::AfterCardPlayed) {
            let _ = crate::relic_effects::dispatch_after_card_played(state, &played_ctx);
        }
        hooks::dispatch_after_card_played(state, &hook_played_ctx);
        // Bead `sts2-rs-wvpjv`: C# `OnPlayWrapper` (CardModel.cs:1472-1523)
        // calls `History.CardPlayFinished` once PER replay iteration, so each
        // replay adds its own `CardPlayFinishedEntry`. GoldAxe's
        // `CalculatedDamageVar` counts those entries — a OneTwoPunch'd card
        // therefore contributes its play count, not 1. Bump the combat-wide
        // counter per replay to keep `player_card_plays_this_combat` equal to
        // `History.CardPlaysFinished.Count` (the initial play was bumped once
        // above before the effect dispatch).
        state.combat_log.player_card_plays_this_combat = state
            .combat_log
            .player_card_plays_this_combat
            .saturating_add(1);
        state.combat_log.card_plays_finished_this_turn = state
            .combat_log
            .card_plays_finished_this_turn
            .saturating_add(1);
        // Each replay is its own C# `CardPlayFinished`, so the HistoryCourse
        // slot re-stamps per iteration — keeping the parent last after any
        // children this replay's dispatch auto-played.
        stamp_last_attack_skill_played(state, &card, card_uid);
        // Bead `sts2-rs-22ttw`: each replay is its own C# CardPlay, so
        // RupturePower's `AfterCardPlayed` deferred Strength flushes once per
        // replay (un-buffing only that replay's own hits).
        flush_rupture_deferred_strength(state);
        // C# `CardModel.OnPlayWrapper` runs the SAME per-iteration tail for
        // a `ModifyCardPlayCount` replay as for an Echo replay (one loop,
        // CardModel.cs:1472-1523): per-turn play buckets, the combat-long
        // ethereal tally, and the `Hook.BeforeCardPlayed` cost shaves
        // (Stomp per Attack, Pinpoint per Skill). Mirror the EchoForm replay
        // block below verbatim — previously only the combat-wide counter was
        // bumped here, so a OneTwoPunch'd Attack shaved Stomp once instead
        // of twice (sweep seed 5404: STOMP cost 3-3=0 in C#, 1 here).
        {
            let idx = card.card_type.index();
            state.player.cards_played_this_turn[idx] =
                state.player.cards_played_this_turn[idx].saturating_add(1);
        }
        if crate::card_effects::card_ethereal_inst(&card) {
            state.combat_log.ethereal_cards_played_this_combat = state
                .combat_log
                .ethereal_cards_played_this_combat
                .saturating_add(1);
        }
        if card.card_type == crate::cards::CardType::Skill {
            state.discount_pinpoint_on_skill_played();
        }
        if state.is_over {
            break;
        }
    }

    // Bead `sts2-rs-8ett`: EchoForm gives this card exactly ONE extra play
    // when it is among the first `Amount` first-in-series plays this turn.
    // C# `EchoFormPower.ModifyCardPlayCount` (`EchoFormPower.cs:18-27`,
    // v0.107.1) returns `playCount + 1` iff `num < Amount`, where `num` is the
    // count of first-in-series plays already made this turn — so each of the
    // first `Amount` distinct cards is doubled at most once (NOT one card
    // played `1 + Amount` times). Both inputs were captured at play-start
    // (`echo_form_amount_at_play_start` / `first_in_series_before`) so an
    // EchoForm gained mid-turn echoes the cards after it and the EchoForm
    // power card never echoes its own play. The single replay below re-runs
    // the full before/after-card-played dispatch once, matching the prior
    // per-unit loop body. Mirror+legacy union: EchoForm is applied to the
    // creature mirror, so the captured amount uses `player_power_amount`,
    // which dedups the union (COLD_SNAP seed 488).
    let echo_this_card =
        i64::from(first_in_series_before) < i64::from(echo_form_amount_at_play_start);
    if echo_this_card {
        // C# `EchoFormPower.cs:32` calls `Flash()` in
        // `AfterModifyingCardPlayCount`, which `Hook` fires ONLY on the models
        // that actually changed the play count — i.e. exactly when
        // `ModifyCardPlayCount` returned `playCount + 1`, which is this
        // `echo_this_card` predicate (`num < Amount`, both inputs captured at
        // play start). The flash therefore rides the decision, not the replay
        // body, so it is emitted before the replay runs.
        //
        // KNOWN DEVIATION (ordering, shared with the Feral flash below): C#
        // resolves the play COUNT — and so fires `AfterModifyingCardPlayCount`
        // — BEFORE the play loop starts, whereas Rust captures the two inputs
        // at play start but only acts on them here, after the initial play has
        // resolved and its after-play dispatch has run. The count and the
        // condition are right; only the position in the row stream is late, so
        // a replay shows EchoForm's flash between the two plays rather than
        // ahead of the first. Emitting it at the capture site would mean
        // splitting the decision from its use — reshaping live play logic to
        // host a log line, which the porting rules forbid.
        crate::event_log::flash_player_power(state, crate::PowerKind::EchoForm);
        {
            // Re-run the FULL before-play dispatch per Echo replay (C# fires
            // `Hook.BeforeCardPlayed` per play iteration) — re-arms Storm AND
            // the Afterimage/SerpentForm/Oblivion/Strangle/Monologue snapshots
            // and re-fires DanseMacabre/SpiritOfAsh/Veilpiercer, same as the
            // `extra_plays` loop above (STORM seed 517; Afterimage seed 262).
            crate::hooks::dispatch_before_card_played_powers(
                state,
                resolved_cost,
                card_is_ethereal,
            );
            // C# `Free*Power.BeforeCardPlayed` decrements per play iteration —
            // consume the matching Free* stack on this Echo replay too, same as
            // the `extra_plays` loop above (gh#180 PANACHE HAND_COST residual).
            let _ = consume_free_card_power(state, card.card_type);
            // Bead `sts2-rs-stomp`: Stomp's shave is a `BeforeCardPlayed`
            // hook — pre-body, once per Echo replay (see the initial-play
            // site above).
            if card.card_type == crate::cards::CardType::Attack {
                state.discount_stomp_on_attack_played();
            }
            // Re-run the card effect.
            let _ =
                crate::card_effects::dispatch_for_card(state, card_uid, card.card_id, target_uid);
            // C# fires the affliction `OnPlay` once per play iteration
            // (CardModel.cs:1513-1522), so a Weighted card's `LoseEnergy` drains
            // another `Amount` on every Echo replay too — same as the
            // `extra_plays` loop above. Bead `sts2-rs-7ert6`.
            if weighted_drain != 0 {
                state.player.energy = state.player.energy.saturating_sub(weighted_drain).max(0);
            }
            // Bead `sts2-rs-replay-choice`: resolve this Echo replay's choice
            // inline before `after_card_played`, same as the `extra_plays`
            // loop above (a choice-card under EchoForm would otherwise lose
            // every replay's selection to slot-overwrite).
            if state.pending_choice.is_some() {
                resolve_inline_auto_play_choices(state);
            }
            // Re-fire both after_card_played buses so Rage / Afterimage /
            // PhantomBlades / Storm / SerpentForm (hooks) and event-reward
            // relics (relic_effects) tick once per replay.
            if relic_channel_live(state, PlannedChannel::AfterCardPlayed) {
                let _ = crate::relic_effects::dispatch_after_card_played(state, &played_ctx);
            }
            hooks::dispatch_after_card_played(state, &hook_played_ctx);
            // Bead `sts2-rs-22ttw`: flush RupturePower's deferred Strength per
            // Echo replay (each replay is its own C# `AfterCardPlayed`).
            flush_rupture_deferred_strength(state);
            // Bead `sts2-rs-r6l9`: count each Echo replay in the
            // per-turn play buckets (matches Python's per-iteration
            // append in `combat.py:872-935`).
            {
                let idx = card.card_type.index();
                state.player.cards_played_this_turn[idx] =
                    state.player.cards_played_this_turn[idx].saturating_add(1);
            }
            // Bead `sts2-rs-wvpjv`: each Echo replay also records a fresh
            // `CardPlayFinishedEntry` in C# (CardModel.cs:1472-1523 loops
            // `CardPlayFinished` once per `i < playCount2`). Keep the
            // combat-wide counter feeding GoldAxe's `CalculatedDamageVar`
            // equal to `History.CardPlaysFinished.Count`.
            state.combat_log.player_card_plays_this_combat = state
                .combat_log
                .player_card_plays_this_combat
                .saturating_add(1);
            state.combat_log.card_plays_finished_this_turn = state
                .combat_log
                .card_plays_finished_this_turn
                .saturating_add(1);
            // Each Echo replay is its own C# `CardPlayFinished` — re-stamp
            // the HistoryCourse slot so the parent stays last after any
            // children this replay auto-played.
            stamp_last_attack_skill_played(state, &card, card_uid);
            // Combat-long ETHEREAL tally also counts each Echo replay (mirrors
            // the per-turn tally above).
            if crate::card_effects::card_ethereal_inst(&card) {
                state.combat_log.ethereal_cards_played_this_combat = state
                    .combat_log
                    .ethereal_cards_played_this_combat
                    .saturating_add(1);
            }
            // (EchoForm now grants a single extra play, so there is no replay
            // loop to break out of on a combat-ending replay — the cleanup
            // below already no-ops on an empty board.)
        }
    }

    // GH #363 A2: charge this hand play against VoidForm. The whole block
    // above — the initial play plus every `extra_plays` / EchoForm replay — is
    // ONE C# `playCount` series (`CardModel.OnPlayWrapper`'s single
    // `for (i = 0; i < playCount; i++)` loop at CardModel.cs:1905-1964), and
    // `VoidFormPower.AfterCardPlayed` only increments on
    // `!IsAutoPlay && IsLastInSeries` — so a series charges exactly ONE
    // VoidForm card, however many times it replays. Bumping here, once, past
    // the replay loops, is that `IsLastInSeries` gate; this is the hand-play
    // path so `IsAutoPlay` is false by construction (the auto-play tally in
    // [`auto_play_tally_finished`] deliberately does NOT call this).
    bump_void_form_play_tally(state);
    // C# `BrilliantScarf.AfterCardPlayed` counts each play iteration of a
    // non-auto play (`OnPlayWrapper`'s `playCount` loop, CardModel.cs:1904-1959;
    // `if (cardPlay.IsAutoPlay) return`): the play, each replay and the Echo
    // replay. This is the hand-play path; auto-plays never reach it.
    if let Some(r) = state
        .relics
        .iter_mut()
        .find(|r| !r.is_melted && r.kind == crate::RelicKind::BrilliantScarf)
    {
        r.counter = r.counter.saturating_add(1 + extra_plays.max(0) + i32::from(echo_this_card));
    }

    // ── Cleanup: move Play → Discard or Exhaust ──────────────────────────
    // Bead `sts2-rs-6ynt.3.4`: branch on the engine-side EXHAUST keyword
    // registry (populated by `sts2-content::cards::dispatch::install_default_registry`
    // from each per-character `CardSpec.keywords`). EXHAUST cards (Slimed,
    // Toxic, SporeMind plus non-status exhausts like Feed / Apotheosis /
    // Brand) route to `PileKind::Exhaust` and fire `after_card_exhausted`
    // on both buses, mirroring the `force_exhaust` branch at
    // `sts2-rl-agent@01c094d:sts2_env/core/combat.py:902-904` and the
    // shape of `auto_play_card_from_draw_top` below.
    //
    // The card may not be on Play any more (e.g. an effect routed it
    // elsewhere — Apotheosis self-destructs on play, Sentinel's
    // dust-from-the-block routes through `card_effects::move_pile`). The
    // `move_card` helper returns `Err` in that case which we silently
    // discard, matching the auto-play guard at line ~2283.
    // Bead `sts2-rs-59atd`: `card_exhausts_inst` honors an upgrade-aware
    // `keyword_override` (e.g. BurningPact drops EXHAUST on upgrade) before
    // falling back to the CardId-keyed EXHAUST registry.
    //
    // Bead `sts2-rs-q6bix` (F-R): CorruptionPower routes the owner's played
    // Skills to Exhaust. C# `CorruptionPower.ModifyCardPlayResultPileTypeAndPosition`
    // returns `(PileType.Exhaust, position)` for owner Skills; we OR that into
    // the exhaust predicate so a Skill played while Corruption is active fires
    // the same after-exhaust hooks (FeelNoPain et al.) as an intrinsically
    // exhausting card. Read the mirror+legacy power union via
    // `player_power_amount`, not raw `state.player.powers` (mirror split).
    // Per C#'s pre-OnPlay result-pile snapshot (see
    // `corruption_active_pre_dispatch` above), use the Corruption state as it
    // stood at play-START, not the post-dispatch state.
    let corruption_exhausts_skill =
        card.card_type == cards::CardType::Skill && corruption_active_pre_dispatch;
    // C# `GetResultPileType()` (`CardModel.cs:1608-1619`): `IsDupe || Power`
    // → `PileType.None` (removed from combat) BEFORE the Exhaust-keyword
    // check; Corruption's hook override (owner Skill → Exhaust) then beats
    // the None. A dupe-flagged card hand-played later (Anger's poisoned
    // clone, sweep seed 5192) therefore VANISHES instead of discarding.
    let played_card_is_dupe = card.combat_var(VarKey::IsDupe) != 0;
    // The exhaust disposal defers while a `pending_choice` is open, exactly
    // like the Discard branch below: C# `OnPlayWrapper`'s disposal
    // (`CardModel.cs:1538-1553`) runs strictly after `OnPlay` returns, and
    // an Exhaust-keyword card CAN open a choice (base THINKING_AHEAD's
    // put-back grid; sweep seed 4356 — the eager exhaust fired DarkEmbrace's
    // draw into a spurious reshuffle before the choice resolved).
    // `flush_played_cards_after_choice` routes the deferred card to Exhaust
    // (incl. both exhaust buses) once the choice closes.
    // FeralPower (C# `FeralPower.ModifyCardPlayResultPileTypeAndPosition`): the
    // first `Amount` 0-energy owner Attacks each turn return to Hand/Top instead
    // of their normal result pile (exhaust for DramaticEntrance, discard for a
    // plain 0-cost Attack). `resolved_cost` is the energy actually spent
    // (`ResourceInfo.EnergyValue`), not the printed cost. The per-turn tally
    // `zero_cost_attacks_played_this_turn` is C#'s `zeroCostAttacksPlayed`
    // (seeded on AfterApplied from this turn's history, reset AfterSideTurnStart):
    // it is incremented for EVERY 0-cost owner Attack below so a pre-Feral attack
    // still counts against the budget. Corruption's Skill→Exhaust override is
    // disjoint (Skills vs Attacks), so no precedence conflict.
    let is_zero_cost_attack = card.card_type == cards::CardType::Attack && resolved_cost == 0;
    let feral_returns_to_hand = is_zero_cost_attack
        && !played_card_is_dupe
        && state.pending_choice.is_none()
        && (state.combat_log.zero_cost_attacks_played_this_turn as i32)
            < state.player_power_amount(crate::PowerKind::Feral);
    if feral_returns_to_hand {
        // C# `FeralPower.cs:60` calls `Flash()` in
        // `AfterModifyingCardPlayResultPileOrPosition`, which `Hook` fires only
        // on the models that actually changed the result pile — i.e. exactly
        // when `ModifyCardPlayResultPileTypeAndPosition` returned
        // `(Hand, Top)`. That is this `feral_returns_to_hand` predicate (owner
        // + Attack + `EnergyValue == 0` + `zeroCostAttacksPlayed < Amount`), so
        // the flash rides the redirect decision. The auto-play path carries the
        // same emission for the same C# line, in `finish_auto_played_card`.
        //
        // KNOWN DEVIATION (ordering, shared with the EchoForm flash in this
        // same function): C# runs `AfterModifyingCardPlayResultPileOrPosition`
        // BEFORE the play loop, so the game flashes ahead of the card's own
        // damage numbers. Rust resolves the result pile here, in
        // `finish_card_play`, after the card has resolved and after the
        // after-play dispatch. The COUNT and the CONDITION are right; only the
        // position in the row stream is late, so a replay shows Feral's flash
        // just after the hit instead of just before it. Moving it earlier means
        // hoisting the redirect decision out of the cleanup branch — reshaping
        // live play logic to host a log line, which the porting rules forbid.
        crate::event_log::flash_player_power(state, crate::PowerKind::Feral);
        if let Some(pos) = state.player.play.iter().position(|&u| u == card_uid) {
            state.player.play.remove(pos);
            // C# `CardPilePosition.Top` for the Hand pile = index 0 (drawn/seen
            // first). Overflow-to-discard at HAND_LIMIT, like move_card_to_hand.
            if state.player.hand.len() < crate::cards::HAND_LIMIT {
                state.player.hand.insert(0, card_uid);
            } else {
                state.player.discard.push(card_uid);
            }
        }
    } else if ((crate::card_effects::card_exhausts_inst(&card) && !played_card_is_dupe)
        || corruption_exhausts_skill)
        && state.pending_choice.is_none()
    {
        let _ = cards::move_card(
            &mut state.player,
            card_uid,
            PileKind::Play,
            PileKind::Exhaust,
        );
        fire_after_card_exhausted(
            state,
            card.card_id,
            card_uid,
            target_uid,
            card.tags,
            card.card_type,
        );
    } else if card.card_type == cards::CardType::Power || played_card_is_dupe {
        // Bead `sts2-rs-3k85q`: Powers are removed from combat on play
        // (not routed to any pile). Mirrors Python pin
        // `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1245`
        // (`_default_card_play_result` returns `PileType.NONE` for
        // `CardType.POWER`) and C# `Inflame.cs` / `CardModel.OnPlay`
        // which never enqueue a `CardPileCmd.Add` for Power cards.
        // Routing them to Discard caused +1 discard_count drift vs
        // C# headless on every Power play (parity_fuzz sub-bug D of
        // sts2-rs-8ukqd, 13/27 attributed drifts were INFLAME).
        // A dupe-flagged card shares the `PileType.None` fate (sweep
        // seed 5192).
        if let Some(pos) = state.player.play.iter().position(|&u| u == card_uid) {
            state.player.play.remove(pos);
        }
    } else if state.pending_choice.is_none() {
        // Bead `sts2-rs-q6bix` (PILE_COUNT, NOSTALGIA_CARD): NostalgiaPower
        // sends the owner's first `Amount` Attacks/Skills played each turn to
        // the TOP of the draw pile instead of discard. C#
        // `NostalgiaPower.ModifyCardPlayResultPileTypeAndPosition`: for an
        // Attack/Skill destined for Discard, count this turn's prior
        // Attack/Skill plays (`History.CardPlaysStarted`) and redirect to
        // `(PileType.Draw, CardPilePosition.Top)` while that count `< Amount`.
        // Rust tallies the current play in `cards_played_this_turn` at the
        // `after_card_played` site above (before this routing block), so the
        // equivalent guard is `attacks+skills incl. current <= Amount`. Read
        // the stack off the mirror+legacy union (`player_power_amount`).
        // Corruption's Skill→Exhaust redirect (first branch) takes
        // precedence, matching C#'s `pileType != Discard` early-out.
        let is_attack_or_skill = matches!(
            card.card_type,
            cards::CardType::Attack | cards::CardType::Skill
        );
        let attacks_skills_this_turn = state.player.cards_played_this_turn
            [cards::CardType::Attack.index()]
        .saturating_add(state.player.cards_played_this_turn[cards::CardType::Skill.index()]);
        // C# `ShiningStrike.OnPlay` (ShiningStrike.cs): a non-exhausting copy
        // returns to the TOP of the draw pile (`CardPileCmd.Add(this,
        // PileType.Draw, Top)`). Deferred from the card handler to this
        // single move-out-of-Play step so the card stays on the Play pile
        // across replays (OneTwoPunch / EchoForm) — `current_damage` reads
        // `play.last()` on every replay dispatch, and an inline reinsert
        // emptied the Play pile mid-resolve and crashed the next replay.
        // EXHAUST copies take the first branch above (Play→Exhaust) and never
        // reach here, matching C#'s `!Keywords.Contains(Exhaust)` guard.
        let is_shining_strike = card.card_id == cards::CardId::ShiningStrike;
        // C# `ParticleWall.GetResultPileTypeForCardPlay` (ParticleWall.cs:31-39):
        // a Discard-bound result routes to Hand instead. Deferred from the
        // OnPlay body to this single move-out-of-Play step (bead
        // `sts2-rs-4nhp1`) so the card stays on the Play pile across replays —
        // `current_block` reads `play.last()` on every replay dispatch, and an
        // inline move emptied the pile mid-resolve and crashed the next replay
        // (the ShiningStrike-class bug). A dupe ParticleWall took the
        // `played_card_is_dupe` branch above (removed from combat), matching C#'s
        // `IsDupe -> PileType.None` resolving before the override
        // (`CardModel.cs:2071-2083`), so it never reaches here.
        let is_particle_wall = card.card_id == cards::CardId::ParticleWall;
        if is_shining_strike {
            if let Some(pos) = state.player.play.iter().position(|&u| u == card_uid) {
                state.player.play.remove(pos);
                state.player.draw.insert(0, card_uid);
                // C# `CardPileCmd.Add(this, PileType.Draw, Top)` — a literal
                // physical insert at draw-top, same class as Headbutt/
                // ThinkingAhead. Pin so MCTS PIMC re-shuffles preserve the
                // known next-drawn card. Bead `sts2-rs-09x5n`.
                cards::pin_top(&mut state.player, card_uid);
            }
        } else if is_particle_wall {
            // `move_card_to_hand` removes from the Play pile and pushes to Hand
            // (overflow -> Discard at HAND_LIMIT), the accepted position from
            // bead `sts2-rs-653b`.
            state.move_card_to_hand(card_uid);
        } else if let Some(which) = resolve_draw_top_redirect(
            state,
            is_attack_or_skill,
            i64::from(attacks_skills_this_turn),
            rebound_amount_at_play_start,
        ) {
            // Nostalgia or Rebound sends a Discard-bound result to draw-top,
            // whichever the owner picked up first (GH #650), and only that one
            // gets its after-hook — `NostalgiaPower.cs:56`'s flash, or
            // Rebound's flash-and-decrement.
            fire_draw_top_redirect(state, which);
            if let Some(pos) = state.player.play.iter().position(|&u| u == card_uid) {
                state.player.play.remove(pos);
                state.player.draw.insert(0, card_uid);
                // `(PileType.Draw, CardPilePosition.Top)` is a definite-position
                // insert — pin it so a PIMC re-shuffle preserves the known top
                // card, same as ShiningStrike above. Bead `sts2-rs-09x5n`.
                cards::pin_top(&mut state.player, card_uid);
            }
        } else {
            let _ = cards::move_card(
                &mut state.player,
                card_uid,
                PileKind::Play,
                PileKind::Discard,
            );
        }
        // Bead `sts2-rs-rj1aj`: C# `CardModel.OnPlayWrapper` clears WhenPlayed
        // temporary costs in `EnergyCost.AfterCardPlayedCleanup()`
        // (`CardModel.cs:1555`) — fired AFTER `AfterCardPlayed` and AFTER the
        // card moved to its result pile. `SetToFreeThisTurn` adds an
        // `EndOfTurn | WhenPlayed` modifier (`CardEnergyCost.SetThisTurnOrUntilPlayed`),
        // so a Discovery/InfernalBlade free card's 0-cost clears the moment it
        // is played, not just at turn end. Mirror that late clear here so a
        // free card that returns to hand and is replayed THIS turn pays its
        // full cost on the second play (seed 1798: a Discovery'd UPPERCUT played
        // twice — C# charges 0 then 2). Only on the resolved (non-deferred)
        // path: when a choice is still pending the card sits in play limbo and
        // C# defers `AfterCardPlayedCleanup` until the choice closes.
        state.cards[card_idx].set_combat_var(crate::cards::VarKey::FreeThisTurn, 0);
        // Bead `sts2-rs-rocketpunch-statusgen`: RocketPunch's
        // `SetUntilPlayed(0)` is a `WhenPlayed`-only Absolute(0) modifier, so a
        // RocketPunch made free by a prior status-generation clears the moment
        // it is PLAYED (not at turn end — it survives `clear_per_turn_card_vars`,
        // unlike `FreeThisTurn`). Clear it on this same late
        // `AfterCardPlayedCleanup` path so a replayed RocketPunch pays full cost.
        state.cards[card_idx].set_combat_var(crate::cards::VarKey::FreeUntilPlayed, 0);
        // Bead `sts2-rs-mnjxg`: SneckoOil sets the randomized
        // cost via C# `EnergyCost.SetThisTurnOrUntilPlayed` — the SAME
        // `EndOfTurn | WhenPlayed` modifier as `SetToFreeThisTurn` above. So a
        // Snecko-rolled card's cost reverts to base the moment it is PLAYED, not
        // just at turn end. Unwind the banked `SneckoCostDeltaThisTurn` here
        // (mirroring `restore_snecko_costs_for_turn_start`) so a card that is
        // played and then returns to hand THIS turn (e.g. via BottledPotential's
        // shuffle+redraw, or Pillage/Havoc) is at its BASE cost on the replay —
        // not the stale roll (coverage seed 5880: a SneckoOil'd REND rolled to 1,
        // played, then BottledPotential redrew it; C# shows base 2, RS kept 1 →
        // LEGAL_ACTION divergence at energy 1).
        let snecko_banked =
            state.cards[card_idx].combat_var(crate::cards::VarKey::SneckoCostDeltaThisTurn);
        if snecko_banked != 0 {
            let c = &mut state.cards[card_idx];
            c.cost = c.cost.saturating_sub(snecko_banked as i8);
            c.set_combat_var(crate::cards::VarKey::SneckoCostDeltaThisTurn, 0);
            // The expiring Snecko roll was the absolute modifier the
            // upgrade-clamp flag tracks; the cost is base-owned again.
            c.set_combat_var(crate::cards::VarKey::CostAbsolutelySet, 0);
        }
    }
    // else: the card's OnPlay opened a `pending_choice` (Headbutt's
    // PutBack, Armaments' upgrade-in-hand select, …). C# holds the played
    // card in a "being-played" limbo — NOT yet in any pile — until the
    // awaited choice resolves, then flushes it to discard. We mirror that
    // by leaving the card on the Play pile here and deferring the
    // Play→Discard flush to [`flush_played_cards_after_choice`], invoked
    // from `resolve_choice` once the choice closes. Bead `sts2-rs-8ukqd`
    // (bug D): without this, every discard-destined choice-card shows +1
    // `discard_count` vs C# headless at the mid-choice parity snapshot
    // (parity_fuzz attributed 20 ARMAMENTS + 4 HEADBUTT trials). Exhaust /
    // Power destinations are not deferred — no v0.103.2 content opens a
    // choice from an exhausting or Power card, so those branches keep
    // their eager move.

    // ── Per-turn played-card counters ────────────────────────────────────
    // Bead `sts2-rs-j9x8`: track Skills played this turn so MakeItSo's
    // late-effect can gate "every N skills played → return to hand".
    // Mirrors Python's `_played_cards_this_turn.append(card)` at
    // `sts2-rl-agent@01c094d:sts2_env/core/combat.py:886` — the
    // append happens AFTER `fire_after_card_played` and BEFORE
    // `_fire_after_card_played_late`, so the late-effect sweep sees
    // the just-played card in the count.
    if card.card_type == cards::CardType::Skill {
        state.player.skills_played_this_turn =
            state.player.skills_played_this_turn.saturating_add(1);
    }
    // FeralPower budget tally (C# `zeroCostAttacksPlayed`): increment for EVERY
    // 0-energy owner Attack played this turn, whether or not Feral redirected it
    // above — a 0-cost attack played before Feral was applied still consumes the
    // budget. Read BEFORE this bump by the `feral_returns_to_hand` gate.
    if is_zero_cost_attack {
        state.combat_log.zero_cost_attacks_played_this_turn = state
            .combat_log
            .zero_cost_attacks_played_this_turn
            .saturating_add(1);
    }

    // Bead `sts2-rs-3k85q`: removed a duplicate `move_card(Play→Discard)`
    // that lived here as a stale fallback from the pre-exhaust-registry
    // refactor — the if/else above already routes the played card to its
    // correct destination (Exhaust / Power-removed / Discard). The second
    // move was a no-op for normal cards (card already gone from Play, so
    // `move_card` returned `Err` swallowed by `let _`) but masked the
    // intent of the cleanup block.

    // ── Late-effect sweep ────────────────────────────────────────────────
    // Bead `sts2-rs-j9x8`: walk every card in every pile (Hand /
    // Draw / Discard / Exhaust / Play) and fire any registered
    // late-effect for that card's `CardId`. Mirrors Python's
    // `_fire_after_card_played_late(played_card)` at
    // `sts2-rl-agent@01c094d:sts2_env/core/combat.py:907`. First
    // consumer is MakeItSo (Regent Rare): when N Skills have been
    // played this turn, the watched MakeItSo card is moved from
    // Discard / Exhaust back to Hand. The sweep is one level deeper
    // than `after_card_played` (which is power/relic listener side);
    // late-effects key off the *watched* card's identity rather than
    // a power's owner, so the dispatch site differs.
    crate::card_effects::fire_late_effects(state, &hook_played_ctx);

    // ── VoidForm-style end-turn-after-current-play rider ─────────────────
    // Bead `sts2-rs-3guy`: if the resolved card handler called the Rust
    // analogue of Python's `combat.request_end_turn_after_current_play()`
    // (raised `PlayerCombatState.request_end_turn_after_play`), advance
    // to `PlayerEndTurnPhaseOne` now so the player's remaining turn is
    // skipped. Mirrors the consumer at
    // `sts2-rl-agent@01c094d:sts2_env/core/combat.py:909-911` which runs
    // AFTER the discard/exhaust move + `_fire_after_card_played_late`
    // and BEFORE the implicit return-to-play-loop step — same slot in
    // the Rust pipeline (after `fire_late_effects` above, before the
    // combat-end check below). VoidForm (regent.py:842-846) is the
    // current sole producer at pin 01c094d.
    //
    // Guarded by `phase == PlayerPlayPhase` so a card body that already
    // moved phases (e.g. via a `pending_choice`) does not get
    // double-bumped, and short-circuited when combat ended on the
    // played card so we don't stomp a `CombatWon`/`Lost` end state.
    if state.player.request_end_turn_after_play {
        state.player.request_end_turn_after_play = false;
        if state.phase == CombatPhase::PlayerPlayPhase && !state.is_over {
            // Run the FULL end-turn pipeline synchronously, exactly as
            // C#'s `PlayerCmd.EndTurn(owner, canBackOut: false)` in
            // `VoidForm.OnPlay` (VoidForm.cs:26) does — player end-turn →
            // enemy turn → next player turn. `end_turn_dispatch` is the
            // same chain `Action::EndTurn` runs.
            //
            // It is NOT enough to set `phase = PlayerEndTurnPhaseOne` and
            // return (the prior behavior): nothing pumps that phase. With
            // the engine parked there, `legal_actions` only offers
            // `Action::EndTurn` (lib.rs §"phase != PlayerPlayPhase"),
            // which `apply` no-ops out of play phase — so the enemy turn
            // NEVER ran and the player was immortal after a VoidForm.
            // Regent fuzz seed 130: C#'s TestSubject Bites for 20 and
            // downs the 7-hp player; Rust skipped the enemy turn and the
            // player survived at 7 hp (combat_terminal cs=true rs=false).
            return end_turn_dispatch(state);
        }
    }

    // ── Combat-end check ─────────────────────────────────────────────────
    // Strike may have killed the last enemy → CombatWon.
    check_combat_end(state)
}

/// Flush any cards left on the Play pile to Discard after a pending choice
/// resolves. Companion to the deferred-flush branch in [`play_card`]: when a
/// card's OnPlay opens a `pending_choice`, the Play→Discard move is deferred
/// (C# holds the played card in limbo until the awaited choice completes),
/// leaving the card on the Play pile. Once `resolve_choice` closes the choice
/// it calls this to perform the deferred flush, matching C#'s "card flushes to
/// discard after OnPlay returns" timing. Bead `sts2-rs-8ukqd` (bug D).
///
/// In normal play the Play pile is emptied by each `play_card`, so the only
/// way a card lingers here is the deferral above; choices opened outside a
/// card play (turn-setup, relic/potion) leave Play empty and this is a no-op.
pub fn flush_played_cards_after_choice(state: &mut CombatState) {
    let remaining: smallvec::SmallVec<[crate::CardUid; 4]> =
        state.player.play.iter().copied().collect();
    for uid in remaining {
        // A choice-opening card (Armaments / Headbutt / …) was held in Play
        // limbo while its `pending_choice` resolved, so the immediate
        // result-pile routing in `play_card` (the `pending_choice.is_none()`
        // block) was SKIPPED for it. C# applies the SAME result-pile modifiers
        // when the card finally flushes after the awaited selection completes,
        // so replicate that routing here — Corruption (Skill → Exhaust) and
        // Nostalgia (first-N Attack/Skill → draw-pile Top) — instead of an
        // unconditional Discard. Bead `sts2-rs-qu5wg`: under NostalgiaPower a
        // played-then-upgraded ARMAMENTS went to draw-top in C# (`draw_n` cs=9
        // rs=8) but Rust flushed it straight to discard (SLIMES seed 2387).
        let idx = uid.0.get() as usize - 1;
        let Some(card) = state.cards.get(idx).copied() else {
            let _ = cards::move_card(&mut state.player, uid, PileKind::Play, PileKind::Discard);
            continue;
        };
        // C# `CardModel.OnPlayWrapper` fires `EnergyCost.AfterCardPlayedCleanup()`
        // (drop WhenPlayed cost modifiers, `CardModel.cs:1555`) when the deferred
        // play finally flushes after its awaited choice closes — the same
        // WhenPlayed expiry the immediate path performs in `play_card` (beads
        // `sts2-rs-rj1aj`/`sts2-rs-mnjxg`). Without it a choice-opening free
        // card that returns to hand the same turn keeps its temporary cost
        // (sweep seed 3284: a SkillPotion-free ARMAMENTS redrawn through a
        // reshuffle stayed cost 0; C# charges base 1).
        {
            let c = &mut state.cards[idx];
            c.set_combat_var(crate::cards::VarKey::FreeThisTurn, 0);
            let snecko_banked = c.combat_var(crate::cards::VarKey::SneckoCostDeltaThisTurn);
            if snecko_banked != 0 {
                c.cost = c.cost.saturating_sub(snecko_banked as i8);
                c.set_combat_var(crate::cards::VarKey::SneckoCostDeltaThisTurn, 0);
                c.set_combat_var(crate::cards::VarKey::CostAbsolutelySet, 0);
            }
        }
        let corruption_exhausts_skill = card.card_type == cards::CardType::Skill
            && state.player_power_amount(crate::PowerKind::Corruption) > 0;
        // C# `GetResultPileType` precedence: Corruption's hook override
        // beats the dupe's `PileType.None`, which beats intrinsic Exhaust
        // (sweep seed 5192).
        let flushed_is_dupe = card.combat_var(VarKey::IsDupe) != 0;
        if flushed_is_dupe && !corruption_exhausts_skill {
            if let Some(pos) = state.player.play.iter().position(|&u| u == uid) {
                state.player.play.remove(pos);
            }
            continue;
        }
        if crate::card_effects::card_exhausts_inst(&card) || corruption_exhausts_skill {
            let _ = cards::move_card(&mut state.player, uid, PileKind::Play, PileKind::Exhaust);
            fire_after_card_exhausted(state, card.card_id, uid, None, card.tags, card.card_type);
            continue;
        }
        let is_attack_or_skill = matches!(
            card.card_type,
            cards::CardType::Attack | cards::CardType::Skill
        );
        let attacks_skills_this_turn = state.player.cards_played_this_turn
            [cards::CardType::Attack.index()]
        .saturating_add(state.player.cards_played_this_turn[cards::CardType::Skill.index()]);
        // Same fold as the immediate `play_card` routing block (GH #650). C#
        // resolves the result pile once, at the top of `OnPlayWrapper`, so a
        // play whose `OnPlay` opened a choice carries the same redirect when
        // the choice closes.
        //
        // This path has no play-start snapshot (it flushes whatever is left on
        // the Play pile), so Rebound reads the LIVE stack — the same
        // approximation Nostalgia's turn tally makes here. Unreachable by
        // construction at this pin: the only card granting Rebound is the
        // Rebound card, whose `OnPlay` deals damage and applies a power without
        // opening a choice, so a card can never both create a Rebound stack and
        // land in this flush. A future Rebound source that opens a choice needs
        // the snapshot threaded here too.
        if let Some(which) = resolve_draw_top_redirect(
            state,
            is_attack_or_skill,
            i64::from(attacks_skills_this_turn),
            state.player_power_amount(crate::PowerKind::Rebound),
        ) {
            fire_draw_top_redirect(state, which);
            if let Some(pos) = state.player.play.iter().position(|&u| u == uid) {
                state.player.play.remove(pos);
                state.player.draw.insert(0, uid);
                // Pin so a subsequent PIMC re-shuffle doesn't scramble the
                // known top card. Bead `sts2-rs-09x5n`.
                cards::pin_top(&mut state.player, uid);
            }
        } else {
            let _ = cards::move_card(&mut state.player, uid, PileKind::Play, PileKind::Discard);
        }
    }
}

/// Auto-play the top card of the draw pile, force-exhausting it after the
/// effect resolves. Mirrors Python's `auto_play_card(card, force_exhaust=True)`
/// at `sts2-rl-agent@01c094d:sts2_env/core/combat.py:2067-2089` (which routes
/// into `_execute_card_play(..., spend_energy=False, force_exhaust=True)` at
/// `combat.py:829-867`).
///
/// Pipeline (matches the `spend_energy=False, force_exhaust=True` shape of
/// `_execute_card_play` + `_resume_pending_play`, modulo the deferred
/// `before_card_played` / `EchoForm` replay paths — see divergence notes):
///
///   1. Reshuffle discard into draw if draw is empty.
///   2. If draw is still empty, return `false` (no auto-play happened).
///   3. Pop the top card and push it onto the Play pile (no energy spent,
///      no cost check, no `before_card_played` channel — Python's auto-play
///      does fire `before_card_played` per replay but we elide it here at
///      this pin to avoid Havoc / Free* power double-counting; tracked under
///      the parity follow-up for `sts2-rs-tlyk` if a future bead surfaces a
///      listener that needs it).
///   4. Resolve target (Enemy → first hittable enemy; Self/None/AllEnemies →
///      `None`), mirroring Python's `_resolve_target(card, None)` at
///      `combat.py:1288-1311` for an auto-played card with no explicit target.
///   5. Dispatch the on-play effect.
///   6. Fire both `after_card_played` buses (`relic_effects` + hooks), so
///      Rage / Afterimage / `PhantomBlades` / event-reward relics tick for
///      the auto-played card.
///   7. Move Play → Exhaust and fire both `after_card_exhausted` buses
///      (mirrors Python's `force_exhaust` branch at `combat.py:902-904`).
///
/// **Returns** `true` iff a card was auto-played and exhausted.
///
/// **`EchoForm` divergence (`sts2-rs-tlyk`):** Python's `_execute_card_play`
/// uses `modify_card_play_count` to obtain the replay count, which lets
/// `EchoForm` replay an auto-played card. We do not replay here. Listed under
/// `tests/parity/known_divergences.md` for follow-up if the gap proves
/// observable.
pub fn auto_play_card_from_draw_top(state: &mut CombatState, force_exhaust: bool) -> bool {
    // Reshuffle discard → draw if needed. C# `CardPileCmd.AutoPlayFromDrawPile`
    // routes through `ShuffleIfNecessary` → `Shuffle` → **`Hook.AfterShuffle`**
    // (`CardPileCmd.cs:857`) — so the relic after-shuffle bus AND
    // `StratagemPower.AfterShuffle`'s grid-select fire BEFORE the top card is
    // taken (sweep seeds 5336/2502: under Stratagem, C# pulls the grid pick
    // to hand first and Havoc/Cascade auto-plays what's left; Rust played the
    // pre-grid top card). A Stratagem grid larger than its take opens a
    // pending choice — resolved inline with the harness's first-legal pick
    // (the auto-play runs inside one `apply()`, same as the replay loops).
    if state.player.draw.is_empty() && !state.player.discard.is_empty() {
        crate::cards::discard_pile_into_draw_pile(
            &mut state.player,
            &state.cards,
            &mut state.run_rng.shuffle,
        );
        let ctx = crate::relic_effects::ShuffleCtx {
            from_reshuffle: true,
        };
        let _ = crate::relic_effects::dispatch_after_shuffle_bus(state, &ctx);
        hooks::dispatch_after_shuffle(state, &hooks::CombatLifecycleCtx::default());
        if state.pending_choice.is_some() {
            resolve_inline_auto_play_choices(state);
        }
    }
    let Some(&top_uid) = state.player.draw.first() else {
        return false;
    };
    // Pop the top of draw and push it onto the Play pile, preserving any
    // currently-resolving card underneath (e.g. Havoc itself).
    state.player.draw.remove(0);
    // Bead `sts2-rs-09x5n`: raw physical-top pop, bypasses
    // `cards::pop_next_card` — clear a stale pin on `top_uid` so a later
    // `pop_next_card` doesn't short-circuit to a deterministic pop for a
    // now-possibly-heterogeneous draw and skip a chance node.
    cards::unpin(&mut state.player, top_uid);
    state.player.play.push(top_uid);
    resolve_auto_played_card(state, top_uid, force_exhaust);
    true
}

/// `IAmInvincible.AfterTurnEnd` rider: while the top of the player's draw
/// pile is an `IAmInvincible`, auto-play it (gaining its block). Mirrors C#
/// `IAmInvincible.cs::AfterTurnEnd` — `if drawPile.FirstOrDefault() == this {
/// AutoPlayFromDrawPile(1, Top, forceExhaust: false) }`. The C# hook is
/// per-card-instance, but because each play removes the card from draw, the
/// observable net is a top-of-pile loop: a single copy plays once, and stacked
/// copies each fire as they become the new top. Combat-end aware. No reshuffle:
/// the C# auto-play is gated on the card already being on draw-top, so an empty
/// draw pile is a no-op (we never trigger `ShuffleIfNecessary`).
fn auto_play_i_am_invincible_from_draw_top(state: &mut CombatState) {
    // Bound the loop by the live draw-pile size to guarantee termination even
    // if a future on-play effect ever re-inserts a card on top.
    let mut budget = state.player.draw.len();
    while budget > 0 {
        budget -= 1;
        if state.is_over {
            return;
        }
        let Some(&top_uid) = state.player.draw.first() else {
            return;
        };
        let top_idx = top_uid.0.get() as usize - 1;
        let is_invincible = state
            .cards
            .get(top_idx)
            .is_some_and(|c| c.card_id == cards::CardId::IAmInvincible);
        if !is_invincible {
            return;
        }
        state.player.draw.remove(0);
        // Bead `sts2-rs-09x5n`: raw physical-top pop, bypasses
        // `cards::pop_next_card` — see `auto_play_card_from_draw_top`.
        cards::unpin(&mut state.player, top_uid);
        state.player.play.push(top_uid);
        resolve_auto_played_card(state, top_uid, false);
    }
}

/// Auto-play one **specific** card (by uid) that currently lives in the
/// draw pile, mirroring C# `CardCmd.AutoPlay(choiceContext, card, null)`
/// for a card the caller has already selected from draw (e.g. Catastrophe
/// `StableShuffle(...).FirstOrDefault()`). Removes the card from draw,
/// pushes it onto Play, and resolves its on-play effect + buses, routing
/// it to its natural destination pile (discard / self-exhaust /
/// removed-for-Power). Returns `false` (no-op) if the uid is not in draw.
/// Bead `sts2-rs-q6bix`.
pub fn auto_play_specific_card(
    state: &mut CombatState,
    uid: crate::CardUid,
    force_exhaust: bool,
) -> bool {
    let Some(pos) = state.player.draw.iter().position(|&u| u == uid) else {
        return false;
    };
    state.player.draw.remove(pos);
    state.player.play.push(uid);
    resolve_auto_played_card(state, uid, force_exhaust);
    true
}

/// Auto-play one **specific** card (by uid) that currently lives in the
/// **discard** pile, mirroring C# `CardCmd.AutoPlay(choiceContext, card,
/// target)` for a card the caller selected out of the discard pile (e.g.
/// BeatDown `Discard.Where(Attack && !Unplayable).StableShuffle(...)
/// .Take(N)`). Removes the card from discard, pushes it onto Play, and
/// resolves its on-play effect + buses, routing it to its natural
/// destination pile (discard / self-exhaust / removed-for-Power). The
/// target roll (a `CombatTargets.NextItem` read for `TargetType::Enemy`,
/// i.e. C# `AnyEnemy`) happens inside [`resolve_auto_played_card`], so this
/// shares the exact RNG-cursor shape of the draw-source variant. The uid is
/// located by scanning Discard, then Draw, Hand, and finally **Exhaust** — the
/// last covers a self-exhausting Skill replayed in a loop (DecisionsDecisions
/// auto-plays the picked Skill `RepeatVar(3)` times; a card like BigBang
/// self-exhausts on play 1 and is pulled back out of Exhaust on plays 2/3, per
/// C# `OnPlayWrapper`'s unconditional re-Add to Play, `CardModel.cs:1880`).
/// Returns `false` (no-op) if the uid is in none of those piles. Beads
/// `sts2-rs-br2cm` / `sts2-rs-6em8x`; issue #177.
pub fn auto_play_specific_card_from_discard(
    state: &mut CombatState,
    uid: crate::CardUid,
    force_exhaust: bool,
) -> bool {
    // C# `BeatDown` captures the take-list CardModel *references* before the
    // auto-play loop, then `CardCmd.AutoPlay(item)` plays each BY REFERENCE
    // regardless of which pile it now sits in (`CardCmd.cs:33-109`: it only
    // `Add`s to Play when the card is pile-less). A sibling auto-play (e.g.
    // BeatDown -> Headbutt, whose `FromSimpleGrid` puts a discard card on
    // Draw-Top) or a mid-loop reshuffle can relocate this captured card out of
    // discard — C# still plays it, RS used to bail with `return false` and
    // silently skip it. Scan discard, then the other live piles. Bead
    // `sts2-rs-6em8x` (sweep a4096, seeds 1338/2478/5172).
    //
    // `Exhaust` is searched LAST so a self-exhausting Skill replayed in a loop
    // (e.g. DecisionsDecisions -> BigBang, RepeatVar(3)) is re-found on the 2nd
    // and 3rd iterations after it self-exhausted on the 1st. C# plays the
    // captured `CardModel` reference regardless of pile: `OnPlayWrapper`
    // UNCONDITIONALLY re-adds the card to Play for an auto-play
    // (`CardModel.cs:1880`), so an exhausted card is pulled back out of Exhaust
    // into Play, fires `OnPlay` again, and re-routes to Exhaust
    // (`CardModel.cs:1985-1986`). Non-exhaust Skills still resolve from
    // Discard/Draw/Hand first, so this only affects the self-exhaust case.
    // Sweep 5164 (DecisionsDecisions|BigBang, issue #177).
    let from = [
        crate::cards::PileKind::Discard,
        crate::cards::PileKind::Draw,
        crate::cards::PileKind::Hand,
        crate::cards::PileKind::Exhaust,
    ]
    .into_iter()
    .find(|&p| crate::cards::pile_iter(&state.player, p).any(|u| u == uid));
    let Some(from) = from else {
        return false;
    };
    let _ = crate::cards::move_card(&mut state.player, uid, from, crate::cards::PileKind::Play);
    resolve_auto_played_card(state, uid, force_exhaust);
    true
}

/// Auto-play one **specific** card (by uid) that currently lives in the
/// **exhaust** pile, at an EXPLICIT target, mirroring C# `CardCmd.AutoPlay(
/// choiceContext, card, cardPlay.Target)` for KnifeTrap, whose `OnPlay`
/// iterates `Exhaust.Cards.Where(Shiv)` and auto-plays each at the KnifeTrap's
/// own target (`KnifeTrap.cs:38-49`). C# `OnPlayWrapper` re-adds the played
/// card to Play (`CardModel.cs:1880`) and routes a self-exhausting Shiv back to
/// the END of the exhaust pile via `CardCmd.Exhaust` (`CardModel.cs:1986`), so
/// the played Shivs are reordered to exhaust-tail in iteration order — exactly
/// what this `Exhaust → Play → resolve → Exhaust` path reproduces. Because the
/// caller supplies the target, NO `combat_targets` RNG read is consumed (see
/// [`resolve_auto_played_card_at_target`]). Returns `false` (no-op) if the uid
/// is not in the exhaust pile.
pub fn auto_play_specific_card_from_exhaust_at_target(
    state: &mut CombatState,
    uid: crate::CardUid,
    target: Option<CreatureUid>,
    force_exhaust: bool,
) -> bool {
    if !state.player.exhaust.contains(&uid) {
        return false;
    }
    let _ = crate::cards::move_card(
        &mut state.player,
        uid,
        crate::cards::PileKind::Exhaust,
        crate::cards::PileKind::Play,
    );
    resolve_auto_played_card_at_target(state, uid, target, force_exhaust);
    true
}

/// Auto-play up to `count` cards off the top of the draw pile, mirroring
/// C# `CardPileCmd.AutoPlayFromDrawPile` (Cascade / DistilledChaos /
/// Mayhem). **Two-phase**, exactly like the decomp:
///
///   1. **Collect** up to `count` top cards into the Play pile, calling
///      `ShuffleIfNecessary` (reshuffle discard → draw when draw is empty)
///      before each pick. Crucially the collected cards sit in **Play**,
///      not discard — so once the draw pile empties with nothing left in
///      discard to reshuffle, collection stops. This is why Cascade for 10
///      on a 2-card draw plays exactly 2: the cards can't cycle back into
///      draw mid-collection.
///   2. **Play** each collected card in collection order, routing it to its
///      natural pile (discard, self-exhaust, or removed for a Power) unless
///      `force_exhaust` is set.
///
/// A single-card auto-play (Havoc, count = 1) is equivalent to
/// [`auto_play_card_from_draw_top`]; the distinction only matters for
/// `count > 1`, where the per-card-loop-with-reshuffle shape would wrongly
/// replay discarded cards. Bead `sts2-rs-q6bix` (F-F).
pub fn auto_play_n_from_draw_top(state: &mut CombatState, count: i32, force_exhaust: bool) {
    // Phase 1: collect up to `count` top cards into Play.
    let mut collected: smallvec::SmallVec<[crate::CardUid; 8]> = smallvec::SmallVec::new();
    for _ in 0..count {
        if state.player.draw.is_empty() && !state.player.discard.is_empty() {
            crate::cards::discard_pile_into_draw_pile(
                &mut state.player,
                &state.cards,
                &mut state.run_rng.shuffle,
            );
            // C# `ShuffleIfNecessary` fires `Hook.AfterShuffle` per gather
            // reshuffle (`CardPileCmd.cs:857`) — relic bus + Stratagem grid,
            // resolved inline like the single-card variant above.
            let ctx = crate::relic_effects::ShuffleCtx {
                from_reshuffle: true,
            };
            let _ = crate::relic_effects::dispatch_after_shuffle_bus(state, &ctx);
            hooks::dispatch_after_shuffle(state, &hooks::CombatLifecycleCtx::default());
            if state.pending_choice.is_some() {
                resolve_inline_auto_play_choices(state);
            }
        }
        let Some(&top_uid) = state.player.draw.first() else {
            break;
        };
        state.player.draw.remove(0);
        // Bead `sts2-rs-09x5n`: raw physical-top pop, bypasses
        // `cards::pop_next_card` — see `auto_play_card_from_draw_top`.
        cards::unpin(&mut state.player, top_uid);
        state.player.play.push(top_uid);
        // Mark this card as a batch-pending play: it now sits on the Play pile
        // (phase 1) but its own play has NOT started, so the Nostalgia
        // result-pile redirect must not count it as a started Attack/Skill play
        // when an EARLIER batch card is routed. Cleared when its play starts in
        // `resolve_auto_played_card_at_target`. Bead `sts2-rs-1sew4`.
        {
            let idx = top_uid.0.get() as usize - 1;
            if let Some(c) = state.cards.get_mut(idx) {
                c.set_combat_var(crate::cards::VarKey::PendingBatchAutoPlay, 1);
            }
        }
        collected.push(top_uid);
    }
    // Phase 2: play each collected card in order.
    //
    // Parity bug `combat_select_ironclad`: a collected card whose `OnPlay`
    // opens a SURFACING card-select suspends (`resolve_auto_played_card` set
    // `pending_autoplay_resume` and returned without finishing its tail). When
    // that happens we must stash the NOT-YET-played remainder on the resume
    // marker and stop — the resolver finishes the suspended card's tail then
    // plays the remainder (with `autoplay_inline_resolve` set so their further
    // choices drain inline, mirroring C# `AutoResolveInline` for a resumed
    // play). Cascade-for-N where the FIRST collected card surfaces leaves N-1
    // cards on Play to be resumed.
    let mut it = collected.into_iter();
    for uid in it.by_ref() {
        resolve_auto_played_card(state, uid, force_exhaust);
        if let Some(resume) = state.pending_autoplay_resume.as_mut() {
            // The card WE just resolved (`uid`) is the one that suspended. If it
            // surfaced directly its finish is the resume's top-level `finish`;
            // but if it auto-played a nested card that surfaced FIRST, `uid`'s
            // finish is the LAST parent-chain frame instead (seed 2752 HAVOC).
            // Either way, OUR not-yet-played siblings belong with `uid`'s frame
            // — they must replay only after `uid`'s tail runs — so attach them
            // to the parent frame when present, else the top-level slot.
            if let Some(parent) = resume.parent_chain.last_mut() {
                parent.remaining = it.collect();
                parent.force_exhaust = force_exhaust;
            } else {
                resume.remaining = it.collect();
                resume.force_exhaust = force_exhaust;
            }
            return;
        }
    }
}

/// Drain every `pending_choice` opened by an auto-played card's OnPlay,
/// resolving each INLINE the same way the parity harness's
/// `_drain_rust_choices` does: apply the FIRST legal `ResolveChoice` from
/// [`crate::legal_actions`], looping until no choice remains. Bead
/// `sts2-rs-y1fic`.
///
/// **Why mirror the harness exactly.** A non-auto-play card that opens a
/// `CardSelectCmd` is resolved by the player/agent picking a selection, which
/// the harness drives by taking `legal_actions()[0]` and applying it. C#'s
/// headless `ICardSelector` resolves an auto-play's inline choice through the
/// SAME selection-resolution path. So to keep auto-play parity-faithful — both
/// to C# and to how the harness drains a normal choice — the inline resolver
/// must pick the identical first-legal selection (option index 0 / the
/// min-pick prefix), not invent a bespoke "exhaust all" / "take none" rule.
/// Each iteration is bounded (the loop terminates once `pending_choice` clears
/// and stays clear); the `max` guard is a defensive cap against a pathological
/// self-reopening choice.
/// C# `Hook.ShouldPlay` union for a player card — every listener that can
/// veto a play at this pin:
///
/// * `ChainsOfBindingPower.ShouldPlay` (bead `sts2-rs-f0own`): once a
///   Bound-afflicted card has been played this turn, further Bound cards
///   are blocked. Gated on the player actually owning ChainsOfBinding so a
///   stale `chains_bound_card_played_this_turn` flag can't strand plays.
/// * `SlothPower.ShouldPlay` (`SlothPower.cs:22-29`, bead `sts2-rs-ybtmu`):
///   at most `Amount` card plays per turn.
/// * `RingingPower.ShouldPlay` (bead `sts2-rs-xk9yo`): exactly one card
///   play per turn.
/// * `VelvetChoker.ShouldPlay` (GH #742): at most six card plays per turn —
///   the relic half, [`hooks::relic_should_play_blocked`].
/// * `SmoggyPower.ShouldPlay` (`SmoggyPower.cs:64-71`): a player-owned card
///   carrying the `Smog` card-affliction cannot be played while the player
///   has SmoggyPower. Since `SmoggyPower.AfterCardPlayed` smogs every
///   unafflicted player Skill the moment a Skill is played, this is a
///   one-Skill-per-turn lockout (LivingFog ADVANCED_GAS). Verified against a
///   live-game wire capture 2026-08-03: the game reports such cards as
///   `playable:false` / `unplayable_reason:"BlockedByHook"`, and MCTS lines
///   that planned a second Skill were rejected at the wire.
///
/// Checked by `play_card` AND by the auto-play resolver — C#
/// `CardCmd.AutoPlay` runs the same hook before playing (`CardCmd.cs:38-46`)
/// and reroutes blocked cards via `MoveToResultPileWithoutPlaying` (sweep
/// seeds 3931/5571: a Ringing/Bound-blocked Cascade/Stampede auto-play
/// actually resolved in Rust).
pub fn player_should_play_blocked(state: &CombatState, card: &crate::cards::Card) -> bool {
    if hooks::relic_should_play_blocked(state) {
        return true;
    }
    if matches!(
        card.affliction,
        Some(crate::card_afflictions::CardAffliction {
            kind: crate::card_afflictions::CardAfflictionKind::Bound,
            ..
        })
    ) && state.combat_log.chains_bound_card_played_this_turn
    {
        let player_has_chains = state.player.powers.iter().any(|puid| {
            state
                .powers
                .get_by_id(puid.0)
                .is_some_and(|p| p.kind == crate::powers::PowerKind::ChainsOfBinding)
        });
        if player_has_chains {
            return true;
        }
    }
    // C# `SmoggyPower.ShouldPlay` (`SmoggyPower.cs:64-71`):
    // `card.Owner != Owner.Player ? true : !(card.Affliction is Smog)` — i.e.
    // the play is vetoed exactly when a player-owned card carries `Smog`. Gated
    // on the player actually holding SmoggyPower (the C# listener only exists
    // while the power does), read through the mirror+legacy union because both
    // ADVANCED_GAS and the wire reconstruction install it on the legacy surface.
    if matches!(
        card.affliction,
        Some(crate::card_afflictions::CardAffliction {
            kind: crate::card_afflictions::CardAfflictionKind::Smog,
            ..
        })
    ) && state.player_power_amount(crate::powers::PowerKind::Smoggy) > 0
    {
        return true;
    }
    // Both Sloth (`BeforeCardPlayed` counter) and Ringing
    // (`CardPlaysStarted` scan) count STARTED plays — an in-flight parent
    // play (Cascade/Havoc gathering its auto-plays) counts.
    if let Some(sloth_amount) = state.player.powers.iter().find_map(|puid| {
        state
            .powers
            .get_by_id(puid.0)
            .filter(|p| p.kind == crate::powers::PowerKind::Sloth)
            .map(|p| p.amount)
    }) {
        if state.player.card_plays_started_this_turn >= sloth_amount.max(0) as u32 {
            return true;
        }
    }
    if state.player.card_plays_started_this_turn >= 1 {
        let player_has_ringing = state.player.powers.iter().any(|puid| {
            state
                .powers
                .get_by_id(puid.0)
                .is_some_and(|p| p.kind == crate::powers::PowerKind::Ringing)
        });
        if player_has_ringing {
            return true;
        }
    }
    false
}

fn resolve_inline_auto_play_choices(state: &mut CombatState) {
    // Bound matches the harness's `_drain_rust_choices(max_iters=16)`.
    for _ in 0..16 {
        if state.pending_choice.is_none() {
            return;
        }
        // The auto-play loop runs entirely inside one `apply()`, so a choice
        // opened mid-loop can never surface to the harness between collected
        // cards. Resolve it here by replaying the harness's first-legal pick.
        //
        // IMPORTANT: mirror the harness's `_drain_rust_choices` drive exactly.
        // The harness drives the FIRST legal selection whose index list is
        // non-empty (the min-pick prefix `[0..min_select.max(1)]`), passing the
        // WHOLE list through the `choose` binding — so a multi-pick choice
        // (HIDDEN_DAGGERS `DiscardFromHand` min=max=2 ⇒ `[0,1]`) discards ALL
        // required cards, not just option 0. The prior code collapsed every
        // selection to its first index (`[idx]`), which silently dropped the
        // SECOND required HIDDEN_DAGGERS discard (Silent parity seeds 81/214 —
        // bead `sts2-rs-hd-multipick-discard-uhsk1`).
        //
        // For a skippable / `min_select==0` choice (e.g. Purity's
        // `ExhaustFromHand` min0/max3), `legal_actions` lists the empty-skip
        // selection FIRST; the harness skips it and takes the first non-empty
        // `[0]` (C# headless never skips — it always picks option 0). We mirror
        // that by selecting the first non-empty selection, falling back to the
        // first action's leading index `[idx]` only when EVERY legal action is
        // the empty skip (e.g. an empty-options choice) — byte-identical to the
        // harness's `index`-default-0 fallback.
        //
        // Bead `sts2-rs-sopgf`: a `max_select > 1` choice is now decomposed
        // into SEQUENTIAL single picks. To keep the drained set byte-identical
        // to the prior "first-K" result, drive the lowest-index still-unpicked
        // option each iteration UNTIL `picked.len()` reaches `min_select.max(1)`
        // (the prior `for k in min.max(1)..=hi` loop's first non-empty K — the
        // harness/C# headless NEVER skips, it picks option 0 even on a
        // `min_select == 0` choice like Purity's exhaust), THEN take the
        // FINALIZE (empty selection). The outer loop re-enters until
        // `pending_choice` clears at FINALIZE.
        let selection = {
            let pc = state
                .pending_choice
                .as_ref()
                .expect("checked is_some above");
            if pc.max_select > 1 {
                let target = usize::from(pc.min_select).max(1);
                if pc.picked.len() >= target {
                    // Target satisfied: FINALIZE (runs the kind-resolver over
                    // the accumulated set, clearing the choice).
                    smallvec::SmallVec::<[u8; 4]>::new()
                } else {
                    // Append the lowest-index option not yet picked.
                    #[allow(clippy::cast_possible_truncation)]
                    let next = (0..pc.options.len() as u8)
                        .find(|i| !pc.picked.contains(i))
                        .unwrap_or(0u8);
                    smallvec::smallvec![next]
                }
            } else {
                // Single-pick / empty-options: replay the harness's first
                // non-empty (else first-leading-index) selection.
                let acts = crate::legal_actions(state);
                let mut first_idx: u8 = 0;
                let mut saw_choice = false;
                let mut chosen: Option<smallvec::SmallVec<[u8; 4]>> = None;
                for a in acts {
                    if let crate::Action::ResolveChoice { selection } = a {
                        if !saw_choice {
                            first_idx = selection.first().copied().unwrap_or(0u8);
                            saw_choice = true;
                        }
                        if !selection.is_empty() {
                            chosen = Some(selection);
                            break;
                        }
                    }
                }
                if !saw_choice {
                    // `pending_choice` is set but `legal_actions` emitted no
                    // `ResolveChoice` (only possible if the state is terminal /
                    // out-of-phase). Bail rather than spin.
                    return;
                }
                chosen.unwrap_or_else(|| smallvec::smallvec![first_idx])
            }
        };
        // Resolve via the shared choice body with `flush_played = false`: the
        // collected sibling auto-play cards still sit on the Play pile and must
        // not be swept to discard by this mid-loop resolution (each is routed
        // by its own `resolve_auto_played_card` cleanup).
        let _ = crate::resolve_choice_action(state, &selection, /*flush_played=*/ false);
    }
}

/// Resolve a single card that is already sitting on the Play pile as an
/// auto-play: pick its target, dispatch the on-play effect, fire the
/// `after_card_played` buses, bump the per-turn play counter, then route
/// the card to its destination pile. `force_exhaust` mirrors C#
/// `card.ExhaustOnNextPlay` — when set the card always exhausts; otherwise
/// it follows natural routing (discard / self-exhaust / removed-for-Power).
/// Shared by [`auto_play_card_from_draw_top`],
/// [`auto_play_n_from_draw_top`], and the hand-side
/// `StampedePower` auto-play in [`crate::hooks`]. Bead `sts2-rs-q6bix` (F-F).
/// Record one FINISHED auto-play in the per-turn play bookkeeping that C#'s
/// `History.CardPlaysFinished` feeds: the per-turn `cards_played_this_turn`
/// bucket (read by Conflagration / Juggling / Finisher / LunarBlast / FTL),
/// the combat-long ethereal tally, and the Stomp/Pinpoint per-card cost shaves.
/// Mirrors the identical hand-play sequence (combat_loop.rs:4735-4764). Called
/// once for the initial auto-play AND once per OneTwoPunch/base_replay replay,
/// so a replayed scaling Attack observes its own earlier plays — matching C#'s
/// one-`CardPlayFinished`-per-OnPlay-iteration semantics
/// (bead `sts2-rs-distilled-chaos-replay-scale`).
fn auto_play_tally_finished(
    state: &mut CombatState,
    card_type: crate::cards::CardType,
    card_idx: usize,
) {
    let idx = card_type.index();
    state.player.cards_played_this_turn[idx] =
        state.player.cards_played_this_turn[idx].saturating_add(1);
    if state
        .cards
        .get(card_idx)
        .is_some_and(crate::card_effects::card_ethereal_inst)
    {
        state.combat_log.ethereal_cards_played_this_combat = state
            .combat_log
            .ethereal_cards_played_this_combat
            .saturating_add(1);
    }
    if card_type == crate::cards::CardType::Attack {
        state.discount_stomp_on_attack_played();
    }
    if card_type == crate::cards::CardType::Skill {
        state.discount_pinpoint_on_skill_played();
    }
}

pub(crate) fn resolve_auto_played_card(
    state: &mut CombatState,
    top_uid: crate::CardUid,
    force_exhaust: bool,
) {
    resolve_auto_played_card_at_target(
        state,
        top_uid,
        /*target_override=*/ None,
        force_exhaust,
    );
}

/// As [`resolve_auto_played_card`], but the caller supplies an EXPLICIT target
/// for an `Enemy`-targeted auto-play instead of letting the resolver roll one
/// off the `combat_targets` stream. Mirrors C# `CardCmd.AutoPlay(choiceContext,
/// card, target)` with a NON-null `target` (e.g. KnifeTrap auto-playing each
/// exhaust-pile Shiv at `cardPlay.Target`): AutoPlay only rolls
/// `Rng.CombatTargets.NextItem` when `target == null` (`CardCmd.cs:78-79`), so a
/// caller-supplied target consumes NO RNG read. `None` reproduces the legacy
/// random-roll behavior byte-for-byte.
pub(crate) fn resolve_auto_played_card_at_target(
    state: &mut CombatState,
    top_uid: crate::CardUid,
    target_override: Option<CreatureUid>,
    force_exhaust: bool,
) {
    crate::cards::emit_pile_trace(
        &state.player,
        &state.cards,
        "ap",
        state.run_rng.shuffle.counter(),
        Some(top_uid),
    );
    // This card's play is STARTING now — clear the batch-pending marker so the
    // Nostalgia result-pile redirect (in `finish_auto_played_card`) stops
    // treating it as a not-yet-played pre-loaded sibling. A non-batch auto-play
    // never carries the flag, so this is a no-op for them. Bead `sts2-rs-1sew4`.
    {
        let idx = top_uid.0.get() as usize - 1;
        if let Some(c) = state.cards.get_mut(idx) {
            c.set_combat_var(crate::cards::VarKey::PendingBatchAutoPlay, 0);
        }
    }
    // Bead `sts2-rs-8ett`: an auto-played card (Havoc/Cascade/DistilledChaos/
    // KnifeTrap/Catastrophe/IAmInvincible…) is a first-in-series card play in
    // C# — `CardCmd.AutoPlay` records a `CardPlayStartedEntry` with
    // `IsFirstInSeries`, so it advances `EchoFormPower`'s `num` exactly like a
    // manual play. Count it here so a card played AFTER an auto-play (or after
    // an EchoForm gained via an auto-played power card) sees the correct
    // first-in-series index (Defect sweep `PLAYER_STATE|SUPERCRITICAL` seed
    // 2737 / `MONSTER_STATE|DARK_SHACKLES` seed 4641: DistilledChaos auto-plays
    // EchoForm mid-turn, then a manual card must NOT echo because `num >=
    // Amount`). The auto-play itself is not echo-replayed here — that matches
    // the pre-existing behavior; only the count is corrected.
    state.player.cards_first_in_series_this_turn = state
        .player
        .cards_first_in_series_this_turn
        .saturating_add(1);
    bump_echo_form_series_tally(state);
    // GH #637: Rebound stacks BEFORE this auto-play's `OnPlay`, for the result
    // pile decided in `finish_auto_played_card` below. C# resolves the pile at
    // the top of `OnPlayWrapper` for auto-plays too (`IsAutoPlay = true` goes
    // through the same wrapper), so an auto-played Rebound card must NOT
    // redirect itself — Stampede auto-playing Rebound off the draw top is a
    // reachable path for exactly that.
    let rebound_at_auto_play_start = state.player_power_amount(crate::PowerKind::Rebound).max(0);
    // The on-play handlers read the card being played via `play.last()`
    // (`card_effects::current_card_{block,damage,upgraded,energy_spent}` and
    // the per-card equivalents). `auto_play_n_from_draw_top` pre-collects ALL
    // cards into Play (Phase 1) before resolving them (Phase 2), so for every
    // collected card EXCEPT the last, `play.last()` points at a SIBLING — the
    // handler then reads the wrong card's base_block / base_damage / upgrade
    // and silently drops its effect (e.g. an auto-played Defend gaining 0
    // block because it read a sibling Strike's base_block=0; the Cascade
    // `player.block` coverage-sweep divergence). Restore the "card being
    // played is play.last()" invariant by rotating `top_uid` to the end of
    // Play before dispatch. Single auto-plays (Havoc) already have it last, so
    // this is a no-op there. Bead `sts2-rs-gpsvo`.
    if let Some(pos) = state.player.play.iter().position(|&u| u == top_uid) {
        if pos != state.player.play.len() - 1 {
            let u = state.player.play.remove(pos);
            state.player.play.push(u);
        }
    }
    // Read card metadata.
    let top_idx = top_uid.0.get() as usize - 1;
    let top_card = &state.cards[top_idx];
    let top_card_id = top_card.card_id;
    // catastrophe-autoplay bug 1: fold the same dynamic Shiv/FanOfKnives
    // TargetType override the manual play path uses (see `play_card` ~5293).
    // C# `Shiv.cs:23-32` makes Shiv `AllEnemies` (auto-hits every enemy, no
    // target roll) while FanOfKnives is active, so an AUTO-played Shiv (via
    // Catastrophe / DistilledChaos) must NOT roll `Rng.CombatTargets.NextItem`
    // for a single Enemy target — that consumed read desynced the stream for
    // the rest of combat. Resolve the effective type BEFORE the Enemy-target
    // roll below. Shared predicate — GH #335 (see
    // `cards::effective_target_type`).
    let top_target_type = cards::effective_target_type(state, top_card);
    let top_card_type = top_card.card_type;
    let top_card_tags = top_card.tags;
    // Bead `sts2-rs-ph5mx`: snapshot the powered-hit counter before this
    // auto-played card's effect runs, so the Vigor/Gigantification consume can
    // tell whether the card ACTUALLY executed a powered attack (C# latches on
    // `BeforeAttack`). An X-cost AoE attack auto-played at X=0 (Whirlwind via
    // Cascade) is an Attack-type card that deals zero hits — it must NOT consume
    // Vigor (seed 3057).
    state.player.powered_hits_at_card_play_start = state.player.powered_hits_this_turn;
    state.player.attack_command_suppressed_this_play = false;

    // UNPLAYABLE cards (curses / status — Burn, Dazed, Wound, Slimed,
    // AscendersBane, …) that surface off the draw-pile top are NOT played.
    // C# `CardCmd.AutoPlay` short-circuits at the very top
    // (`CardCmd.cs:41-45`): if the card has `CardKeyword.Unplayable` it calls
    // `MoveToResultPileWithoutPlaying`, which routes the card to Discard
    // (or Exhaust if `ExhaustOnNextPlay`/Exhaust keyword) WITHOUT running the
    // on-play effect, WITHOUT firing `AfterCardPlayed`, and WITHOUT bumping
    // the cards-played counter (`CardModel.cs:1621-1638`). The normal
    // `play_card` path never hits this because unplayable cards are filtered
    // out of `legal_actions`, but the auto-play-from-draw primitives
    // (DistilledChaos / Cascade / Mayhem / Havoc) bypass that filter, so we
    // must replicate the C# guard here or the auto-played curse/status would
    // wrongly dispatch its on-play body and fire `after_card_played`
    // listeners. Bead `sts2-rs-ane8r`.
    if crate::card_effects::card_unplayable_inst(&state.cards[top_idx]) {
        if state.player.play.contains(&top_uid) {
            let self_exhausts = crate::card_effects::card_exhausts_inst(&state.cards[top_idx]);
            let dest = if force_exhaust || self_exhausts {
                PileKind::Exhaust
            } else {
                PileKind::Discard
            };
            let _ = cards::move_card(&mut state.player, top_uid, PileKind::Play, dest);
            // C# `MoveToResultPileWithoutPlaying` exhausts via `CardCmd.Exhaust`
            // when `ExhaustOnNextPlay`/Exhaust-keyword, which fires the
            // exhaust hooks; the plain Discard branch fires nothing. Mirror
            // that: only dispatch the exhaust buses when we actually
            // exhausted. (DistilledChaos passes `force_exhaust = false` and
            // unplayable status cards lack the Exhaust keyword, so this branch
            // is dead for DistilledChaos itself and only matters for a future
            // force-exhaust auto-play of an unplayable card.)
            if matches!(dest, PileKind::Exhaust) {
                fire_after_card_exhausted(
                    state,
                    top_card_id,
                    top_uid,
                    None,
                    top_card_tags,
                    top_card_type,
                );
            }
        }
        return;
    }

    // AnyAlly cards (BelieveInYou, Coordinate, DemonicShield, Ignition,
    // InterceptCard, Largesse, Lift, Mimic) require a living ally to target.
    // C# `CardCmd.AutoPlay` (CardCmd.cs:68-80) checks if the card's
    // TargetType is AnyAlly and there are no living allies, then calls
    // `MoveToResultPileWithoutPlaying` without consuming an RNG read. This
    // mirrors the legal-actions gate in `legal_actions` (lib.rs ~7129). Bead
    // sts2-rs-ngwsg: without this guard, auto-played AnyAlly cards like
    // DemonicShield from Cascade would execute unconditionally even with no
    // ally present, causing unintended effects (e.g. DemonicShield's 1 HP
    // self-damage executing in single-player where no ally exists).
    if crate::card_effects::card_is_any_ally(top_card_id) && !state.has_any_ally_card_target() {
        if state.player.play.contains(&top_uid) {
            // C# `MoveToResultPileWithoutPlaying` (`CardModel.cs:1621-1638`):
            // a card pulled off Play without being played EXHAUSTS when
            // `ExhaustOnNextPlay || Keywords.Contains(Exhaust)`, otherwise
            // discards. DemonicShield is AnyAlly + intrinsic Exhaust, so in
            // single-player (no ally) Cascade's `forceExhaust:false` auto-play
            // must still route it to Exhaust (and fire the exhaust hooks), not
            // Discard. The prior hardcoded Discard dropped it into the discard
            // pile (fuzz seed 1111: discard cs=3 rs=4, exhaust cs=4 rs=3).
            // Bead `sts2-rs-ngwsg` follow-up. Same shape as the unplayable
            // guard above.
            let self_exhausts = crate::card_effects::card_exhausts_inst(&state.cards[top_idx]);
            let dest = if force_exhaust || self_exhausts {
                PileKind::Exhaust
            } else {
                PileKind::Discard
            };
            let _ = cards::move_card(&mut state.player, top_uid, PileKind::Play, dest);
            if matches!(dest, PileKind::Exhaust) {
                fire_after_card_exhausted(
                    state,
                    top_card_id,
                    top_uid,
                    None,
                    top_card_tags,
                    top_card_type,
                );
            }
        }
        return;
    }

    // C# `CardCmd.AutoPlay` checks `Hook.ShouldPlay` BEFORE playing
    // (`CardCmd.cs:38-46`): a Ringing/Sloth/Bound-blocked auto-play is
    // rerouted via `MoveToResultPileWithoutPlaying` — no effect dispatch,
    // no after-play buses, no play counters (sweep seeds 3931/5571: a
    // Ringing-blocked Cascade gather and a Chains-bound Stampede pick
    // actually resolved in Rust, over-killing the board). Same routing
    // shape as the unplayable/AnyAlly guards above.
    // Shared borrows only — `player_should_play_blocked` reads both `state`
    // and the card immutably, so no `Card` clone is needed here.
    if player_should_play_blocked(state, &state.cards[top_idx]) {
        if state.player.play.contains(&top_uid) {
            let self_exhausts = crate::card_effects::card_exhausts_inst(&state.cards[top_idx]);
            let dest = if force_exhaust || self_exhausts {
                PileKind::Exhaust
            } else {
                PileKind::Discard
            };
            let _ = cards::move_card(&mut state.player, top_uid, PileKind::Play, dest);
            if matches!(dest, PileKind::Exhaust) {
                fire_after_card_exhausted(
                    state,
                    top_card_id,
                    top_uid,
                    None,
                    top_card_tags,
                    top_card_type,
                );
            }
        }
        return;
    }

    // C# `History.CardPlayStartedEntry` — the auto-play passed ShouldPlay
    // and is now a started play for later Ringing/Sloth checks this turn.
    state.player.card_plays_started_this_turn =
        state.player.card_plays_started_this_turn.saturating_add(1);

    // Bead `sts2-rs-xautox`: an AUTO-PLAYED X-cost card captures its X from
    // the player's CURRENT energy. C# `CardCmd.AutoPlay` (CardCmd.cs:82-84):
    // `if (CostsX && !skipXCapture) CapturedXValue = playerCombatState.Energy`
    // — it captures the energy as X WITHOUT draining it (auto-plays are free).
    // RS left `energy_spent` at 0, so an auto-played X-cost card (Volley,
    // Whirlwind, Skewer, Tempest, MultiCast, …) resolved 0 ticks and did
    // nothing — and a per-tick RNG reader like Volley skipped its
    // `combat_targets` reads, desyncing the stream for the rest of combat
    // (seed 1376: a DistilledChaos-auto-played Volley → combat_targets cs=2
    // rs=1). Stamp it here (no energy drain, mirroring C#).
    if state.cards[top_idx].is_x_cost() {
        state.cards[top_idx].energy_spent = i32::from(state.player.energy.max(0));
    } else {
        // GH #741: a non-X auto-play is free (C# `EnergySpent = 0`) but its
        // `EnergyValue` is `EnergyCost.GetAmountToSpend()` — the card's
        // modifier-resolved cost (`CardCmd.AutoPlay`). Stamp that, so a value
        // left by an earlier manual play of this card is never read here.
        state.cards[top_idx].energy_spent = crate::hooks::effective_card_cost(state, top_uid);
    }

    // UI play-log: this auto-play passed every early-return guard (unplayable,
    // AnyAlly-without-ally, ShouldPlay-blocked) above and WILL resolve, so it
    // is recorded here — after the X-cost capture so `energy_spent` reflects the
    // captured X. A non-X auto-play is free, so it logs 0 (the card's stamped
    // `energy_spent` is its C# `EnergyValue`, not energy paid). Free play
    // -> `auto_played = true`, no player-chosen target.
    {
        let inst = &state.cards[top_idx];
        let energy = crate::cards::observational_energy_spent(inst);
        let (id, up) = (inst.card_id, inst.upgraded);
        record_play_into(&mut state.combat_log, id, up, energy, true, None);
        crate::event_log::record(
            state,
            crate::event_log::EventLogEntry::CardPlayStarted {
                card: id,
                upgraded: up,
                energy_spent: energy,
                auto_played: true,
                target: None,
            },
        );
    }

    // Resolve target — mirrors Python's `_resolve_target(card, None)`.
    // For `Enemy`-targeted cards, the canonical Havoc behavior picks a
    // random alive enemy (matching `combat.random_enemy_of(player)` in
    // the Python sim and `get_random_enemy` in C#).
    // C# `CardCmd.AutoPlay` rolls `Rng.CombatTargets.NextItem` for an
    // `AnyEnemy` card ONLY when the caller passed no target (`CardCmd.cs:76-86`).
    // A caller-supplied target (KnifeTrap → its `cardPlay.Target`) short-circuits
    // the roll, so honor `target_override` first and only fall back to the random
    // pick — keeping the `combat_targets` cursor untouched in the explicit case.
    let target_uid: Option<CreatureUid> = match top_target_type {
        TargetType::Enemy => target_override.or_else(|| state.get_random_enemy()),
        TargetType::Self_ | TargetType::None | TargetType::AllEnemies | TargetType::Random => None,
    };
    // C# fires `Hook.BeforeCardPlayed` for auto-plays too (`OnPlayWrapper`,
    // isAutoPlay=true) — `SurroundedPower.BeforeCardPlayed` re-faces the
    // surrounded player toward the auto-play's target, deciding which
    // KaiserCrab arm gets the 1.5x back-attack (sweep seed 5802:
    // DistilledChaos hit the Rocket, C# flipped facing, Rust kept the stale
    // side). No-op without a player-owned Surrounded power.
    if let Some(t) = target_uid {
        hooks::update_surrounded_facing(state, t);
    }

    // Dispatch the on-play effect. For Enemy-targeted auto-plays with no
    // alive enemy (e.g. the only enemy died from a prior card effect or
    // intent-side hit before Havoc resolved its draw-top), skip the
    // handler dispatch to avoid the `target.expect("…: Enemy-targeted")`
    // panic in every enemy-targeted card. Energy/exhaust/after-play
    // bookkeeping below still runs so the card resolves as a no-op,
    // matching Python's `play_card` behaviour when `_resolve_target`
    // returns None (combat.py:812-813). GitHub issue #31.
    // Bead `sts2-rs-rnyme` follow-up: the combat-wide card-play counter
    // feeding GoldAxe's C# `CalculatedDamageVar` is bumped AFTER the on-play
    // dispatch below (post-`OnPlay`, pre-`AfterCardPlayed`), mirroring C#
    // `History.CardPlayFinished` (CardModel.cs:1524). An auto-played GoldAxe
    // must NOT count itself, and — critically — must NOT count the in-flight
    // PARENT auto-play (Havoc / Cascade / Catastrophe) whose `OnPlay` is
    // still executing this very `resolve_auto_played_card`. The previous
    // play-start bump (here + the normal path) leaked that in-flight parent
    // into the child's read, over-scaling GoldAxe by 1. See the bump after
    // `resolve_inline_auto_play_choices` below.

    // Consume free-card powers (Free{Attack,Skill,Power}) for auto-played
    // cards, matching C# `CardModel.OnPlayWrapper`, which calls
    // `Hook.BeforeCardPlayed` unconditionally for both isAutoPlay=true and
    // false. Mirrors the same call in `play_card` (line ~3715) so that e.g.
    // BeatDown+1 / Ovicopter auto-plays of Attacks decrement the FreeAttack
    // stack correctly (round-8 OVICOPTER / THIEVING_HOPPER).
    let _ = consume_free_card_power(state, top_card_type);

    // C# computes the auto-played card's result pile pre-OnPlay too
    // (OnPlayWrapper, CardModel.cs:1459). Snapshot Corruption's
    // Skill→Exhaust redirect from BEFORE this card's effect dispatches, so a
    // Skill auto-played while Corruption is active exhausts — matching a
    // normal corrupted Skill play — while a card whose effect turns
    // Corruption ON mid-flight does not retroactively exhaust itself. Bead
    // `sts2-rs-q6bix` (CASCADE+HAVOC under Corruption: auto-played HAVOC
    // exhausts in C# but RS routed it to discard).
    let corruption_active_pre_dispatch =
        state.player_power_amount(crate::PowerKind::Corruption) > 0;

    // Bead `sts2-rs-jnf4a`: an AUTO-PLAYED card honors its `Weighted`
    // affliction (Doormaker GraspPower afflicts every player card; the
    // cards CATASTROPHE/Cascade/Havoc/etc. auto-play off the draw top are
    // afflicted too). C# `CardModel.OnPlayWrapper` runs the affliction
    // `OnPlay` for every play including auto-plays (`IsAutoPlay = true`
    // flows through the same wrapper, CardModel.cs:1516), so
    // `Weighted.OnPlay` drains `Amount` extra energy. Capture the drain
    // here and apply it AFTER the effect dispatch (mirroring the hand-play
    // path at ~3908/4086: an effect that gains energy then reaches the same
    // clamp-at-0 as C#). The auto-play resolver previously dropped it, so a
    // Cascade/Catastrophe that auto-played Weighted cards under Grasp left
    // RS with surplus energy (Doormaker PLAYER_STATE cluster).
    let auto_weighted_drain: i8 = if let Some(crate::card_afflictions::CardAffliction {
        kind: crate::card_afflictions::CardAfflictionKind::Weighted,
        stacks,
        ..
    }) = state.cards[top_idx].affliction
    {
        i8::try_from(stacks.clamp(0, i32::from(i8::MAX))).expect("clamp keeps value in i8 range")
    } else {
        0
    };

    // Power-side `BeforeCardPlayed` dispatch for auto-plays. C# `OnPlayWrapper`
    // fires `Hook.BeforeCardPlayed` for auto-plays too (isAutoPlay=true,
    // CardModel.cs:1513), so an auto-played card must trigger the SAME
    // before-card-played power reactions a hand-played card does — not just
    // Storm. The prior partial wiring (`sts2-rs-zlx.4` follow-up) armed only
    // the Storm/Subroutine snapshot via `arm_storm_snapshot`, so an auto-played
    // (MAYHEM / Cascade / Havoc / Stampede / Catastrophe) >=2-cost card did not
    // grant DanseMacabre block, an Ethereal one did not grant SpiritOfAsh block
    // nor consume a Veilpiercer charge, and the Afterimage / Monologue /
    // Strangle / SerpentForm / Oblivion before-play AMOUNT snapshots were never
    // armed (so their symmetric `dispatch_after_card_played` below ticked 0).
    // Parity fuzz Necrobinder seed 410: a MAYHEM-auto-played >=2-cost card left
    // RS player.block 0 vs C# 4 (DanseMacabre). `dispatch_before_card_played_powers`
    // ends by calling `arm_storm_snapshot`, so Storm/Subroutine stay covered.
    // `resolved_cost` mirrors the hand-play path (X-cost → just-stamped
    // `energy_spent`, else the modifier-resolved `effective_card_cost`); C#
    // `EnergyCost.GetResolved()`.
    let auto_resolved_cost = if state.cards[top_idx].is_x_cost() {
        state.cards[top_idx].energy_spent
    } else {
        crate::hooks::effective_card_cost(state, top_uid)
    };
    let auto_card_is_ethereal = card_is_ethereal_at_play(state, &state.cards[top_idx]);

    // Bead `sts2-rs-afterimage-nested`: C# `AfterimagePower` (and Strangle /
    // SerpentForm / Oblivion / Storm / Subroutine / Monologue) key their
    // before-play snapshot per CardModel (`amountsForPlayedCards[card]`), so a
    // PARENT card's pending snapshot SURVIVES the nested cards its `OnPlay`
    // auto-plays. RS stores that snapshot in a single per-power transient
    // (`counter`, or `skip_next_tick` for Monologue), which this nested
    // auto-play is about to arm — and whose symmetric `after_card_played` grant
    // ZEROES — clobbering the caller's still-pending snapshot before ITS
    // after-play grant fires. CATASTROPHE auto-plays 2 cards under Afterimage:
    // the two nested grants consumed the shared counter, so CATASTROPHE's own
    // +1 block was lost (Silent fuzz seed 289 player.block cs=3 rs=2). Emulate
    // C#'s per-card keying via the call stack: snapshot the caller's transient
    // arm fields now (after the caller armed them, before THIS play re-arms),
    // and restore them at function exit so the caller's after-play grant reads
    // the value it armed. Monologue's `counter` holds its persistent
    // StrengthApplied accumulator (only `skip_next_tick` is its arm), so it is
    // restored by flag, not counter.
    let saved_play_snapshots: smallvec::SmallVec<
        [(std::num::NonZeroU32, crate::PowerKind, i32, bool); 8],
    > = state
        .powers
        .iter()
        .filter(|p| {
            matches!(
                p.kind,
                crate::PowerKind::Afterimage
                    | crate::PowerKind::SerpentForm
                    | crate::PowerKind::Oblivion
                    | crate::PowerKind::Strangle
                    | crate::PowerKind::Storm
                    | crate::PowerKind::Subroutine
                    | crate::PowerKind::Monologue
            )
        })
        .map(|p| (p.id, p.kind, p.counter, p.skip_next_tick))
        .collect();

    crate::hooks::dispatch_before_card_played_powers(
        state,
        auto_resolved_cost,
        auto_card_is_ethereal,
    );

    let skip_dispatch = matches!(top_target_type, TargetType::Enemy) && target_uid.is_none();

    // catastrophe bug 4: fold `ModifyCardPlayCount` + `AfterModifyingCardPlayCount`
    // BEFORE the first OnPlay dispatch — mirroring C# `CardModel.OnPlayWrapper`
    // (CardModel.cs:1896: `playCount = await GeneratePlayCount(...)` runs BEFORE
    // the `for (i < playCount)` loop) and the hand-play path (combat_loop.rs:~5653,
    // which folds the count before its `dispatch` at ~5717). The resolver formerly
    // computed this in `finish_auto_played_card` AFTER the first dispatch + both
    // `AfterCardPlayed` buses, so the OneTwoPunch / relic decrement that
    // `AfterModifyingCardPlayCount` routes fired one play LATE: the stale power
    // lingered in `state.powers` during the first play's before/on/after hooks,
    // shifting a later power's array slot (the seed-1576 RADIANCE-slot family) and
    // diverging from C#, which has already consumed it. Compute the total extra
    // plays here and thread it through `AutoPlayFinish` so the replay loop in
    // `finish_auto_played_card` reuses it (no second fold — the OneTwoPunch
    // decrement must fire exactly once, C# `AfterModifyingCardPlayCount` once).
    // Gated on `!skip_dispatch` exactly as the former finish-side fold was (an
    // Enemy-target auto-play with no alive enemy plays zero times).
    let auto_extra_plays: i32 = if skip_dispatch {
        0
    } else {
        let base_replay = state.cards[top_idx].base_replay_count.max(0)
            + crate::enchantment_hooks::enchant_play_count_bonus(&state.cards[top_idx]);
        let count_ctx = hooks::CardPlayCountCtx {
            owner: hooks::CreatureRef::Player,
            card_id: Some(top_card_id),
            card_uid: Some(top_uid),
            card_type: Some(top_card_type),
            card_tags: top_card_tags,
            target: target_uid,
        };
        let power_extra = hooks::dispatch_modify_card_play_count(state, &count_ctx);
        let live = relic_guarded_channel_live(state, GuardedChannel::RelicModifyCardPlayCount);
        let relic_extra = dispatch_if_live(state, live, 0, |state| {
            crate::relic_effects::dispatch_relic_modify_card_play_count(state, &count_ctx)
        });
        let after_count_ctx = hooks::CardCtx {
            card_id: Some(top_card_id),
            card_uid: Some(top_uid),
            card_tags: top_card_tags,
            card_type: Some(top_card_type),
        };
        // Fires once per card play (matches C# + hand-play): routes the
        // OneTwoPunch decrement/remove deferred effects.
        hooks::dispatch_after_modifying_card_play_count(state, &after_count_ctx);
        (base_replay + power_extra + relic_extra).max(0)
    };

    // C# `CardCmd.AutoPlay` (CardCmd.cs:106-131) captures a `HasStarCostX` card's
    // `LastStarsSpent = current Stars` WITHOUT spending them (no SpendResources).
    // Stamp the captured value (stars+1 sentinel) so `play_stardust` uses it as
    // the hit count without calling `spend_stars` — an auto-played Stardust
    // (Mayhem) hits `Stars` times but leaves the stars intact. regent-legal-action
    // Fix B (seed 2010). Manual plays never set this var, so they still spend.
    if top_card_id == cards::CardId::Stardust {
        let captured = i32::from(state.player.stars).max(0);
        state.cards[top_idx].set_combat_var(crate::cards::VarKey::AutoPlayStarX, captured + 1);
    }
    if !skip_dispatch {
        let _ = crate::card_effects::dispatch_for_card(state, top_uid, top_card_id, target_uid);
    }
    if auto_weighted_drain != 0 {
        state.player.energy = state
            .player
            .energy
            .saturating_sub(auto_weighted_drain)
            .max(0);
    }

    // Bead `sts2-rs-y1fic`: resolve any card-select choice opened by the
    // auto-played card's OnPlay INLINE, before moving to the next collected
    // card. In C# an auto-played card whose `OnPlay` opens a
    // `CardSelectCmd.FromHand` / `FromSimpleGrid` (Purity exhaust, SeekerStrike
    // add-to-hand, ...) `await`s that selection inside `OnPlayWrapper.OnPlay`
    // (`CardModel.cs:1498`) — the headless `ICardSelector` completes the
    // `TaskCompletionSource` and `OnPlay` RESUMES, fully applying the
    // exhaust / add-to-hand, all BEFORE the auto-play caller moves on to the
    // next card (and before `AfterCardPlayed` fires / the card leaves the Play
    // pile, `CardModel.cs:1538`). The Rust port instead left the choice sitting
    // in `state.pending_choice` and continued the auto-play loop: the next
    // collected card resolved (and could clobber the still-pending choice via
    // `request_card_choice`), and the choice was only drained by the harness
    // AFTER the whole `apply()` returned — producing wrong exhaust counts and
    // late add-to-hand ordering (seed 1598 Purity exhaust, seed 4196
    // SeekerStrike VOLLEY tail). Drain it here exactly as the parity harness's
    // `_drain_rust_choices` would: take the first legal `ResolveChoice` and
    // apply it, looping until no choice remains (a resolved choice could open
    // another). Single auto-plays (Havoc) that open no choice short-circuit on
    // the first `is_none()` check, so this is a no-op there.
    //
    // Parity bug `combat_select_ironclad`: but the FIRST surfacing card-select
    // an auto-play's `OnPlay` opens must NOT be drained — it SURFACES, exactly
    // like the headless `HeadlessCardSelector` (which auto-resolves inline only
    // for `count==0`, `count==1 && min>=1`, or while `AutoResolveInline` is set
    // during a resume — `RunSimulator.cs:4804-4836,2165`). When such a choice is
    // pending and we are NOT resuming (`autoplay_inline_resolve == false`) and
    // combat is live, suspend: stash this card's post-dispatch tail + the
    // remaining collected cards in `pending_autoplay_resume` and return so
    // `legal_actions` surfaces the `card_select`. The resume path
    // (`resolve_choice_action`) raises `autoplay_inline_resolve` so any FURTHER
    // nested choice drains inline (mirrors C# `AutoResolveInline`, preserving the
    // `y1fic` chained-auto-play deadlock fix). Only THIS site gates the first
    // surface; `resolve_inline_auto_play_choices` below (and in the replay loop)
    // still drains unconditionally — it is reached only once the first-surface
    // gate has passed (a genuinely-inline reshuffle choice, a nested choice, or
    // a resume).
    if auto_play_choice_should_surface(state) {
        let fin = AutoPlayFinish {
            top_uid,
            target_uid,
            force_exhaust,
            skip_dispatch,
            corruption_active_pre_dispatch,
            auto_resolved_cost,
            auto_card_is_ethereal,
            extra_plays: auto_extra_plays,
            saved_play_snapshots,
            rebound_at_auto_play_start,
        };
        // Nested auto-play (seed 2752 DISTILLED_CHAOS → HAVOC → BURNING_PACT):
        // an INNER auto-play this card dispatched into already surfaced and
        // parked its own finish on `pending_autoplay_resume`. We are the OUTER
        // card unwinding up to the SAME gate while that inner select is still
        // pending. Do NOT clobber the inner finish (that dropped the inner card
        // entirely — it never left the Play pile); PUSH our tail onto the parent
        // chain (inner→outer order) so the resume consumer finishes the inner
        // card first, then us. Bead `sts2-rs` (#181), follow-up to
        // `sts2-rs-cata-suspend`.
        if let Some(resume) = state.pending_autoplay_resume.as_mut() {
            // GH #500: a loop continuation recorded since the inner card
            // surfaced belongs to THIS card's `OnPlay` — Catastrophe / BeatDown
            // set it on the resume marker immediately after their
            // `auto_play_specific_card` call came back suspended, and we are
            // that very card unwinding to the gate. Move it onto our frame so
            // the resolver runs the loop's remaining iterations BEFORE our tail
            // and before the siblings the enclosing batch will attach to us.
            // Left in the top-level slot it ran AFTER the whole parent chain, so
            // a Mayhem-batch sibling jumped the queue ahead of Catastrophe's own
            // next iteration and both engines picked different draw-top cards
            // from then on (Ironclad seed 189: C# `CATASTROPHE -> SEEKER_STRIKE,
            // HAVOC -> SEEKER_STRIKE, ARMAMENTS` vs Rust `CATASTROPHE ->
            // SEEKER_STRIKE, ARMAMENTS, HAVOC -> CATASTROPHE`). The slot keeps
            // its meaning for a HAND-played loop card, whose tail is
            // `pending_play` and runs after the chain either way.
            let continuation = resume.continuation.take();
            resume.parent_chain.push(AutoPlayParentFrame {
                finish: fin,
                remaining: smallvec::SmallVec::new(),
                force_exhaust,
                continuation,
            });
            return;
        }
        state.pending_autoplay_resume = Some(crate::PendingAutoplayResume {
            finish: fin,
            remaining: smallvec::SmallVec::new(),
            force_exhaust,
            // The auto-play loop that drove this card (Catastrophe / BeatDown)
            // fills `continuation` AFTER its `auto_play_specific_card*` call
            // observes this suspend; the pre-collected draw-top variants
            // (Cascade / Havoc / Mayhem) leave it `None`.
            continuation: None,
            parent_chain: smallvec::SmallVec::new(),
        });
        return;
    }
    resolve_inline_auto_play_choices(state);

    let fin = AutoPlayFinish {
        top_uid,
        target_uid,
        force_exhaust,
        skip_dispatch,
        corruption_active_pre_dispatch,
        auto_resolved_cost,
        auto_card_is_ethereal,
        extra_plays: auto_extra_plays,
        saved_play_snapshots,
        rebound_at_auto_play_start,
    };
    finish_auto_played_card(state, fin);
}

/// Captured locals needed to run the post-dispatch *tail* of a single
/// auto-played card ([`finish_auto_played_card`]). Parity bug
/// `combat_select_ironclad`: when the auto-played card's `OnPlay` opens a
/// SURFACING card-select we suspend rather than drain — the
/// `AfterCardPlayed`/replay/disposal tail must run AFTER the surfaced pick is
/// answered, so these locals are stashed on [`crate::PendingAutoplayResume`]
/// and the resolver re-enters the tail. Driving the surfaced choice first-legal
/// (as the parity harness does) reproduces the prior inline-drain behavior
/// byte-for-byte; only the agent-visible decision is new. Mirrors
/// [`CardPlayFinish`] for top-level plays.
pub struct AutoPlayFinish {
    pub(crate) top_uid: crate::CardUid,
    pub(crate) target_uid: Option<CreatureUid>,
    pub(crate) force_exhaust: bool,
    pub(crate) skip_dispatch: bool,
    pub(crate) corruption_active_pre_dispatch: bool,
    pub(crate) auto_resolved_cost: i32,
    pub(crate) auto_card_is_ethereal: bool,
    /// Number of ADDITIONAL replays (beyond the first play) this auto-play owes,
    /// = `base_replay_count + ModifyCardPlayCount(power) + ModifyCardPlayCount(relic)`.
    /// Computed BEFORE the first dispatch (catastrophe bug 4) — with
    /// `AfterModifyingCardPlayCount` already fired there — so the replay loop in
    /// [`finish_auto_played_card`] must NOT re-fold it.
    pub(crate) extra_plays: i32,
    pub(crate) saved_play_snapshots:
        smallvec::SmallVec<[(std::num::NonZeroU32, crate::PowerKind, i32, bool); 8]>,
    /// Rebound stacks captured at auto-play start, before the card's `OnPlay`
    /// (GH #637) — the auto-play twin of `CardPlayFinish::
    /// rebound_amount_at_play_start`. See `rebound_redirects_to_draw_top`.
    pub(crate) rebound_at_auto_play_start: i32,
}

impl std::fmt::Debug for AutoPlayFinish {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoPlayFinish")
            .field("top_uid", &self.top_uid)
            .field("force_exhaust", &self.force_exhaust)
            .field("skip_dispatch", &self.skip_dispatch)
            .finish_non_exhaustive()
    }
}

impl Clone for AutoPlayFinish {
    fn clone(&self) -> Self {
        Self {
            top_uid: self.top_uid,
            target_uid: self.target_uid,
            force_exhaust: self.force_exhaust,
            skip_dispatch: self.skip_dispatch,
            corruption_active_pre_dispatch: self.corruption_active_pre_dispatch,
            auto_resolved_cost: self.auto_resolved_cost,
            auto_card_is_ethereal: self.auto_card_is_ethereal,
            extra_plays: self.extra_plays,
            saved_play_snapshots: self.saved_play_snapshots.clone(),
            rebound_at_auto_play_start: self.rebound_at_auto_play_start,
        }
    }
}

impl PartialEq for AutoPlayFinish {
    fn eq(&self, other: &Self) -> bool {
        self.top_uid == other.top_uid
            && self.target_uid == other.target_uid
            && self.force_exhaust == other.force_exhaust
            && self.skip_dispatch == other.skip_dispatch
            && self.corruption_active_pre_dispatch == other.corruption_active_pre_dispatch
            && self.auto_resolved_cost == other.auto_resolved_cost
            && self.auto_card_is_ethereal == other.auto_card_is_ethereal
            && self.extra_plays == other.extra_plays
            && self.saved_play_snapshots == other.saved_play_snapshots
            && self.rebound_at_auto_play_start == other.rebound_at_auto_play_start
    }
}

impl Eq for AutoPlayFinish {}

/// One outer auto-play whose post-dispatch tail is still owed because an
/// auto-play it nested INTO surfaced a card-select first.
///
/// Parity bug (seed 2752 `DISTILLED_CHAOS` → `HAVOC` → `BURNING_PACT`): an
/// auto-played card (HAVOC) can itself auto-play the next draw-top card
/// (BURNING_PACT) from its `OnPlay`. When that inner card surfaces a
/// card-select, it parks its own [`AutoPlayFinish`] on
/// [`crate::PendingAutoplayResume::finish`] and returns. Control then unwinds
/// UP into the OUTER card's `resolve_auto_played_card`, which reaches the same
/// surface gate while the inner select is still pending. The single
/// `pending_autoplay_resume` slot cannot hold both finishes, so the outer card
/// pushes a frame here (inner→outer order) instead of clobbering the inner
/// finish. On resume the inner finish runs first, then each parent frame's
/// finish + its own remaining collected siblings, mirroring C#'s nested
/// `await CardCmd.AutoPlay` unwinding inside one `DoSelectCards`. Follow-up to
/// bead `sts2-rs-cata-suspend` (which covered the re-selection-loop variant via
/// [`crate::AutoPlayLoopCont`] but left the pre-collected
/// [`auto_play_n_from_draw_top`] nested-surface path uncovered).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoPlayParentFrame {
    /// The outer auto-played card's post-dispatch tail.
    pub(crate) finish: AutoPlayFinish,
    /// The not-yet-played collected siblings of THIS outer card (the remainder
    /// of the [`auto_play_n_from_draw_top`] batch that was driving it), played
    /// in order once this frame's `finish` runs. Empty for the single-card
    /// auto-play drivers (Havoc / specific-card).
    pub remaining: smallvec::SmallVec<[crate::CardUid; 8]>,
    /// `force_exhaust` threaded through this frame's `remaining`.
    pub force_exhaust: bool,
    /// The re-selection auto-play LOOP this frame's card was running when the
    /// nested card surfaced (Catastrophe's iteration count, BeatDown's list) —
    /// see [`crate::AutoPlayLoopCont`]. Runs BEFORE `finish`: C#'s `OnPlay`
    /// cannot return, and so the card cannot reach its `AfterCardPlayed` or its
    /// result pile, until every `await CardCmd.AutoPlay` in its loop has
    /// completed. `None` for the pre-collected batch drivers (Cascade / Havoc /
    /// Mayhem / DistilledChaos), which express their remainder as `remaining`.
    /// GH #500.
    pub continuation: Option<crate::AutoPlayLoopCont>,
}

/// Should the `pending_choice` an auto-play's `OnPlay` just opened SURFACE
/// (suspend the auto-play) rather than drain inline? Mirrors the headless
/// `HeadlessCardSelector.GetSelectedCards` inline-resolution boundary
/// (`RunSimulator.cs:4804-4836`): inline ONLY when there is no choice, or it
/// has 0 options, or exactly 1 option with `min_select >= 1`, or while the
/// engine is RESUMING an auto-play (`autoplay_inline_resolve` — the C#
/// `AutoResolveInline` mirror). A combat-ending play has no live decision to
/// surface, so it drains too. Anything else SURFACES.
fn auto_play_choice_should_surface(state: &CombatState) -> bool {
    if state.autoplay_inline_resolve || state.is_over {
        return false;
    }
    let Some(pc) = state.pending_choice.as_ref() else {
        return false;
    };
    // C# `optList.Count == 0` → empty task; `Count == 1 && minSelect >= 1`
    // → auto-select. Both resolve inline (no surface).
    if pc.options.is_empty() {
        return false;
    }
    if pc.options.len() == 1 && pc.min_select >= 1 {
        return false;
    }
    true
}

/// Post-dispatch tail of a single auto-played card: the combat-wide play
/// counter, `AfterCardPlayed` buses, per-turn tallies, the OneTwoPunch /
/// HiddenGem replay loop, and the Play→Discard/Exhaust disposal. Extracted
/// from [`resolve_auto_played_card`] (parity bug `combat_select_ironclad`) so
/// the same tail runs either inline (no surfacing choice) or deferred
/// (re-entered from [`crate::resolve_choice_action`] after a surfaced `OnPlay`
/// select is answered). Statement order is unchanged from the former inline
/// body, so driving a surfaced choice first-legal reproduces the prior
/// behavior byte-for-byte.
pub(crate) fn finish_auto_played_card(state: &mut CombatState, fin: AutoPlayFinish) {
    let AutoPlayFinish {
        top_uid,
        target_uid,
        force_exhaust,
        skip_dispatch,
        corruption_active_pre_dispatch,
        auto_resolved_cost,
        auto_card_is_ethereal,
        extra_plays,
        saved_play_snapshots,
        rebound_at_auto_play_start,
    } = fin;
    let top_idx = top_uid.0.get() as usize - 1;
    let top_card = &state.cards[top_idx];
    let top_card_id = top_card.card_id;
    let top_card_type = top_card.card_type;
    let top_card_tags = top_card.tags;

    // Bead `sts2-rs-rnyme` follow-up: record this auto-play in the
    // combat-wide counter NOW — after the on-play dispatch + inline choice
    // resolution, before `AfterCardPlayed` — mirroring C#
    // `History.CardPlayFinished` (CardModel.cs:1524), which fires for every
    // resolved play including auto-plays (`IsAutoPlay = true` flows through
    // the same wrapper). Runs even when `skip_dispatch` is true
    // (Enemy-targeted auto-play with no alive enemy), matching C# recording
    // `CardPlayFinished` unconditionally after `OnPlay`. Unplayable
    // curses/status returned early above (C# `MoveToResultPileWithoutPlaying`
    // records no `CardPlayFinished`), so they stay excluded.
    state.combat_log.player_card_plays_this_combat = state
        .combat_log
        .player_card_plays_this_combat
        .saturating_add(1);
    state.combat_log.card_plays_finished_this_turn = state
        .combat_log
        .card_plays_finished_this_turn
        .saturating_add(1);
    // Event log (C# `CardPlayFinishedEntry`) at the same pin, which also
    // CLOSES this play's source frame so the parent that auto-played it
    // reclaims attribution for anything it dispatches next.
    {
        let up = state.cards[top_idx].upgraded;
        crate::event_log::record(
            state,
            crate::event_log::EventLogEntry::CardPlayFinished {
                card: top_card_id,
                upgraded: up,
            },
        );
    }

    // C# `CardEnergyCost.AfterCardPlayedCleanup()` fires for auto-plays too
    // (same `OnPlayWrapper`), retiring `WhenPlayed`-flagged modifiers — the
    // wire-cost bank AND the free riders (`SetToFreeThisTurn` is
    // `SetThisTurnOrUntilPlayed(0)`). Mirrors the hand-play edge at
    // `play_card`.
    state.restore_wire_cost_expiring_on_play(top_uid);

    // An auto-play is a REAL card play (`!IsDupe`) — C#'s HistoryCourse
    // scan over `History.CardPlaysFinished` sees it, so a Stampede/Mayhem
    // auto-played Attack/Skill becomes "last round's last Attack/Skill"
    // exactly like a hand play (sweep seeds 5645/4095: C# duped the
    // auto-played PILLAGE/BASH, Rust duped the earlier hand play).
    if matches!(
        top_card_type,
        crate::cards::CardType::Attack | crate::cards::CardType::Skill
    ) && state
        .cards
        .get(top_idx)
        .is_some_and(|c| c.combat_var(VarKey::IsDupe) == 0)
    {
        let upgraded = state.cards.get(top_idx).is_some_and(|c| c.upgraded);
        state.combat_log.last_attack_skill_played = Some((top_card_id, upgraded, Some(top_uid)));
    }

    // Fire after_card_played on both buses.
    let played_ctx = crate::relic_effects::CardEventCtx {
        card_id: top_card_id,
        card_uid: Some(top_uid),
        target: target_uid,
        card_tags: top_card_tags,
        energy_spent: 0,
    };
    if relic_channel_live(state, PlannedChannel::AfterCardPlayed) {
        let _ = crate::relic_effects::dispatch_after_card_played(state, &played_ctx);
    }
    let hook_played_ctx = CardCtx {
        card_id: Some(top_card_id),
        card_uid: Some(top_uid),
        card_tags: top_card_tags,
        card_type: Some(top_card_type),
    };
    hooks::dispatch_after_card_played(state, &hook_played_ctx);

    // Bead `sts2-rs-50un1`: C# `CardCmd.AutoPlay` runs the full `OnPlayWrapper`,
    // whose `AfterCardPlayed` fires `RupturePower.AfterCardPlayed` — applying
    // the Strength owed for any self-HP-loss the auto-played card dealt. An
    // auto-played self-HP-loss card (OFFERING/Hemokinesis played off the draw
    // top by Cascade/Havoc) accumulates that owed Strength on the Rupture
    // instance's `counter` via the `after_damage_received` reaction, so flush
    // it here to mirror the hand-play flush at `play_card` (combat_loop.rs:4450)
    // and the HISTORY_COURSE dupe-replay flush (event_reward.rs). Idempotent
    // no-op without an active Rupture / deferred amount.
    flush_rupture_deferred_strength(state);

    // Bead `sts2-rs-distilled-chaos-replay-scale`: per-turn play tally for the
    // INITIAL auto-play, BEFORE the OneTwoPunch / base_replay replay loop below
    // — mirroring the hand-play order (combat_loop.rs:4735). A scaling attack
    // replayed below (Conflagration's `CalculatedDamageVar` counts the player's
    // Attacks FINISHED this turn) must observe THIS play already counted, so
    // each replay deals one Attack more than the last (C# records a
    // `CardPlayFinishedEntry` per OnPlay iteration, CardModel.cs:1524). The
    // bump previously ran ONCE *after* the replay loop, so a DistilledChaos /
    // Catastrophe-auto-played Conflagration under OneTwoPunch under-scaled its
    // replay (seed 1090: VINE_SHAMBLER hp +3). Each replay re-bumps inside the
    // loop. Stomp/Pinpoint cost shaves + the ethereal tally move with it, and
    // the formerly-post-loop copy is deleted. Runs ungated on `force_exhaust`
    // (r6l9: a force-exhausted Havoc play still counts) and on `skip_dispatch`
    // (C# records `CardPlayFinished` even for an Enemy-targeted no-alive-enemy
    // play).
    auto_play_tally_finished(state, top_card_type, top_idx);
    // C# `Hook.AfterCardPlayed` ends with every listener's
    // `AfterCardPlayedLate` (Hook.cs:431-437) — auto-plays included, since
    // `CardCmd.AutoPlay` runs the same `OnPlayWrapper`. Fired after the tally
    // so MakeItSo's skill count sees this play (C# records `CardPlayFinished`
    // before `AfterCardPlayed`), and once per replay below. RightHandHand reads
    // the `EnergyValue` stamped above (GH #741 follow-up).
    crate::card_effects::fire_late_effects(state, &hook_played_ctx);

    // Bead `sts2-rs-xk9yo`: an AUTO-PLAYED card honors its per-instance
    // `base_replay_count` just like a hand-played one. C#
    // `CardModel.OnPlayWrapper` (CardModel.cs:1464-1538) computes
    // `playCount = GetEnchantedReplayCount() + 1` and runs the WHOLE
    // play body (OnPlay + AfterCardPlayed + CardPlayFinished) that many
    // times — and the wrapper is shared by auto-plays (`IsAutoPlay = true`
    // just flows through as a CardPlay flag). The Rust auto-play resolver
    // dispatched the effect exactly ONCE, dropping HiddenGem's replay
    // enchant when the enchanted card was later auto-played off the draw
    // top by Cascade / Havoc / Catastrophe (seed 1248: HiddenGem+1 enchants
    // FightMe with ReplayCount 3; Cascade auto-plays it → C# resolves it 4×
    // for +16 Strength / +4 enemy Strength, RS resolved it once for +4 / +1).
    // Mirror the hand-play replay loop (`extra_plays` arm above): re-dispatch
    // the effect, drain any inline choice it opens, re-fire BOTH
    // `after_card_played` buses, and bump the combat-wide play counter once
    // per replay (each replay is its own C# `CardPlayFinished`). The card
    // stays on the Play pile across replays; the single move-to-result-pile
    // below runs once.
    //
    // Bead `sts2-rs-t12kj` (seed 1576, CATASTROPHE auto-playing under
    // OneTwoPunch): C# `CardModel.OnPlayWrapper` (CardModel.cs:1464-1466)
    // folds `Hook.ModifyCardPlayCount` + fires `AfterModifyingCardPlayCount`
    // UNCONDITIONALLY — `isAutoPlay` gates only the pile-add path, NOT the
    // play-count machinery. So an auto-played Attack under OneTwoPunch(1)
    // hits TWICE (and the power decrements to 0 + is removed), exactly like a
    // hand-played one.
    //
    // catastrophe bug 4: the fold + `AfterModifyingCardPlayCount` (OneTwoPunch
    // decrement/remove) now runs in `resolve_auto_played_card_at_target` BEFORE
    // the first dispatch (mirroring C# `GeneratePlayCount` at CardModel.cs:1896,
    // which precedes the play loop, and the hand-play order). `fin.extra_plays`
    // carries the already-computed replay count here so the loop must NOT re-fold
    // — re-folding would fire `AfterModifyingCardPlayCount` a second time and
    // double-consume OneTwoPunch. EchoForm is NOT in this channel at this pin (it
    // rides its own `cards_first_in_series_this_turn` counter, see hand-play
    // comment). Gated on `!skip_dispatch` for symmetry — `extra_plays` is already
    // 0 when `skip_dispatch`, so the guard is belt-and-suspenders.
    if !skip_dispatch {
        for _ in 0..extra_plays {
            if state.is_over {
                break;
            }
            // C# `CardModel.OnPlayWrapper` fires `Hook.BeforeCardPlayed` per
            // play iteration (CardModel.cs:1496) for auto-plays too, so each
            // replay of an auto-played card re-arms the Afterimage / SerpentForm
            // / Oblivion / Strangle / Monologue snapshots (+ DanseMacabre /
            // SpiritOfAsh / Veilpiercer / Storm) — same as the hand-play
            // `extra_plays` loop. Without this, a HiddenGem-enchanted card
            // auto-played by Cascade/Catastrophe under Afterimage granted block
            // only on its first play (the after-play grant consumes the armed
            // counter; the loop never re-armed it).
            crate::hooks::dispatch_before_card_played_powers(
                state,
                auto_resolved_cost,
                auto_card_is_ethereal,
            );
            let _ = crate::card_effects::dispatch_for_card(state, top_uid, top_card_id, target_uid);
            resolve_inline_auto_play_choices(state);
            state.combat_log.player_card_plays_this_combat = state
                .combat_log
                .player_card_plays_this_combat
                .saturating_add(1);
            state.combat_log.card_plays_finished_this_turn = state
                .combat_log
                .card_plays_finished_this_turn
                .saturating_add(1);
            if relic_channel_live(state, PlannedChannel::AfterCardPlayed) {
                let _ = crate::relic_effects::dispatch_after_card_played(state, &played_ctx);
            }
            hooks::dispatch_after_card_played(state, &hook_played_ctx);
            // Bead `sts2-rs-50un1`: each auto-play replay is its own C#
            // `AfterCardPlayed`, so flush RupturePower's deferred Strength per
            // replay (mirrors the hand-play `extra_plays` flush at 4530).
            flush_rupture_deferred_strength(state);
            // Bead `sts2-rs-distilled-chaos-replay-scale`: each replay is its
            // own C# `CardPlayFinished`, so re-tally the per-turn play bucket
            // (+ ethereal / Stomp / Pinpoint) per replay — exactly like the
            // hand-play replay loop (combat_loop.rs:4828) — so a SECOND replay
            // of a scaling Attack sees the FIRST replay counted.
            auto_play_tally_finished(state, top_card_type, top_idx);
            crate::card_effects::fire_late_effects(state, &hook_played_ctx);
        }
    }

    // (Per-turn play tally + Stomp/Pinpoint shave + ethereal count moved to
    // `auto_play_tally_finished`, called for the initial play BEFORE the replay
    // loop and once per replay INSIDE it — see bead
    // `sts2-rs-distilled-chaos-replay-scale` above. Formerly a single
    // post-loop bump here, which under-counted a replayed scaling Attack.)

    // Cleanup routing. C# `CardPileCmd.AutoPlayFromDrawPile` sets
    // `card.ExhaustOnNextPlay = forceExhaust` before the auto-play, so the
    // card exhausts iff `force_exhaust` OR the card self-exhausts (its own
    // EXHAUST keyword). Only Havoc passes `force_exhaust = true`; Cascade /
    // DistilledChaos / Mayhem / Imbued pass `false`, routing the played
    // card to its NATURAL pile (discard, or exhaust if it self-exhausts,
    // or nowhere for a Power). Mirrors the normal-play cleanup branch in
    // `play_card`. The card is located by position (not `last()`) because
    // the two-phase multi-card path keeps several collected cards on Play
    // at once. The card may not be on Play any more (an effect routed it
    // elsewhere) — guard against that.
    // FeralPower redirect for an auto-played 0-energy owner Attack (Mayhem /
    // Cascade / Catastrophe), mirroring the hand-play path in `finish_card_play`:
    // the first `Amount` 0-cost owner Attacks each turn return to Hand/Top
    // instead of exhaust/discard. `auto_resolved_cost` is C#
    // `EnergyCost.GetResolved()` (== ResourceInfo.EnergyValue). The tally is
    // bumped once below for every 0-cost auto-played Attack.
    let auto_is_zero_cost_attack =
        top_card_type == cards::CardType::Attack && auto_resolved_cost == 0;
    if state.player.play.contains(&top_uid) {
        let self_exhausts = crate::card_effects::card_exhausts_inst(&state.cards[top_idx]);
        // CorruptionPower routes the owner's played Skills to Exhaust — the
        // auto-play mirror of the same branch in `play_card`, using the
        // pre-dispatch Corruption snapshot (C# pre-OnPlay result-pile).
        let corruption_exhausts_skill =
            top_card_type == cards::CardType::Skill && corruption_active_pre_dispatch;
        let feral_returns_to_hand = auto_is_zero_cost_attack
            && (state.combat_log.zero_cost_attacks_played_this_turn as i32)
                < state.player_power_amount(crate::PowerKind::Feral);
        if feral_returns_to_hand {
            // C# `FeralPower.cs:60` — the auto-play mirror of the
            // `finish_card_play` emission (Havoc / Mayhem / Cascade); see the
            // comment there for the hook and the ordering deviation.
            //
            // NOTE this guard is LOOSER than the hand-play one: it drops
            // `!played_card_is_dupe` and `pending_choice.is_none()`, both of
            // which the manual path also requires. That asymmetry is
            // pre-existing engine behaviour, not something the flash
            // introduces — the emission sits inside the `if` whose body
            // performs the observable redirect, so it fires exactly when this
            // path redirects, which is the contract. Any fix to the asymmetry
            // moves the flash with it.
            crate::event_log::flash_player_power(state, crate::PowerKind::Feral);
            if let Some(pos) = state.player.play.iter().position(|&u| u == top_uid) {
                state.player.play.remove(pos);
                if state.player.hand.len() < crate::cards::HAND_LIMIT {
                    state.player.hand.insert(0, top_uid);
                } else {
                    state.player.discard.push(top_uid);
                }
            }
        } else if top_card_type == cards::CardType::Power {
            // Powers leave combat on play (no pile) — see the Power arm in
            // `play_card` (bead `sts2-rs-3k85q`).
            if let Some(pos) = state.player.play.iter().position(|&u| u == top_uid) {
                state.player.play.remove(pos);
            }
        } else if force_exhaust || self_exhausts || corruption_exhausts_skill {
            let _ = cards::move_card(
                &mut state.player,
                top_uid,
                PileKind::Play,
                PileKind::Exhaust,
            );
            fire_after_card_exhausted(
                state,
                top_card_id,
                top_uid,
                None,
                top_card_tags,
                top_card_type,
            );
        } else if top_card_id == cards::CardId::ShiningStrike {
            // C# `ShiningStrike.OnPlay` (ShiningStrike.cs): a non-exhausting
            // copy returns to the TOP of the draw pile (`CardPileCmd.Add(this,
            // PileType.Draw, Top)`). C#'s `OnPlayWrapper` runs this for
            // AUTO-plays too (`IsAutoPlay = true`), but the auto-play resolver
            // only handled Nostalgia-redirect or discard — so an auto-played
            // SHINING_STRIKE (e.g. a CATASTROPHE pick) wrongly landed in
            // Discard, shrinking the next StableShuffle pool by one card (one
            // fewer Fisher-Yates read) and changing CATASTROPHE's subsequent
            // picks (Regent parity seed 265). EXHAUST copies took the
            // Play→Exhaust branch above and never reach here, matching C#'s
            // `!Keywords.Contains(Exhaust)` guard. Mirrors the hand-play path
            // (`play_card` ShiningStrike branch).
            if let Some(pos) = state.player.play.iter().position(|&u| u == top_uid) {
                state.player.play.remove(pos);
                state.player.draw.insert(0, top_uid);
                // Definite-position insert, same class as the hand-play
                // ShiningStrike branch — pin top-of-draw. Bead `sts2-rs-09x5n`.
                cards::pin_top(&mut state.player, top_uid);
            }
        } else {
            // Natural routing: discard — UNLESS NostalgiaPower redirects the
            // owner's first `Amount` Attacks/Skills played this turn to the TOP
            // of the draw pile. C# `NostalgiaPower.ModifyCardPlayResultPileType
            // AndPosition` fires for EVERY play whose natural result pile is
            // Discard, including auto-plays (`OnPlayWrapper` runs with
            // `IsAutoPlay = true` through the same result-pile modifiers). The
            // hand-play path applies this at the `play_card` routing block
            // (~4419) and `flush_played_cards_after_choice` (~4573); the
            // auto-play resolver dropped it, so a STAMPEDE-auto-played Attack
            // under Nostalgia went to Discard here while C# sent it to draw-top
            // — at the next turn-start AggressionPower then saw one EXTRA Attack
            // in discard (4-vs-3 `combat_card_selection` shuffle) and the
            // redirected card never re-entered the draw, cascading the whole
            // turn's draw/hand (PILE_COUNT|end_turn cluster a4096, rep seed
            // 1393: STAMPEDE auto-plays ASHEN_STRIKE → C# draw-top → re-drawn
            // turn 5; RS discard). Bead `sts2-rs-1sew4`. The Corruption
            // (Skill→Exhaust) branch above already took precedence, mirroring
            // C#'s `pileType != Discard` early-out. The current play was tallied
            // in `cards_played_this_turn` just above (before this routing block),
            // so the guard counts it inclusively, exactly like the hand-play path.
            let is_attack_or_skill = matches!(
                top_card_type,
                cards::CardType::Attack | cards::CardType::Skill
            );
            let tallied_this_turn = state.player.cards_played_this_turn
                [cards::CardType::Attack.index()]
            .saturating_add(state.player.cards_played_this_turn[cards::CardType::Skill.index()]);
            // C# `NostalgiaPower` reads `History.CardPlaysStarted` — every
            // Attack/Skill play STARTED this turn — whereas Rust bumps
            // `cards_played_this_turn` only when a play FINISHES (at ~5138 /
            // the `play_card` after-played site). The two agree for a
            // turn-end auto-play (StampedePower: no enclosing card play, Play
            // holds only `top_uid`), but DIVERGE when this auto-play is nested
            // inside another card's OnPlay — e.g. CATASTROPHE (a Skill) plays
            // a card off the draw pile mid-OnPlay. C#'s `num` then already
            // counts the enclosing Catastrophe (started, not finished), so for
            // Nostalgia=1 `num >= Amount` ⇒ the nested play is NOT redirected;
            // Rust hasn't tallied Catastrophe yet and would wrongly redirect.
            // The enclosing started-but-unfinished plays are the OTHER
            // Attack/Skill cards still on the Play pile — EXCEPT a two-phase
            // batch auto-play (`auto_play_n_from_draw_top`, DistilledChaos /
            // Cascade) PRE-LOADS the whole top-N onto Play in phase 1 and plays
            // them in phase 2, so a not-yet-played batch sibling sits on Play
            // but has NOT started (no C# `CardPlayStarted` recorded). Those
            // siblings carry the `PendingBatchAutoPlay` marker (set in phase 1,
            // cleared when a card's own play starts), so skip them here.
            // Counting them over-stated C#'s started total and suppressed a
            // Nostalgia redirect that should fire (Ironclad seed 5513: under
            // Nostalgia=1 a DistilledChaos batch [Spite, Aggression, BeatDown]
            // should route the first-played Spite to the DRAW top — Rust instead
            // discarded it, exposing it to a BeatDown auto-played LATER in the
            // same batch, which re-attacked it for +6 enemy damage and left it
            // in discard). A genuine nested enclosing play (Catastrophe) never
            // carries the marker, so it still counts. Bead `sts2-rs-1sew4`.
            let enclosing_attacks_skills = state
                .player
                .play
                .iter()
                .filter(|&&u| u != top_uid)
                .filter(|&&u| {
                    let idx = u.0.get() as usize - 1;
                    state.cards.get(idx).is_some_and(|c| {
                        matches!(
                            c.card_type,
                            cards::CardType::Attack | cards::CardType::Skill
                        ) && c.combat_var(crate::cards::VarKey::PendingBatchAutoPlay) == 0
                    })
                })
                .count();
            let started_this_turn =
                i64::from(tallied_this_turn).saturating_add(enclosing_attacks_skills as i64);
            // Same fold as the hand-play routing block (GH #650). C#
            // `OnPlayWrapper` runs the same result-pile modifiers with
            // `IsAutoPlay = true` and neither power's arm reads `isAutoPlay`
            // — bead `sts2-rs-1sew4` records the parity bug from this site
            // missing the redirect entirely, which is the triplication this
            // shared helper removes.
            if let Some(which) = resolve_draw_top_redirect(
                state,
                is_attack_or_skill,
                started_this_turn,
                rebound_at_auto_play_start,
            ) {
                fire_draw_top_redirect(state, which);
                if let Some(pos) = state.player.play.iter().position(|&u| u == top_uid) {
                    state.player.play.remove(pos);
                    state.player.draw.insert(0, top_uid);
                    // Definite-position insert — pin it. Bead `sts2-rs-09x5n`.
                    cards::pin_top(&mut state.player, top_uid);
                }
            } else {
                let _ = cards::move_card(
                    &mut state.player,
                    top_uid,
                    PileKind::Play,
                    PileKind::Discard,
                );
            }
        }
    }

    // FeralPower budget tally: an auto-played 0-cost owner Attack consumes the
    // budget too (C# records a `CardPlayStartedEntry` for auto-plays). Bumped
    // after the routing read above, matching the hand-play path.
    if auto_is_zero_cost_attack {
        state.combat_log.zero_cost_attacks_played_this_turn = state
            .combat_log
            .zero_cost_attacks_played_this_turn
            .saturating_add(1);
    }

    // Bead `sts2-rs-afterimage-nested`: restore the caller's before-play
    // snapshot transients that this nested play armed/consumed. See the
    // `saved_play_snapshots` capture above. Mirrors C#'s per-card
    // `amountsForPlayedCards` keying: the caller's pending tick is intact when
    // its own `after_card_played` grant fires after this nested play returns.
    for (id, kind, counter, skip) in saved_play_snapshots {
        if let Some(p) = state.powers.get_mut_by_id(id) {
            match kind {
                crate::PowerKind::Monologue => p.skip_next_tick = skip,
                _ => p.counter = counter,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// --- per-turn card-vars cleanup helper (bead `sts2-rs-c68o`) -------------

/// End-of-turn cleanup of per-turn `combat_vars` flags on every card in
/// every pile. Bead `sts2-rs-c68o` wires the first concrete clear:
/// `VarKey::FreeThisTurn` (set by `InfernalBlade`'s random Attack
/// spawn). Mirrors Python's loop over `state.all_piles` calling
/// `card.end_of_turn_cleanup()` at
/// `sts2-rl-agent@01c094d:sts2_env/cards/base.py:160-170`. Walks the
/// full card arena (not just the player's piles) because the cleanup
/// is per-card-instance and the arena already gates by live combat;
/// staying arena-scoped avoids 5 pile-iterator allocations per turn-end.
fn clear_per_turn_card_vars(state: &mut CombatState) {
    for card in &mut state.cards {
        // Single-scan form of `if combat_var(k) != 0 { set_combat_var(k, 0) }`
        // (perf 2026-07-04): both `combat_var` and `set_combat_var` operate on
        // the FIRST slot matching the key, so zeroing that slot directly is
        // state-identical (writing 0 over an already-0 slot included) while
        // walking the slots once instead of twice. A card whose
        // "may be nonzero" bit is clear holds 0 in every FreeThisTurn slot,
        // so the write would be a no-op and the card is skipped (J02).
        if card.combat_vars.maybe_nonzero(VarKey::FreeThisTurn) {
            card.combat_vars.zero(VarKey::FreeThisTurn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CombatPhase, CombatState, Creature, CreatureUid, Intent, IntentHistory, MonsterKind, Side,
        StepResult,
    };

    use smallvec::SmallVec;
    use std::num::NonZeroU32;

    #[test]
    fn record_helper_respects_flag() {
        let mut te = crate::TurnEvents::default();
        record_play_into(
            &mut te,
            crate::cards::CardId::StrikeIronclad,
            false,
            1,
            false,
            None,
        );
        assert!(te.card_play_log.is_empty(), "flag off => no append");
        te.record_card_play_log = true;
        record_play_into(
            &mut te,
            crate::cards::CardId::StrikeIronclad,
            false,
            1,
            false,
            Some(2),
        );
        assert_eq!(te.card_play_log.len(), 1);
        assert_eq!(te.card_play_log[0].target, Some(2));
        assert_eq!(
            te.card_play_log[0].card_id,
            crate::cards::CardId::StrikeIronclad
        );
    }

    /// Build a `CombatState` with `n` Cultists pre-loaded into the creature
    /// arena and `enemy_uids`. Cultists start with `intent: None` so the
    /// roll-intent step is observable. RNG is seeded for determinism.
    fn state_with_cultists(n: u32) -> CombatState {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        // A combat fixture must have a LIVING player: `CombatState::default`
        // leaves `current_hp == 0`, which the player-death chokepoints now read
        // as a defeat and clear the player's powers (bead `sts2-rs-2e9q2`).
        // Give the player full HP so card-play / power-mechanic tests model a
        // real, alive player.
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        for i in 1..=n {
            let id = NonZeroU32::new(i).expect("non-zero");
            state.creatures.push(Creature {
                id,
                current_hp: 50,
                max_hp: 50,
                block: 0,
                powers: SmallVec::new(),
                species: Some(MonsterKind::ShrinkerBeetle),
                intent: None,
                intent_history: IntentHistory::default(),
                monster_id: crate::MonsterId("ShrinkerBeetle"),
                ai: None,
            });
            state.enemy_uids.push(CreatureUid(id));
        }
        state
    }

    // ── FairyInABottle automatic trigger (A8 audit §7a companion) ───────

    /// Gating the MANUAL drink of `PotionUsage.Automatic` potions must
    /// not break the AUTOMATIC path: the Fairy still revives its owner
    /// at `floor(0.3 * MaxHp)` and is consumed from the belt (C#
    /// `FairyInABottle.AfterPreventingDeath -> OnUse`).
    #[test]
    fn fairy_auto_revive_fires_despite_manual_drink_gate() {
        let mut state = state_with_cultists(1);
        state.player.max_hp = 70;
        state.player.current_hp = 0;
        let uid = state.init_player_creature();
        state
            .player
            .potions
            .push(crate::potions::PotionKind::FairyInABottle);
        assert!(try_fairy_in_a_bottle_prevent_death(&mut state));
        assert_eq!(state.player.current_hp, 21, "floor(0.3 * 70) = 21");
        let c = state.creatures.iter().find(|c| c.id == uid.0).unwrap();
        assert_eq!(c.current_hp, 21, "creature mirror sees the revive");
        assert!(
            state.player.potions.is_empty(),
            "the automatic trigger (not the drink arm) consumes the Fairy"
        );
    }

    // ── Scalar reset ────────────────────────────────────────────────────

    /// `start_combat` resets `round` to 1 and `player.energy` to 0,
    /// mirroring Python `combat.py:638-642`.
    #[test]
    fn start_combat_resets_round_and_energy() {
        let mut state = state_with_cultists(1);
        state.round = 99;
        state.player.energy = 42;
        state.player.stars = 7;

        let result = start_combat(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.round, 1);
        assert_eq!(state.player.energy, 0);
        assert_eq!(state.player.stars, 0);
    }

    /// `start_combat` sets `side` to Player even if it was Enemy on
    /// entry (mirrors Python `self.current_side = CombatSide.PLAYER`
    /// at combat.py:639).
    #[test]
    fn start_combat_sets_side_to_player() {
        let mut state = state_with_cultists(0);
        state.side = Side::Enemy;

        start_combat(&mut state);

        assert_eq!(state.side, Side::Player);
    }

    // ── Phase advance ───────────────────────────────────────────────────

    /// `start_combat` advances the phase from `Setup` to `PlayerTurnStart`.
    /// P3-2 (sts2-rs-9l9) extends this to `PlayerPlayPhase` once the
    /// turn-start handler is implemented.
    #[test]
    fn start_combat_advances_phase_to_player_turn_start() {
        let mut state = state_with_cultists(0);
        assert_eq!(state.phase, CombatPhase::Setup);

        start_combat(&mut state);

        assert_eq!(state.phase, CombatPhase::PlayerTurnStart);
    }

    // ── Intent roll ─────────────────────────────────────────────────────

    /// Every enemy with `intent: None` on entry has `intent: Some(_)`
    /// after `start_combat`, mirroring Python's `for enemy in
    /// self.alive_enemies: ai.roll_move(...)` loop at combat.py:659-660.
    #[test]
    fn start_combat_rolls_initial_intent_for_each_enemy() {
        let mut state = state_with_cultists(3);
        for c in &state.creatures {
            assert!(c.intent.is_none());
        }

        start_combat(&mut state);

        // Bead `sts2-rs-d03` Phase 1: `start_combat` now also pushes
        // a player-Creature mirror (with `intent: None`). Filter
        // creatures by `enemy_uids` so we only assert on enemies.
        let enemy_ids: smallvec::SmallVec<[u32; 8]> =
            state.enemy_uids.iter().map(|u| u.get()).collect();
        for c in state
            .creatures
            .iter()
            .filter(|c| enemy_ids.contains(&c.id.get()))
        {
            assert!(
                c.intent.is_some(),
                "enemy intent should be Some after start_combat"
            );
        }
    }

    /// Bead `sts2-rs-rnyme` follow-up: an auto-played card
    /// (Havoc / Cascade / DistilledChaos / Mayhem / …) records a
    /// `CardPlayFinished` in C# (`CardModel.cs:1523`), so
    /// `resolve_auto_played_card` must bump the combat-wide
    /// `player_card_plays_this_combat` counter that feeds GoldAxe's
    /// `CalculatedDamageVar` (`GoldAxe.cs:25`). Previously only
    /// `cards_played_this_turn` was bumped, leaving the combat-wide
    /// counter 1-low per auto-play and under-scaling GoldAxe.
    #[test]
    fn auto_played_card_bumps_combat_wide_card_play_counter() {
        let mut state = state_with_cultists(1);
        // One Attack on the Play pile, as `auto_play_*` would have
        // collected it before resolving.
        let uid = crate::CardUid::new(1).expect("non-zero");
        state.cards.push(crate::cards::Card {
            uid,
            card_id: crate::cards::CardId::StrikeIronclad,
            cost: 1,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        state.player.play.push(uid);
        // Seed a non-zero prior combat-wide count to prove the bump is
        // additive (not an absolute assignment).
        state.combat_log.player_card_plays_this_combat = 3;

        resolve_auto_played_card(&mut state, uid, false);

        assert_eq!(
            state.combat_log.player_card_plays_this_combat, 4,
            "auto-played card must bump the combat-wide card-play counter exactly once"
        );
    }

    /// Bead `sts2-rs-zlx.4` follow-up (parity fuzz Necrobinder seed 410): an
    /// auto-played (MAYHEM / Cascade / Havoc) card must fire the player's
    /// `BeforeCardPlayed` power reactions just like a hand-played one — C#
    /// `OnPlayWrapper` runs `Hook.BeforeCardPlayed` for auto-plays too. Here
    /// DanseMacabre(4) grants 4 block when a >=2-cost card auto-plays, and
    /// NOTHING when a 1-cost card auto-plays (the EnergyVar(2) threshold). The
    /// prior partial wiring armed only the Storm snapshot, so an auto-played
    /// >=2-cost card left the player with 0 block.
    #[test]
    #[serial_test::serial(global_registry)]
    fn auto_played_card_fires_danse_macabre_block() {
        use crate::{Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mk = |state: &mut CombatState, cost: i8| -> crate::CardUid {
            let uid = crate::CardUid::new(1).expect("non-zero");
            state.cards.clear();
            state.cards.push(crate::cards::Card {
                uid,
                card_id: crate::cards::CardId::StrikeIronclad,
                cost,
                card_type: crate::cards::CardType::Attack,
                target_type: crate::cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                combat_vars: crate::cards::CombatVars::new(),
                enchantments: crate::cards::CardEnchantments::new(),
                tags: crate::cards::CardTags::NONE,
                energy_spent: 0,
                base_replay_count: 0,
                keyword_override: None,
                affliction: None,
            });
            state.player.play.clear();
            state.player.play.push(uid);
            uid
        };
        let install_danse = |state: &mut CombatState| {
            let pid = std::num::NonZeroU32::new(4104).unwrap();
            state.powers.retain(|p| p.id != pid);
            state.player.powers.retain(|u| u.0 != pid);
            state.powers.push(Power {
                id: pid,
                kind: PowerKind::DanseMacabre,
                amount: 4,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            state.player.powers.push(crate::PowerUid(pid));
        };

        // >=2-cost auto-play: DanseMacabre grants 4 block.
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        install_danse(&mut state);
        let uid = mk(&mut state, 2);
        state.player.block = 0;
        resolve_auto_played_card(&mut state, uid, false);
        assert_eq!(
            state.player.block, 4,
            "auto-played >=2-cost card must fire DanseMacabre block"
        );

        // 1-cost auto-play: below the EnergyVar(2) threshold → no block.
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        install_danse(&mut state);
        let uid = mk(&mut state, 1);
        state.player.block = 0;
        resolve_auto_played_card(&mut state, uid, false);
        assert_eq!(
            state.player.block, 0,
            "a 1-cost auto-play is below DanseMacabre's energy threshold"
        );
    }

    /// Bead `sts2-rs-afterimage-nested` (Silent seed 289): when a card's
    /// `OnPlay` auto-plays nested cards (CATASTROPHE) while the player holds
    /// `AfterimagePower`, C# keys the per-play block snapshot per CardModel
    /// (`amountsForPlayedCards`), so the PARENT's pending tick survives the
    /// nested plays. RS stores that snapshot in a single per-power transient;
    /// `resolve_auto_played_card` now saves+restores it around the nested play.
    /// Here we simulate a parent mid-play (Afterimage armed `counter == 1`) and
    /// assert the nested auto-play (a) grants its OWN +1 block and (b) leaves
    /// the parent's armed snapshot intact for its later after-play grant.
    #[test]
    #[serial_test::serial(global_registry)]
    fn nested_auto_play_preserves_parent_afterimage_snapshot() {
        use crate::{Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let pid = std::num::NonZeroU32::new(7001).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Afterimage,
            amount: 1,
            // Simulate the parent card's BeforeCardPlayed arm (snapshot == 1).
            counter: 1,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let uid = crate::CardUid::new(1).expect("non-zero");
        state.cards.clear();
        state.cards.push(crate::cards::Card {
            uid,
            card_id: crate::cards::CardId::StrikeIronclad,
            cost: 1,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        state.player.play.clear();
        state.player.play.push(uid);
        state.player.block = 0;

        resolve_auto_played_card(&mut state, uid, false);

        assert_eq!(
            state.player.block, 1,
            "nested auto-play grants its own Afterimage block"
        );
        assert_eq!(
            state.powers.get_by_id(pid).expect("afterimage").counter,
            1,
            "parent Afterimage snapshot must survive the nested auto-play"
        );
    }

    /// Regent parity seed 265: an AUTO-played (non-exhaust) SHINING_STRIKE must
    /// return to the TOP of the draw pile, exactly like the hand-play path —
    /// C# `ShiningStrike.OnPlay`'s `CardPileCmd.Add(this, Draw, Top)` runs for
    /// auto-plays too (`OnPlayWrapper`, `IsAutoPlay = true`). The auto-play
    /// resolver previously routed it to Discard, which shrank CATASTROPHE's
    /// next StableShuffle pool and changed its picks.
    #[test]
    #[serial_test::serial(global_registry)]
    fn auto_played_shining_strike_returns_to_draw_top() {
        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let uid = crate::CardUid::new(1).expect("non-zero");
        state.cards.clear();
        state.cards.push(crate::cards::Card {
            uid,
            card_id: crate::cards::CardId::ShiningStrike,
            cost: 1,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 8,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        state.player.play.clear();
        state.player.play.push(uid);
        state.player.draw.clear();
        state.player.discard.clear();

        resolve_auto_played_card(&mut state, uid, /*force_exhaust=*/ false);

        assert_eq!(
            state.player.draw.first().copied(),
            Some(uid),
            "auto-played non-exhaust SHINING_STRIKE returns to the draw-pile top",
        );
        assert!(
            !state.player.discard.contains(&uid),
            "SHINING_STRIKE must NOT land in discard on the auto-play path",
        );
        assert!(
            !state.player.play.contains(&uid),
            "SHINING_STRIKE leaves the play pile",
        );
        // Bead `sts2-rs-09x5n`: this is a definite-position insert
        // (`CardPileCmd.Add(this, Draw, Top)`), same class as
        // Headbutt/ThinkingAhead — the auto-play resolver must pin it too,
        // not just the hand-play path.
        assert_eq!(
            state.player.pinned_top.first().copied(),
            Some(uid),
            "auto-played SHINING_STRIKE's draw-top return must pin Top(0)"
        );
    }

    /// Bead `sts2-rs-09x5n` (reviewer gap): the HAND-PLAY ShiningStrike
    /// branch in `play_card`'s routing epilogue (`is_shining_strike`) must
    /// pin the card it returns to draw-top — driven through a full
    /// `play_card()` so the epilogue itself is exercised, not just the
    /// dispatch handler. Also proves the pin is live knowledge under
    /// BeliefDraw: the next `pop_next_card` takes the pinned card with
    /// ZERO RNG reads even though a >=2-card heterogeneous middle sits
    /// beneath it.
    #[test]
    #[serial_test::serial(global_registry)]
    fn hand_played_shining_strike_returns_to_draw_top_pins_and_pops_rng_free() {
        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;
        state.player.energy = 3;
        state.player.deferred_draw_order = true;

        let mk_card = |uid_n: u32, id: crate::cards::CardId| crate::cards::Card {
            uid: crate::CardUid::new(uid_n).expect("non-zero"),
            card_id: id,
            cost: 1,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 8,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        };
        state.cards.clear();
        state
            .cards
            .push(mk_card(1, crate::cards::CardId::ShiningStrike));
        state
            .cards
            .push(mk_card(2, crate::cards::CardId::StrikeIronclad));
        state.cards.push(mk_card(3, crate::cards::CardId::Anger));
        let ss_uid = crate::CardUid::new(1).expect("non-zero");
        state.player.hand.clear();
        state.player.hand.push(ss_uid);
        // A 2-card heterogeneous unknown middle beneath the returned card,
        // so the RNG-free pop below is non-trivial.
        state.player.draw.clear();
        state.player.draw.push(crate::CardUid::new(2).expect("nz"));
        state.player.draw.push(crate::CardUid::new(3).expect("nz"));
        state.player.discard.clear();
        state.player.play.clear();

        let res = play_card(&mut state, 0, Some(0));
        assert_eq!(res, StepResult::Advanced, "hand play resolves");

        assert_eq!(
            state.player.draw.first().copied(),
            Some(ss_uid),
            "hand-played non-exhaust SHINING_STRIKE returns to the draw-pile top"
        );
        assert!(
            !state.player.discard.contains(&ss_uid),
            "must NOT land in discard"
        );
        assert_eq!(
            state.player.pinned_top.first().copied(),
            Some(ss_uid),
            "the hand-play ShiningStrike epilogue must pin Top(0)"
        );

        // BeliefDraw: popping the pinned card is player-KNOWN — no RNG.
        let before = state.run_rng.shuffle.counter();
        let popped = cards::pop_next_card(&mut state.player, &mut state.run_rng.shuffle);
        assert_eq!(popped, ss_uid, "the pinned card pops first");
        assert_eq!(
            state.run_rng.shuffle.counter(),
            before,
            "popping the pinned ShiningStrike must consume NO RNG (no chance node)"
        );
    }

    /// Bead `sts2-rs-1sew4` companion / `sts2-rs-09x5n`: an auto-played
    /// (non-exhaust) Attack under `NostalgiaPower` must redirect to the TOP
    /// of the draw pile, exactly like the hand-play path AND pin that
    /// position — MCTS PIMC re-shuffles must not scramble a card the player
    /// just definitely watched land on top of their draw pile.
    #[test]
    #[serial_test::serial(global_registry)]
    fn auto_played_nostalgia_redirect_returns_to_draw_top_and_pins() {
        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let pid = NonZeroU32::new(1).expect("non-zero");
        state.powers.push(crate::Power {
            id: pid,
            kind: crate::PowerKind::Nostalgia,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let uid = crate::CardUid::new(1).expect("non-zero");
        state.cards.clear();
        state.cards.push(crate::cards::Card {
            uid,
            card_id: crate::cards::CardId::StrikeIronclad,
            cost: 1,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        state.player.play.clear();
        state.player.play.push(uid);
        state.player.draw.clear();
        state.player.discard.clear();

        resolve_auto_played_card(&mut state, uid, /*force_exhaust=*/ false);

        assert_eq!(
            state.player.draw.first().copied(),
            Some(uid),
            "NostalgiaPower redirects the first-Amount auto-played Attack \
             to the draw-pile top"
        );
        assert!(
            !state.player.discard.contains(&uid),
            "must NOT land in discard when Nostalgia redirects it"
        );
        assert_eq!(
            state.player.pinned_top.first().copied(),
            Some(uid),
            "the Nostalgia auto-play redirect must pin Top(0)"
        );
    }

    /// Bead `sts2-rs-qu5wg` companion / `sts2-rs-09x5n`: the DEFERRED flush
    /// path (`flush_played_cards_after_choice`, fired once a choice-opening
    /// card's `PendingChoice` resolves) applies the same NostalgiaPower
    /// draw-top redirect as the immediate `play_card` routing block — and
    /// must pin it too.
    #[test]
    fn flush_played_cards_after_choice_nostalgia_redirect_pins_draw_top() {
        use crate::{Power, PowerKind};

        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };

        let pid = std::num::NonZeroU32::new(1).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Nostalgia,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let uid = crate::CardUid::new(1).expect("non-zero");
        state.cards.push(crate::cards::Card {
            uid,
            card_id: crate::cards::CardId::StrikeIronclad,
            cost: 1,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        // A choice-opening card (Headbutt/Armaments-shape) held the played
        // card in Play limbo; `cards_played_this_turn` is NOT yet tallied
        // for it (the immediate routing block was skipped while the choice
        // was open) — the flush path itself counts it inclusively, same as
        // the immediate path.
        state.player.play.push(uid);

        flush_played_cards_after_choice(&mut state);

        assert_eq!(
            state.player.draw.first().copied(),
            Some(uid),
            "the deferred flush applies the Nostalgia draw-top redirect"
        );
        assert!(state.player.play.is_empty(), "Play pile drained");
        assert!(!state.player.discard.contains(&uid));
        assert_eq!(
            state.player.pinned_top.first().copied(),
            Some(uid),
            "the deferred-flush Nostalgia redirect must pin Top(0)"
        );
    }

    /// Bead `sts2-rs-afterimage-nested` companion (Silent seed 262 family):
    /// the auto-play replay loop fires the FULL `BeforeCardPlayed` dispatch per
    /// iteration (C# `OnPlayWrapper`, CardModel.cs:1496), so a HiddenGem-style
    /// `base_replay_count` card auto-played under Afterimage(1) grants block on
    /// EVERY play, not just the first. Previously the loop only re-armed Storm.
    #[test]
    #[serial_test::serial(global_registry)]
    fn auto_played_replay_rearms_afterimage_per_play() {
        use crate::{Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let pid = std::num::NonZeroU32::new(7002).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Afterimage,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let uid = crate::CardUid::new(1).expect("non-zero");
        state.cards.clear();
        state.cards.push(crate::cards::Card {
            uid,
            card_id: crate::cards::CardId::StrikeIronclad,
            cost: 1,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            // HiddenGem-style replay: +2 → plays 3× total.
            base_replay_count: 2,
            keyword_override: None,
            affliction: None,
        });
        state.player.play.clear();
        state.player.play.push(uid);
        state.player.block = 0;

        resolve_auto_played_card(&mut state, uid, false);

        assert_eq!(
            state.player.block, 3,
            "Afterimage must tick once per replay (1 initial + 2 replays)"
        );
    }

    /// Bead `sts2-rs-t12kj` (seed 1576): an auto-played Attack must
    /// honor the player's `OneTwoPunch` power exactly like a hand-played one
    /// — C# `CardModel.OnPlayWrapper` folds `ModifyCardPlayCount` +
    /// `AfterModifyingCardPlayCount` regardless of `isAutoPlay`. So the
    /// auto-played Strike hits TWICE under OneTwoPunch(1), and the power
    /// decrements to 0 and is removed.
    #[test]
    #[serial_test::serial(global_registry)]
    fn auto_played_attack_honors_one_two_punch() {
        use crate::{Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        // Install OneTwoPunch(1) on the player.
        let pid = std::num::NonZeroU32::new(7611).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::OneTwoPunch,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let enemy_uid = state.enemy_uids[0];
        let enemy_idx = state
            .creatures
            .iter()
            .position(|c| c.id == enemy_uid.0)
            .expect("enemy creature");
        let hp_before = state.creatures[enemy_idx].current_hp;

        // One Attack on the Play pile, as `auto_play_*` would have collected
        // it before resolving.
        let uid = crate::CardUid::new(1).expect("non-zero");
        state.cards.push(crate::cards::Card {
            uid,
            card_id: crate::cards::CardId::StrikeIronclad,
            cost: 1,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        state.player.play.push(uid);

        resolve_auto_played_card(&mut state, uid, false);

        let dealt = hp_before - state.creatures[enemy_idx].current_hp;
        assert_eq!(
            dealt, 12,
            "auto-played Attack under OneTwoPunch(1) should hit twice (6 + 6 = 12)"
        );
        assert!(
            !state.powers.iter().any(|p| p.id == pid),
            "OneTwoPunch(1) should be consumed (removed) by the auto-play"
        );
        assert!(
            !state.player.powers.iter().any(|u| u.0 == pid),
            "player.powers should not retain the popped OneTwoPunch uid"
        );
    }

    /// Replay primitive (bead `sts2-rs-v0107-content-port-wb8lk.3`): a card
    /// whose instance carries `base_replay_count = 1` ("Replay 1") re-resolves
    /// its play exactly one EXTRA time when played from hand — mirroring C#
    /// `CardModel.GeneratePlayCount` (`BaseReplayCount + 1`) and the
    /// per-iteration `OnPlay` loop. A 6-damage Strike with Replay 1 therefore
    /// hits the target twice for 12 total.
    #[test]
    #[serial_test::serial(global_registry)]
    fn replay_one_resolves_card_twice() {
        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        let _ = start_combat(&mut state);
        state.player.energy = 3;

        let enemy_uid = state.enemy_uids[0];
        let enemy_idx_in_creatures = state
            .creatures
            .iter()
            .position(|c| c.id == enemy_uid.0)
            .expect("enemy creature");
        let hp_before = state.creatures[enemy_idx_in_creatures].current_hp;

        let uid = crate::CardUid::new((state.cards.len() + 1) as u32).expect("non-zero");
        state.cards.push(crate::cards::Card {
            uid,
            card_id: crate::cards::CardId::StrikeIronclad,
            cost: 1,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            // Replay 1 → 1 initial play + 1 replay = 2 resolutions.
            base_replay_count: 1,
            keyword_override: None,
            affliction: None,
        });
        state.player.hand.push(uid);
        state.phase = CombatPhase::PlayerPlayPhase;
        let hand_idx = (state.player.hand.len() - 1) as u8;

        let enemy_idx: u8 = 0;
        let _ = play_card(&mut state, hand_idx, Some(enemy_idx));

        let dealt = hp_before - state.creatures[enemy_idx_in_creatures].current_hp;
        assert_eq!(
            dealt, 12,
            "Replay 1 Strike should resolve twice (6 + 6 = 12)"
        );
    }

    /// Regression (bead `sts2-rs-4nhp1`): ParticleWall's return-to-hand is a
    /// result-pile override applied at the single move-out-of-Play disposal
    /// step, NOT inside `OnPlay`. Doing the move inside the handler removed the
    /// resolving card from the Play pile mid-resolve, so a replay
    /// (`base_replay_count` > 0, EchoForm, …) re-dispatched the OnPlay body
    /// after the card had left Play and `current_block`'s `play.last()` panicked
    /// with "dispatch: empty Play pile" (crashed ~2.8% of heuristic Regent runs;
    /// full-run seed 22 is the deterministic repro — the OnPlay-side panic is
    /// gated there, since the content play-handler map isn't linked into this
    /// core-crate harness). This test drives the disposal routing directly: a
    /// played ParticleWall must survive its replay ON the Play pile and be
    /// routed to Hand (not Discard) exactly once when its plays finish.
    #[test]
    #[serial_test::serial(global_registry)]
    fn particle_wall_defers_return_to_hand_to_disposal() {
        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        let _ = start_combat(&mut state);
        state.player.energy = 3;

        let uid = crate::CardUid::new((state.cards.len() + 1) as u32).expect("non-zero");
        state.cards.push(crate::cards::Card {
            uid,
            card_id: crate::cards::CardId::ParticleWall,
            cost: 0,
            card_type: crate::cards::CardType::Skill,
            target_type: crate::cards::TargetType::Self_,
            upgraded: false,
            base_damage: 0,
            base_block: 9,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            // Replay 1 → 1 initial play + 1 replay = 2 resolutions; pre-fix the
            // second dispatch hit an empty Play pile.
            base_replay_count: 1,
            keyword_override: None,
            affliction: None,
        });
        state.player.hand.push(uid);
        state.phase = CombatPhase::PlayerPlayPhase;
        let hand_idx = (state.player.hand.len() - 1) as u8;

        // Must not panic ("dispatch: empty Play pile") across the replay.
        let _ = play_card(&mut state, hand_idx, None);

        assert_eq!(
            state.player.hand.iter().filter(|&&u| u == uid).count(),
            1,
            "ParticleWall must be routed back to Hand exactly once by the disposal step"
        );
        assert!(
            !state.player.play.contains(&uid),
            "ParticleWall must leave the Play pile after its plays finish"
        );
        assert!(
            !state.player.discard.contains(&uid),
            "ParticleWall's Discard result must be overridden to Hand, not Discard"
        );
    }

    /// Bead `sts2-rs-rnyme` follow-up: an UNPLAYABLE auto-surfaced card
    /// (curse / status off the draw top) is routed without playing
    /// (C# `MoveToResultPileWithoutPlaying`, which records NO
    /// `CardPlayFinished`), so it must NOT bump the combat-wide counter.
    #[test]
    fn auto_played_unplayable_card_does_not_bump_counter() {
        let mut state = state_with_cultists(1);
        let uid = crate::CardUid::new(1).expect("non-zero");
        state.cards.push(crate::cards::Card {
            uid,
            // Dazed is an unplayable status card.
            card_id: crate::cards::CardId::Dazed,
            cost: 0,
            card_type: crate::cards::CardType::Status,
            target_type: crate::cards::TargetType::None,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: Some(crate::cards::KeywordOverride::UNPLAYABLE),
            affliction: None,
        });
        state.player.play.push(uid);
        state.combat_log.player_card_plays_this_combat = 3;

        resolve_auto_played_card(&mut state, uid, false);

        assert_eq!(
            state.combat_log.player_card_plays_this_combat, 3,
            "unplayable auto-surfaced card must NOT bump the combat-wide card-play counter"
        );
    }

    /// Bead `sts2-rs-ztx5u`: CubexConstruct gains 13 spawn block via
    /// [`CombatState::on_spawn_block`] (C# `AfterAddedToRoom`
    /// `GainBlock(13)`), but `start_combat` clears all enemy block
    /// before the player's first turn — matching the v0.103.2 headless
    /// harness, which reports CUBEX_CONSTRUCT `block=0` at turn 0.
    #[test]
    fn start_combat_clears_enemy_spawn_block() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        let uid = state.spawn_enemy(MonsterKind::CubexConstruct);
        let block_of = |s: &CombatState, uid: CreatureUid| {
            s.creatures
                .iter()
                .find(|c| c.id == uid.0)
                .expect("spawned")
                .block
        };
        // The spawn-time grant is present before combat starts.
        assert_eq!(
            block_of(&state, uid),
            13,
            "on_spawn_block grants 13 at spawn (AfterAddedToRoom)"
        );
        start_combat(&mut state);
        assert_eq!(
            block_of(&state, uid),
            0,
            "enemy spawn block must be cleared before the player's first turn"
        );
    }

    // The `start_combat_assigns_cultist_initial_buff_intent` test was
    // removed in bead `sts2-rs-v1u5` along with the STS1 Cultist stub
    // whose turn-0 = Buff (Incantation) behavior it pinned.

    // ── Combat-end short-circuit ────────────────────────────────────────

    /// If `is_over=true` and `player_won=true` on entry, `start_combat`
    /// short-circuits with `CombatWon` and does NOT mutate scalars or
    /// roll intents. This is the defensive guard for the canonical
    /// caller invoking us after combat already resolved.
    #[test]
    fn start_combat_short_circuits_when_combat_over_won() {
        let mut state = state_with_cultists(1);
        state.is_over = true;
        state.player_won = true;
        state.round = 99;
        state.player.energy = 42;

        let result = start_combat(&mut state);

        assert_eq!(result, StepResult::CombatWon);
        // Scalars unchanged; intent NOT rolled.
        assert_eq!(state.round, 99);
        assert_eq!(state.player.energy, 42);
        assert!(state.creatures[0].intent.is_none());
        assert_eq!(state.phase, CombatPhase::Setup);
    }

    /// If `is_over=true` and `player_won=false`, return `CombatLost`.
    #[test]
    fn start_combat_short_circuits_when_combat_over_lost() {
        let mut state = state_with_cultists(1);
        state.is_over = true;
        state.player_won = false;

        let result = start_combat(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert!(state.creatures[0].intent.is_none());
        assert_eq!(state.phase, CombatPhase::Setup);
    }

    // ── Determinism ─────────────────────────────────────────────────────

    /// Same seed → same enemy intents. Cultist's intent is fixed
    /// (turn-0 = Buff), so the determinism check uses a `JawWorm`
    /// state where the first-turn roll is RNG-driven (one of three
    /// moves). Two state copies seeded identically must produce
    /// identical results.
    #[test]
    fn start_combat_is_deterministic_given_seed() {
        fn build(seed: u64) -> CombatState {
            let mut state = CombatState {
                rng: crate::dotnet_rng::Rng::new((seed) as i32),
                ..Default::default()
            };
            let id = NonZeroU32::new(1).expect("non-zero");
            state.creatures.push(Creature {
                id,
                current_hp: 44,
                max_hp: 44,
                block: 0,
                powers: SmallVec::new(),
                species: Some(MonsterKind::FuzzyWurmCrawler),
                monster_id: crate::MonsterId("FuzzyWurmCrawler"),
                intent: None,
                intent_history: IntentHistory::default(),
                ai: None,
            });
            state.enemy_uids.push(CreatureUid(id));
            state
        }

        let mut a = build(7);
        let mut b = build(7);
        let mut c = build(7);
        start_combat(&mut a);
        start_combat(&mut b);
        start_combat(&mut c);

        assert_eq!(a.creatures[0].intent, b.creatures[0].intent);
        assert_eq!(b.creatures[0].intent, c.creatures[0].intent);
    }

    /// Bead `sts2-rs-wlbg`: `start_combat` shuffles the draw pile, so
    /// different seeds produce different opening orders for a deck
    /// staged in canonical list order. Before this fix the deck was
    /// served TOML-list order regardless of seed.
    #[test]
    fn start_combat_shuffles_draw_pile() {
        use crate::CardUid;
        fn build_with_deck(seed: u64) -> CombatState {
            let mut state = CombatState {
                rng: crate::dotnet_rng::Rng::new(seed as i32),
                // Bead `sts2-rs-943w7`: shuffle is now driven by
                // `state.run_rng.shuffle`, so the seed must thread through
                // here too — otherwise both seeds reuse `RunRngSet::new(0)`
                // and produce identical shuffles, breaking this test.
                run_rng: crate::run_rng_set::RunRngSet::new(seed as i32),
                ..Default::default()
            };
            // 20-card deck of distinct CardUids in canonical order.
            // Large enough that ~all seeds produce distinct shuffles.
            for i in 1..=20u32 {
                state
                    .player
                    .draw
                    .push(CardUid(NonZeroU32::new(i).expect("non-zero")));
            }
            state
        }

        // Two different seeds → two different draw orders.
        let mut a = build_with_deck(1);
        let mut b = build_with_deck(2);
        let pre = a.player.draw.clone();
        start_combat(&mut a);
        start_combat(&mut b);
        assert_ne!(
            a.player.draw, pre,
            "seed 1 should have shuffled the deck off TOML order"
        );
        assert_ne!(
            a.player.draw, b.player.draw,
            "different seeds should yield different shuffles"
        );

        // Same seed → same draw order (determinism preserved).
        let mut c = build_with_deck(1);
        start_combat(&mut c);
        assert_eq!(a.player.draw, c.player.draw);
    }

    // The `start_combat_jaw_worm_distribution_varies_across_seeds` test
    // was removed in bead `sts2-rs-v1u5` along with the STS1 JawWorm
    // stub whose 3-way turn-0 distribution it pinned. STS2-native
    // monster AI placeholders are deterministic, so the
    // "≥2 distinct intents over 20 seeds" property no longer holds at
    // the `MonsterKind::roll_intent` layer — the canonical AI path
    // routes through `MonsterAi::current_move` instead.

    // ── No-enemy degenerate case ────────────────────────────────────────

    /// `start_combat` on a no-enemy state is a no-op for the intent
    /// loop and still resets scalars + advances phase. Defensive
    /// shape check.
    #[test]
    fn start_combat_with_no_enemies_still_advances_phase() {
        let mut state = state_with_cultists(0);
        state.round = 50;

        let result = start_combat(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.round, 1);
        assert_eq!(state.phase, CombatPhase::PlayerTurnStart);
    }

    // ── mid-draw combat-end reshuffle gate (sts2-rs-draw-end-reshuffle) ──

    /// `combat_is_ending` mirrors C# `CombatManager.IsEnding`: ending iff the
    /// player is dead OR no alive PRIMARY enemy remains.
    #[test]
    fn combat_is_ending_predicate_cases() {
        // Alive primary → not ending.
        assert!(!combat_is_ending(&state_with_cultists(1)));

        // All primaries dead → ending.
        let mut dead = state_with_cultists(2);
        dead.creatures[0].current_hp = 0;
        dead.creatures[1].current_hp = 0;
        assert!(combat_is_ending(&dead));

        // One dead, one alive → not ending (combat continues).
        let mut mixed = state_with_cultists(2);
        mixed.creatures[0].current_hp = 0;
        assert!(!combat_is_ending(&mixed));

        // Player dead → ending (C# pending-loss).
        let mut ploss = state_with_cultists(1);
        ploss.player.current_hp = 0;
        assert!(combat_is_ending(&ploss));

        // Degenerate empty roster → NOT ending (conservative: a real combat
        // always retains the dying corpse, so the gate stays inert here rather
        // than spuriously treating a fixture as resolved).
        assert!(!combat_is_ending(&state_with_cultists(0)));
    }

    /// Regression (PARITY86): when an on-draw effect downs the last enemy
    /// MID-DRAW, the `is_over` flip is deferred to the turn-start tail, so a
    /// naive draw loop would keep going and RESHUFFLE the discard pile for the
    /// remaining requested cards — over-reading the run-level `shuffle` stream
    /// and desyncing every later combat. C# `CardPileCmd.Shuffle` early-returns
    /// on `IsOverOrEnding`; `draw_suspending` mirrors that via
    /// `combat_is_ending`, so a combat-ending draw with an empty draw pile does
    /// NOT reshuffle.
    #[test]
    fn draw_suspending_skips_reshuffle_when_combat_is_ending() {
        let mut state = state_with_cultists(1);
        // Down the only enemy. `is_over` stays false (the turn-start flip is
        // deferred), exactly as during the mid-draw kill.
        state.creatures[0].current_hp = 0;
        assert!(!state.is_over);
        assert!(combat_is_ending(&state));
        // Empty draw pile + non-empty discard: a reshuffle WOULD fire if ungated.
        for i in 10u32..=15 {
            state
                .player
                .discard
                .push(CardUid::new(i).expect("non-zero"));
        }
        let shuf_before = state.run_rng.shuffle.counter();
        let moved = state.draw_suspending(5, true);
        assert_eq!(moved, 0, "no card drawn once combat is ending + draw empty");
        assert_eq!(
            state.run_rng.shuffle.counter(),
            shuf_before,
            "a combat-ending draw must NOT reshuffle (C# Shuffle IsOverOrEnding early-return)"
        );
        assert_eq!(state.player.discard.len(), 6, "discard left untouched");
    }

    /// Contrapositive: with an alive enemy the same empty-draw/non-empty-discard
    /// draw reshuffles as usual — the gate is inert during normal combat.
    #[test]
    fn draw_suspending_reshuffles_when_enemy_alive() {
        let mut state = state_with_cultists(1); // enemy alive (hp 50)
        assert!(!combat_is_ending(&state));
        for i in 10u32..=15 {
            state
                .player
                .discard
                .push(CardUid::new(i).expect("non-zero"));
        }
        let shuf_before = state.run_rng.shuffle.counter();
        let moved = state.draw_suspending(3, true);
        assert!(moved > 0, "draws after a normal reshuffle when enemy alive");
        assert!(
            state.run_rng.shuffle.counter() > shuf_before,
            "an alive-enemy draw reshuffles the discard pile as usual"
        );
    }

    // ── P3-2 (sts2-rs-9l9) start_player_turn ────────────────────────────

    use crate::CardUid;

    /// Build a `CombatState` ready to invoke `start_player_turn`.
    /// `round` is set to `round`, phase to `PlayerTurnStart`, RNG
    /// seeded for determinism. The draw pile contains `draw_n` cards
    /// with sequential UIDs (`1..=draw_n`) so draw assertions can
    /// observe the FIFO pop order Python uses (`combat.py:1124`
    /// pops index 0).
    fn state_for_turn_start(round: u16, draw_n: u32) -> CombatState {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerTurnStart;
        state.round = round;
        for i in 1..=draw_n {
            state.player.draw.push(CardUid::new(i).expect("non-zero"));
        }
        state
    }

    /// Bolas / ThrummingHatchet `BeforeHandDraw`: a card played LAST round
    /// returns to hand at this turn's start; one played the same round (or
    /// never) does not. Bead `sts2-rs-219ds`.
    #[test]
    fn before_hand_draw_returns_self_returning_attack_played_last_round() {
        let mut state = state_for_turn_start(3, 0);
        let mk =
            |state: &mut CombatState, id: crate::cards::CardId, played_round: i32| -> CardUid {
                let uid = CardUid::new(u32::try_from(state.cards.len() + 1).expect("fits"))
                    .expect("non-zero");
                let mut card = crate::cards::Card {
                    uid,
                    card_id: id,
                    cost: 0,
                    card_type: crate::cards::CardType::Attack,
                    target_type: crate::cards::TargetType::Enemy,
                    upgraded: false,
                    base_damage: 3,
                    base_block: 0,
                    combat_vars: crate::cards::CombatVars::new(),
                    enchantments: crate::cards::CardEnchantments::new(),
                    tags: crate::cards::CardTags::NONE,
                    energy_spent: 0,
                    base_replay_count: 0,
                    keyword_override: None,
                    affliction: None,
                };
                if played_round != 0 {
                    card.combat_vars
                        .push((VarKey::ReturnIfPlayedRound, played_round));
                }
                state.cards.push(card);
                uid
            };
        // round == 3, so "last round" == 2.
        let returns = mk(&mut state, cards::CardId::Bolas, 2); // played last round → returns
        let stale = mk(&mut state, cards::CardId::ThrummingHatchet, 1); // played round 1 → stays
        let fresh = mk(&mut state, cards::CardId::Bolas, 0); // never played → stays
        state.player.discard.extend([returns, stale, fresh]);
        // Fill draw so the natural turn-start draw doesn't reshuffle the
        // discard pile (which would scatter `stale`/`fresh`). These dummies
        // have no backing `Card` entry, which the sweep's `.get()` tolerates.
        for i in 100u32..115 {
            state.player.draw.push(CardUid::new(i).expect("non-zero"));
        }

        start_player_turn(&mut state);

        assert!(
            state.player.hand.contains(&returns),
            "Bolas played last round returns to hand"
        );
        assert!(
            state.player.discard.contains(&stale),
            "card played two rounds ago stays in discard"
        );
        assert!(
            state.player.discard.contains(&fresh),
            "never-played card stays in discard"
        );
    }

    /// Round > 1: block on entry must be cleared to 0.
    /// Mirrors Python's `owner.clear_block(self)` at combat.py:699.
    #[test]
    fn turn_start_clears_block_after_round_one() {
        let mut state = state_for_turn_start(2, 0);
        state.player.block = 10;

        let result = start_player_turn(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(
            state.player.block, 0,
            "round > 1 must clear block via combat.py:699 path",
        );
    }

    /// Bead `sts2-rs-ybtmu`: a burrowed enemy (Tunneler) RETAINS its block
    /// across the enemy turn-start clear — C# `BurrowedPower.ShouldClearBlock`
    /// returns false for its owner (BurrowedPower.cs:13-20). A non-burrowed
    /// enemy in the same roster still clears to 0.
    #[test]
    fn enemy_turn_start_burrowed_retains_block() {
        use crate::powers::PowerKind;
        use crate::{Power, PowerUid};
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.round = 2;
        state.phase = CombatPhase::EnemyTurnStart;
        let burrowed_uid = state.spawn_enemy(MonsterKind::Tunneler);
        let plain_uid = state.spawn_enemy(MonsterKind::Nibbit);

        // Give both enemies 15 block; arm Burrowed on the first only.
        let bidx = state
            .creatures
            .iter()
            .position(|c| c.id == burrowed_uid.0)
            .expect("burrowed enemy");
        let pidx = state
            .creatures
            .iter()
            .position(|c| c.id == plain_uid.0)
            .expect("plain enemy");
        state.creatures[bidx].block = 15;
        state.creatures[pidx].block = 15;
        let pid = std::num::NonZeroU32::new(9001).expect("nz");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Burrowed,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures[bidx].powers.push(PowerUid(pid));

        enemy_turn_start(&mut state);

        assert_eq!(
            state.creatures[bidx].block, 15,
            "burrowed enemy retains block (ShouldClearBlock=false for owner)"
        );
        assert_eq!(
            state.creatures[pidx].block, 0,
            "non-burrowed enemy clears block normally"
        );
    }

    /// Bead `sts2-rs-8ukqd` (parity-fuzz R2): `start_player_turn` must
    /// re-assert `state.side = Side::Player`. The enemy turn flips it to
    /// `Side::Enemy`; if it is not restored, every player-turn-start /
    /// `after_block_cleared` relic+power that proxies "creature is owner"
    /// via `state.side == Side::Player` silently no-ops from round 2 on
    /// (HornCleat, SparklingRouge, CaptainsWheel, SturdyClamp). Mirrors
    /// the enemy-side `state.side = Side::Enemy` assertion at turn start.
    #[test]
    fn turn_start_resets_side_to_player() {
        let mut state = state_for_turn_start(2, 0);
        state.side = Side::Enemy;

        start_player_turn(&mut state);

        assert_eq!(
            state.side,
            Side::Player,
            "start_player_turn must restore side to Player so owner-gated \
             turn-start / block-cleared listeners fire (sts2-rs-8ukqd R2)",
        );
    }

    /// Bead `sts2-rs-ioty0` (q6bix ROLLING_BOULDER): RollingBoulderPower's
    /// `AfterPlayerTurnStart` deals `Amount` Unpowered damage to every
    /// hittable enemy, then ramps `Amount += 5`. Two enemies each lose the
    /// boulder amount; the power then sits at amount+5 for next turn.
    #[test]
    fn turn_start_rolling_boulder_hits_all_enemies_and_ramps() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerTurnStart;
        state.round = 2;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        // Two enemies (uids 1, 2) at 30 HP each.
        for i in 1..=2u32 {
            let id = NonZeroU32::new(i).expect("non-zero");
            state.creatures.push(Creature {
                id,
                current_hp: 30,
                max_hp: 30,
                block: 0,
                powers: SmallVec::new(),
                species: Some(MonsterKind::ShrinkerBeetle),
                intent: None,
                intent_history: IntentHistory::default(),
                monster_id: crate::MonsterId("ShrinkerBeetle"),
                ai: None,
            });
            state.enemy_uids.push(CreatureUid(id));
        }
        // Player mirror creature + RollingBoulder(5).
        let player_uid = state.init_player_creature();
        crate::card_effects::apply_power_to_creature(
            &mut state,
            player_uid,
            crate::PowerKind::RollingBoulder,
            5,
        );

        start_player_turn(&mut state);

        for &CreatureUid(id) in &state.enemy_uids {
            let hp = state
                .creatures
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.current_hp)
                .expect("enemy present");
            assert_eq!(hp, 25, "each enemy took 5 Unpowered boulder damage");
        }
        assert_eq!(
            state.player_power_amount(crate::PowerKind::RollingBoulder),
            10,
            "RollingBoulder ramps 5 -> 10 (+DamageVar(5)) after firing",
        );
    }

    /// Bead `sts2-rs-adsld` (cascade triage, TURRET_OPERATOR_WEAK): the
    /// turn-start `AfterPlayerTurnStart` cluster (RollingBoulder) must fire
    /// BEFORE the `AfterSideTurnStart` walk (Rampart), matching C#
    /// `CombatManager` (`AfterPlayerTurnStart` at `CombatManager.cs:562`
    /// precedes `Hook.AfterSideTurnStart` at line 437). With the correct
    /// ordering RollingBoulder's Unpowered damage lands on the TurretOperator
    /// at BARE HP and Rampart's 25 block is granted afterward — so the turret
    /// ends the player's turn-start at `hp = 41 - boulder` AND `block = 25`.
    /// The pre-fix inverted order granted the 25 block first, so the boulder
    /// was eaten by the shield (`hp` unchanged, `block = 25 - boulder`) — the
    /// exact TURRET_OPERATOR_WEAK hp/block divergence.
    #[test]
    fn turn_start_rolling_boulder_precedes_rampart_block() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerTurnStart;
        state.round = 2;
        state.player.current_hp = 50;
        state.player.max_hp = 50;

        // LivingShield (uid 10) carrying Rampart(25) — re-grants 25 retained
        // block to each living TurretOperator on every PLAYER turn-start.
        let rampart_pid = NonZeroU32::new(1).expect("non-zero");
        state.powers.push(crate::Power {
            id: rampart_pid,
            kind: crate::PowerKind::Rampart,
            amount: 25,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let shield_id = NonZeroU32::new(10).expect("non-zero");
        state.creatures.push(Creature {
            id: shield_id,
            current_hp: 55,
            max_hp: 55,
            block: 0,
            powers: SmallVec::from_buf_and_len([crate::PowerUid(rampart_pid); 8], 1),
            species: Some(MonsterKind::LivingShield),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("LivingShield"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(shield_id));

        // TurretOperator (uid 11) at 41 HP, 0 block.
        let turret_id = NonZeroU32::new(11).expect("non-zero");
        state.creatures.push(Creature {
            id: turret_id,
            current_hp: 41,
            max_hp: 41,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::TurretOperator),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("TurretOperator"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(turret_id));

        // Player mirror + RollingBoulder(10).
        let player_uid = state.init_player_creature();
        crate::card_effects::apply_power_to_creature(
            &mut state,
            player_uid,
            crate::PowerKind::RollingBoulder,
            10,
        );

        start_player_turn(&mut state);

        let turret = state
            .creatures
            .iter()
            .find(|c| c.id == turret_id)
            .expect("turret present");
        assert_eq!(
            turret.current_hp, 31,
            "boulder (10) hits the turret at bare HP BEFORE Rampart blocks: 41 -> 31",
        );
        assert_eq!(
            turret.block, 25,
            "Rampart grants its 25 block AFTER the boulder lands",
        );
    }

    /// Bead `sts2-rs-181` (CEREMONIAL_BEAST_BOSS seed 3515, PREP_TIME +
    /// MAYHEM_CARD): in v0.107.1 `MayhemPower` is `AfterAutoPrePlayPhaseEntered`
    /// (`MayhemPower.cs:16`), whose AutoPrePlay phase C# `CombatManager` runs at
    /// `:570` — AFTER `Hook.AfterSideTurnStart` (`:524`). So a turn-start
    /// `PrepTimePower`'s Vigor (granted in `after_side_turn_start`) is LIVE when
    /// Mayhem's auto-played powered attack resolves and is CONSUMED by it. The
    /// prior port classed Mayhem as v0.103.2's `AfterPlayerTurnStart` (before
    /// `AfterSideTurnStart`), firing it in the cluster walk so the freshly-granted
    /// Vigor(4) dangled. This pins the corrected ordering: Mayhem's Strike deals
    /// 6 + 4 = 10 and leaves no Vigor on the player.
    #[test]
    #[serial_test::serial(global_registry)]
    fn turn_start_mayhem_autoplay_consumes_prep_time_vigor() {
        crate::card_effects::ensure_installed();

        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerTurnStart;
        state.round = 2;
        state.player.current_hp = 80;
        state.player.max_hp = 80;

        // Single enemy at 50 HP.
        let enemy_id = NonZeroU32::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id: enemy_id,
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::ShrinkerBeetle),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("ShrinkerBeetle"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(enemy_id));

        // Player mirror + PrepTime(4) (turn-start Vigor grant) + Mayhem(1).
        let player_uid = state.init_player_creature();
        crate::card_effects::apply_power_to_creature(
            &mut state,
            player_uid,
            crate::PowerKind::PrepTime,
            4,
        );
        crate::card_effects::apply_power_to_creature(
            &mut state,
            player_uid,
            crate::PowerKind::Mayhem,
            1,
        );

        // Fill the draw pile with powered Strikes. The natural turn-start draw
        // pulls several into hand; Mayhem(1) then auto-plays the Strike still on
        // top of the draw pile (a POWERED attack, so it benefits from + consumes
        // Vigor).
        for _ in 0..12u32 {
            let uid = crate::CardUid::new(u32::try_from(state.cards.len() + 1).expect("fits"))
                .expect("non-zero");
            state.cards.push(crate::cards::Card {
                uid,
                card_id: crate::cards::CardId::StrikeIronclad,
                cost: 1,
                card_type: crate::cards::CardType::Attack,
                target_type: crate::cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                combat_vars: crate::cards::CombatVars::new(),
                enchantments: crate::cards::CardEnchantments::new(),
                tags: crate::cards::CardTags::NONE,
                energy_spent: 0,
                base_replay_count: 0,
                keyword_override: None,
                affliction: None,
            });
            state.player.draw.push(uid);
        }

        let enemy_idx = state
            .creatures
            .iter()
            .position(|c| c.id == enemy_id)
            .expect("enemy present");
        let hp_before = state.creatures[enemy_idx].current_hp;

        start_player_turn(&mut state);

        let dealt = hp_before - state.creatures[enemy_idx].current_hp;
        assert_eq!(
            dealt, 10,
            "Mayhem's auto-played Strike consumes PrepTime's turn-start Vigor(4): 6 + 4 = 10",
        );
        assert_eq!(
            state.player_power_amount(crate::PowerKind::Vigor),
            0,
            "Vigor must be consumed by Mayhem's auto-played attack, not left dangling (seed 3515)",
        );
        assert_eq!(
            state.player_power_amount(crate::PowerKind::PrepTime),
            4,
            "PrepTime is a Counter Buff with no decrement — it persists across turn start",
        );
    }

    /// Bead `sts2-rs-r2ixj`: `InfernoPower.AfterPlayerTurnStart`
    /// (`InfernoPower.cs`) is a C# `AfterPlayerTurnStart` power, NOT
    /// `AfterSideTurnStart`. Rust historically folded its self-damage into the
    /// `after_side_turn_start` walk; the `adsld` reorder moved that walk after
    /// the auto-play cluster, so the folded power fired too late (and, when a
    /// Mayhem auto-play applied a fresh Inferno mid-cluster, fired for an
    /// instance C#'s `AfterPlayerTurnStart` snapshot excludes). The fix
    /// dispatches it in the `AfterPlayerTurnStart` cluster over the pre-auto-play
    /// snapshot. This test pins the cluster dispatch: a pre-existing Inferno
    /// self-damages the player (`counter` HP, Unblockable) at turn start, which
    /// retaliates `amount` Unpowered to every enemy (`InfernoPower.cs:49`).
    #[test]
    fn turn_start_inferno_self_damage_and_retaliation_fire_in_cluster() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerTurnStart;
        state.round = 2;
        state.player.current_hp = 50;
        state.player.max_hp = 50;

        // A single enemy at 40 HP, no block — the Inferno retaliation target.
        let enemy_id = NonZeroU32::new(11).expect("non-zero");
        state.creatures.push(Creature {
            id: enemy_id,
            current_hp: 40,
            max_hp: 40,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::TurretOperator),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("TurretOperator"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(enemy_id));

        // Player mirror + a pre-existing Inferno(amount=9) with self_damage
        // (`counter`) = 2.
        let player_uid = state.init_player_creature();
        crate::card_effects::apply_power_to_creature(
            &mut state,
            player_uid,
            crate::PowerKind::Inferno,
            9,
        );
        let inferno_pid = state
            .player_power_uids()
            .into_iter()
            .find(|u| {
                state
                    .powers
                    .get_by_id(u.0)
                    .is_some_and(|p| p.kind == crate::PowerKind::Inferno)
            })
            .expect("Inferno applied");
        state
            .powers
            .get_mut_by_id(inferno_pid.0)
            .expect("Inferno row")
            .counter = 2;

        start_player_turn(&mut state);

        assert_eq!(
            state.player.current_hp, 48,
            "Inferno self_damage(2) is Unblockable self-damage at turn start: 50 -> 48",
        );
        let enemy = state
            .creatures
            .iter()
            .find(|c| c.id == enemy_id)
            .expect("enemy present");
        assert_eq!(
            enemy.current_hp, 31,
            "the self-damage retaliates Inferno's amount(9) to every enemy: 40 -> 31",
        );
    }

    /// Bead `sts2-rs-jxz`: Regent `TyrannyPower` at turn start. C#
    /// `TyrannyPower.ModifyHandDraw` draws `Amount` MORE cards (5 -> 6) and
    /// `TyrannyPower.AfterPlayerTurnStart` opens a `FromHand(prefs(Amount,
    /// Amount))` exhaust choice. With a hand larger than `Amount` the choice
    /// opens (`ExhaustFromHand`, min == max == Amount) and suspends the cluster;
    /// resolving it exhausts the picked card and resumes to `PlayerPlayPhase`.
    #[test]
    fn turn_start_tyranny_draws_extra_then_opens_exhaust_choice() {
        let mut state = state_for_turn_start(2, 10);
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        // The exhaust resolver indexes `state.cards`, so back each draw-pile
        // uid (1..=10) with a blank card row.
        for i in 1..=10 {
            state.cards.push(crate::cards::Card {
                uid: CardUid::new(i).expect("non-zero"),
                card_id: crate::cards::CardId::StrikeRegent,
                cost: 1,
                card_type: crate::cards::CardType::Attack,
                target_type: crate::cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                combat_vars: crate::cards::CombatVars::new(),
                enchantments: crate::cards::CardEnchantments::new(),
                tags: crate::cards::CardTags::NONE,
                energy_spent: 0,
                base_replay_count: 0,
                keyword_override: None,
                affliction: None,
            });
        }
        let player_uid = state.init_player_creature();
        crate::card_effects::apply_power_to_creature(
            &mut state,
            player_uid,
            crate::PowerKind::Tyranny,
            1,
        );

        let result = start_player_turn(&mut state);

        // ModifyHandDraw folded +1 onto BASE_DRAW=5 → 6 cards drawn.
        assert_eq!(
            state.player.hand.len(),
            6,
            "Tyranny ModifyHandDraw draws Amount(1) extra: BASE_DRAW 5 -> 6",
        );
        // AfterPlayerTurnStart opened the exhaust choice (hand 6 > Amount 1) and
        // suspended the cluster.
        assert_eq!(result, StepResult::PendingChoice);
        let pc = state.pending_choice.as_ref().expect("exhaust choice open");
        assert_eq!(pc.kind, crate::ChoiceKind::ExhaustFromHand);
        assert_eq!(pc.min_select, 1, "Tyranny exhausts EXACTLY Amount(1)");
        assert_eq!(pc.max_select, 1);
        assert_eq!(pc.options.len(), 6, "every hand card is an exhaust option");
        assert!(state.pending_cluster_resume.is_some(), "cluster suspended");

        // Resolve: pick option 0 → 1 card exhausted, cluster resumes to play.
        let exhaust_target = pc.options[0].card_uid.expect("option carries a uid");
        let res = crate::apply(
            &mut state,
            crate::Action::ResolveChoice {
                selection: smallvec::smallvec![0_u8],
            },
        );
        assert_eq!(res, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
        assert_eq!(state.player.hand.len(), 5, "one card exhausted: 6 -> 5");
        assert_eq!(state.player.exhaust.len(), 1);
        assert_eq!(state.player.exhaust[0], exhaust_target);
        assert!(state.pending_choice.is_none());
        assert!(state.pending_cluster_resume.is_none());
    }

    /// Silent `ToolsOfTheTradePower` at turn start — the DISCARD twin of the
    /// Tyranny exhaust above, and the same suspend. C#
    /// `ToolsOfTheTradePower.ModifyHandDraw` draws `Amount` MORE (5 -> 6) and
    /// `AfterPlayerTurnStart` **awaits** `CardSelectCmd.FromHandForDiscard`
    /// (`ToolsOfTheTradePower.cs:27-37`), so the rest of the cluster waits
    /// behind the pick.
    ///
    /// The assertion that matters is `result == PendingChoice`. This arm used
    /// to park the choice and fall through, which ran the cluster tail first
    /// AND reported `Advanced` with `pending_choice` set — a transition the
    /// engine had not made. The live advisor branches on that result and sent
    /// every one of these screens to the rarity heuristic
    /// (`logs/f7-mirror-check15`, 9 decisions, "the turn ended but parked no
    /// choice (advanced)").
    #[test]
    fn turn_start_tools_of_the_trade_draws_extra_then_suspends_on_discard_choice() {
        let mut state = state_for_turn_start(2, 10);
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        // The discard resolver indexes `state.cards`, so back each draw-pile
        // uid (1..=10) with a blank card row.
        for i in 1..=10 {
            state.cards.push(crate::cards::Card {
                uid: CardUid::new(i).expect("non-zero"),
                card_id: crate::cards::CardId::StrikeSilent,
                cost: 1,
                card_type: crate::cards::CardType::Attack,
                target_type: crate::cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                combat_vars: crate::cards::CombatVars::new(),
                enchantments: crate::cards::CardEnchantments::new(),
                tags: crate::cards::CardTags::NONE,
                energy_spent: 0,
                base_replay_count: 0,
                keyword_override: None,
                affliction: None,
            });
        }
        let player_uid = state.init_player_creature();
        crate::card_effects::apply_power_to_creature(
            &mut state,
            player_uid,
            crate::PowerKind::ToolsOfTheTrade,
            1,
        );

        let result = start_player_turn(&mut state);

        assert_eq!(
            state.player.hand.len(),
            6,
            "ToolsOfTheTrade ModifyHandDraw draws Amount(1) extra: BASE_DRAW 5 -> 6",
        );
        assert_eq!(
            result,
            StepResult::PendingChoice,
            "the awaited discard select suspends the cluster, so end_turn must \
             REPORT the park -- not `Advanced` with a choice sitting open",
        );
        let pc = state.pending_choice.as_ref().expect("discard choice open");
        assert_eq!(pc.kind, crate::ChoiceKind::DiscardFromHand);
        assert_eq!(
            pc.min_select, 1,
            "ToolsOfTheTrade discards EXACTLY Amount(1)"
        );
        assert_eq!(pc.max_select, 1);
        assert_eq!(pc.options.len(), 6, "every hand card is a discard option");
        assert!(state.pending_cluster_resume.is_some(), "cluster suspended");

        // Resolve: pick option 0 → 1 card discarded, cluster resumes to play.
        let discard_target = pc.options[0].card_uid.expect("option carries a uid");
        let res = crate::apply(
            &mut state,
            crate::Action::ResolveChoice {
                selection: smallvec::smallvec![0_u8],
            },
        );
        assert_eq!(res, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
        assert_eq!(state.player.hand.len(), 5, "one card discarded: 6 -> 5");
        assert!(
            state.player.discard.contains(&discard_target),
            "the picked card reached the discard pile",
        );
        assert!(state.pending_choice.is_none());
        assert!(state.pending_cluster_resume.is_none());
    }

    /// Bead `sts2-rs-axdgh` (TURRET_OPERATOR_WEAK): the RELIC analogue of the
    /// RollingBoulder ordering above. MercuryHourglass's 3 Unpowered damage
    /// fires on C# `AfterPlayerTurnStart` (the player-turn-start cluster),
    /// BEFORE the side-hook Rampart grant — so it lands on the TurretOperator's
    /// BARE HP (41 -> 38) and Rampart's 25 block is granted afterward. The
    /// pre-fix wiring put MercuryHourglass on `after_side_turn_start`, draining
    /// AFTER Rampart (Step 9), so the 3 was eaten by the shield
    /// (`hp = 41 / block = 22`) — the exact init-state turret divergence on
    /// parity seeds 2133/4462/5321.
    #[test]
    #[serial_test::serial(global_registry)]
    fn turn_start_mercury_hourglass_precedes_rampart_block() {
        crate::relic_effects::install_default_registry();
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerTurnStart;
        state.round = 2;
        state.player.current_hp = 50;
        state.player.max_hp = 50;

        // LivingShield (uid 10) carrying Rampart(25).
        let rampart_pid = NonZeroU32::new(1).expect("non-zero");
        state.powers.push(crate::Power {
            id: rampart_pid,
            kind: crate::PowerKind::Rampart,
            amount: 25,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let shield_id = NonZeroU32::new(10).expect("non-zero");
        state.creatures.push(Creature {
            id: shield_id,
            current_hp: 55,
            max_hp: 55,
            block: 0,
            powers: SmallVec::from_buf_and_len([crate::PowerUid(rampart_pid); 8], 1),
            species: Some(MonsterKind::LivingShield),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("LivingShield"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(shield_id));

        // TurretOperator (uid 11) at 41 HP, 0 block.
        let turret_id = NonZeroU32::new(11).expect("non-zero");
        state.creatures.push(Creature {
            id: turret_id,
            current_hp: 41,
            max_hp: 41,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::TurretOperator),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("TurretOperator"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(turret_id));

        // Player mirror + MercuryHourglass relic.
        let _ = state.init_player_creature();
        let relic_id = NonZeroU32::new(100).expect("non-zero");
        state.relics.push(crate::Relic {
            id: relic_id,
            kind: crate::RelicKind::MercuryHourglass,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        start_player_turn(&mut state);

        let turret = state
            .creatures
            .iter()
            .find(|c| c.id == turret_id)
            .expect("turret present");
        assert_eq!(
            turret.current_hp, 38,
            "MercuryHourglass (3) hits the turret at bare HP BEFORE Rampart blocks: 41 -> 38",
        );
        assert_eq!(
            turret.block, 25,
            "Rampart grants its 25 block AFTER the hourglass damage lands",
        );
    }

    /// Bead `sts2-rs-axdgh`: FestivePopper (round-1, 9 Unpowered damage) is the
    /// 9-damage sibling of the MercuryHourglass case — C# fires it on
    /// `AfterPlayerTurnStart`, so its damage must precede the side-hook Rampart
    /// grant and land on the TurretOperator's bare HP (41 -> 32), with Rampart
    /// arming 25 afterward. Round 1 (the relic's only active round). Parity
    /// seed 4462.
    #[test]
    #[serial_test::serial(global_registry)]
    fn turn_start_festive_popper_precedes_rampart_block() {
        crate::relic_effects::install_default_registry();
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerTurnStart;
        state.round = 1;
        state.player.current_hp = 50;
        state.player.max_hp = 50;

        let rampart_pid = NonZeroU32::new(1).expect("non-zero");
        state.powers.push(crate::Power {
            id: rampart_pid,
            kind: crate::PowerKind::Rampart,
            amount: 25,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let shield_id = NonZeroU32::new(10).expect("non-zero");
        state.creatures.push(Creature {
            id: shield_id,
            current_hp: 55,
            max_hp: 55,
            block: 0,
            powers: SmallVec::from_buf_and_len([crate::PowerUid(rampart_pid); 8], 1),
            species: Some(MonsterKind::LivingShield),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("LivingShield"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(shield_id));

        let turret_id = NonZeroU32::new(11).expect("non-zero");
        state.creatures.push(Creature {
            id: turret_id,
            current_hp: 41,
            max_hp: 41,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::TurretOperator),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("TurretOperator"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(turret_id));

        let _ = state.init_player_creature();
        state.relics.push(crate::Relic {
            id: NonZeroU32::new(100).expect("non-zero"),
            kind: crate::RelicKind::FestivePopper,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        start_player_turn(&mut state);

        let turret = state
            .creatures
            .iter()
            .find(|c| c.id == turret_id)
            .expect("turret present");
        assert_eq!(
            turret.current_hp, 32,
            "FestivePopper (9) hits the turret at bare HP BEFORE Rampart: 41 -> 32",
        );
        assert_eq!(
            turret.block, 25,
            "Rampart grants its 25 block AFTER the popper damage lands",
        );
    }

    /// Round 1: block on entry stays untouched (Python's `if
    /// self.round_number > 1` guard at combat.py:698 short-circuits
    /// the clear). Defensive — fresh combat state shouldn't carry
    /// non-zero block, but the guard is observable here.
    #[test]
    fn turn_start_does_not_clear_block_round_one() {
        let mut state = state_for_turn_start(1, 0);
        state.player.block = 10;

        start_player_turn(&mut state);

        assert_eq!(
            state.player.block, 10,
            "round 1 must leave block unchanged per combat.py:698 guard",
        );
    }

    /// `should_reset_energy` defaults to true (no relic listener
    /// returns false at this pin), so `state.player.energy` after
    /// `start_player_turn` must equal the modified max energy.
    /// With no listeners, `max_delta == 0` and the result is
    /// `BASE_MAX_ENERGY = 3`.
    #[test]
    fn turn_start_resets_energy_to_max() {
        let mut state = state_for_turn_start(1, 0);
        state.player.energy = 0;
        state.player.max_energy = 0; // canonical fresh state -> uses BASE_MAX_ENERGY

        start_player_turn(&mut state);

        let expected = i8::try_from(BASE_MAX_ENERGY).expect("BASE_MAX_ENERGY fits in i8");
        assert_eq!(
            state.player.energy, expected,
            "should_reset_energy=true path sets energy to BASE_MAX_ENERGY",
        );
        assert_eq!(state.player.max_energy, expected);
    }

    /// A persistent `ModifyMaxEnergy` power (PyrePower) adds a CONSTANT
    /// `+Amount` to the per-turn energy reset — the effective max is
    /// recomputed fresh from `BASE_MAX_ENERGY` every turn, NOT compounded
    /// by reading the previously-stored effective `max_energy` back as the
    /// base. Regression for sts2-rs-gpsvo: in the coverage sweep Pyre's
    /// energy grew +1/turn (cs4/rs5 @t6) because the base was read from the
    /// stored effective value instead of the immutable `BASE_MAX_ENERGY`.
    /// The earlier `q6bix_pyre_power_adds_max_energy` test only exercised a
    /// single `dispatch_modify_max_energy` call, so the multi-turn compound
    /// was invisible.
    #[test]
    fn turn_start_modify_max_energy_does_not_compound_across_turns() {
        let mut state = state_for_turn_start(2, 10);
        // Attach PyrePower(1) to the player (legacy power list — the
        // attachment style proven to fold +1 via `dispatch_modify_max_energy`
        // in `q6bix_pyre_power_adds_max_energy`). The compounding bug under
        // test lives in the reset arithmetic, independent of attachment.
        let pid = std::num::NonZeroU32::new(1).expect("nz");
        state.powers.push(crate::Power {
            id: pid,
            kind: crate::PowerKind::Pyre,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        // Turn A: BASE_MAX_ENERGY(3) + PyrePower(1) = 4.
        start_player_turn(&mut state);
        assert_eq!(
            state.player.energy, 4,
            "turn A: BASE_MAX_ENERGY(3) + PyrePower(1) = 4",
        );

        // Re-arm the next turn-start WITHOUT replaying Pyre, and refill the
        // draw pile so the turn-start draw doesn't underflow.
        state.phase = CombatPhase::PlayerTurnStart;
        state.round += 1;
        for i in 100u32..115 {
            state.player.draw.push(CardUid::new(i).expect("non-zero"));
        }

        // Turn B: STILL 3 + 1 = 4 — the +1 must not accumulate to +2 (=5).
        start_player_turn(&mut state);
        assert_eq!(
            state.player.energy, 4,
            "turn B: recompute 3 + 1 = 4, NOT compound to 5",
        );
    }

    /// Even with non-zero starting energy, the reset path overwrites
    /// (does NOT accumulate) when `should_reset_energy=true`. Locks
    /// the Python `combat.py:703-704` branch.
    #[test]
    fn turn_start_overwrites_existing_energy_when_resetting() {
        let mut state = state_for_turn_start(1, 0);
        state.player.energy = 99;

        start_player_turn(&mut state);

        let expected = i8::try_from(BASE_MAX_ENERGY).expect("BASE_MAX_ENERGY fits in i8");
        assert_eq!(state.player.energy, expected);
    }

    /// `BASE_DRAW = 5` cards drawn into the hand from a non-empty
    /// draw pile. Mirrors `_draw_cards_for_creature(owner, 5, ...)`
    /// at combat.py:712. With no `modify_hand_draw` listeners the
    /// chain is identity (`delta = 0 -> draw_count = BASE_DRAW`).
    #[test]
    fn turn_start_draws_base_draw_cards() {
        let mut state = state_for_turn_start(1, 10);

        start_player_turn(&mut state);

        let expected_draw = usize::try_from(BASE_DRAW).expect("BASE_DRAW fits in usize");
        assert_eq!(
            state.player.hand.len(),
            expected_draw,
            "BASE_DRAW=5 cards moved from draw pile into hand",
        );
        assert_eq!(state.player.draw.len(), 10 - expected_draw);
        // Pop order: Python pops front (combat.py:1124), so hand[0]
        // should be the first-pushed UID.
        assert_eq!(state.player.hand[0], CardUid::new(1).expect("non-zero"));
    }

    /// Phase advances `PlayerTurnStart` -> `PlayerPlayPhase`.
    /// Mirrors the Python no-explicit-phase-flag invariant via the
    /// post-condition that `_continue_player_turn_setup` returns to
    /// the play loop.
    #[test]
    fn turn_start_advances_phase_to_play() {
        let mut state = state_for_turn_start(1, 0);
        assert_eq!(state.phase, CombatPhase::PlayerTurnStart);

        let result = start_player_turn(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
    }

    /// gh #6 / `sts2-rs-jjwam` (vvi6g follow-up): the player-turn-start
    /// path must run the FULL `check_combat_end` (minion-collapse), not a
    /// bare `is_over` read. A turn-start auto-play (Mayhem) can kill the
    /// last PRIMARY enemy while a SECONDARY minion stays alive — e.g.
    /// FOGMOG's `IllusionPower` EyeWithTeeth, revived to full on the prior
    /// enemy turn. C# fires `Kill(teammates)` on the primary's death; Rust
    /// must resolve the same victory at Step 10 of the turn-start cluster.
    /// Here the primary is already a corpse (hp 0, still in `enemy_uids`)
    /// and the minion is alive: starting the player turn must collapse the
    /// minion and report `CombatWon`. Previously the cheap `is_over` read
    /// let combat continue with the live minion (FOGMOG seeds 2351/5302).
    #[test]
    #[serial_test::serial(global_registry)]
    fn turn_start_collapses_minion_when_only_primary_corpse_remains() {
        let mut state = state_for_turn_start(2, 0);
        state.player.current_hp = 30;
        state.player.max_hp = 50;
        let mut mk_enemy = |hp: i32| -> CreatureUid {
            let id = NonZeroU32::new(u32::try_from(state.creatures.len() + 1).expect("fits"))
                .expect("non-zero");
            state.creatures.push(Creature {
                id,
                current_hp: hp,
                max_hp: 50,
                block: 0,
                powers: SmallVec::new(),
                species: Some(MonsterKind::ShrinkerBeetle),
                intent: None,
                intent_history: IntentHistory::default(),
                monster_id: crate::MonsterId("ShrinkerBeetle"),
                ai: None,
            });
            state.enemy_uids.push(CreatureUid(id));
            CreatureUid(id)
        };
        // Primary already dead (corpse retained in `enemy_uids`).
        let _primary = mk_enemy(0);
        // Secondary minion still alive.
        let minion_uid = mk_enemy(6);
        crate::card_effects::apply_power_to_creature(&mut state, minion_uid, PowerKind::Minion, 1);

        let result = start_player_turn(&mut state);

        assert_eq!(
            result,
            StepResult::CombatWon,
            "last primary dead + lone alive minion at turn start must resolve as victory"
        );
        assert!(state.is_over && state.player_won, "victory flags set");
        let minion_hp = state
            .creatures
            .iter()
            .find(|c| c.id == minion_uid.0)
            .map_or(99, |c| c.current_hp);
        assert!(
            minion_hp <= 0,
            "the surviving minion must be collapsed when the turn-start check resolves victory"
        );
    }

    /// `is_over=true` short-circuits to `CombatWon` / `CombatLost`
    /// without firing any hooks or mutating state. Same defensive
    /// guard as `start_combat` (combat.py's `_check_combat_end`
    /// short-circuit path) and per the bead spec's done-when.
    #[test]
    fn turn_start_short_circuits_when_combat_over_won() {
        let mut state = state_for_turn_start(2, 5);
        state.is_over = true;
        state.player_won = true;
        state.player.block = 99;
        state.player.energy = 42;

        let result = start_player_turn(&mut state);

        assert_eq!(result, StepResult::CombatWon);
        // No state mutation: block / energy / hand / phase unchanged.
        assert_eq!(state.player.block, 99);
        assert_eq!(state.player.energy, 42);
        assert_eq!(state.player.hand.len(), 0);
        assert_eq!(state.player.draw.len(), 5);
        assert_eq!(state.phase, CombatPhase::PlayerTurnStart);
    }

    #[test]
    fn turn_start_short_circuits_when_combat_over_lost() {
        let mut state = state_for_turn_start(2, 5);
        state.is_over = true;
        state.player_won = false;

        let result = start_player_turn(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert_eq!(state.phase, CombatPhase::PlayerTurnStart);
    }

    /// `after_side_turn_start(PLAYER)` walks every listener.
    /// At this pin no `PowerKind` overrides the listener arm so the
    /// dispatcher is observably a no-op walk — but the call must not
    /// panic, must not skip the dispatch, and must complete before
    /// the phase advance. Locked structurally by the
    /// determinism check (same seed, same input -> same final state)
    /// + the phase-advance assertion above.
    ///
    /// **Phase 6+ test extension**: once a power overrides
    /// `after_side_turn_start` (e.g., Combust deals damage at
    /// turn-start), this test extends to assert the side effect
    /// is observed exactly once.
    #[test]
    fn turn_start_fires_after_side_turn_start_player() {
        let mut state = state_for_turn_start(1, 0);

        let result = start_player_turn(&mut state);

        // Structural smoke: the call returns Advanced, phase is
        // PlayerPlayPhase. The dispatcher walk doesn't panic with
        // empty listener tables (no powers, no relics at this pin).
        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
    }

    /// On round 1, `_prepare_opening_draw_for_owner` sorts innate
    /// cards to the front of the draw pile. At this pin no card
    /// has the `is_innate` flag (Phase 6 territory), so the sort
    /// is a no-op and this test asserts the no-op behavior:
    /// draw pile order is preserved, `draw_count` is the unmodified
    /// `BASE_DRAW`.
    ///
    /// **Phase 6+ test extension**: once `cards::Card::is_innate`
    /// lands, this test extends to push a mix of innate / non-innate
    /// cards and assert the innate cards land in hand in front-of-pile
    /// order even when there are more than `BASE_DRAW` non-innate cards.
    #[test]
    fn turn_start_innate_cards_sort_to_top_round_one() {
        let mut state = state_for_turn_start(1, 7);

        start_player_turn(&mut state);

        // No innate flag yet -> draw pile order preserved, BASE_DRAW
        // cards drawn from the front.
        let expected_draw = usize::try_from(BASE_DRAW).expect("BASE_DRAW fits in usize");
        assert_eq!(state.player.hand.len(), expected_draw);
        // Hand contains UIDs 1..=BASE_DRAW in pop order.
        for (i, uid) in state.player.hand.iter().enumerate() {
            let expected = CardUid::new(u32::try_from(i + 1).expect("idx fits")).expect("non-zero");
            assert_eq!(*uid, expected, "hand[{i}] should be UID {}", i + 1);
        }
        // Remaining draw pile preserves trailing UIDs in order.
        let remaining: Vec<u32> = state.player.draw.iter().map(|u| u.get()).collect();
        assert_eq!(remaining, vec![6, 7]);
    }

    /// Bead `sts2-rs-2un`: at round 1, an Innate card buried in the draw
    /// pile is moved to the top and drawn into the opening hand. Mirrors
    /// C# `CombatManager` round-1 opening draw (Innate cards
    /// `MoveToTopInternal`, `handDraw = max(handDraw, innateCount)`).
    /// q6bix sweep: MIND_BLAST / DRAMATIC_ENTRANCE want hand_ids[0].
    #[test]
    fn turn_start_innate_card_sorts_to_front_round_one() {
        let mut state = state_for_turn_start(1, 0);
        // Seven cards in the draw pile; the LAST (uid 7) is Innate and sits
        // beyond BASE_DRAW reach. Without the sort it would never be drawn.
        for i in 1..=7u32 {
            let uid = CardUid::new(i).expect("non-zero");
            state.player.draw.push(uid);
            state.cards.push(crate::cards::Card {
                uid,
                card_id: crate::cards::CardId::StrikeIronclad,
                cost: 1,
                card_type: crate::cards::CardType::Attack,
                target_type: crate::cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                combat_vars: crate::cards::CombatVars::new(),
                enchantments: crate::cards::CardEnchantments::new(),
                tags: crate::cards::CardTags::NONE,
                energy_spent: 0,
                base_replay_count: 0,
                keyword_override: (i == 7).then_some(crate::cards::KeywordOverride::INNATE),
                affliction: None,
            });
        }

        start_player_turn(&mut state);

        // The Innate card (uid 7) was pulled to the top and drawn first.
        assert_eq!(
            state.player.hand.first().map(|u| u.get()),
            Some(7),
            "Innate card sorts to the front of the opening hand"
        );
        assert!(
            state.player.hand.iter().any(|u| u.get() == 7),
            "Innate card is in the opening hand"
        );
    }

    /// Bead `sts2-rs-09x5n`: the round-1 Innate move-to-top reorder must
    /// `cards::pin_top` each card it moves — the player has definite
    /// knowledge of the resulting top-of-draw stack, same class as
    /// Headbutt/ThinkingAhead. Under `deferred_draw_order` (BeliefDraw)
    /// this must make the Innate card's draw deterministic (no chance
    /// node / no RNG read) even though it sits amid a heterogeneous
    /// multi-card pile, while the REST of the opening draw (a plain
    /// >=2-card unknown middle) still consumes RNG per card. Without the
    /// pin, `pop_next_card` would treat the Innate draw as just another
    /// random pick from the unknown middle — spuriously opening a chance
    /// node for information the player already has.
    #[test]
    fn turn_start_innate_move_to_top_pins_and_skips_chance_node() {
        let mut state = state_for_turn_start(1, 0);
        state.player.deferred_draw_order = true;
        // 6 non-innate cards (uid 1..=6) + 1 Innate card buried at the
        // bottom (uid 7).
        for i in 1..=7u32 {
            let uid = CardUid::new(i).expect("non-zero");
            state.player.draw.push(uid);
            state.cards.push(crate::cards::Card {
                uid,
                card_id: crate::cards::CardId::StrikeIronclad,
                cost: 1,
                card_type: crate::cards::CardType::Attack,
                target_type: crate::cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                combat_vars: crate::cards::CombatVars::new(),
                enchantments: crate::cards::CardEnchantments::new(),
                tags: crate::cards::CardTags::NONE,
                energy_spent: 0,
                base_replay_count: 0,
                keyword_override: (i == 7).then_some(crate::cards::KeywordOverride::INNATE),
                affliction: None,
            });
        }

        let rng_before = state.run_rng.shuffle.counter();
        start_player_turn(&mut state);

        // Pop order honors the pin: the Innate card is drawn first.
        assert_eq!(
            state.player.hand.first().map(|u| u.get()),
            Some(7),
            "pinned Innate card is drawn first"
        );
        let draw_count = usize::try_from(BASE_DRAW).expect("BASE_DRAW fits in usize");
        assert_eq!(state.player.hand.len(), draw_count);
        // 1 deterministic pop (the pinned Innate card) + (BASE_DRAW - 1)
        // draws from the remaining >=2-card unknown middle, each of which
        // consumes exactly one RNG read (`pop_next_card_multi_card_segment_consumes_rng`).
        let expected_rng_reads = u32::try_from(draw_count - 1).expect("fits");
        assert_eq!(
            state.run_rng.shuffle.counter(),
            rng_before + expected_rng_reads,
            "the pinned Innate draw must consume NO RNG (no chance node); \
             the remaining draws must still consume RNG (chance node kept)"
        );
        // Pin lifecycle: drawing the pinned card clears the pin (bead
        // `sts2-rs-o6xw`'s unpin-on-draw invariant via `draw_signaling`).
        assert!(
            state.player.pinned_top.is_empty(),
            "the Innate pin is consumed once its card is drawn"
        );
    }

    // ── Stale-pin bookkeeping in physical-top consumers (sts2-rs-09x5n) ────
    //
    // `auto_play_card_from_draw_top` / `auto_play_n_from_draw_top` /
    // `auto_play_i_am_invincible_from_draw_top` / `auto_play_collect_suspending`
    // all pop the physical draw-top via a raw `draw.remove(0)`, bypassing
    // `cards::pop_next_card` (and its paired `cards::unpin` call). If the
    // popped card happened to be pinned (Headbutt/ThinkingAhead/Innate/
    // Nostalgia/ShiningStrike all place KNOWN cards at draw-top), the pin
    // must be cleared here too — otherwise `pinned_top` stays non-empty
    // after the card has already left the draw pile, and the NEXT
    // `pop_next_card` wrongly takes the "player-known" deterministic branch
    // for what is now an ordinary unknown middle segment.

    /// Build a `deferred_draw_order` state with `top_id` PINNED to
    /// physical draw-top and a 2-card heterogeneous middle behind it —
    /// the exact shape the physical-top consumers pop from.
    fn state_with_pinned_top_card(top_id: crate::cards::CardId) -> CombatState {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.player.deferred_draw_order = true;
        let mk_card = |uid_n: u32, id: crate::cards::CardId| crate::cards::Card {
            uid: CardUid::new(uid_n).expect("non-zero"),
            card_id: id,
            cost: 1,
            card_type: crate::cards::CardType::Skill,
            target_type: crate::cards::TargetType::Self_,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        };
        state.cards.push(mk_card(1, top_id));
        state
            .cards
            .push(mk_card(2, crate::cards::CardId::StrikeIronclad));
        state.cards.push(mk_card(3, crate::cards::CardId::Anger));
        for i in 1..=3u32 {
            state.player.draw.push(CardUid::new(i).expect("non-zero"));
        }
        cards::pin_top(&mut state.player, CardUid::new(1).expect("non-zero"));
        state
    }

    /// After a physical-top consumer pops the pinned card: (1) the pin must
    /// not survive it (no stale `pinned_top` entry), and (2) a subsequent
    /// `pop_next_card` over the remaining >=2-card heterogeneous middle must
    /// still consume RNG — proving no stale pin suppressed that chance node.
    fn assert_pin_cleared_and_next_draw_is_chance(state: &mut CombatState) {
        assert!(
            state.player.pinned_top.is_empty(),
            "the consumed card's pin must not survive it leaving the draw pile"
        );
        let before = state.run_rng.shuffle.counter();
        let _ = cards::pop_next_card(&mut state.player, &mut state.run_rng.shuffle);
        assert_eq!(
            state.run_rng.shuffle.counter(),
            before + 1,
            "a stale pin must not suppress the chance node for the next \
             (still-heterogeneous, still >=2-card) draw"
        );
    }

    /// GH #363 A2 — an AUTO-PLAY must not charge VoidForm. C#
    /// `VoidFormPower.AfterCardPlayed` (VoidFormPower.cs:66-73) increments its
    /// private `cardsPlayedThisTurn` only on
    /// `!cardPlay.IsAutoPlay && cardPlay.IsLastInSeries`, so Mayhem's
    /// turn-start auto-play and Bombardment's self-auto-play from exhaust
    /// leave every free card intact. The engine had been reading
    /// `count_player_cards_played_this_turn` — its analogue of C#
    /// `History.CardPlaysFinished`, which auto-plays DO bump — so the mirror
    /// burned charges the world still had (8 records, all mirror-low, on the
    /// 2026-08-03 live Regent A1 corpus).
    #[test]
    fn an_auto_play_does_not_charge_void_form() {
        let mut state = state_with_pinned_top_card(crate::cards::CardId::DefendIronclad);
        let power_id = NonZeroU32::new(77).expect("non-zero");
        state.powers.push(crate::Power::new(
            power_id,
            crate::powers::PowerKind::VoidForm,
            2,
        ));
        state.player.powers.push(crate::PowerUid(power_id));
        assert_eq!(
            void_form_cards_played_this_turn(&state),
            0,
            "fresh VoidForm has spent no charges"
        );

        let played = auto_play_card_from_draw_top(&mut state, false);
        assert!(played, "the fixture auto-played a card");

        // The all-types bucket DID move — that is C# History.CardPlaysFinished,
        // and it is exactly the quantity VoidForm must not read.
        assert_eq!(
            state.count_player_cards_played_this_turn(None),
            1,
            "the auto-play lands in the History.CardPlaysFinished analogue"
        );
        assert_eq!(
            void_form_cards_played_this_turn(&state),
            0,
            "but it must NOT consume a VoidForm free card"
        );
    }

    /// GH #363 A2 — the owner's turn start zeroes VoidForm's tally, per C#
    /// `VoidFormPower.BeforeSideTurnStart` (VoidFormPower.cs:75-83).
    #[test]
    fn void_form_tally_resets_at_the_owners_turn_start() {
        let mut state = state_for_turn_start(2, 0);
        let power_id = NonZeroU32::new(77).expect("non-zero");
        state.powers.push(crate::Power::new(
            power_id,
            crate::powers::PowerKind::VoidForm,
            2,
        ));
        state.player.powers.push(crate::PowerUid(power_id));
        bump_void_form_play_tally(&mut state);
        bump_void_form_play_tally(&mut state);
        assert_eq!(
            void_form_cards_played_this_turn(&state),
            2,
            "two plays charged"
        );

        start_player_turn(&mut state);

        assert_eq!(
            void_form_cards_played_this_turn(&state),
            0,
            "the owner's turn start restores every free card"
        );
    }

    #[test]
    fn auto_play_card_from_draw_top_unpins_stale_top_pin() {
        let mut state = state_with_pinned_top_card(crate::cards::CardId::DefendIronclad);
        let played = auto_play_card_from_draw_top(&mut state, false);
        assert!(played, "a card was auto-played (Havoc-style consume)");
        assert_pin_cleared_and_next_draw_is_chance(&mut state);
    }

    #[test]
    fn auto_play_n_from_draw_top_unpins_stale_top_pin() {
        let mut state = state_with_pinned_top_card(crate::cards::CardId::DefendIronclad);
        auto_play_n_from_draw_top(&mut state, 1, false);
        assert_pin_cleared_and_next_draw_is_chance(&mut state);
    }

    #[test]
    fn auto_play_i_am_invincible_from_draw_top_unpins_stale_top_pin() {
        let mut state = state_with_pinned_top_card(crate::cards::CardId::IAmInvincible);
        auto_play_i_am_invincible_from_draw_top(&mut state);
        assert!(
            !state
                .player
                .draw
                .contains(&CardUid::new(1).expect("non-zero")),
            "the IAmInvincible copy was auto-played off draw-top"
        );
        assert_pin_cleared_and_next_draw_is_chance(&mut state);
    }

    #[test]
    fn auto_play_collect_suspending_unpins_stale_top_pin() {
        let mut state = state_with_pinned_top_card(crate::cards::CardId::DefendIronclad);
        let ap = crate::PendingAutoplay {
            remaining: 1,
            collected: SmallVec::new(),
            force_exhaust: false,
        };
        let suspended = auto_play_collect_suspending(&mut state, ap);
        assert!(
            suspended.is_none(),
            "no Stratagem grid-select opened; the collect should complete"
        );
        assert_pin_cleared_and_next_draw_is_chance(&mut state);
    }

    /// Combat-log placeholder is reset on entry. At this pin
    /// `TurnEvents` is `{ _placeholder: () }` so the structural
    /// reset is a no-op against the default value, but the
    /// assignment locks the Python clear at combat.py:674-678
    /// against future Phase-6 list expansion.
    #[test]
    fn turn_start_clears_combat_log() {
        let mut state = state_for_turn_start(2, 0);
        // Pre-populate (placeholder POD; once Phase 6 lands the
        // real list types, this test extends to push real events).
        state.combat_log = crate::TurnEvents::default();

        start_player_turn(&mut state);

        assert_eq!(state.combat_log, crate::TurnEvents::default());
    }

    /// Bead `sts2-rs-l2nz`: a `pending_choice` live on entry to
    /// `start_player_turn` (or one opened by a
    /// `before_side_turn_start` listener) defers the rest of
    /// turn-setup. The function returns
    /// [`StepResult::PendingChoice`] and sets
    /// `pending_turn_setup_resume = true`; block / energy / draw
    /// are NOT applied. Resolution via `apply(ResolveChoice ...)`
    /// runs the deferred body.
    #[test]
    fn turn_start_defers_on_pending_choice_then_resumes() {
        use crate::{apply, Action};
        use smallvec::SmallVec;

        let mut state = state_for_turn_start(1, 0);
        let block_before = state.player.block;
        let energy_before = state.player.energy;
        let hand_before = state.player.hand.len();
        state.pending_choice = Some(crate::PendingChoice {
            kind: crate::ChoiceKind::PutBack,
            options: Vec::new(),
            min_select: 0,
            max_select: 0,
            can_skip: true,
            post_effect: None,
            picked: SmallVec::new(),
        });

        let result = start_player_turn(&mut state);
        assert_eq!(result, StepResult::PendingChoice);
        assert!(state.pending_turn_setup_resume);
        assert!(state.pending_choice.is_some());
        // Setup body (steps 5-11) deferred.
        assert_eq!(state.player.block, block_before);
        assert_eq!(state.player.energy, energy_before);
        assert_eq!(state.player.hand.len(), hand_before);

        // Resolving the choice (skip with empty selection) must run
        // the deferred body and clear the resume flag.
        let _ = apply(
            &mut state,
            Action::ResolveChoice {
                selection: SmallVec::new(),
            },
        );
        assert!(state.pending_choice.is_none());
        assert!(!state.pending_turn_setup_resume);
        assert_eq!(state.phase, crate::CombatPhase::PlayerPlayPhase);
    }

    /// Determinism: same seed + same input state -> same final state.
    /// Locks the Python's deterministic-given-seed guarantee at the
    /// turn-start boundary (no shuffle is invoked when draw pile is
    /// non-empty, so RNG isn't actually consumed at this pin; the
    /// test still asserts byte-equality across two runs).
    #[test]
    fn turn_start_is_deterministic_given_seed() {
        let mut a = state_for_turn_start(2, 6);
        a.player.block = 5;
        let mut b = a.clone();

        start_player_turn(&mut a);
        start_player_turn(&mut b);

        assert_eq!(a.player.energy, b.player.energy);
        assert_eq!(a.player.max_energy, b.player.max_energy);
        assert_eq!(a.player.block, b.player.block);
        assert_eq!(a.player.hand, b.player.hand);
        assert_eq!(a.player.draw, b.player.draw);
        assert_eq!(a.phase, b.phase);
    }

    // ── P3-3 (sts2-rs-2ch) end_player_turn_phase_one + phase_two ────────

    /// Build a `CombatState` ready to invoke `end_player_turn_phase_one`.
    /// Phase is set to `PlayerPlayPhase`, RNG seeded for determinism.
    /// `hand_n` cards are pushed into the hand pile with sequential UIDs
    /// (`1..=hand_n`) so flush-order assertions can observe Python's
    /// pile order.
    fn state_for_end_turn_phase_one(hand_n: u32) -> CombatState {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerPlayPhase;
        state.round = 1;
        // A live combat: phase_one's step-3 now runs the FULL
        // `check_combat_end` (C# `CheckWinCondition`), so the default
        // hp-0 / no-enemy state would short-circuit as CombatLost/Won
        // before the phase advance under test.
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let enemy_id = NonZeroU32::new(900).expect("nz");
        state.creatures.push(Creature {
            id: enemy_id,
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
        state.enemy_uids.push(CreatureUid(enemy_id));
        for i in 1..=hand_n {
            state.player.hand.push(CardUid::new(i).expect("non-zero"));
        }
        state
    }

    /// Build a `CombatState` already in `PlayerEndTurnPhaseOne`, ready
    /// for `end_player_turn_phase_two`.
    fn state_for_end_turn_phase_two(hand_n: u32) -> CombatState {
        let mut state = state_for_end_turn_phase_one(hand_n);
        state.phase = CombatPhase::PlayerEndTurnPhaseOne;
        state
    }

    /// `end_player_turn_phase_one` advances `PlayerPlayPhase` ->
    /// `PlayerEndTurnPhaseOne` and returns `Advanced`. Mirrors the
    /// `before_turn_end` head of Python's `end_player_turn`
    /// (combat.py:937-948).
    ///
    /// **Phase 6+ test extension**: once a `PowerKind` overrides
    /// `before_turn_end` (e.g. a future buff that fires at end-turn),
    /// this test extends to assert the side effect is observed
    /// exactly once.
    #[test]
    fn end_turn_phase_one_advances_phase() {
        let mut state = state_for_end_turn_phase_one(0);

        let result = end_player_turn_phase_one(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::PlayerEndTurnPhaseOne);
    }

    /// `end_player_turn_phase_one` does not mutate hand / discard
    /// piles — that's `phase_two`'s territory. Locks the partition.
    #[test]
    fn end_turn_phase_one_leaves_piles_untouched() {
        let mut state = state_for_end_turn_phase_one(5);

        end_player_turn_phase_one(&mut state);

        assert_eq!(state.player.hand.len(), 5);
        assert_eq!(state.player.discard.len(), 0);
        assert_eq!(state.player.exhaust.len(), 0);
    }

    /// `is_over=true` short-circuits `phase_one` to `CombatWon` /
    /// `CombatLost` without firing `before_turn_end` or advancing
    /// phase. Mirrors Python's `if self.is_over: return` at
    /// combat.py:941-942.
    #[test]
    fn end_turn_phase_one_short_circuits_when_combat_over_won() {
        let mut state = state_for_end_turn_phase_one(3);
        state.is_over = true;
        state.player_won = true;

        let result = end_player_turn_phase_one(&mut state);

        assert_eq!(result, StepResult::CombatWon);
        // Phase unchanged.
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
        // Hand untouched.
        assert_eq!(state.player.hand.len(), 3);
    }

    #[test]
    fn end_turn_phase_one_short_circuits_when_combat_over_lost() {
        let mut state = state_for_end_turn_phase_one(3);
        state.is_over = true;
        state.player_won = false;

        let result = end_player_turn_phase_one(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
    }

    /// Parity regression (bug `card_reward_gold`, seed A9P139 Defect A9):
    /// when an END-OF-TURN orb passive (Lightning zap) downs the LAST enemy
    /// on the same turn-end, every `OnTurnEndInHand` effect — here DEBT's
    /// 10-gold loss — must STILL resolve, and combat must still end Won.
    ///
    /// C# `CombatManager.DoTurnEnd` (`CombatManager.cs:966-1005`) runs
    /// `OrbQueue.BeforeTurnEnd` (orb passive, 968), ethereal exhaust
    /// (984-987), then `card.OnTurnEndInHand` (995) ATOMICALLY, with the
    /// single `CheckWinCondition` (963) only AFTER the whole DoTurnEnd batch
    /// and the `BeforeFlush` walk (959). DEBT (`Debt.cs:26-30`) therefore
    /// always loses gold even when the orb passive kills the last enemy on
    /// that turn-end.
    ///
    /// The prior Rust split ran a premature combat-end re-check in
    /// `phase_one` (the removed Step 3.6) right after `orbs::passive_tick`,
    /// flipping `is_over` and short-circuiting `phase_two` BEFORE the
    /// turn-end-in-hand loop — dropping the 10-gold loss (Rust ran +10).
    /// With the re-check gone, the win is detected later at the
    /// `flush_tail` check (mirroring C#), after DEBT resolves.
    #[test]
    fn debt_gold_loss_resolves_when_orb_passive_kills_last_enemy() {
        use crate::cards::Card;

        // Phase-one state: one enemy at 3 HP (a Lightning passive deals
        // `3 + focus` = 3 with no Focus, exactly lethal).
        let mut state = state_for_end_turn_phase_one(1);
        let enemy_uid = state.enemy_uids[0];
        {
            let enemy = state
                .creatures
                .iter_mut()
                .find(|c| c.id == enemy_uid.0)
                .expect("enemy present");
            enemy.current_hp = 3;
            enemy.max_hp = 3;
        }

        // A channeled Lightning orb (Cracked-Core analogue).
        state.player.orbs = crate::orbs::OrbSlots::with_default_capacity();
        state.player.orbs.channel(crate::orbs::Orb::Lightning);

        // DEBT in hand (uid 1, with a matching `state.cards` entry so the
        // turn-end-in-hand filter resolves the card_id).
        let debt_uid = crate::CardUid::new(1).expect("non-zero");
        state.cards.push(Card {
            uid: debt_uid,
            card_id: crate::cards::CardId::Debt,
            cost: 0,
            card_type: crate::cards::CardType::Curse,
            target_type: crate::cards::TargetType::None,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });

        // Pre-turn-end gold (cf. A9P139: 89 before the loss).
        state.gold = 89;

        // Phase one: the orb passive zaps the last enemy dead, but with the
        // Step-3.6 re-check removed it must NOT flip `is_over` here.
        let r1 = end_player_turn_phase_one(&mut state);
        assert_eq!(
            r1,
            StepResult::Advanced,
            "phase_one must advance, not end combat at the orb passive"
        );
        assert!(
            !state.is_over,
            "combat must not be marked over inside phase_one"
        );
        assert_eq!(state.phase, CombatPhase::PlayerEndTurnPhaseOne);
        // The enemy is dead (HP clamped to 0) from the Lightning zap.
        let enemy_hp = state
            .creatures
            .iter()
            .find(|c| c.id == enemy_uid.0)
            .map_or(0, |c| c.current_hp);
        assert_eq!(
            enemy_hp, 0,
            "Lightning passive should have killed the enemy"
        );

        // Phase two: DEBT resolves (gold -= min(10, gold)) BEFORE the
        // flush_tail win check, then combat ends Won.
        let r2 = end_player_turn_phase_two(&mut state);
        assert_eq!(
            r2,
            StepResult::CombatWon,
            "combat must end Won after the turn-end resolves"
        );
        assert_eq!(
            state.gold, 79,
            "DEBT must lose 10 gold even on the orb-kill turn (89 -> 79)"
        );
    }

    /// `end_player_turn_phase_two` flushes every hand card to discard
    /// when `dispatch_should_flush` returns true (the Phase-2 default
    /// — relic-only chain, no listener overrides at this pin).
    /// Mirrors combat.py:1039-1043.
    ///
    /// **Phase 6+ test extension**: once Runic Pyramid (or another
    /// Relic that overrides `should_flush`) lands, this test pairs
    /// with a `keeps_hand_when_should_flush_returns_false` test.
    #[test]
    fn end_turn_phase_two_flushes_hand_to_discard() {
        let mut state = state_for_end_turn_phase_two(4);

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.player.hand.len(), 0, "hand flushed");
        assert_eq!(state.player.discard.len(), 4, "all 4 cards in discard");
        // Discard order preserves the original hand order: combat.py
        // appends in iteration order, which is the SmallVec's natural
        // forward iteration.
        let discard_uids: Vec<u32> = state.player.discard.iter().map(|u| u.get()).collect();
        assert_eq!(discard_uids, vec![1, 2, 3, 4]);
        // Exhaust pile remains empty — no ethereal cards in the
        // Phase-3 starter deck.
        assert_eq!(state.player.exhaust.len(), 0);
    }

    /// q6bix F-G: while the player has an active `RetainHandPower`
    /// (amount > 0), C# `RetainHandPower.ShouldFlush(owner)` returns
    /// `false` — the WHOLE hand is retained at end of turn (not a count
    /// budget) — and `AfterTurnEnd` decrements the power by 1. The prior
    /// Rust port drained RetainHand into the `pending_retain_count`
    /// budget (retaining only `amount` cards) and never decremented it.
    #[test]
    fn f_g_retain_hand_retains_whole_hand_and_decrements() {
        use std::num::NonZeroU32;

        let mut state = state_for_end_turn_phase_two(3);
        // RetainHand(2) on the player (legacy surface — the union helper
        // `player_power_amount` picks it up regardless of the mirror).
        let pid = NonZeroU32::new(1).expect("nz");
        state.powers.push(crate::Power {
            id: pid,
            kind: crate::PowerKind::RetainHand,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(
            state.player.hand.len(),
            3,
            "whole hand retained while RetainHand is active"
        );
        assert_eq!(state.player.discard.len(), 0, "nothing flushed");
        let amt = state
            .powers
            .iter()
            .find(|p| p.kind == crate::PowerKind::RetainHand)
            .map(|p| p.amount);
        assert_eq!(amt, Some(1), "RetainHand decremented 2 -> 1 at turn end");
    }

    /// `ConsumingShadowPower.AfterTurnEnd` (`ConsumingShadowPower.cs`): at the
    /// player's turn end, with a non-empty orb queue, evoke the LAST orb once
    /// per power stack. Here a single Dark orb (fresh, `stored = 0`, evoke
    /// value `6`) is evoked by a ConsumingShadow(1) stack, dealing 6 to the
    /// lowest-HP enemy and consuming the orb. Regression for the prior no-op
    /// (the Python-pin "lose 1 HP / deal X to random" mis-port).
    #[test]
    fn consuming_shadow_power_evokes_last_orb_at_turn_end() {
        use std::num::NonZeroU32;
        let mut state = state_for_end_turn_phase_two(2);
        state.player.character = Some(crate::potions::CharacterId::Defect);
        // One fresh Dark orb in a 3-slot belt (evoke value = stored(0) + 6).
        state.player.orbs = crate::orbs::OrbSlots::with_capacity(3);
        state.player.orbs.channel(crate::orbs::Orb::dark());
        // ConsumingShadow(1) on the player.
        let pid = NonZeroU32::new(7).expect("nz");
        state.powers.push(crate::Power {
            id: pid,
            kind: crate::PowerKind::ConsumingShadow,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let enemy = state.enemy_uids[0];
        let hp_of = |st: &CombatState, u: CreatureUid| -> i32 {
            st.creatures
                .iter()
                .find(|c| c.id == u.0)
                .map(|c| c.current_hp)
                .expect("enemy")
        };
        let hp_before = hp_of(&state, enemy);

        let result = end_player_turn_phase_two(&mut state);
        assert_eq!(result, StepResult::Advanced);

        let hp_after = hp_of(&state, enemy);
        assert_eq!(
            hp_before - hp_after,
            6,
            "ConsumingShadow(1) should evoke the Dark orb for 6 at turn end",
        );
        assert_eq!(
            state
                .player
                .orbs
                .slots
                .iter()
                .filter(|o| !o.is_empty())
                .count(),
            0,
            "the evoked orb is consumed",
        );
        // The power persists (Counter buff, no self-removal/decrement).
        assert_eq!(
            state.player_power_amount(crate::PowerKind::ConsumingShadow),
            1,
            "ConsumingShadow persists across turn end",
        );
    }

    /// Bead `sts2-rs-q6bix.6` (Lucky Tonic): BufferPower fully absorbs an
    /// HP-loss event aimed at its owner (the player) and consumes exactly
    /// one stack per absorbed event. Mirrors C#
    /// `BufferPower.ModifyHpLostAfterOstyLate` (returns 0) +
    /// `AfterModifyingHpLostAfterOsty` (decrement).
    fn state_with_player_buffer(amount: i32) -> CombatState {
        use std::num::NonZeroU32;
        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        let pid = NonZeroU32::new(1).expect("nz");
        state.powers.push(crate::Power {
            id: pid,
            kind: crate::PowerKind::Buffer,
            amount,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));
        state
    }

    #[test]
    fn buffer_absorbs_hp_loss_and_is_consumed() {
        let mut state = state_with_player_buffer(1);

        let _ = apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(CreatureUid::new(2).unwrap()),
            10,
            ValueProps::MOVE,
            None,
        );

        assert_eq!(
            state.player.current_hp, 50,
            "Buffer absorbed all 10 HP loss"
        );
        assert_eq!(
            state.player_power_amount(crate::PowerKind::Buffer),
            0,
            "Buffer 1 -> 0 (consumed + removed) after one absorbed hit",
        );
    }

    /// Regent `ReflectPower`: a powered enemy attack absorbed by the player's
    /// block reflects the BLOCKED amount back at the dealer (Unpowered), per
    /// C# `ReflectPower.AfterDamageReceived`. The power itself persists until
    /// its owner-side turn-start decrement (exercised by the pure-helper test
    /// `powers::regent::reflect_block_and_tick` + the side-turn dispatch arm).
    #[test]
    fn reflect_power_reflects_blocked_powered_damage_to_dealer() {
        use std::num::NonZeroU32;
        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        state.player.block = 10;
        let pid = NonZeroU32::new(1).expect("nz");
        state.powers.push(crate::Power {
            id: pid,
            kind: crate::PowerKind::Reflect,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));
        let eid = NonZeroU32::new(2).expect("nz");
        state.creatures.push(crate::Creature {
            id: eid,
            current_hp: 30,
            max_hp: 30,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(crate::MonsterKind::Nibbit),
            intent: None,
            intent_history: crate::IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(crate::CreatureUid(eid));

        // Powered attack of 6 fully absorbed by 10 block → blocked = 6.
        let _ = apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(CreatureUid::new(2).unwrap()),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(state.player.current_hp, 50, "all 6 absorbed by block");
        assert_eq!(
            state.creatures[0].current_hp, 24,
            "Reflect deals the blocked 6 back to the dealer"
        );

        // An UNPOWERED hit does NOT reflect (gate: props.is_powered()).
        state.player.block = 10;
        let _ = apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(CreatureUid::new(2).unwrap()),
            4,
            ValueProps::UNPOWERED,
            None,
        );
        assert_eq!(
            state.creatures[0].current_hp, 24,
            "unpowered blocked hit does not reflect"
        );
    }

    #[test]
    fn buffer_two_stacks_decrements_one_per_hit() {
        let mut state = state_with_player_buffer(2);
        let dealer = CreatureRef::Enemy(CreatureUid::new(2).unwrap());

        let _ = apply_damage_to_player(&mut state, dealer, 10, ValueProps::MOVE, None);

        assert_eq!(state.player.current_hp, 50, "first hit fully absorbed");
        assert_eq!(
            state.player_power_amount(crate::PowerKind::Buffer),
            1,
            "Buffer 2 -> 1 after one absorbed hit",
        );
    }

    #[test]
    fn buffer_not_consumed_when_block_absorbs_fully() {
        let mut state = state_with_player_buffer(1);
        state.player.block = 20;

        let _ = apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(CreatureUid::new(2).unwrap()),
            10,
            ValueProps::MOVE,
            None,
        );

        // Block ate the whole hit (unblocked == 0) → the modify stage never
        // runs → Buffer is NOT consumed (matches C#: Buffer guards HP loss,
        // not block absorption).
        assert_eq!(state.player.current_hp, 50);
        assert_eq!(state.player.block, 10, "block absorbed 10 of 20");
        assert_eq!(
            state.player_power_amount(crate::PowerKind::Buffer),
            1,
            "Buffer stack preserved when block fully absorbs the hit",
        );
    }

    /// Bead `sts2-rs-o8ixl` (q6bix RESTLESSNESS): a hand card carrying the
    /// `Retain` keyword stays IN hand at end of turn while its plain
    /// neighbours flush to discard. Mirrors C#
    /// `EndPlayerTurnPhaseTwoInternal` partitioning the hand by
    /// `ShouldRetainThisTurn` (true for `Keywords.Contains(Retain)`).
    /// The keyword is resolved per-instance via `Card::keyword_override`
    /// so this exercises the upgrade-aware path without touching the
    /// process-global registry.
    #[test]
    fn end_turn_phase_two_keeps_retain_keyword_card_in_hand() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerEndTurnPhaseOne;
        state.round = 1;
        // Three cards in hand: uid 2 carries the RETAIN keyword override,
        // uids 1 and 3 are plain and should flush.
        for i in 1..=3u32 {
            let uid = CardUid::new(i).expect("non-zero");
            let keyword_override = (i == 2).then_some(crate::cards::KeywordOverride::RETAIN);
            state.cards.push(crate::cards::Card {
                uid,
                card_id: crate::cards::CardId::StrikeIronclad,
                cost: 1,
                card_type: crate::cards::CardType::Attack,
                target_type: crate::cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                combat_vars: crate::cards::CombatVars::new(),
                enchantments: crate::cards::CardEnchantments::new(),
                tags: crate::cards::CardTags::NONE,
                energy_spent: 0,
                base_replay_count: 0,
                keyword_override,
                affliction: None,
            });
            state.player.hand.push(uid);
        }

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::Advanced);
        let hand_uids: Vec<u32> = state.player.hand.iter().map(|u| u.get()).collect();
        assert_eq!(hand_uids, vec![2], "only the Retain card stays in hand");
        let discard_uids: Vec<u32> = state.player.discard.iter().map(|u| u.get()).collect();
        assert_eq!(discard_uids, vec![1, 3], "the two plain cards flush");
        assert_eq!(state.player.exhaust.len(), 0, "no ethereal cards");
    }

    /// Bead `sts2-rs-l7k3d`: while the player has an active `HexPower`
    /// stack, the end-of-turn ethereal sweep treats EVERY hand card as
    /// ethereal — non-ethereal cards expel to exhaust instead of flushing
    /// to discard. Models the C# `HexPower` net observable (Hexed → all
    /// cards Ethereal). Contrast with `end_turn_phase_two_flushes_hand_to_discard`,
    /// where the same non-ethereal cards flush to discard absent Hex.
    #[test]
    fn end_turn_phase_two_hex_expels_all_hand_cards_to_exhaust() {
        use std::num::NonZeroU32;

        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerEndTurnPhaseOne;
        state.round = 1;
        // Two plain (non-ethereal) Strike cards in hand.
        for i in 1..=2u32 {
            let uid = CardUid::new(i).expect("non-zero");
            state.cards.push(crate::cards::Card {
                uid,
                card_id: crate::cards::CardId::StrikeIronclad,
                cost: 1,
                card_type: crate::cards::CardType::Attack,
                target_type: crate::cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                combat_vars: crate::cards::CombatVars::new(),
                enchantments: crate::cards::CardEnchantments::new(),
                tags: crate::cards::CardTags::NONE,
                energy_spent: 0,
                base_replay_count: 0,
                keyword_override: None,
                affliction: None,
            });
            state.player.hand.push(uid);
        }
        // Apply HexPower(2) to the player.
        let pow_uid = NonZeroU32::new(100).expect("non-zero");
        state.powers.push(crate::Power {
            id: pow_uid,
            kind: crate::PowerKind::Hex,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pow_uid));

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.player.hand.len(), 0, "hand emptied");
        assert_eq!(
            state.player.exhaust.len(),
            2,
            "Hex makes both cards ethereal → exhaust"
        );
        assert_eq!(
            state.player.discard.len(),
            0,
            "Hexed cards must NOT flush to discard"
        );
    }

    /// `end_player_turn_phase_two` keeps the hand intact when a relic
    /// drives `dispatch_should_flush` to `false`. RunicPyramid is the
    /// canonical override (`sts2-rs-70anj.1`, Python
    /// `shop_event.py::RunicPyramid::should_flush == False`). Pairs
    /// with [`end_turn_phase_two_flushes_hand_to_discard`].
    #[test]
    fn end_turn_phase_two_keeps_hand_with_runic_pyramid() {
        use std::num::NonZeroU32;

        let mut state = state_for_end_turn_phase_two(4);
        state.relics.push(crate::Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: crate::RelicKind::RunicPyramid,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.player.hand.len(), 4, "hand retained");
        assert_eq!(state.player.discard.len(), 0, "no cards flushed");
        assert_eq!(state.player.exhaust.len(), 0);
    }

    /// `end_player_turn_phase_two` keeps the hand intact on round 1
    /// only when RingingTriangle is owned (`sts2-rs-70anj.4`, Python
    /// `shop_event.py::RingingTriangle::should_flush`).
    #[test]
    fn end_turn_phase_two_keeps_hand_on_round_one_with_ringing_triangle() {
        use std::num::NonZeroU32;

        let mut state = state_for_end_turn_phase_two(4);
        state.round = 1;
        state.relics.push(crate::Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: crate::RelicKind::RingingTriangle,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.player.hand.len(), 4, "hand retained round 1");
        assert_eq!(state.player.discard.len(), 0, "no cards flushed");
    }

    /// RingingTriangle does NOT retain past round 1 — hand flushes
    /// normally on round 2 (`sts2-rs-70anj.4`).
    #[test]
    fn end_turn_phase_two_flushes_hand_on_round_two_with_ringing_triangle() {
        use std::num::NonZeroU32;

        let mut state = state_for_end_turn_phase_two(4);
        state.round = 2;
        state.relics.push(crate::Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: crate::RelicKind::RingingTriangle,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.player.hand.len(), 0, "hand flushed round 2");
        assert_eq!(state.player.discard.len(), 4, "all cards moved to discard");
    }

    /// `end_player_turn_phase_two` advances `PlayerEndTurnPhaseOne` ->
    /// `EnemyTurnStart`. The intermediate `PlayerEndTurnPhaseTwo`
    /// state is observable inside the function but not at the
    /// boundary; Python's single `end_player_turn` is split for the
    /// Rust phase-machine's structured-replay needs, but the post-
    /// condition matches the Python tail-call to `_execute_enemy_turn`.
    #[test]
    fn end_turn_phase_two_advances_phase_to_enemy_turn_start() {
        let mut state = state_for_end_turn_phase_two(0);

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::EnemyTurnStart);
    }

    /// `end_player_turn_phase_two` is a no-op for the empty-hand case
    /// — no flush, no exhaust, no discard. Defensive shape check that
    /// the dispatchers walk cleanly with empty piles.
    #[test]
    fn end_turn_phase_two_handles_empty_hand() {
        let mut state = state_for_end_turn_phase_two(0);

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.player.hand.len(), 0);
        assert_eq!(state.player.discard.len(), 0);
        assert_eq!(state.player.exhaust.len(), 0);
        assert_eq!(state.phase, CombatPhase::EnemyTurnStart);
    }

    /// Hand UIDs without matching arena entries are NOT identified as
    /// ethereal — the registry lookup short-circuits at the missing
    /// `state.cards[idx]` slot. This test locks the legacy
    /// `state_for_end_turn_phase_two` behavior (synthetic UIDs only,
    /// no arena cards): every card flushes to discard and the exhaust
    /// pile stays empty regardless of registry state.
    #[test]
    fn end_turn_phase_two_no_ethereal_when_hand_uids_lack_arena() {
        let mut state = state_for_end_turn_phase_two(3);

        end_player_turn_phase_two(&mut state);

        assert_eq!(state.player.exhaust.len(), 0);
        assert_eq!(state.player.discard.len(), 3);
    }

    // ── 6ynt.3.3: ETHEREAL end-of-turn expel ────────────────────────────

    /// Arena-aware end-turn fixture: spawns each `(card_id, ethereal?)`
    /// entry into the player's hand (real `Card` POD in `state.cards`)
    /// and seeds the engine-side ETHEREAL registry per the `ethereal?`
    /// flag. Returns the post-state plus the UID list in hand-insertion
    /// order so callers can assert pile destinations by UID.
    fn run_end_turn_with_arena_cards(
        entries: &[(crate::cards::CardId, bool)],
    ) -> (CombatState, Vec<CardUid>) {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerEndTurnPhaseOne;
        state.round = 1;
        let mut uids = Vec::with_capacity(entries.len());
        for (idx, (card_id, is_ethereal)) in entries.iter().enumerate() {
            let uid =
                CardUid::new(u32::try_from(idx + 1).expect("uid fits u32")).expect("non-zero uid");
            state.cards.push(crate::cards::Card {
                uid,
                card_id: *card_id,
                cost: 0,
                card_type: crate::cards::CardType::Status,
                target_type: crate::cards::TargetType::None,
                upgraded: false,
                base_damage: 0,
                base_block: 0,
                combat_vars: crate::cards::CombatVars::new(),
                enchantments: crate::cards::CardEnchantments::new(),
                tags: crate::cards::CardTags::NONE,
                energy_spent: 0,
                base_replay_count: 0,
                keyword_override: None,
                affliction: None,
            });
            state.player.hand.push(uid);
            if *is_ethereal {
                crate::card_effects::register_card_ethereal(*card_id);
            }
            uids.push(uid);
        }
        end_player_turn_phase_two(&mut state);
        (state, uids)
    }

    /// Macro: each canonical ETHEREAL card spawned alone in hand expels
    /// to exhaust at end-of-turn (and never lands in discard). Mirrors
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:992-999`.
    macro_rules! ethereal_card_test {
        ($name:ident, $variant:ident) => {
            #[test]
            fn $name() {
                let (state, uids) =
                    run_end_turn_with_arena_cards(&[(crate::cards::CardId::$variant, true)]);
                let uid = uids[0];
                assert_eq!(
                    state.player.exhaust.as_slice(),
                    &[uid],
                    concat!(
                        "ETHEREAL ",
                        stringify!($variant),
                        " must expel to exhaust at end-of-turn"
                    ),
                );
                assert!(
                    state.player.discard.is_empty(),
                    concat!(
                        "ETHEREAL ",
                        stringify!($variant),
                        " must NOT flush to discard"
                    ),
                );
                assert!(state.player.hand.is_empty());
            }
        };
    }

    ethereal_card_test!(end_turn_phase_two_dazed_expels_to_exhaust, Dazed);
    ethereal_card_test!(
        end_turn_phase_two_ascenders_bane_expels_to_exhaust,
        AscendersBane
    );
    ethereal_card_test!(end_turn_phase_two_clumsy_expels_to_exhaust, Clumsy);
    ethereal_card_test!(end_turn_phase_two_void_expels_to_exhaust, Void);

    /// Mixed hand: an ETHEREAL card (Dazed) expels to exhaust while a
    /// non-ETHEREAL card (Wound — UNPLAYABLE but NOT ETHEREAL) flushes
    /// to discard. Locks the per-card branching of the predicate.
    #[test]
    fn end_turn_phase_two_non_ethereal_card_flushes_to_discard() {
        let (state, uids) = run_end_turn_with_arena_cards(&[
            (crate::cards::CardId::Dazed, true),
            (crate::cards::CardId::Wound, false),
        ]);
        let dazed_uid = uids[0];
        let wound_uid = uids[1];
        assert_eq!(
            state.player.exhaust.as_slice(),
            &[dazed_uid],
            "Dazed (ETHEREAL) lands in exhaust",
        );
        assert_eq!(
            state.player.discard.as_slice(),
            &[wound_uid],
            "Wound (non-ETHEREAL) flushes to discard",
        );
        assert!(state.player.hand.is_empty());
    }

    /// `is_over=true` short-circuits `phase_two` without flushing the
    /// hand or advancing phase. Mirrors the defensive guard at the
    /// top of the function (Python combat.py:959-960 / 965-966
    /// re-entry guards).
    #[test]
    fn end_turn_phase_two_short_circuits_when_combat_over_won() {
        let mut state = state_for_end_turn_phase_two(3);
        state.is_over = true;
        state.player_won = true;

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::CombatWon);
        // No mutation: hand intact, phase unchanged.
        assert_eq!(state.player.hand.len(), 3);
        assert_eq!(state.player.discard.len(), 0);
        assert_eq!(state.phase, CombatPhase::PlayerEndTurnPhaseOne);
    }

    #[test]
    fn end_turn_phase_two_short_circuits_when_combat_over_lost() {
        let mut state = state_for_end_turn_phase_two(3);
        state.is_over = true;
        state.player_won = false;

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert_eq!(state.player.hand.len(), 3);
        assert_eq!(state.phase, CombatPhase::PlayerEndTurnPhaseOne);
    }

    /// Determinism: same seed + same input state -> same final state.
    /// Two state copies seeded identically must produce byte-equal
    /// post-condition (piles + phase) after both phases run.
    #[test]
    fn end_turn_pipeline_is_deterministic_given_seed() {
        let mut a = state_for_end_turn_phase_one(5);
        let mut b = a.clone();

        end_player_turn_phase_one(&mut a);
        end_player_turn_phase_two(&mut a);
        end_player_turn_phase_one(&mut b);
        end_player_turn_phase_two(&mut b);

        assert_eq!(a.player.hand, b.player.hand);
        assert_eq!(a.player.discard, b.player.discard);
        assert_eq!(a.player.exhaust, b.player.exhaust);
        assert_eq!(a.phase, b.phase);
    }

    // ── P3-4 (sts2-rs-x6i) enemy_turn_start ─────────────────────────────

    /// Build a `CombatState` with `n` Cultists pre-loaded into the creature
    /// arena and `enemy_uids`, phase set to `EnemyTurnStart`, side to
    /// `Player` (so the test can observe the side-flip to Enemy). RNG
    /// seeded for determinism.
    fn state_for_enemy_turn_start(n: u32) -> CombatState {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::EnemyTurnStart;
        state.side = Side::Player;
        for i in 1..=n {
            let id = NonZeroU32::new(i).expect("non-zero");
            state.creatures.push(Creature {
                id,
                current_hp: 50,
                max_hp: 50,
                block: 0,
                powers: SmallVec::new(),
                species: Some(MonsterKind::ShrinkerBeetle),
                intent: Some(Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 0,
                }),
                intent_history: IntentHistory::default(),
                monster_id: crate::MonsterId("ShrinkerBeetle"),
                ai: None,
            });
            state.enemy_uids.push(CreatureUid(id));
        }
        state
    }

    /// `enemy_turn_start` flips `state.side` from Player to Enemy.
    /// Mirrors Python's `self.current_side = CombatSide.ENEMY` at
    /// `combat.py:1061`.
    #[test]
    fn enemy_turn_start_sets_side_to_enemy() {
        let mut state = state_for_enemy_turn_start(1);
        assert_eq!(state.side, Side::Player);

        let result = enemy_turn_start(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.side, Side::Enemy);
    }

    /// Every alive enemy's block is cleared to 0 after `enemy_turn_start`.
    /// Mirrors Python's `enemy.clear_block(self)` loop at `combat.py:1066`.
    #[test]
    fn enemy_turn_start_clears_enemy_block() {
        let mut state = state_for_enemy_turn_start(2);
        state.creatures[0].block = 10;
        state.creatures[1].block = 7;

        enemy_turn_start(&mut state);

        assert_eq!(state.creatures[0].block, 0);
        assert_eq!(state.creatures[1].block, 0);
    }

    /// The `after_block_cleared` dispatcher is invoked once per ALIVE
    /// enemy. At this pin no `PowerKind` overrides the listener arm so
    /// the dispatcher is observably a no-op walk — but the call must
    /// not panic and must complete before the side-turn-start walk.
    /// Locked structurally by the phase-advance + side-flip assertions
    /// above; the dead-enemy skip is locked by the explicit
    /// `enemy_turn_start_skips_block_clear_for_dead_enemies` test.
    ///
    /// **Phase 6+ test extension**: once a `PowerKind` overrides
    /// `after_block_cleared` (e.g., Reactive Shell or a future
    /// thorns-on-clear listener), this test extends to assert the
    /// side effect is observed exactly once per alive enemy.
    #[test]
    fn enemy_turn_start_fires_after_block_cleared_per_enemy() {
        let mut state = state_for_enemy_turn_start(3);
        state.creatures[0].block = 5;
        state.creatures[1].block = 3;
        state.creatures[2].block = 0;

        let result = enemy_turn_start(&mut state);

        // Structural smoke: dispatcher walked cleanly with empty
        // listener tables; final state shows all 3 enemies' block=0.
        assert_eq!(result, StepResult::Advanced);
        for c in &state.creatures {
            assert_eq!(c.block, 0);
        }
    }

    /// `after_side_turn_start(ENEMY)` walks every listener with
    /// explicit `Side::Enemy`. At this pin no `PowerKind` overrides
    /// the listener arm so the dispatcher is observably a no-op walk
    /// — but the call must not panic and must complete before the
    /// phase advance. Locked structurally by the phase-advance
    /// assertion + the determinism test below.
    ///
    /// **Phase 6+ test extension**: once enemy-owned Poison is on a
    /// creature, this test extends to assert the Poison HP-tick is
    /// observed (via the Effect queue inside
    /// `dispatch_after_side_turn_start`).
    #[test]
    fn enemy_turn_start_fires_after_side_turn_start_enemy() {
        let mut state = state_for_enemy_turn_start(1);

        let result = enemy_turn_start(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::EnemyTurnExec);
    }

    /// If `is_over=true` and `player_won=true` on entry,
    /// `enemy_turn_start` short-circuits with `CombatWon` and does NOT
    /// flip side / clear block / fire hooks / advance phase. This is
    /// the defensive guard for re-entrant callers (the canonical caller
    /// guards before invoking us).
    #[test]
    fn enemy_turn_start_returns_combat_won_if_all_enemies_dead_at_entry() {
        let mut state = state_for_enemy_turn_start(2);
        // Simulate "all enemies dead → combat resolved" entry.
        state.creatures[0].current_hp = 0;
        state.creatures[1].current_hp = 0;
        state.is_over = true;
        state.player_won = true;
        state.creatures[0].block = 99;
        state.creatures[1].block = 99;

        let result = enemy_turn_start(&mut state);

        assert_eq!(result, StepResult::CombatWon);
        // No mutation: side / phase / block unchanged.
        assert_eq!(state.side, Side::Player);
        assert_eq!(state.phase, CombatPhase::EnemyTurnStart);
        assert_eq!(state.creatures[0].block, 99);
        assert_eq!(state.creatures[1].block, 99);
    }

    /// `is_over=true` + `player_won=false` short-circuits to
    /// `CombatLost`.
    #[test]
    fn enemy_turn_start_short_circuits_when_combat_over_lost() {
        let mut state = state_for_enemy_turn_start(1);
        state.is_over = true;
        state.player_won = false;
        state.creatures[0].block = 42;

        let result = enemy_turn_start(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert_eq!(state.side, Side::Player);
        assert_eq!(state.phase, CombatPhase::EnemyTurnStart);
        assert_eq!(state.creatures[0].block, 42);
    }

    /// Phase advances `EnemyTurnStart` -> `EnemyTurnExec`. P3-5
    /// (sts2-rs-hrz) consumes the `EnemyTurnExec` phase via the
    /// per-enemy intent-perform loop.
    #[test]
    fn enemy_turn_start_advances_phase_to_exec() {
        let mut state = state_for_enemy_turn_start(1);
        assert_eq!(state.phase, CombatPhase::EnemyTurnStart);

        let result = enemy_turn_start(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::EnemyTurnExec);
    }

    /// Dead enemies (`current_hp == 0`) are skipped by the block-clear
    /// loop. Mirrors Python's `self.alive_enemies` filter which
    /// excludes dead creatures from the per-enemy walk at
    /// `combat.py:1065-1067`. The dead enemy's block is preserved
    /// (not cleared) because `clear_block` is never called on it.
    #[test]
    fn enemy_turn_start_skips_block_clear_for_dead_enemies() {
        let mut state = state_for_enemy_turn_start(2);
        // Enemy 0: alive with block 10. Enemy 1: dead with block 99.
        state.creatures[0].current_hp = 50;
        state.creatures[0].block = 10;
        state.creatures[1].current_hp = 0;
        state.creatures[1].block = 99;

        enemy_turn_start(&mut state);

        // Alive enemy's block cleared.
        assert_eq!(state.creatures[0].block, 0);
        // Dead enemy's block left untouched (Python's alive_enemies
        // filter never invoked clear_block on this creature).
        assert_eq!(state.creatures[1].block, 99);
    }

    /// CRUELTY coverage-sweep repro shape (sts2-rs-gpsvo): a DEAD,
    /// removable corpse FIRST in `enemy_uids`, an alive enemy carrying
    /// block LAST (mirrors a SmallSlime dying turn3, then the surviving
    /// SlitheringStrangler retaining TWACK block into its next turn). The
    /// later alive enemy's block MUST still clear — the dead-first
    /// ordering must not shadow the `filter_map` index resolution.
    #[test]
    fn enemy_turn_start_clears_block_for_alive_enemy_after_dead_corpse() {
        let mut state = state_for_enemy_turn_start(2);
        // enemy_uids[0]: dead removable corpse (block 99, must stay).
        state.creatures[0].current_hp = 0;
        state.creatures[0].block = 99;
        // enemy_uids[1]: alive, carrying 5 block from a prior move.
        state.creatures[1].current_hp = 24;
        state.creatures[1].block = 5;

        // Sanity: the corpse is the c8why removable kind.
        assert!(
            state.is_removed_corpse(CreatureUid(state.creatures[0].id)),
            "enemy 0 should be a removable corpse"
        );

        enemy_turn_start(&mut state);

        assert_eq!(
            state.creatures[1].block, 0,
            "alive enemy after a dead corpse must clear its block",
        );
        assert_eq!(state.creatures[0].block, 99, "dead corpse block untouched");
    }

    /// Determinism: same seed + same input state -> same final state.
    /// At this pin no listener consumes RNG inside the turn-start
    /// pipeline, but the determinism check locks the property for
    /// Phase 6+ when enemy-owned Poison ticks via the Effect queue.
    #[test]
    fn enemy_turn_start_is_deterministic_given_seed() {
        let mut a = state_for_enemy_turn_start(2);
        a.creatures[0].block = 5;
        a.creatures[1].block = 3;
        let mut b = a.clone();

        enemy_turn_start(&mut a);
        enemy_turn_start(&mut b);

        assert_eq!(a.side, b.side);
        assert_eq!(a.phase, b.phase);
        for (ca, cb) in a.creatures.iter().zip(b.creatures.iter()) {
            assert_eq!(ca.block, cb.block);
            assert_eq!(ca.current_hp, cb.current_hp);
        }
    }

    // ── P3-5 (sts2-rs-hrz) enemy_turn_exec ──────────────────────────────

    use crate::{Power, PowerKind, PowerUid};

    /// Build a `CombatState` with `n` enemies of the given `kind`, each
    /// holding `intent`. Phase = `EnemyTurnExec`, side = `Enemy`. Player
    /// at full HP / no block / no powers. RNG seeded for determinism.
    fn state_for_enemy_turn_exec(n: u32, kind: MonsterKind, intent: Intent) -> CombatState {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::EnemyTurnExec;
        state.side = Side::Enemy;
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        state.player.block = 0;
        state.player.max_energy = 3;
        for i in 1..=n {
            let id = NonZeroU32::new(i).expect("non-zero");
            state.creatures.push(Creature {
                id,
                current_hp: 50,
                max_hp: 50,
                block: 0,
                powers: SmallVec::new(),
                species: Some(kind),
                intent: Some(intent),
                intent_history: IntentHistory::default(),
                monster_id: crate::MonsterId("ShrinkerBeetle"),
                ai: None,
            });
            state.enemy_uids.push(CreatureUid(id));
        }
        state
    }

    /// Bead `sts2-rs-lam0b`: an enemy's `Vigor(6)` adds its bonus to the
    /// FIRST powered attack (`50 - (6 + 6) = 38`) and is then REMOVED
    /// (one-shot, C# `VigorPower.AfterAttack`). A second attack from the
    /// same enemy gets NO bonus (`38 - 6 = 32`), proving the decay — the
    /// pre-fix behaviour left enemy Vigor lingering so every attack kept
    /// the +6.
    #[test]
    fn enemy_vigor_adds_to_first_attack_then_decays() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        let enemy_uid = CreatureUid(NonZeroU32::new(1).expect("nz"));
        let pid = NonZeroU32::new(7001).expect("nz");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Vigor,
            amount: 6,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures[0].powers.push(PowerUid(pid));

        // First attack: 6 base + 6 Vigor = 12 → 50 - 12 = 38.
        let r1 = perform_intent(&mut state, enemy_uid, Some(Intent::Attack { damage: 6 }));
        assert_eq!(r1, StepResult::Advanced);
        assert_eq!(
            state.player.current_hp, 38,
            "first attack gets the Vigor bonus"
        );

        // Vigor removed from every surface.
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Vigor),
            "spent Vigor must be removed from state.powers"
        );
        assert!(
            state.creatures[0].powers.is_empty(),
            "spent Vigor must be unlinked from the enemy's powers"
        );

        // Second attack: no Vigor left → 6 base only → 38 - 6 = 32.
        let r2 = perform_intent(&mut state, enemy_uid, Some(Intent::Attack { damage: 6 }));
        assert_eq!(r2, StepResult::Advanced);
        assert_eq!(
            state.player.current_hp, 32,
            "second attack has no Vigor bonus"
        );
    }

    /// Attach a `Suck(amount)` power to `creatures[0]` (FossilStalker's spawn
    /// buff — C# `FossilStalker.cs:45-49`).
    fn attach_suck(state: &mut CombatState, amount: i32) {
        let pid = NonZeroU32::new(7201).expect("nz");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Suck,
            amount,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures[0].powers.push(PowerUid(pid));
    }

    fn enemy_power_amount(state: &CombatState, kind: PowerKind) -> i32 {
        state.creatures[0]
            .powers
            .iter()
            .filter_map(|u| state.powers.iter().find(|p| p.id == u.0))
            .filter(|p| p.kind == kind)
            .map(|p| p.amount)
            .sum()
    }

    /// C# `SuckPower.AfterAttack` (`SuckPower.cs:22-47`): FossilStalker's
    /// spawn `Suck(3)` grants the owner `Strength = Amount × unblocked-hit
    /// count` ONCE per attack COMMAND, after the whole hit chain. The 2-hit
    /// LASH (3×2) must therefore deal 3 + 3 = 6 — the second hit is NOT
    /// inflated by Strength earned from the first (the naive per-hit port
    /// this mechanic was deferred over) — and only then leave Strength(6) on
    /// the stalker, which a follow-up LATCH does get.
    #[test]
    fn suck_grants_strength_once_after_the_chain_not_per_hit() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::FossilStalker,
            Intent::MultiAttack { damage: 3, hits: 2 },
        );
        let enemy_uid = CreatureUid(NonZeroU32::new(1).expect("nz"));
        attach_suck(&mut state, 3);

        let r = perform_intent(
            &mut state,
            enemy_uid,
            Some(Intent::MultiAttack { damage: 3, hits: 2 }),
        );
        assert_eq!(r, StepResult::Advanced);
        assert_eq!(
            state.player.current_hp,
            50 - 6,
            "both LASH hits land at base 3 — Suck Strength must not inflate \
             later hits of the same attack"
        );
        assert_eq!(
            enemy_power_amount(&state, PowerKind::Strength),
            6,
            "post-chain Strength = Suck(3) × 2 unblocked hits"
        );
        assert_eq!(
            enemy_power_amount(&state, PowerKind::Suck),
            3,
            "Suck is a permanent counter power — it does not decay on use"
        );

        // Follow-up single attack DOES benefit: 12 + 6 = 18.
        let r2 = perform_intent(&mut state, enemy_uid, Some(Intent::Attack { damage: 12 }));
        assert_eq!(r2, StepResult::Advanced);
        assert_eq!(
            state.player.current_hp,
            44 - 18,
            "the NEXT attack carries the Strength earned by the previous one"
        );
        assert_eq!(
            enemy_power_amount(&state, PowerKind::Strength),
            9,
            "the unblocked follow-up hit stacks another Suck(3) × 1"
        );
    }

    /// `SuckPower.cs:37-39` counts a hit only when a `DamageResult` has
    /// `UnblockedDamage > 0` — C#'s realised HP delta (`Creature.cs:382`).
    /// With 3 player block, LASH's first 3-damage hit is fully absorbed and
    /// must NOT count; the second connects. Strength = 3 × 1.
    #[test]
    fn suck_ignores_fully_blocked_hits() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::FossilStalker,
            Intent::MultiAttack { damage: 3, hits: 2 },
        );
        let enemy_uid = CreatureUid(NonZeroU32::new(1).expect("nz"));
        attach_suck(&mut state, 3);
        state.player.block = 3;

        let r = perform_intent(
            &mut state,
            enemy_uid,
            Some(Intent::MultiAttack { damage: 3, hits: 2 }),
        );
        assert_eq!(r, StepResult::Advanced);
        assert_eq!(state.player.block, 0, "first hit eats the block");
        assert_eq!(state.player.current_hp, 47, "second hit lands for 3");
        assert_eq!(
            enemy_power_amount(&state, PowerKind::Strength),
            3,
            "only the unblocked hit feeds Suck: Strength = 3 × 1"
        );
    }

    /// The mixed case the C# `UnblockedDamage > 0` test really encodes: a hit
    /// that only PARTIALLY breaks through block still counts, in full, once —
    /// `SuckPower.AfterAttack` counts HITS with any unblocked damage
    /// (`SuckPower.cs:37-40`), it does not weight them by how much got
    /// through. With 4 block against LASH 3×2 the first hit is fully absorbed
    /// (block 4→1, no HP delta, not counted) and the second bleeds 2 through
    /// the 1 remaining block (counted) — Strength = Suck(3) × 1, NOT × 2 and
    /// NOT scaled by the 2 damage that landed.
    #[test]
    fn suck_counts_a_partially_blocked_hit_once_not_by_damage() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::FossilStalker,
            Intent::MultiAttack { damage: 3, hits: 2 },
        );
        let enemy_uid = CreatureUid(NonZeroU32::new(1).expect("nz"));
        attach_suck(&mut state, 3);
        state.player.block = 4;

        let r = perform_intent(
            &mut state,
            enemy_uid,
            Some(Intent::MultiAttack { damage: 3, hits: 2 }),
        );
        assert_eq!(r, StepResult::Advanced);
        assert_eq!(state.player.block, 0, "hit 1 eats 3, hit 2 eats the last 1");
        assert_eq!(
            state.player.current_hp, 48,
            "only the 2 damage past block reaches HP"
        );
        assert_eq!(
            enemy_power_amount(&state, PowerKind::Strength),
            3,
            "one hit had UnblockedDamage > 0 → Suck(3) × 1"
        );
    }

    /// A fully-blocked single attack has zero hits with `UnblockedDamage > 0`
    /// — `SuckPower.AfterAttack`'s `num > 0` gate (`SuckPower.cs:42`) means
    /// no Strength at all.
    #[test]
    fn suck_grants_nothing_when_the_attack_is_fully_blocked() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::FossilStalker, Intent::Attack { damage: 12 });
        let enemy_uid = CreatureUid(NonZeroU32::new(1).expect("nz"));
        attach_suck(&mut state, 3);
        state.player.block = 20;

        let r = perform_intent(&mut state, enemy_uid, Some(Intent::Attack { damage: 12 }));
        assert_eq!(r, StepResult::Advanced);
        assert_eq!(state.player.current_hp, 50, "hit fully absorbed");
        assert_eq!(state.player.block, 8);
        assert_eq!(
            enemy_power_amount(&state, PowerKind::Strength),
            0,
            "a fully-blocked attack feeds Suck nothing"
        );
    }

    /// `preview_damage_to_player` reports what the intent icon shows, which is
    /// NOT the base number in the move table once the dealer is Weak. C#
    /// `AttackIntent.GetSingleDamage` bakes the same pipeline a real hit takes,
    /// so the previewed number must equal the delivered one — that equality is
    /// the point of sharing the body with `apply_damage_to_player`.
    #[test]
    fn preview_damage_matches_what_the_attack_then_delivers() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 9 });
        let enemy_uid = CreatureUid(NonZeroU32::new(1).expect("nz"));
        let pid = NonZeroU32::new(7101).expect("nz");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Weak,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures[0].powers.push(PowerUid(pid));

        // Weak is 3/4 on a powered attack: floor(9 * 3/4) = 6, not the base 9.
        let previewed = preview_damage_to_player(
            &state,
            CreatureRef::Enemy(enemy_uid),
            9,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(previewed, 6, "a Weak dealer telegraphs the baked number");

        let before = state.player.current_hp;
        perform_intent(&mut state, enemy_uid, Some(Intent::Attack { damage: 9 }));
        assert_eq!(
            before - state.player.current_hp,
            previewed,
            "the telegraph must not be able to disagree with the hit"
        );
    }

    /// The C# `Math.Max(0, (int)num)` clamp is observable: a 1-damage multi-hit
    /// from a Weak dealer bakes to `floor(1 * 3/4) = 0`, and the game really
    /// does paint a 0. Repro from the rung-3.5 corpus — Exoskeleton
    /// `Attack 1 x3` and TrackerRubyRaider `Attack 1 x8`, both Weak, both
    /// reported as `Attack 0` on the wire.
    #[test]
    fn preview_damage_clamps_a_weakened_one_damage_hit_to_zero() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 1 });
        let enemy_uid = CreatureUid(NonZeroU32::new(1).expect("nz"));
        let pid = NonZeroU32::new(7102).expect("nz");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Weak,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures[0].powers.push(PowerUid(pid));

        assert_eq!(
            preview_damage_to_player(
                &state,
                CreatureRef::Enemy(enemy_uid),
                1,
                ValueProps::MOVE,
                None,
            ),
            0,
            "floor(1 * 3/4) = 0 — the telegraph reads zero, not one"
        );
    }

    /// The DEALER's own Vigor belongs in the telegraph. C#
    /// `../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.MonsterMoves.Intents/AttackIntent.cs:84-93`
    /// bakes the icon with
    /// `Hook.ModifyDamage(…, target: me.Creature, dealer: owner, ValueProp.Move,
    /// ModifyDamageHookType.All, …)`, and `Hook.ModifyDamageInternal`
    /// (`Hook.cs:2655-2670`) folds EVERY listener on both sides — including the
    /// monster's own `VigorPower.ModifyDamageAdditive`
    /// (`Models.Powers/VigorPower.cs:57-77`), which returns `Amount` whenever
    /// `Owner == dealer` on a powered attack (`ValueProp.Move` with no
    /// `Unpowered` ⇒ `IsPoweredAttack()`).
    ///
    /// Pinned after a 2026-08 mirror-fidelity report claimed the engine omitted
    /// the dealer's Vigor from the baked telegraph (`state_dict.rs`
    /// `effective_amount`). It does not — a TerrorEel holding Vigor 6 on its
    /// 16-damage CRASH_MOVE telegraphs 22 on both sides. This test exists so a
    /// future refactor of the additive fold cannot silently make that report
    /// true.
    #[test]
    fn preview_damage_includes_the_dealers_own_vigor() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::TerrorEel, Intent::Attack { damage: 16 });
        let enemy_uid = CreatureUid(NonZeroU32::new(1).expect("nz"));
        let pid = NonZeroU32::new(7103).expect("nz");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Vigor,
            amount: 6,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures[0].powers.push(PowerUid(pid));

        // C# TerrorEel.cs:71 `SingleAttackIntent(CrashDamage = 16)` + Vigor 6.
        assert_eq!(
            preview_damage_to_player(
                &state,
                CreatureRef::Enemy(enemy_uid),
                16,
                ValueProps::MOVE,
                None,
            ),
            22,
            "the dealer's Vigor is part of the painted intent number"
        );

        // …and the telegraph must not be able to disagree with the hit.
        let before = state.player.current_hp;
        perform_intent(&mut state, enemy_uid, Some(Intent::Attack { damage: 16 }));
        assert_eq!(before - state.player.current_hp, 22);
    }

    /// `Intent::Attack { damage: 6 }` from a single Cultist hits the
    /// player at full HP with no block → `player.current_hp` = 50 - 6 = 44.
    /// Mirrors the canonical Cultist Dark-Strike attack (Python
    /// `attack_intent(damage=6)`).
    #[test]
    fn enemy_attack_intent_applies_damage_to_player() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });

        let result = enemy_turn_exec(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.player.current_hp, 44);
        assert_eq!(state.player.block, 0);
    }

    /// `Intent::MultiAttack { damage: 3, hits: 3 }` runs three discrete
    /// `apply_damage` invocations → 9 total HP loss (no block, no
    /// powers). Mirrors Python's `multi_attack_intent` semantics
    /// (`sts2_env/monsters/intents.py`).
    #[test]
    fn enemy_multi_attack_intent_applies_damage_per_hit() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::ShrinkerBeetle,
            Intent::MultiAttack { damage: 3, hits: 3 },
        );

        enemy_turn_exec(&mut state);

        assert_eq!(state.player.current_hp, 41);
        assert_eq!(state.player.block, 0);
    }

    /// Bead `sts2-rs-f99e7`: `Intent::DeathBlow { damage: 8 }` (GasBomb
    /// EXPLODE) deals damage AND kills the dealer. Mirrors C#
    /// `GasBomb.ExplodeMove`: `DamageCmd.Attack(8)` →
    /// `CreatureCmd.Kill(base.Creature)`.
    #[test]
    fn enemy_death_blow_intent_damages_player_and_kills_dealer() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::ShrinkerBeetle,
            Intent::DeathBlow { damage: 8 },
        );
        assert_eq!(state.creatures[0].current_hp, 50, "bomb starts at 50 HP");

        let result = enemy_turn_exec(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.player.current_hp, 42, "player takes the 8 damage");
        assert_eq!(
            state.creatures[0].current_hp, 0,
            "DeathBlow kills the dealer after the hit lands"
        );
    }

    /// Bead `sts2-rs-8f336`: `Intent::Escape` (FatGremlin FLEE) flees
    /// combat: no damage to the player, dealer ends at 0 HP. Mirrors
    /// C# `FatGremlin.FleeMove` → `CreatureCmd.Escape(base.Creature)`.
    /// Rust keeps corpses (see
    /// `project_dead_enemy_array_removal_divergence`).
    #[test]
    fn enemy_escape_intent_removes_dealer_without_damage() {
        let mut state = state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Escape);
        assert_eq!(state.creatures[0].current_hp, 50, "monster starts alive");

        let result = enemy_turn_exec(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(
            state.player.current_hp, 50,
            "Escape deals no damage to the player"
        );
        assert_eq!(
            state.creatures[0].current_hp, 0,
            "Escape removes the dealer (current_hp = 0)"
        );
    }

    /// Bead `sts2-rs-ck9z6` (GH #166): `BattlewornDummyTimeLimit(3)` ticks
    /// down once per enemy-side turn end and, on the third (amount == 1)
    /// tick, flags `event_combat_timed_out` and flees the dummy (hp → 0),
    /// ending the combat as a no-reward win. Mirrors C#
    /// `BattlewornDummyTimeLimitPower.AfterTurnEnd`.
    #[test]
    fn battleworn_dummy_time_limit_expires_and_flees() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::BattleFriendV1, Intent::Defend { block: 0 });
        // 300-HP dummy the player cannot kill in time; carries the 3-turn timer.
        state.creatures[0].current_hp = 300;
        state.creatures[0].max_hp = 300;
        let enemy_id = state.creatures[0].id;
        let pid = NonZeroU32::new(9100).expect("nz");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::BattlewornDummyTimeLimit,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures[0].powers.push(PowerUid(pid));

        let timer_amount = |s: &CombatState| s.powers.get_by_id(pid).map(|p| p.amount);

        // Round 1: 3 → 2, dummy alive, not yet timed out.
        state.phase = CombatPhase::EnemyTurnEnd;
        assert_eq!(enemy_turn_end(&mut state), StepResult::Advanced);
        assert_eq!(timer_amount(&state), Some(2));
        assert!(!state.event_combat_timed_out);
        assert_eq!(state.creatures[0].current_hp, 300);

        // Round 2: 2 → 1.
        state.phase = CombatPhase::EnemyTurnEnd;
        assert_eq!(enemy_turn_end(&mut state), StepResult::Advanced);
        assert_eq!(timer_amount(&state), Some(1));
        assert!(!state.event_combat_timed_out);

        // Round 3: amount == 1 → expire: flag set, dummy flees, combat won.
        state.phase = CombatPhase::EnemyTurnEnd;
        assert_eq!(enemy_turn_end(&mut state), StepResult::CombatWon);
        assert!(
            state.event_combat_timed_out,
            "expiry must flag RanOutOfTime"
        );
        assert_eq!(
            state.creatures[0].current_hp, 0,
            "expired dummy flees (hp 0)"
        );
        assert!(state.is_over && state.player_won, "combat ends as a win");
        // The flee must NOT be counted as a gold-scaling escape.
        assert_eq!(state.escaped_creatures, 0);
        let _ = enemy_id;
    }

    /// Bead `sts2-rs-qk2du`: `Intent::Defend { block: 10 }` makes the acting
    /// monster gain 10 block (previously a Phase-5 no-op stub that dropped
    /// the block). Mirrors C# `CreatureCmd.GainBlock` in monster `*Move`
    /// methods. No damage to the player.
    #[test]
    fn enemy_defend_intent_grants_monster_block() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Defend { block: 10 });
        assert_eq!(state.creatures[0].block, 0, "monster starts with no block");

        let result = enemy_turn_exec(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(
            state.creatures[0].block, 10,
            "monster gained 10 block from its Defend intent"
        );
        assert_eq!(state.player.current_hp, 50, "Defend deals no damage");
    }

    /// Player block of 10 absorbs an Attack(6) entirely → block = 4,
    /// hp unchanged. Mirrors the standard block-absorbs-damage path
    /// from existing parity fixtures (block-absorbs-damage-seed0.json).
    #[test]
    fn enemy_attack_intent_respects_player_block() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        state.player.block = 10;

        enemy_turn_exec(&mut state);

        assert_eq!(state.player.block, 4);
        assert_eq!(state.player.current_hp, 50);
    }

    /// Player carries Vulnerable(2) → Attack(4) gets multiplied by 3/2
    /// → 4 * 1.5 = 6 damage taken (single floor at end). Locks the
    /// rounding-audit invariant for the enemy → player damage path
    /// using the same Vulnerable arm exercised by VTHIN-2 in the
    /// player → enemy direction.
    #[test]
    fn enemy_attack_intent_respects_vulnerable_on_player() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 4 });
        // Attach Vulnerable(2) to the player. Direct dict-style insert
        // bypasses apply_power's debuff guard — fine for unit tests
        // (mirrors VTHIN-2 fixture pattern).
        let power_uid = PowerUid::new(1).expect("non-zero");
        state.powers.push(Power {
            id: power_uid.0,
            kind: PowerKind::Vulnerable,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(power_uid);

        enemy_turn_exec(&mut state);

        // 4 raw → modify_damage applies Vulnerable's 3/2 → floor(6.0) = 6.
        assert_eq!(state.player.current_hp, 44);
    }

    /// DieForYou: a living Osty intercepts a POWERED hit aimed at the player —
    /// the unblocked damage lands on the Osty's HP, the player is unharmed.
    #[test]
    fn die_for_you_redirects_powered_hit_to_living_osty() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        let osty = state.summon_osty(10).expect("osty");
        let osty_idx = state.creatures.iter().position(|c| c.id == osty.0).unwrap();

        let r = apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(CreatureUid(NonZeroU32::new(1).unwrap())),
            6,
            ValueProps::MOVE,
            None,
        );
        assert!(r.is_none());
        assert_eq!(state.player.current_hp, 50, "player tanked by Osty");
        assert_eq!(state.creatures[osty_idx].current_hp, 4, "Osty took the 6");
    }

    /// DieForYou overkill: damage beyond the Osty's HP flows back to the player;
    /// the Osty drops to 0 (corpse retained — not an enemy, not reaped).
    #[test]
    fn die_for_you_overkill_flows_back_to_player() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        let osty = state.summon_osty(10).expect("osty");
        let osty_idx = state.creatures.iter().position(|c| c.id == osty.0).unwrap();

        apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(CreatureUid(NonZeroU32::new(1).unwrap())),
            15,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(state.creatures[osty_idx].current_hp, 0, "Osty killed");
        assert_eq!(state.player.current_hp, 45, "5 overkill hit the player");
        assert_eq!(
            state.player.pet_uid,
            Some(osty),
            "corpse retained for revive"
        );
    }

    /// DieForYou: player block absorbs FIRST, then the unblocked remainder is
    /// redirected to the Osty (Osty's own block is irrelevant).
    #[test]
    fn die_for_you_redirects_only_post_block_remainder() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        state.player.block = 4;
        let osty = state.summon_osty(10).expect("osty");
        let osty_idx = state.creatures.iter().position(|c| c.id == osty.0).unwrap();

        apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(CreatureUid(NonZeroU32::new(1).unwrap())),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(state.player.block, 0, "player block consumed first");
        assert_eq!(state.player.current_hp, 50, "no HP lost by the player");
        assert_eq!(
            state.creatures[osty_idx].current_hp, 8,
            "Osty took the 2 unblocked"
        );
    }

    /// DieForYou does NOT redirect non-powered damage (Poison/Constrict-style
    /// self-damage, block-grant riders) — the player takes it directly.
    #[test]
    fn die_for_you_does_not_redirect_unpowered() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        let osty = state.summon_osty(10).expect("osty");
        let osty_idx = state.creatures.iter().position(|c| c.id == osty.0).unwrap();

        apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(CreatureUid(NonZeroU32::new(1).unwrap())),
            6,
            ValueProps::UNPOWERED,
            None,
        );
        assert_eq!(state.creatures[osty_idx].current_hp, 10, "Osty untouched");
        assert_eq!(state.player.current_hp, 44, "player took the unpowered hit");
    }

    /// NecroMastery + DieForYou: a redirected attack that kills the Osty AND
    /// overkills the player THROUGH TO DEATH reflects NOTHING. C# defers every
    /// `AfterCurrentHpChanged` to a post-batch results loop
    /// (`CreatureCmd.cs:253-263`) that runs AFTER the player's overkill
    /// `LoseHpInternal` (`CreatureCmd.cs:163-169`), and the reflect — dealt by
    /// the player — no-ops when its dealer `IsDead` (`CreatureCmd.cs:122`). So a
    /// player who dies to the overkill deals no reflect (Necrobinder fuzz seed
    /// 208: OwlMagistrate VERDICT 33 vs an 8-HP Osty at player 3 HP).
    #[test]
    fn necro_mastery_no_reflect_when_overkill_kills_the_player() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::ShrinkerBeetle,
            Intent::Attack { damage: 33 },
        );
        state.player.current_hp = 3;
        let pid = NonZeroU32::new(9001).expect("nz");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::NecroMastery,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(PowerUid(pid));
        let osty = state.summon_osty(8).expect("osty");
        let osty_idx = state.creatures.iter().position(|c| c.id == osty.0).unwrap();

        apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(CreatureUid(NonZeroU32::new(1).unwrap())),
            33,
            ValueProps::MOVE,
            None,
        );

        assert_eq!(state.creatures[osty_idx].current_hp, 0, "Osty killed");
        assert_eq!(state.player.current_hp, 0, "overkill killed the player");
        assert_eq!(
            state.creatures[0].current_hp, 50,
            "no reflect: a dead player-dealer deals nothing"
        );
    }

    /// NecroMastery + DieForYou: when the Osty dies but the player SURVIVES the
    /// overkill, the reflect DOES fire (player-dealer alive) — `osty_loss *
    /// Amount` Unblockable damage to every hittable enemy. Mirror of the
    /// death-suppression case above so the gate can't silently swallow the
    /// common survive-the-hit path.
    #[test]
    fn necro_mastery_reflects_when_player_survives_the_overkill() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::ShrinkerBeetle,
            Intent::Attack { damage: 12 },
        );
        // Player at 50 HP, Osty 8 → Osty dies, 4 overkill, player 50→46 (alive).
        let pid = NonZeroU32::new(9001).expect("nz");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::NecroMastery,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(PowerUid(pid));
        let osty = state.summon_osty(8).expect("osty");
        let osty_idx = state.creatures.iter().position(|c| c.id == osty.0).unwrap();

        apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(CreatureUid(NonZeroU32::new(1).unwrap())),
            12,
            ValueProps::MOVE,
            None,
        );

        assert_eq!(state.creatures[osty_idx].current_hp, 0, "Osty killed");
        assert_eq!(state.player.current_hp, 46, "4 overkill, player survives");
        assert_eq!(
            state.creatures[0].current_hp, 34,
            "reflect fired: osty_loss 8 * NecroMastery 2 = 16"
        );
    }

    /// DieForYou with a dead Osty does not redirect — the player takes the hit
    /// (a re-summon would be needed to restore the tank).
    #[test]
    fn die_for_you_dead_osty_does_not_redirect() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        state.summon_osty(10).expect("osty");
        state.kill_osty();

        apply_damage_to_player(
            &mut state,
            CreatureRef::Enemy(CreatureUid(NonZeroU32::new(1).unwrap())),
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(state.player.current_hp, 44, "dead Osty cannot tank");
    }

    /// After all enemies have acted, each surviving enemy has a fresh
    /// intent rolled with the persisted `intent_history`. After
    /// performing Attack(6), the Cultist's history records Attack +
    /// `turn_index=1`, so the next roll branches into Attack(6) again
    /// (the Cultist's `turn_index >= 1 -> Dark Strike` arm). The
    /// intent is `Some` both before AND after exec.
    ///
    /// Pre-o7r this test asserted `Some(Intent::Buff { kind: PowerKind::Strength, amount: 0 })` — that was the
    /// pre-fix bug where every turn re-rolled with a default history
    /// and Cultist collapsed to a permanent Incantation loop.
    #[test]
    fn enemy_turn_exec_rolls_next_intent_after_acting() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });

        enemy_turn_exec(&mut state);
        // Bead `sts2-rs-c8why`: the next-intent roll moved to `enemy_turn_end`
        // (C# rolls after AfterTurnEnd self-damage/deaths resolve).
        enemy_turn_end(&mut state);

        // Post-turn-end the next-intent roll fires; with no attached AI
        // the roll lands on the `MonsterKind::roll_intent` placeholder
        // for ShrinkerBeetle (Attack 5).
        assert_eq!(
            state.creatures[0].intent,
            Some(Intent::Attack { damage: 5 })
        );
        // The history slot was advanced by the perform step.
        assert_eq!(state.creatures[0].intent_history.turn_index, 1);
    }

    /// Bead `sts2-rs-iod`: Step 4 must advance the per-enemy
    /// `MonsterAi` for DSL-driven enemies. Build a minimal 2-state
    /// cycle (A <-> B, both `Intent::Defend { block: 0 }` so player HP doesn't
    /// move and combat-end short-circuits don't fire), walk three
    /// turns, assert `move_history()` grows by one per turn AND the
    /// per-turn intent alternates with the AI's `current_move`.
    /// A creature added MID-COMBAT whose AI opens on a RandomBranch must
    /// resolve that branch off the run `monster_ai` stream, exactly once, the
    /// first time `roll_next_enemy_intents` reaches it.
    ///
    /// C# `CreatureCmd.Add` runs the new machine from its initial state, so a
    /// branch initial costs one `Rng.MonsterAi.NextFloat` like any other
    /// telegraph. Rust's factories pre-resolve that branch against a THROWAWAY
    /// rng (a factory has no run stream), and the normal roll path
    /// early-returns while the AI has not performed a move — so before the
    /// `initial_resolved` flag the summon kept the throwaway answer and the
    /// run cursor stayed one read behind C# for the REST of the combat. Every
    /// later intent roll, for every monster, then diverged. Found on
    /// TwoTailedRat's CALL_FOR_BACKUP (GH #544).
    ///
    /// The test asserts the READ, not the move: a test that only checked the
    /// telegraph would pass on the throwaway-rng answer whenever the two
    /// streams happened to agree.
    #[test]
    fn mid_combat_summon_resolves_its_branch_initial_on_the_run_stream() {
        use crate::monster_ai::{
            ConditionFn, ConditionalBranch, ConditionalBranchState, MonsterAi, MonsterState,
            MoveRepeatType, MoveState, RandomBranchState, WeightedBranch,
        };
        use smallvec::smallvec as sv;
        use std::collections::BTreeMap;

        fn noop(_: &mut CombatState, _: CreatureUid) {}
        fn always(_: &CombatState) -> bool {
            true
        }

        fn branch_ai() -> MonsterAi {
            let mut states = BTreeMap::new();
            for id in ["A", "B"] {
                states.insert(
                    id,
                    MonsterState::Move(MoveState {
                        id,
                        effect: noop,
                        intents: sv![Intent::Defend { block: 0 }],
                        follow_up: Some("PICK"),
                        must_perform_once: false,
                    }),
                );
            }
            states.insert(
                "PICK",
                MonsterState::RandomBranch(RandomBranchState {
                    id: "PICK",
                    branches: sv![
                        WeightedBranch {
                            state_id: "A",
                            repeat_type: MoveRepeatType::CanRepeatForever,
                            max_times: 1,
                            weight: 1.0,
                            cooldown: 0,
                        },
                        WeightedBranch {
                            state_id: "B",
                            repeat_type: MoveRepeatType::CanRepeatForever,
                            max_times: 1,
                            weight: 1.0,
                            cooldown: 0,
                        },
                    ],
                }),
            );
            // A ConditionalBranch entry point, mirroring the rats' `RAND` →
            // `RAND_NORMAL` shape: it consumes no RNG itself, so the read has
            // to come from the RandomBranch behind it.
            states.insert(
                "ENTRY",
                MonsterState::ConditionalBranch(ConditionalBranchState {
                    id: "ENTRY",
                    branches: sv![ConditionalBranch {
                        condition: ConditionFn::Stateless(always),
                        state_id: "PICK",
                    }],
                }),
            );
            MonsterAi::new_with_combat(states, "ENTRY", &CombatState::default())
        }

        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::EnemyTurnExec;
        state.side = Side::Enemy;
        state.player.current_hp = 50;
        state.player.max_hp = 50;

        // The factory's pre-resolution must NOT have touched the run stream.
        let summon = state.spawn_enemy(crate::MonsterKind::ShrinkerBeetle);
        state.set_creature_ai(summon, branch_ai());
        assert_eq!(
            state.run_rng.monster_ai.counter(),
            0,
            "attaching an AI may not read the run stream"
        );

        roll_next_enemy_intents(&mut state);
        assert_eq!(
            state.run_rng.monster_ai.counter(),
            1,
            "the summon's branch initial owes exactly one run-stream read"
        );

        // And exactly once: the next pass is an ordinary follow-up roll, not a
        // second initial resolution.
        roll_next_enemy_intents(&mut state);
        assert_eq!(state.run_rng.monster_ai.counter(), 1, "no re-resolution");
    }

    #[test]
    fn turn_exec_advances_monster_ai_for_dsl_enemies() {
        use crate::monster_ai::{MonsterAi, MonsterState, MoveState};
        use smallvec::smallvec as sv;
        use std::collections::BTreeMap;

        fn noop(_: &mut CombatState, _: CreatureUid) {}

        let mut states = BTreeMap::new();
        states.insert(
            "A",
            MonsterState::Move(MoveState {
                id: "A",
                effect: noop,
                intents: sv![Intent::Defend { block: 0 }],
                follow_up: Some("B"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "B",
            MonsterState::Move(MoveState {
                id: "B",
                effect: noop,
                intents: sv![Intent::Defend { block: 0 }],
                follow_up: Some("A"),
                must_perform_once: false,
            }),
        );
        let ai = MonsterAi::new(states, "A");
        assert_eq!(ai.current_move().id, "A");
        assert_eq!(ai.move_history().len(), 0);

        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::EnemyTurnExec;
        state.side = Side::Enemy;
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        state.player.max_energy = 3;
        let id = NonZeroU32::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id,
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::ShrinkerBeetle),
            intent: Some(Intent::Defend { block: 0 }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("DslCycler"),
            ai: Some(ai),
        });
        state.enemy_uids.push(CreatureUid(id));

        // Bead `sts2-rs-c8why` follow-up: the next-intent roll now lives in
        // `enemy_turn_end` (after `after_turn_end`), so each enemy turn is
        // `enemy_turn_exec` (perform) + `enemy_turn_end` (roll). Drive both.
        let result = enemy_turn_exec(&mut state);
        assert_eq!(result, StepResult::Advanced);
        enemy_turn_end(&mut state);
        let ai_ref = state.creatures[0].ai.as_ref().expect("ai present");
        assert_eq!(ai_ref.move_history().len(), 1);
        assert_eq!(ai_ref.move_history()[0], "A");
        assert_eq!(ai_ref.current_move().id, "B");
        assert_eq!(state.creatures[0].intent, Some(Intent::Defend { block: 0 }));

        state.phase = CombatPhase::EnemyTurnExec;
        enemy_turn_exec(&mut state);
        enemy_turn_end(&mut state);
        let ai_ref = state.creatures[0].ai.as_ref().expect("ai present");
        assert_eq!(ai_ref.move_history().len(), 2);
        assert_eq!(ai_ref.move_history()[1], "B");
        assert_eq!(ai_ref.current_move().id, "A");

        state.phase = CombatPhase::EnemyTurnExec;
        enemy_turn_exec(&mut state);
        enemy_turn_end(&mut state);
        let ai_ref = state.creatures[0].ai.as_ref().expect("ai present");
        assert_eq!(ai_ref.move_history().len(), 3);
        assert_eq!(ai_ref.move_history()[2], "A");
        assert_eq!(ai_ref.current_move().id, "B");

        assert_eq!(state.player.current_hp, 50);
    }

    /// TestSubject MULTI_CLAW escalation (2026-07-17 matched-seed gap
    /// audit): `roll_next_enemy_intents` must add the owner's PainfulStabs
    /// power counter (the Rust home of C#'s `ExtraMultiClawCount`) to the
    /// static base hit count when telegraphing a "MULTI_CLAW" move —
    /// `MultiClawTotalCount => BaseMultiClawCount + ExtraMultiClawCount`
    /// (TestSubject.cs:163). Counter 0 keeps the base telegraph.
    #[test]
    fn roll_next_intents_adds_painful_stabs_counter_to_multi_claw_hits() {
        use crate::monster_ai::{MonsterAi, MonsterState, MoveState};
        use smallvec::smallvec as sv;
        use std::collections::BTreeMap;

        fn noop(_: &mut CombatState, _: CreatureUid) {}

        fn build_state(claw_counter: i32) -> CombatState {
            let mut states = BTreeMap::new();
            states.insert(
                "MULTI_CLAW",
                MonsterState::Move(MoveState {
                    id: "MULTI_CLAW",
                    effect: noop,
                    intents: sv![Intent::MultiAttack {
                        damage: 10,
                        hits: 3
                    }],
                    follow_up: Some("MULTI_CLAW"),
                    must_perform_once: false,
                }),
            );
            let ai = MonsterAi::new(states, "MULTI_CLAW");

            let mut state = CombatState::default();
            let id = NonZeroU32::new(1).expect("non-zero");
            let pid = NonZeroU32::new(1).expect("non-zero");
            let mut powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
            powers.push(PowerUid(pid));
            let mut power = Power::new(pid, PowerKind::PainfulStabs, 1);
            power.counter = claw_counter;
            state.powers.push(power);
            state.creatures.push(Creature {
                id,
                current_hp: 200,
                max_hp: 200,
                block: 0,
                powers,
                species: Some(MonsterKind::ShrinkerBeetle),
                intent: None,
                intent_history: IntentHistory::default(),
                monster_id: crate::MonsterId("TestSubject"),
                ai: Some(ai),
            });
            state.enemy_uids.push(CreatureUid(id));
            state
        }

        // Counter 0 → base telegraph.
        let mut state = build_state(0);
        roll_next_enemy_intents(&mut state);
        assert_eq!(
            state.creatures[0].intent,
            Some(Intent::MultiAttack {
                damage: 10,
                hits: 3
            }),
            "counter 0 keeps the base 3-hit telegraph"
        );

        // Counter 2 (two MULTI_CLAWs performed) → 3 + 2 hits.
        let mut state = build_state(2);
        roll_next_enemy_intents(&mut state);
        assert_eq!(
            state.creatures[0].intent,
            Some(Intent::MultiAttack {
                damage: 10,
                hits: 5
            }),
            "PainfulStabs counter is added to the telegraphed hit count"
        );
    }

    /// Bead `sts2-rs-iod` regression guard: when `creature.ai.is_none()`,
    /// Step 4 MUST keep using the legacy `MonsterKind::roll_intent`
    /// path with the persisted `intent_history`. Re-locks the existing
    /// o7r invariant from a different angle.
    #[test]
    fn turn_exec_legacy_path_unchanged_when_ai_is_none() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        assert!(state.creatures[0].ai.is_none());

        enemy_turn_exec(&mut state);
        // Bead `sts2-rs-c8why`: the roll moved to `enemy_turn_end`.
        enemy_turn_end(&mut state);

        // No-AI path: the post-turn-end roll uses
        // `MonsterKind::roll_intent` placeholder (Attack 5 for
        // ShrinkerBeetle).
        assert_eq!(
            state.creatures[0].intent,
            Some(Intent::Attack { damage: 5 })
        );
        assert_eq!(state.creatures[0].intent_history.turn_index, 1);
    }

    /// If an attack drops the player to 0 HP mid-loop,
    /// `enemy_turn_exec` returns `CombatLost` immediately and does NOT
    /// roll the next intent for any enemy. Mirrors Python's
    /// `if self.is_over: return` short-circuit at combat.py:1081-1082.
    #[test]
    fn enemy_turn_exec_returns_combat_lost_if_player_dies_mid_loop() {
        let mut state =
            state_for_enemy_turn_exec(2, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        state.player.current_hp = 5; // Cultist 0 deals 6 damage → player to 0.

        let result = enemy_turn_exec(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert_eq!(state.player.current_hp, 0);
        assert!(state.is_over);
        assert!(!state.player_won);
        // Phase NOT advanced — caller decides what to do with a
        // resolved combat (the play loop handles `Resolved` projection).
        assert_eq!(state.phase, CombatPhase::EnemyTurnExec);
    }

    /// Phase advances `EnemyTurnExec` -> `EnemyTurnEnd` after a clean
    /// per-enemy walk. P3-6 (sts2-rs-p1n) consumes `EnemyTurnEnd`.
    #[test]
    fn enemy_turn_exec_advances_phase_to_enemy_turn_end() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 1 });
        assert_eq!(state.phase, CombatPhase::EnemyTurnExec);

        let result = enemy_turn_exec(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::EnemyTurnEnd);
    }

    /// `is_over=true` short-circuits `enemy_turn_exec` to the matching
    /// `CombatWon` / `CombatLost` without firing any per-enemy intent
    /// or rolling the next intent. Defensive guard mirroring the other
    /// phase fns.
    #[test]
    fn enemy_turn_exec_short_circuits_when_combat_over_won() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        state.is_over = true;
        state.player_won = true;

        let result = enemy_turn_exec(&mut state);

        assert_eq!(result, StepResult::CombatWon);
        // No mutation: player HP / phase / intent unchanged.
        assert_eq!(state.player.current_hp, 50);
        assert_eq!(state.phase, CombatPhase::EnemyTurnExec);
        assert_eq!(
            state.creatures[0].intent,
            Some(Intent::Attack { damage: 6 })
        );
    }

    #[test]
    fn enemy_turn_exec_short_circuits_when_combat_over_lost() {
        let mut state =
            state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        state.is_over = true;
        state.player_won = false;

        let result = enemy_turn_exec(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert_eq!(state.player.current_hp, 50);
        assert_eq!(state.phase, CombatPhase::EnemyTurnExec);
    }

    /// Dead enemies (`current_hp == 0`) are skipped by both the perform
    /// loop AND the post-walk roll. Mirrors Python's `self.alive_enemies`
    /// filter at `combat.py:1074-1086`.
    #[test]
    fn enemy_turn_exec_skips_dead_enemies() {
        let mut state =
            state_for_enemy_turn_exec(2, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });
        // Enemy 0 alive at full HP; enemy 1 dead.
        state.creatures[1].current_hp = 0;
        // Mark a sentinel intent on the dead enemy that we'll verify is
        // NOT touched (no roll happened).
        state.creatures[1].intent = Some(Intent::Attack { damage: 999 });

        enemy_turn_exec(&mut state);
        // Bead `sts2-rs-c8why`: the next-intent roll moved to `enemy_turn_end`.
        enemy_turn_end(&mut state);

        // Player took damage from the alive enemy only (not 12).
        assert_eq!(state.player.current_hp, 44);
        // Bead sts2-rs-o7r: alive enemy's intent reflects the persisted
        // After performing the attack, the next-intent roll lands on
        // the `MonsterKind::roll_intent` placeholder for ShrinkerBeetle
        // (Attack 5).
        assert_eq!(
            state.creatures[0].intent,
            Some(Intent::Attack { damage: 5 })
        );
        // Dead enemy's intent was NOT re-rolled — stays at the sentinel.
        assert_eq!(
            state.creatures[1].intent,
            Some(Intent::Attack { damage: 999 }),
        );
    }

    /// Non-attack intents (Defend / Buff / Debuff / `DebuffStrong` /
    /// Sleep / Stun / `StatusCard` / Unknown) are stub no-ops at this
    /// Phase-3 pin: player HP / block unchanged, but the post-loop
    /// next-intent roll still fires. Locks the structural call site
    /// for Phase-5 `effect_fn` DSL extension.
    #[test]
    fn enemy_turn_exec_stubs_non_attack_intents() {
        for stub_intent in [
            Intent::Defend { block: 0 },
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            },
            Intent::Debuff {
                kind: PowerKind::Weak,
                amount: 0,
            },
            Intent::DebuffStrong,
            Intent::Sleep,
            Intent::Stun,
            Intent::StatusCard,
            Intent::Unknown,
        ] {
            let mut state = state_for_enemy_turn_exec(1, MonsterKind::ShrinkerBeetle, stub_intent);

            let result = enemy_turn_exec(&mut state);

            assert_eq!(result, StepResult::Advanced, "intent {stub_intent:?}");
            // Player untouched.
            assert_eq!(
                state.player.current_hp, 50,
                "intent {stub_intent:?} should not damage player"
            );
            assert_eq!(state.player.block, 0, "intent {stub_intent:?}");
            // Phase advanced; next intent rolled.
            assert_eq!(state.phase, CombatPhase::EnemyTurnEnd);
            assert!(state.creatures[0].intent.is_some());
        }
    }

    /// Multi-enemy: each enemy's attack fires once. Two cultists with
    /// Attack(6) each → 12 total damage to the player.
    #[test]
    fn enemy_turn_exec_walks_every_alive_enemy() {
        let mut state =
            state_for_enemy_turn_exec(3, MonsterKind::ShrinkerBeetle, Intent::Attack { damage: 6 });

        enemy_turn_exec(&mut state);
        // Bead `sts2-rs-c8why`: the next-intent roll moved to `enemy_turn_end`.
        enemy_turn_end(&mut state);

        // 50 - (3 * 6) = 32.
        assert_eq!(state.player.current_hp, 32);
        // Each enemy's persisted history advances to turn_index=1 +
        // last_kind=Attack after performing Attack(6). The next-roll
        // for ShrinkerBeetle's no-AI placeholder lands on Attack 5.
        for c in &state.creatures {
            assert_eq!(c.intent, Some(Intent::Attack { damage: 5 }));
            assert_eq!(c.intent_history.turn_index, 1);
        }
    }

    /// Determinism: same seed + same input state -> same final state.
    /// Locks the property for Phase-5 RNG-driven `roll_intent` paths
    /// (e.g., `JawWorm`'s weighted picker) once `IntentHistory`
    /// persistence lands.
    #[test]
    fn enemy_turn_exec_is_deterministic_given_seed() {
        let mut a = state_for_enemy_turn_exec(
            2,
            MonsterKind::FuzzyWurmCrawler,
            Intent::Attack { damage: 11 },
        );
        let mut b = a.clone();

        enemy_turn_exec(&mut a);
        enemy_turn_exec(&mut b);

        assert_eq!(a.player.current_hp, b.player.current_hp);
        assert_eq!(a.phase, b.phase);
        for (ca, cb) in a.creatures.iter().zip(b.creatures.iter()) {
            assert_eq!(ca.intent, cb.intent);
            assert_eq!(ca.current_hp, cb.current_hp);
        }
    }

    /// sts2-rs-vvi6g: a 0-HP creature carrying `IllusionPower` revives to
    /// full HP on the enemy turn (C# `IllusionPower.AfterDeath` ->
    /// `REVIVE_MOVE` heal-to-full) and does NOT also perform its own
    /// intent that turn, while a live primary enemy acts normally.
    #[test]
    fn enemy_turn_exec_revives_illusion_at_zero_hp_to_full() {
        let mut state = state_for_enemy_turn_exec(0, MonsterKind::ShrinkerBeetle, Intent::Unknown);

        // Reviving illusion: ShrinkerBeetle (legacy roll_intent) at 0 HP
        // carrying IllusionPower. Its intent is a lethal attack to prove
        // the revive consumes its turn (it must NOT hit the player).
        let illusion_id = NonZeroU32::new(1).expect("non-zero");
        let illusion_power = PowerUid::new(100).expect("non-zero");
        let mut illusion_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        illusion_powers.push(illusion_power);
        state.powers.push(Power {
            id: illusion_power.0,
            kind: PowerKind::Illusion,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures.push(Creature {
            id: illusion_id,
            current_hp: 0,
            max_hp: 6,
            block: 0,
            powers: illusion_powers,
            species: Some(MonsterKind::ShrinkerBeetle),
            intent: Some(Intent::Attack { damage: 99 }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("ShrinkerBeetle"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(illusion_id));

        // Live primary enemy (slot 1) that attacks for 6.
        let primary_id = NonZeroU32::new(2).expect("non-zero");
        state.creatures.push(Creature {
            id: primary_id,
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::ShrinkerBeetle),
            intent: Some(Intent::Attack { damage: 6 }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("ShrinkerBeetle"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(primary_id));

        enemy_turn_exec(&mut state);

        // Illusion revived to full HP (max_hp), not its old 0.
        let illusion = state
            .creatures
            .iter()
            .find(|c| c.id == illusion_id)
            .expect("illusion present");
        assert_eq!(illusion.current_hp, 6, "illusion must revive to max_hp");

        // The illusion's lethal 99-damage intent did NOT fire - only the
        // primary's 6 landed (50 - 6 = 44). Reviving consumes the turn.
        assert_eq!(
            state.player.current_hp, 44,
            "only the primary's 6 damage should land; the revived illusion must not also attack"
        );
    }

    // ── P3-6 (sts2-rs-p1n) enemy_turn_end ───────────────────────────────

    /// Build a `CombatState` ready to invoke `enemy_turn_end`. Phase set
    /// to `EnemyTurnEnd`, side to `Enemy`, round = 1, RNG seeded for
    /// determinism.
    fn state_for_enemy_turn_end() -> CombatState {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::EnemyTurnEnd;
        state.side = Side::Enemy;
        state.round = 1;
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        state
    }

    /// Build a `CombatState` like above but with one Cultist enemy carrying
    /// a `Vulnerable(amount)` debuff. `power_uid_seed` is the `NonZeroU32`
    /// seed used for both the Power's `id` and the Cultist's `CreatureUid`
    /// (which are kept separate by stride to avoid clashes).
    fn state_for_enemy_turn_end_with_cultist_vuln(amount: i32) -> CombatState {
        let mut state = state_for_enemy_turn_end();
        let creature_id = NonZeroU32::new(1).expect("non-zero");
        let power_uid = PowerUid::new(10).expect("non-zero");
        let mut powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        powers.push(power_uid);
        state.creatures.push(Creature {
            id: creature_id,
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers,
            species: Some(MonsterKind::ShrinkerBeetle),
            intent: Some(Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("ShrinkerBeetle"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(creature_id));
        state.powers.push(Power {
            id: power_uid.0,
            kind: PowerKind::Vulnerable,
            amount,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state
    }

    /// `enemy_turn_end` advances `EnemyTurnEnd` -> `PlayerTurnStart` and
    /// increments `round` by 1 on a clean walk. Mirrors Python's
    /// `combat.py:1088-1095 + 976-980` post-condition: the next call to
    /// `_start_player_turn` sees `round_number = old + 1`.
    #[test]
    fn enemy_turn_end_increments_round_and_returns_to_player_turn_start() {
        let mut state = state_for_enemy_turn_end();
        assert_eq!(state.round, 1);
        assert_eq!(state.phase, CombatPhase::EnemyTurnEnd);

        let result = enemy_turn_end(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.round, 2, "round should bump from 1 to 2");
        assert_eq!(
            state.phase,
            CombatPhase::PlayerTurnStart,
            "phase should advance to PlayerTurnStart for the next round",
        );
    }

    /// Cultist with Vulnerable(2) → after `enemy_turn_end` →
    /// Vulnerable(1). Locks the synthetic `on_turn_end_enemy_side`
    /// tick-down semantics for an enemy-owned debuff.
    #[test]
    fn enemy_turn_end_ticks_vulnerable_on_enemy() {
        let mut state = state_for_enemy_turn_end_with_cultist_vuln(2);

        enemy_turn_end(&mut state);

        let vuln = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Vulnerable)
            .expect("Vulnerable instance should still be present (Python keeps stale)");
        assert_eq!(
            vuln.amount, 1,
            "Vulnerable(2) should tick down to Vulnerable(1) at enemy turn-end",
        );
    }

    /// Cultist with Vulnerable(1) → after `enemy_turn_end` →
    /// Vulnerable(0). Python's `on_turn_end_enemy_side` body is
    /// `self.amount -= 1` with no removal-on-zero guard; Python leaves
    /// the stale `Vulnerable(0)` instance in `creature.powers`. The
    /// parity exporter at `bridge_replay.py:329` filters
    /// `power.amount != 0`, so the stale instance is invisible in
    /// serialized state.
    #[test]
    fn enemy_turn_end_removes_zero_amount_debuff() {
        let mut state = state_for_enemy_turn_end_with_cultist_vuln(1);

        enemy_turn_end(&mut state);

        // Bead `sts2-rs-o1mwv` (C.4 follow-up): Vulnerable(1) ticks to 0
        // and is REMOVED from the arena (C# `ShouldRemoveDueToAmount` at
        // `Amount <= 0`), not retained at amount 0.
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Vulnerable),
            "Vulnerable(1) ticking to 0 must be removed from the power arena",
        );

        // And it must not serialize (it's gone, not just amount-0-filtered).
        let serialized = crate::parity::combat_state_to_bridge_state(&state);
        let enemies = serialized
            .get("enemies")
            .and_then(|v| v.as_array())
            .expect("enemies array");
        let cultist = &enemies[0];
        let powers = cultist
            .get("powers")
            .and_then(|v| v.as_array())
            .expect("powers array");
        assert!(powers.is_empty(), "removed Vulnerable must not serialize");
    }

    /// Issue #30: ticking Vulnerable past 0 must clamp at 0, not drift
    /// into deep negatives. In long stalls the `on_turn_end_enemy_side`
    /// listener fires every turn against a Vulnerable(0) instance, and
    /// the pre-fix code happily subtracted 1 each time (observed
    /// Vulnerable(-87) in the 2026-05-20 mistake replays).
    #[test]
    fn enemy_turn_end_repeated_ticks_clamp_at_zero() {
        let mut state = state_for_enemy_turn_end_with_cultist_vuln(1);

        // First tick: 1 -> 0 removes the debuff (bead sts2-rs-o1mwv C.4
        // follow-up). Subsequent ticks must be safe no-ops (no power to
        // tick, no underflow / panic).
        for _ in 0..6 {
            enemy_turn_end(&mut state);
            // Reset phase back to EnemyTurnEnd so we can drive the listener again.
            state.phase = CombatPhase::EnemyTurnEnd;
        }

        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Vulnerable),
            "Vulnerable removed after its first tick to 0; repeated ticks stay safe",
        );
    }

    /// Player-owned Vulnerable also ticks at the enemy turn-end boundary.
    /// Mirrors Python's `fire_after_turn_end(side=ENEMY)` synthetic guard
    /// at `hooks.py:391-403`: the `on_turn_end_enemy_side` walk fires
    /// for EVERY power-listener regardless of `owner.side`, not just
    /// enemy-owned ones.
    #[test]
    fn enemy_turn_end_ticks_player_owned_vulnerable_too() {
        let mut state = state_for_enemy_turn_end();
        let power_uid = PowerUid::new(10).expect("non-zero");
        state.powers.push(Power {
            id: power_uid.0,
            kind: PowerKind::Vulnerable,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(power_uid);

        enemy_turn_end(&mut state);

        let vuln = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Vulnerable)
            .expect("Vulnerable instance still attached");
        assert_eq!(
            vuln.amount, 2,
            "player-owned Vulnerable(3) ticks to Vulnerable(2) at enemy turn-end",
        );
    }

    /// Weak and Frail tick down on the same enemy-turn-end boundary as
    /// Vulnerable. Locks the per-power tick-down map against Python's
    /// `WeakPower.on_turn_end_enemy_side` (common.py:101-105) and
    /// `FrailPower.on_turn_end_enemy_side` (common.py:125-129).
    #[test]
    fn enemy_turn_end_ticks_weak_and_frail() {
        let mut state = state_for_enemy_turn_end();
        let weak_uid = PowerUid::new(10).expect("non-zero");
        let frail_uid = PowerUid::new(11).expect("non-zero");
        state.powers.push(Power {
            id: weak_uid.0,
            kind: PowerKind::Weak,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.powers.push(Power {
            id: frail_uid.0,
            kind: PowerKind::Frail,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(weak_uid);
        state.player.powers.push(frail_uid);

        enemy_turn_end(&mut state);

        let weak = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Weak)
            .expect("Weak instance attached");
        assert_eq!(weak.amount, 1, "Weak(2) ticks to Weak(1)");
        // Bead `sts2-rs-o1mwv` (C.4 follow-up): Frail(1) ticks to 0 and is
        // removed (C# `ShouldRemoveDueToAmount`), not retained at Frail(0).
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Frail),
            "Frail(1) ticking to 0 must be removed",
        );
    }

    /// `is_over=true` short-circuits `enemy_turn_end` to the matching
    /// `CombatWon` / `CombatLost` without firing hooks, ticking debuffs,
    /// or advancing phase. Defensive guard mirroring the other phase fns.
    #[test]
    fn enemy_turn_end_short_circuits_when_combat_over_won() {
        let mut state = state_for_enemy_turn_end_with_cultist_vuln(2);
        state.is_over = true;
        state.player_won = true;

        let result = enemy_turn_end(&mut state);

        assert_eq!(result, StepResult::CombatWon);
        // Phase / round unchanged; Vulnerable did NOT tick.
        assert_eq!(state.phase, CombatPhase::EnemyTurnEnd);
        assert_eq!(state.round, 1);
        let vuln = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Vulnerable)
            .unwrap();
        assert_eq!(
            vuln.amount, 2,
            "tick-down skipped on combat-over short-circuit"
        );
    }

    #[test]
    fn enemy_turn_end_short_circuits_when_combat_over_lost() {
        let mut state = state_for_enemy_turn_end_with_cultist_vuln(2);
        state.is_over = true;
        state.player_won = false;

        let result = enemy_turn_end(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert_eq!(state.phase, CombatPhase::EnemyTurnEnd);
        assert_eq!(state.round, 1);
    }

    /// Bead `sts2-rs-2e9q2`: on player defeat C# runs the generic
    /// `RemoveAllPowersAfterDeath` on the player creature
    /// (`CreatureCmd.cs:385`, reached for `creature.IsPlayer` too), leaving
    /// `player.powers == []`. Rust must mirror that, clearing BOTH the legacy
    /// `state.player.powers` surface (where monster-applied Constrict lands)
    /// AND the player mirror creature's `powers`, plus the central
    /// `state.powers` rows. Root cause of the coverage-sweep
    /// `player.powers.len 1 vs 0` drift: the player dies to Constrict's
    /// end-of-turn self-damage and C# clears the lingering powers (NOT a
    /// Constrict-lifetime / applier-death bug — the applier is alive).
    #[test]
    fn player_defeat_clears_player_powers() {
        let mut state = state_with_cultists(1);
        state.player.max_hp = 80;
        state.player.current_hp = 30;

        // Establish the player mirror creature (alive, so the apply lands).
        let player_id = NonZeroU32::new(100).expect("non-zero");
        state.creatures.push(Creature {
            id: player_id,
            current_hp: 30,
            max_hp: 80,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Player"),
            ai: None,
        });
        state.player.creature_uid = Some(CreatureUid(player_id));

        // Mirror-surface power (Strength → allocates power id 1).
        crate::card_effects::apply_power_to_creature(
            &mut state,
            CreatureUid(player_id),
            PowerKind::Strength,
            2,
        );
        // Legacy-surface power: monster-applied Constrict (the real-bug path),
        // injected with a high id so it can't collide with the allocator.
        let cpow = crate::PowerUid::new(9999).expect("non-zero");
        state.powers.push(crate::Power {
            id: cpow.0,
            kind: PowerKind::Constrict,
            amount: 9,
            counter: 0,
            applier: Some(state.enemy_uids[0]),
            skip_next_tick: false,
        });
        state.player.powers.push(cpow);

        assert!(
            !state.player.powers.is_empty(),
            "precondition: player carries powers before death"
        );

        // Player dies (e.g. to Constrict's end-of-turn self-damage).
        state.player.current_hp = 0;
        let mirror_idx = state
            .creatures
            .iter()
            .position(|c| c.id == player_id)
            .expect("mirror present");
        state.creatures[mirror_idx].current_hp = 0;

        let result = check_combat_end(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert!(state.is_over && !state.player_won);
        // All three player-power surfaces cleared, matching C#'s post-defeat
        // `player.powers == []`.
        assert!(
            state.player.powers.is_empty(),
            "legacy player.powers not cleared on defeat"
        );
        let mirror = state
            .creatures
            .iter()
            .find(|c| c.id == player_id)
            .expect("mirror present");
        assert!(
            mirror.powers.is_empty(),
            "player mirror-creature powers not cleared on defeat"
        );
        assert!(
            state
                .powers
                .iter()
                .all(|p| p.kind != PowerKind::Constrict && p.kind != PowerKind::Strength),
            "central power rows not unlinked on defeat"
        );
    }

    /// `enemy_turn_end` does NOT tick when the recorder bypasses turn
    /// boundaries — the VTHIN-2 fixture invariant. Calling
    /// `apply_damage` on its own does NOT fire the synthetic guard;
    /// only the explicit `enemy_turn_end` (or `dispatch_after_turn_end`
    /// with `Side::Enemy`) does. This test exercises a damage-only path
    /// against a Cultist with Vulnerable(2) and asserts the amount
    /// stays at 2 — locking the byte-equality guarantee for
    /// `vulnerable-damage-seed0.json`.
    #[test]
    fn vulnerable_does_not_tick_on_apply_damage_alone() {
        let mut state = state_for_enemy_turn_end_with_cultist_vuln(2);
        // Apply damage without invoking enemy_turn_end.
        let cultist_uid = state.enemy_uids[0];
        crate::damage::apply_damage(
            &mut state,
            crate::hooks::CreatureRef::Player,
            cultist_uid,
            6,
            crate::hooks::ValueProps::MOVE,
            None,
        );

        let vuln = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Vulnerable)
            .unwrap();
        assert_eq!(
            vuln.amount, 2,
            "apply_damage alone must NOT tick Vulnerable (preserves VTHIN-2 byte-equality)",
        );
    }

    /// Determinism: same seed + same input state -> same final state.
    /// Two state copies seeded identically must produce byte-equal
    /// post-condition (round + phase + power amounts).
    #[test]
    fn enemy_turn_end_is_deterministic_given_seed() {
        let mut a = state_for_enemy_turn_end_with_cultist_vuln(3);
        let mut b = a.clone();

        enemy_turn_end(&mut a);
        enemy_turn_end(&mut b);

        assert_eq!(a.round, b.round);
        assert_eq!(a.phase, b.phase);
        assert_eq!(a.powers, b.powers);
        for (ca, cb) in a.creatures.iter().zip(b.creatures.iter()) {
            assert_eq!(ca.current_hp, cb.current_hp);
            assert_eq!(ca.intent, cb.intent);
        }
    }

    /// `enemy_turn_end` does NOT modify any pile (hand / draw / discard
    /// / exhaust / play) at this Rust pin. The Phase-6 stub for
    /// `_cleanup_cards_end_of_turn` is structurally present but a no-op;
    /// this test locks the no-op behavior so a future Phase-6 revision
    /// won't silently regress pile state at this boundary.
    #[test]
    fn enemy_turn_end_runs_cleanup_on_all_piles_noop_at_phase3_pin() {
        let mut state = state_for_enemy_turn_end();
        // Pre-populate piles to verify they are NOT touched by the
        // Phase-6 cleanup stub.
        state.player.hand.push(CardUid::new(1).unwrap());
        state.player.draw.push(CardUid::new(2).unwrap());
        state.player.discard.push(CardUid::new(3).unwrap());
        state.player.exhaust.push(CardUid::new(4).unwrap());

        enemy_turn_end(&mut state);

        assert_eq!(state.player.hand.len(), 1);
        assert_eq!(state.player.draw.len(), 1);
        assert_eq!(state.player.discard.len(), 1);
        assert_eq!(state.player.exhaust.len(), 1);
    }

    // ── P3-7 (sts2-rs-e3k) check_combat_end + end_turn_dispatch ─────────

    /// `check_combat_end` returns `CombatWon` and sets `is_over=true /
    /// player_won=true` when every enemy in `state.enemy_uids` has
    /// `current_hp <= 0`. Mirrors Python's
    /// `combat.py:_check_combat_end` (line 2456) where
    /// `not self.alive_enemies and not should_stop_combat_ending` triggers
    /// `_end_combat(player_won=True)`.
    #[test]
    fn check_combat_end_returns_combat_won_when_all_enemies_dead() {
        let mut state = state_for_enemy_turn_exec(
            2,
            MonsterKind::ShrinkerBeetle,
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            },
        );
        state.creatures[0].current_hp = 0;
        state.creatures[1].current_hp = 0;
        state.player.current_hp = 30; // alive

        let result = check_combat_end(&mut state);

        assert_eq!(result, StepResult::CombatWon);
        assert!(state.is_over, "is_over should be set after victory");
        assert!(state.player_won, "player_won should be true on victory");
    }

    /// Bead `sts2-rs-h9u`: when the only live enemy dies carrying a power
    /// with a registered on-death "second phase" handler (PhrogParasite's
    /// `Infested`), `check_combat_end` fires the handler and KEEPS combat
    /// open (C# `InfestedPower.ShouldStopCombatFromEnding == true`) instead
    /// of resolving victory — here the handler spawns one reinforcement.
    /// Once the reinforcement also dies, the zeroed trigger does NOT
    /// re-summon and combat resolves as a win.
    #[test]
    #[serial_test::serial(global_registry)]
    fn check_combat_end_infested_death_spawns_then_resolves_when_cleared() {
        fn spawn_one(state: &mut CombatState, _owner: CreatureUid) {
            let _ = state.spawn_enemy(MonsterKind::Wriggler);
        }
        crate::enemy_death_effects::register(PowerKind::Infested, spawn_one);

        let mut state = state_for_enemy_turn_exec(1, MonsterKind::PhrogParasite, Intent::Stun);
        let phrog = state.enemy_uids[0];
        crate::card_effects::apply_power_to_creature(&mut state, phrog, PowerKind::Infested, 4);
        state.creatures[0].current_hp = 0; // phrog dies
        state.player.current_hp = 30; // alive

        // First check: phrog dead, second phase fires, reinforcement spawns.
        let result = check_combat_end(&mut state);
        assert_eq!(
            result,
            StepResult::Advanced,
            "combat continues while the spawned reinforcement lives"
        );
        assert!(
            !state.is_over,
            "is_over must stay false during second phase"
        );
        let alive = state
            .enemy_uids
            .iter()
            .filter(|u| {
                state
                    .creatures
                    .iter()
                    .any(|c| c.id == u.0 && c.current_hp > 0)
            })
            .count();
        assert_eq!(alive, 1, "exactly the spawned reinforcement is alive");
        assert!(
            state
                .powers
                .iter()
                .any(|p| p.kind == PowerKind::Infested && p.amount == 0),
            "trigger power zeroed so it cannot re-summon"
        );

        // Kill the reinforcement; the zeroed Infested must NOT re-summon.
        for c in &mut state.creatures {
            c.current_hp = 0;
        }
        let result2 = check_combat_end(&mut state);
        assert_eq!(result2, StepResult::CombatWon, "no re-summon → victory");
        assert!(state.is_over && state.player_won);
    }

    /// Bead `sts2-rs-jjwam`: when the last PRIMARY enemy dies and only
    /// SECONDARY (minion) enemies remain alive, C# `CreatureCmd` kills the
    /// minions (`Kill(teammates)` when `isPrimaryEnemy && teammates.All(
    /// IsSecondaryEnemy)`, CreatureCmd.cs:392) and combat ends. A creature is
    /// secondary when it carries a power with `OwnerIsSecondaryEnemy` — at
    /// this pin only `PowerKind::Minion` (e.g. FOGMOG's EyeWithTeeth). Here
    /// the primary dies with a live minion: `check_combat_end` must kill the
    /// minion and resolve victory. Surfaced by the 2026-05-29 parity re-audit
    /// (FOGMOG seeds 15/140).
    #[test]
    #[serial_test::serial(global_registry)]
    fn check_combat_end_primary_death_kills_surviving_minions() {
        let mut state = state_for_enemy_turn_exec(2, MonsterKind::ShrinkerBeetle, Intent::Stun);
        // Tag the second enemy as a minion (secondary); leave it alive.
        let minion_uid = state.enemy_uids[1];
        crate::card_effects::apply_power_to_creature(&mut state, minion_uid, PowerKind::Minion, 1);
        // The primary (index 0) dies; the minion stays alive.
        state.creatures[0].current_hp = 0;
        state.player.current_hp = 30;
        assert!(
            state
                .creatures
                .iter()
                .find(|c| c.id == minion_uid.0)
                .is_some_and(|c| c.current_hp > 0),
            "fixture: minion starts alive"
        );

        let result = check_combat_end(&mut state);

        assert_eq!(
            result,
            StepResult::CombatWon,
            "last primary dead + only minion alive must resolve as victory"
        );
        assert!(state.is_over && state.player_won, "victory flags set");
        let minion_hp = state
            .creatures
            .iter()
            .find(|c| c.id == minion_uid.0)
            .map_or(99, |c| c.current_hp);
        assert!(
            minion_hp <= 0,
            "surviving minion must be killed when the last primary dies (C# Kill(teammates))"
        );
    }

    /// Guard for `sts2-rs-jjwam`: a live PRIMARY enemy keeps combat going even
    /// when a minion is also alive — the collapse only fires once ALL primary
    /// enemies are dead.
    #[test]
    #[serial_test::serial(global_registry)]
    fn check_combat_end_minion_collapse_requires_all_primaries_dead() {
        let mut state = state_for_enemy_turn_exec(2, MonsterKind::ShrinkerBeetle, Intent::Stun);
        let minion_uid = state.enemy_uids[1];
        crate::card_effects::apply_power_to_creature(&mut state, minion_uid, PowerKind::Minion, 1);
        // Primary (index 0) still alive; minion alive too.
        state.player.current_hp = 30;

        let result = check_combat_end(&mut state);

        assert_eq!(
            result,
            StepResult::Advanced,
            "a live primary keeps combat going regardless of minions"
        );
        assert!(!state.is_over, "no premature victory while a primary lives");
    }

    /// `check_combat_end` returns `CombatLost` and sets `is_over=true /
    /// player_won=false` when `state.player.current_hp <= 0`. Mirrors
    /// Python's `_check_combat_end` (line 2455) where the
    /// `_get_first_dead_primary_player` branch fires
    /// `_end_combat(player_won=False)`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn check_combat_end_returns_combat_lost_when_player_dead() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::ShrinkerBeetle,
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            },
        );
        state.player.current_hp = 0;

        let result = check_combat_end(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert!(state.is_over);
        assert!(!state.player_won);
    }

    /// `should_die_late` listener that heals the player and votes
    /// prevent — stands in for LizardTail's canonical revive
    /// behaviour. Bead `sts2-rs-p62n0`.
    fn h_revive_player_to_30(state: &mut crate::CombatState) -> crate::EffectResult {
        state.player.current_hp = 30;
        crate::EffectResult::Resolved
    }

    /// Abstain stand-in installed at end-of-test to clear the
    /// prevent listener (the channel registry is a process-global
    /// static; sibling death-flip tests rely on the empty state).
    fn h_should_die_late_clear(_state: &mut crate::CombatState) -> crate::EffectResult {
        crate::EffectResult::NotImplemented
    }

    /// `check_combat_end` skips the combat-lost flip when a
    /// `should_die_late` listener heals the player above 0 HP and
    /// votes prevent — mirrors Python's `core/combat.py:3122-3123`
    /// intercept. Bead `sts2-rs-p62n0`.
    ///
    /// Cleans up the listener at the end so sibling tests
    /// (`check_combat_end_returns_combat_lost_when_player_dead`,
    /// `..._treats_negative_player_hp_as_dead`) see an
    /// abstain-only / empty bus and still hit the standard death-
    /// flip path.
    #[test]
    #[serial_test::serial(global_registry)]
    fn check_combat_end_prevented_by_should_die_late_listener() {
        use crate::relic_effects::{register_should_die_late, ShouldDieLateFn};
        use crate::RelicId;

        register_should_die_late(
            RelicId::LizardTail,
            h_revive_player_to_30 as ShouldDieLateFn,
        );

        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::ShrinkerBeetle,
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            },
        );
        state.player.current_hp = 0;

        let result = check_combat_end(&mut state);

        assert_eq!(
            result,
            StepResult::Advanced,
            "should_die_late + heal must skip the CombatLost flip"
        );
        assert!(!state.is_over, "combat must NOT be flagged over");
        assert_eq!(
            state.player.current_hp, 30,
            "listener's mid-call heal must persist"
        );

        // Cleanup: re-register an abstain listener so the global
        // bus doesn't leak the prevent vote into sibling tests.
        register_should_die_late(
            RelicId::LizardTail,
            h_should_die_late_clear as ShouldDieLateFn,
        );
    }

    /// Negative HP also triggers the player-death branch. Defensive —
    /// Python's `apply_damage` clamps at 0 but a future Rust caller might
    /// not, and the predicate is documented as `<= 0`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn check_combat_end_treats_negative_player_hp_as_dead() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::ShrinkerBeetle,
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            },
        );
        state.player.current_hp = -5;

        let result = check_combat_end(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert!(!state.player_won);
    }

    /// `check_combat_end` returns `Advanced` (no mutation) when both
    /// the player AND at least one enemy are alive. Locks the
    /// "combat ongoing" branch.
    #[test]
    fn check_combat_end_returns_advanced_when_combat_ongoing() {
        let mut state = state_for_enemy_turn_exec(
            2,
            MonsterKind::ShrinkerBeetle,
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            },
        );
        // Mixed alive/dead — at least one alive enemy keeps combat ongoing.
        state.creatures[0].current_hp = 50;
        state.creatures[1].current_hp = 0;
        state.player.current_hp = 30;

        let result = check_combat_end(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert!(
            !state.is_over,
            "is_over should NOT be set on ongoing combat"
        );
        assert!(!state.player_won);
    }

    /// `check_combat_end` is idempotent: calling it twice on a resolved
    /// `(is_over=true, player_won=true)` state returns `CombatWon` both
    /// times WITHOUT re-firing the `after_combat_victory` /
    /// `after_combat_end` hooks. Critical for Phase-7 listener bodies
    /// that increment counters or grant rewards (e.g. `BurningBlood`'s
    /// HEAL=6 fires on the FIRST victory only).
    ///
    /// At this Rust pin the dispatchers are no-op walks, so re-firing
    /// is harmless from a correctness standpoint, but the idempotency
    /// guarantee is part of the helper's contract — locked here.
    #[test]
    fn check_combat_end_idempotent_on_already_over_won() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::ShrinkerBeetle,
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            },
        );
        state.is_over = true;
        state.player_won = true;
        // Player at full HP (would NOT trigger CombatLost) and enemy
        // alive (would NOT trigger CombatWon) — but is_over already set,
        // so the helper returns the recorded outcome without re-checking.
        state.player.current_hp = 50;
        state.creatures[0].current_hp = 50;

        let result = check_combat_end(&mut state);

        assert_eq!(result, StepResult::CombatWon);
        // State unchanged.
        assert!(state.is_over);
        assert!(state.player_won);
    }

    /// Same idempotency guarantee for the `CombatLost` recorded outcome.
    #[test]
    fn check_combat_end_idempotent_on_already_over_lost() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::ShrinkerBeetle,
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            },
        );
        state.is_over = true;
        state.player_won = false;
        state.player.current_hp = 50; // alive but is_over short-circuits
        state.creatures[0].current_hp = 50;

        let result = check_combat_end(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert!(state.is_over);
        assert!(!state.player_won);
    }

    /// Player-death branch takes priority over the all-enemies-dead
    /// branch. If both predicates match (player.hp <= 0 AND no alive
    /// enemies), the helper returns `CombatLost`, mirroring Python's
    /// `_get_first_dead_primary_player` short-circuit at the head of
    /// the `_check_combat_end` elif chain (combat.py:2455 fires
    /// before line 2456).
    #[test]
    #[serial_test::serial(global_registry)]
    fn check_combat_end_player_death_takes_priority_over_enemy_death() {
        let mut state = state_for_enemy_turn_exec(
            1,
            MonsterKind::ShrinkerBeetle,
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            },
        );
        state.player.current_hp = 0;
        state.creatures[0].current_hp = 0;

        let result = check_combat_end(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert!(!state.player_won);
    }

    /// `end_turn_dispatch` walks the full 6-step chain on a clean state
    /// (no combat-end mid-chain): phase advances back to
    /// `PlayerPlayPhase` (after the loop-closing `start_player_turn`)
    /// and `round` increments by 1. Mirrors Python's tail-call to
    /// `_start_player_turn` after the round bump
    /// (combat.py:976-980).
    #[test]
    fn end_turn_dispatch_walks_full_player_to_player_chain() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerPlayPhase;
        state.round = 1;
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        state.player.max_energy = 3;
        // One alive enemy so the chain doesn't short-circuit on
        // CombatWon mid-chain. Cultist with Buff intent (no-op stub
        // at this Phase-3 pin — won't damage the player).
        let id = NonZeroU32::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id,
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::ShrinkerBeetle),
            intent: Some(Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("ShrinkerBeetle"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(id));

        let result = end_turn_dispatch(&mut state);

        assert_eq!(result, StepResult::Advanced);
        // After the chain: phase back at PlayerPlayPhase (loop closed
        // by start_player_turn).
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
        // Round incremented by enemy_turn_end (1 -> 2).
        assert_eq!(state.round, 2);
        // Player still alive (Buff intent is a stub no-op).
        assert_eq!(state.player.current_hp, 50);
        // Side flipped back to Player by start_player_turn (the
        // pipeline does NOT explicitly reset side, but enemy_turn_end
        // doesn't restore Player either — start_player_turn doesn't
        // touch side; it's still Enemy from enemy_turn_start. This
        // is a known Phase-3 deviation; Python's _start_player_turn
        // also doesn't explicitly reset side because the next
        // _execute_enemy_turn flips it. Phase 6 may add an explicit
        // reset.
    }

    /// `end_turn_dispatch` short-circuits with `CombatLost` if a
    /// mid-chain enemy attack drops the player to 0 HP. The chain
    /// stops at `enemy_turn_exec` and downstream phases
    /// (`enemy_turn_end`, `start_player_turn`) are skipped.
    #[test]
    #[serial_test::serial(global_registry)]
    fn end_turn_dispatch_short_circuits_on_combat_lost() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerPlayPhase;
        state.round = 1;
        state.player.current_hp = 5;
        state.player.max_hp = 50;
        state.player.max_energy = 3;
        // Cultist with Attack(6) — kills player in one hit.
        let id = NonZeroU32::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id,
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::ShrinkerBeetle),
            intent: Some(Intent::Attack { damage: 6 }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("ShrinkerBeetle"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(id));

        let result = end_turn_dispatch(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert!(state.is_over);
        assert!(!state.player_won);
        assert_eq!(state.player.current_hp, 0);
        // Round NOT incremented (chain stopped at enemy_turn_exec
        // before enemy_turn_end's bump).
        assert_eq!(state.round, 1);
        // Phase did NOT close the loop back to PlayerPlayPhase.
        assert_eq!(state.phase, CombatPhase::EnemyTurnExec);
    }

    /// If the chain enters with `is_over=true` already set (e.g., a
    /// previous `Action::PlayCard` killed the last enemy via Phase-6
    /// effect), `end_turn_dispatch` short-circuits at the very first
    /// step (`end_player_turn_phase_one`'s entry guard) and returns
    /// the recorded outcome.
    #[test]
    fn end_turn_dispatch_short_circuits_when_combat_already_resolved() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerPlayPhase;
        state.is_over = true;
        state.player_won = true;

        let result = end_turn_dispatch(&mut state);

        assert_eq!(result, StepResult::CombatWon);
        // Phase unchanged (no advance happened).
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
    }

    // P3-stretch (sts2-rs-o7r): IntentHistory persistence on Creature

    /// Build a state with a single Cultist + a fresh Buff intent (the
    /// canonical post-`start_combat` shape), pre-positioned at
    /// `EnemyTurnExec` so we can drive `enemy_turn_exec` directly.
    /// Player has plenty of HP so the intent we care about (Buff or
    /// Attack(6)) won't kill them mid-test.
    fn cultist_at_enemy_turn_exec(initial_intent: Intent) -> CombatState {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            phase: CombatPhase::EnemyTurnExec,
            round: 1,
            ..Default::default()
        };
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        state.player.max_energy = 3;
        let id = NonZeroU32::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id,
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::ShrinkerBeetle),
            intent: Some(initial_intent),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("ShrinkerBeetle"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(id));
        state
    }

    // The `enemy_turn_exec_persists_intent_history_cultist_pattern`
    // test was removed in bead `sts2-rs-v1u5`. It pinned the STS1
    // Cultist multi-state-machine behavior (turn-0 = Buff, turn-1+ =
    // Attack 6) via `MonsterKind::roll_intent`; the STS2-native
    // replacement is DSL-driven and the no-AI placeholder no longer
    // models a multi-turn rotation.

    // The `enemy_turn_exec_persists_intent_history_acid_slime_alternation`
    // test was removed in bead `sts2-rs-v1u5` — it pinned the STS1
    // `AcidSlimeS` no-two-in-a-row alternation rule via
    // `MonsterKind::roll_intent`. The STS2-native `LeafSlimeS`
    // replacement is DSL-driven; the no-AI placeholder is
    // deterministic, so the alternation property no longer holds at
    // this layer.

    /// Locks the dispatcher invariant: `enemy_turn_exec` records the
    /// just-performed intent on the creature BEFORE rolling the next
    /// intent. If a future refactor flips the order (record after the
    /// roll), the next-roll would see stale history and we'd regress
    /// to the pre-fix bug. We verify by inspecting that after a single
    /// `enemy_turn_exec` call, the persisted history is exactly the
    /// intent that was just performed (`turn_index` == 1, `last_kind` ==
    /// the dispatched intent's kind).
    #[test]
    fn enemy_turn_exec_records_intent_history_before_next_roll() {
        let mut state = cultist_at_enemy_turn_exec(Intent::Attack { damage: 6 });

        let result = enemy_turn_exec(&mut state);
        assert_eq!(result, StepResult::Advanced);
        // Bead `sts2-rs-c8why`: perform (exec) records history; the next-intent
        // roll now fires in `enemy_turn_end`. History is recorded BEFORE the
        // roll regardless of which function holds the roll.
        enemy_turn_end(&mut state);

        // Post-condition: intent_history reflects exactly the intent
        // that was performed (Attack), not the intent that was rolled
        // for next turn.
        assert_eq!(state.creatures[0].intent_history.turn_index, 1);
        assert_eq!(
            state.creatures[0].intent_history.last_kind,
            Some(crate::IntentKindTag::Attack),
            "history records the performed intent, not the next-rolled one"
        );
        // The next-intent roll lands on the no-AI placeholder for
        // ShrinkerBeetle (Attack 5). The dispatcher contract under
        // test here is that history was recorded BEFORE the roll —
        // the exact placeholder value is incidental.
        assert_eq!(
            state.creatures[0].intent,
            Some(Intent::Attack { damage: 5 }),
            "next-intent roll saw the persisted history before firing",
        );
    }

    // -----------------------------------------------------------------
    // Bead `sts2-rs-j87` — runtime-wiring sentinel tests.
    //
    // Each test below registers a single sentinel listener on a
    // `relic_effects` channel via a unique RelicId (one per test so
    // tests don't collide on the global registry) and drives the
    // runtime through the call site that should fire that channel.
    // The handler bumps an `AtomicUsize` counter; the test asserts
    // the counter advanced >= 1.
    //
    // These tests lock the parallel `relic_effects::dispatch_*` calls
    // added next to every legacy hooks-family dispatch site so a future
    // refactor cannot silently drop them.
    //
    // The `after_obtained` and `after_room_entered` channels intentionally
    // remain unwired at this pin — RunState has no obtain-relic or
    // enter-room runtime today (still queue-only — see `runstate.rs`'s
    // `offer_relic_rewards`). Those wirings are filed as follow-up
    // sub-beads of `sts2-rs-j87`.
    //
    // The `after_damage_given` channel is also intentionally unwired at
    // this pin: `relic_effects::DamageGivenCtx` requires a
    // `CreatureUid` (NonZeroU32) for the dealer/target slots, but
    // `hooks::DamageCtx` uses `CreatureRef::{Player, Ally, Enemy(uid)}`
    // and `CreatureRef::Player` does not map cleanly to a `CreatureUid`
    // at this pin (the player is `state.player`, not a row in
    // `state.creatures`). The payload-translation work belongs in a
    // follow-up sub-bead.
    // -----------------------------------------------------------------

    use crate::card_effects::EffectResult;
    use crate::relic_effects::{
        self as re, AfterCardPlayedFn, AfterCombatEndFn, AfterCombatVictoryFn, AfterDamageGivenFn,
        AfterEnergyResetFn, AfterSideTurnStartFn, AfterTurnEndFn, BeforeCombatStartFn,
        BeforeSideTurnStartFn, BeforeTurnEndFn, CardEventCtx, DamageGivenCtx,
        ModifyDamageAdditiveFn, ModifyDamageCtx, ModifyHandDrawFn, ModifyHpLostCtx, ModifyHpLostFn,
    };
    use crate::relics::RelicId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static J87_BEFORE_COMBAT_START: AtomicUsize = AtomicUsize::new(0);
    static J87_BEFORE_SIDE_TURN_START: AtomicUsize = AtomicUsize::new(0);
    static J87_AFTER_SIDE_TURN_START_PLAYER: AtomicUsize = AtomicUsize::new(0);
    static J87_AFTER_SIDE_TURN_START_ENEMY: AtomicUsize = AtomicUsize::new(0);
    static J87_AFTER_ENERGY_RESET: AtomicUsize = AtomicUsize::new(0);
    static J87_BEFORE_TURN_END: AtomicUsize = AtomicUsize::new(0);
    static J87_AFTER_TURN_END: AtomicUsize = AtomicUsize::new(0);
    static J87_AFTER_COMBAT_END: AtomicUsize = AtomicUsize::new(0);
    static J87_AFTER_COMBAT_VICTORY: AtomicUsize = AtomicUsize::new(0);
    static J87_AFTER_CARD_PLAYED: AtomicUsize = AtomicUsize::new(0);
    static J87_MODIFY_HAND_DRAW: AtomicUsize = AtomicUsize::new(0);

    fn j87_h_before_combat_start(_s: &mut CombatState) -> EffectResult {
        J87_BEFORE_COMBAT_START.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }
    fn j87_h_before_side_turn_start(_s: &mut CombatState, _side: Side) -> EffectResult {
        J87_BEFORE_SIDE_TURN_START.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }
    fn j87_h_after_side_turn_start(_s: &mut CombatState, side: Side) -> EffectResult {
        match side {
            Side::Player => J87_AFTER_SIDE_TURN_START_PLAYER.fetch_add(1, Ordering::SeqCst),
            Side::Enemy => J87_AFTER_SIDE_TURN_START_ENEMY.fetch_add(1, Ordering::SeqCst),
        };
        EffectResult::Resolved
    }
    fn j87_h_after_energy_reset(_s: &mut CombatState) -> EffectResult {
        J87_AFTER_ENERGY_RESET.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }
    fn j87_h_before_turn_end(_s: &mut CombatState, _side: Side) -> EffectResult {
        J87_BEFORE_TURN_END.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }
    fn j87_h_after_turn_end(_s: &mut CombatState, _side: Side) -> EffectResult {
        J87_AFTER_TURN_END.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }
    fn j87_h_after_combat_end(_s: &mut CombatState) -> EffectResult {
        J87_AFTER_COMBAT_END.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }
    fn j87_h_after_combat_victory(_s: &mut CombatState) -> EffectResult {
        J87_AFTER_COMBAT_VICTORY.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }
    fn j87_h_after_card_played(_s: &mut CombatState, _ctx: &CardEventCtx) -> EffectResult {
        J87_AFTER_CARD_PLAYED.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }
    fn j87_h_modify_hand_draw(_s: &CombatState, base: i32) -> i32 {
        J87_MODIFY_HAND_DRAW.fetch_add(1, Ordering::SeqCst);
        // Return base unchanged: the relic_effects registry is
        // process-global; a non-zero fold would leak into every
        // unrelated `turn_start_draws_*` test that runs after this
        // one in the same process. The counter alone proves the
        // fold composition fires from the runtime.
        base
    }

    /// Construct a minimal cost-0 Self_-target blank POD for j87 sentinel
    /// tests. Uses `AscendersBane` as the card id so that if the default
    /// registry happens to be installed by a concurrently-running test (e.g.
    /// `nh5_modify_damage_ctx_carries_originating_card_id_on_player_strike`
    /// or `yva_all_enemies_target_routing_thunderclap_hits_three_enemies`),
    /// the dispatched handler is `play_noop` — which accepts `None` target
    /// safely — rather than `play_strike_ironclad`, which panics when called
    /// with `None`. Without the registry the dispatch returns
    /// `EffectResult::NotImplemented`, also safe. This makes the test
    /// order-independent under parallel execution (bead `sts2-rs-gc1j`).
    ///
    /// The previous `StrikeIronclad` id caused a parallel-test flake: any
    /// thread that ran `install_default_registry` before this test's
    /// `play_card` call would install `play_strike_ironclad`, which asserts
    /// a non-None target — a precondition violated by the `Self_` target type.
    fn j87_blank_strike(uid_u32: u32) -> crate::cards::Card {
        crate::cards::Card {
            uid: crate::CardUid::new(uid_u32).expect("non-zero"),
            card_id: crate::cards::CardId::AscendersBane,
            cost: 0,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::Self_,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        }
    }

    /// `before_combat_start` listener fires when `start_combat` runs.
    #[test]
    fn j87_before_combat_start_fires_from_start_combat() {
        J87_BEFORE_COMBAT_START.store(0, Ordering::SeqCst);
        re::register_before_combat_start(
            RelicId::BurningBlood,
            j87_h_before_combat_start as BeforeCombatStartFn,
        );
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        assert!(
            J87_BEFORE_COMBAT_START.load(Ordering::SeqCst) >= 1,
            "relic_effects::dispatch_before_combat_start did not fire from start_combat — \
             j87 runtime wiring regressed",
        );
    }

    /// `before_side_turn_start` listener fires when `start_player_turn` runs.
    #[test]
    fn j87_before_side_turn_start_fires_from_start_player_turn() {
        J87_BEFORE_SIDE_TURN_START.store(0, Ordering::SeqCst);
        re::register_before_side_turn_start(
            RelicId::RingOfTheSnake,
            j87_h_before_side_turn_start as BeforeSideTurnStartFn,
        );
        let mut state = state_with_cultists(1);
        // Bead `sts2-rs-m4jaj`: the relic dispatcher fires hooks in owned-relic
        // order, so the relic must be in `state.relics` to be invoked.
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::RingOfTheSnake,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);
        assert!(
            J87_BEFORE_SIDE_TURN_START.load(Ordering::SeqCst) >= 1,
            "relic_effects::dispatch_before_side_turn_start did not fire from start_player_turn",
        );
    }

    /// SturdyClamp's cap runs at the PREVENT site, before HornCleat's grant.
    ///
    /// C# order: `Creature.ClearBlock`'s else-arm fires
    /// `AfterPreventingBlockClear` (the clamp caps retained block to 10),
    /// and `CombatManager` later fires `AfterBlockCleared` unconditionally
    /// (HornCleat grants +14 on round 2). Grant-then-cap — both listeners
    /// on one bus in inventory order — capped the cleat's block too and sat
    /// the mirror at ≡10 while the world showed 24 (19 v34wr records).
    #[test]
    fn sturdy_clamp_caps_before_horn_cleat_grants() {
        crate::relic_effects::ensure_installed();
        let mut state = state_with_cultists(1);
        for (slot, kind) in [
            (1u32, crate::RelicKind::SturdyClamp),
            (2u32, crate::RelicKind::HornCleat),
        ] {
            state.relics.push(crate::Relic {
                id: std::num::NonZeroU32::new(slot).expect("nz"),
                kind,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);
        state.player.block = 24;
        state.round = 2;
        state.phase = crate::CombatPhase::PlayerTurnStart;
        state.side = crate::Side::Player;
        let _ = start_player_turn(&mut state);
        assert_eq!(
            state.player.block,
            10 + 14,
            "retained block capped to 10 FIRST, then HornCleat's round-2 +14"
        );
    }

    /// `after_side_turn_start` fires once per side-turn start (Player side).
    #[test]
    fn j87_after_side_turn_start_fires_on_player_side() {
        J87_AFTER_SIDE_TURN_START_PLAYER.store(0, Ordering::SeqCst);
        re::register_after_side_turn_start(
            RelicId::RingOfTheDrake,
            j87_h_after_side_turn_start as AfterSideTurnStartFn,
        );
        let mut state = state_with_cultists(1);
        // Bead `sts2-rs-b8z17`: the relic dispatcher fires hooks in owned-relic
        // EQUIP order, so the relic must be in `state.relics` to be invoked
        // (same contract as before_side_turn_start / m4jaj).
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::RingOfTheDrake,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);
        assert!(
            J87_AFTER_SIDE_TURN_START_PLAYER.load(Ordering::SeqCst) >= 1,
            "after_side_turn_start did not fire on Player side",
        );
    }

    /// `after_energy_reset` listener fires from `start_player_turn`.
    #[test]
    fn j87_after_energy_reset_fires_from_start_player_turn() {
        J87_AFTER_ENERGY_RESET.store(0, Ordering::SeqCst);
        re::register_after_energy_reset(
            RelicId::CrackedCore,
            j87_h_after_energy_reset as AfterEnergyResetFn,
        );
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);
        assert!(
            J87_AFTER_ENERGY_RESET.load(Ordering::SeqCst) >= 1,
            "after_energy_reset did not fire from start_player_turn",
        );
    }

    /// `before_turn_end` fires from `end_player_turn_phase_one`.
    #[test]
    fn j87_before_turn_end_fires_from_end_turn_phase_one() {
        J87_BEFORE_TURN_END.store(0, Ordering::SeqCst);
        re::register_before_turn_end(
            RelicId::InfusedCore,
            j87_h_before_turn_end as BeforeTurnEndFn,
        );
        let mut state = state_with_cultists(1);
        // Bead `sts2-rs-b8z17`: relic dispatchers fire owned relics only.
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::InfusedCore,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);
        let _ = end_player_turn_phase_one(&mut state);
        assert!(
            J87_BEFORE_TURN_END.load(Ordering::SeqCst) >= 1,
            "before_turn_end did not fire from end_player_turn_phase_one",
        );
    }

    /// `after_turn_end` fires from `end_player_turn_phase_two`.
    #[test]
    fn j87_after_turn_end_fires_from_end_turn_phase_two() {
        J87_AFTER_TURN_END.store(0, Ordering::SeqCst);
        re::register_after_turn_end(
            RelicId::BoundPhylactery,
            j87_h_after_turn_end as AfterTurnEndFn,
        );
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);
        let _ = end_player_turn_phase_one(&mut state);
        let _ = end_player_turn_phase_two(&mut state);
        assert!(
            J87_AFTER_TURN_END.load(Ordering::SeqCst) >= 1,
            "after_turn_end did not fire from end_player_turn_phase_two",
        );
    }

    /// `after_combat_end` fires from `check_combat_end` on player death.
    #[test]
    fn j87_after_combat_end_fires_on_player_death() {
        J87_AFTER_COMBAT_END.store(0, Ordering::SeqCst);
        re::register_after_combat_end(
            RelicId::DivineRight,
            j87_h_after_combat_end as AfterCombatEndFn,
        );
        let mut state = state_with_cultists(1);
        state.player.current_hp = 0;
        let result = check_combat_end(&mut state);
        assert_eq!(result, StepResult::CombatLost);
        assert!(
            J87_AFTER_COMBAT_END.load(Ordering::SeqCst) >= 1,
            "after_combat_end did not fire on player-death branch",
        );
    }

    /// `after_combat_victory` fires when all enemies are dead.
    #[test]
    fn j87_after_combat_victory_fires_on_all_enemies_dead() {
        J87_AFTER_COMBAT_VICTORY.store(0, Ordering::SeqCst);
        re::register_after_combat_victory(
            RelicId::PhylacteryUnbound,
            j87_h_after_combat_victory as AfterCombatVictoryFn,
        );
        let mut state = state_with_cultists(1);
        // Player must be alive — PlayerCombatState::default() leaves
        // current_hp == 0 which the death-branch short-circuit catches
        // before the victory branch runs.
        state.player.current_hp = 50;
        // Kill the only enemy so check_combat_end takes the victory branch.
        state.creatures[0].current_hp = 0;
        let result = check_combat_end(&mut state);
        assert_eq!(result, StepResult::CombatWon);
        assert!(
            J87_AFTER_COMBAT_VICTORY.load(Ordering::SeqCst) >= 1,
            "after_combat_victory did not fire on all-enemies-dead branch",
        );
    }

    /// `after_card_played` fires from `play_card` after the effect dispatch.
    #[test]
    fn j87_after_card_played_fires_from_play_card() {
        J87_AFTER_CARD_PLAYED.store(0, Ordering::SeqCst);
        re::register_after_card_played(
            RelicId::DivineDestiny,
            j87_h_after_card_played as AfterCardPlayedFn,
        );
        // Build a state where the player has one card in hand they can play.
        // The `play_card` pre-conditions (PlayerPlayPhase + non-empty hand
        // + energy) need to be satisfied; we set up a minimal POD card
        // with cost 0 + target Self_ (no enemy resolution).
        let mut state = state_with_cultists(1);
        // Bead `sts2-rs-b8z17`: relic dispatchers fire owned relics only.
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::DivineDestiny,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = start_combat(&mut state);
        let card = j87_blank_strike((state.cards.len() + 1) as u32);
        let new_uid = card.uid;
        state.cards.push(card);
        state.player.hand.push(new_uid);
        state.phase = CombatPhase::PlayerPlayPhase;
        let hand_idx = (state.player.hand.len() - 1) as u8;
        let _ = play_card(&mut state, hand_idx, None);
        assert!(
            J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst) >= 1,
            "after_card_played did not fire from play_card",
        );
    }

    /// `modify_hand_draw` listener participates in the runtime fold
    /// chain (hooks → `relic_effects`) in `start_player_turn`. Asserts the
    /// listener fires; the listener returns its input unchanged so the
    /// global registry doesn't leak draw-count deltas into the rest of
    /// the suite (the registry is process-global; only counters are
    /// safe to mutate from a listener body across-test).
    #[test]
    fn j87_modify_hand_draw_fold_fires_from_runtime() {
        J87_MODIFY_HAND_DRAW.store(0, Ordering::SeqCst);
        re::register_modify_hand_draw(
            RelicId::AmethystAubergine,
            j87_h_modify_hand_draw as ModifyHandDrawFn,
        );
        let mut state = state_with_cultists(1);
        // Seed the draw pile so the draw produces visible work — though
        // the test only asserts the listener counter, not hand size.
        for _ in 0..(BASE_DRAW + 2) {
            let card = j87_blank_strike((state.cards.len() + 1) as u32);
            let new_uid = card.uid;
            state.cards.push(card);
            state.player.draw.push(new_uid);
        }
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);
        assert!(
            J87_MODIFY_HAND_DRAW.load(Ordering::SeqCst) >= 1,
            "modify_hand_draw fold did not fire from start_player_turn — \
             the hooks → relic_effects fold composition regressed",
        );
    }

    // -----------------------------------------------------------------
    // Bead `sts2-rs-6jk` (j87-a) — sentinel tests for the three
    // payload-translation channels that j87 deferred: `after_damage_given`,
    // `modify_damage_additive`, and `modify_hp_lost`. With bead `sts2-rs-d03`
    // Phase 1 closed (player Creature row in `state.creatures` reachable via
    // `state.player.creature_uid`), the `CreatureRef::Player` → `CreatureUid`
    // translation is a real Option-resolving call (not a sentinel). Each
    // test below registers a sentinel listener on its channel via a unique
    // `RelicId`, drives `damage::apply_damage` through one player → cultist
    // hit, and asserts the listener counter advanced.
    // -----------------------------------------------------------------

    static J87A_AFTER_DAMAGE_GIVEN: AtomicUsize = AtomicUsize::new(0);
    static J87A_MODIFY_DAMAGE_ADDITIVE: AtomicUsize = AtomicUsize::new(0);
    static J87A_MODIFY_HP_LOST: AtomicUsize = AtomicUsize::new(0);

    fn j87a_h_after_damage_given(_s: &mut CombatState, _ctx: &DamageGivenCtx) -> EffectResult {
        J87A_AFTER_DAMAGE_GIVEN.fetch_add(1, Ordering::SeqCst);
        EffectResult::Resolved
    }

    fn j87a_h_modify_damage_additive(_s: &CombatState, _ctx: &ModifyDamageCtx) -> i32 {
        J87A_MODIFY_DAMAGE_ADDITIVE.fetch_add(1, Ordering::SeqCst);
        // Return 0 so the runtime numerical balance is unperturbed — the
        // global registry is process-shared with every other damage-routing
        // test in the suite. The counter alone proves the fold composes.
        0
    }

    fn j87a_h_modify_hp_lost(_s: &CombatState, _ctx: &ModifyHpLostCtx, current: i32) -> i32 {
        J87A_MODIFY_HP_LOST.fetch_add(1, Ordering::SeqCst);
        // Identity fold: don't perturb the running amount (process-global
        // registry shared with downstream damage tests).
        current
    }

    /// `relic_effects::dispatch_after_damage_given` fires from
    /// `damage::apply_damage` (player → enemy hit) once `state.player.
    /// creature_uid` resolves via d03's player-Creature initialization.
    #[test]
    fn j87a_after_damage_given_fires_from_apply_damage() {
        J87A_AFTER_DAMAGE_GIVEN.store(0, Ordering::SeqCst);
        re::register_after_damage_given(
            RelicId::FakeMerchantsRug,
            j87a_h_after_damage_given as AfterDamageGivenFn,
        );
        let mut state = state_with_cultists(1);
        // Bead `sts2-rs-b8z17`: relic dispatchers fire owned relics only.
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::FakeMerchantsRug,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = start_combat(&mut state);
        let cultist_uid = state.enemy_uids[0];
        let _ = crate::damage::apply_damage(
            &mut state,
            crate::hooks::CreatureRef::Player,
            cultist_uid,
            6,
            crate::hooks::ValueProps::MOVE,
            None,
        );
        assert!(
            J87A_AFTER_DAMAGE_GIVEN.load(Ordering::SeqCst) >= 1,
            "relic_effects::dispatch_after_damage_given did not fire from \
             damage::apply_damage — 6jk wiring regressed",
        );
    }

    /// `relic_effects::dispatch_modify_damage_additive` fires inside the
    /// `damage::apply_damage` pipeline (folded into `raw_damage` before
    /// `hooks::modify_damage`).
    #[test]
    fn j87a_modify_damage_additive_fires_from_apply_damage() {
        J87A_MODIFY_DAMAGE_ADDITIVE.store(0, Ordering::SeqCst);
        re::register_modify_damage_additive(
            RelicId::FestivePopper,
            j87a_h_modify_damage_additive as ModifyDamageAdditiveFn,
        );
        let mut state = state_with_cultists(1);
        // Bead `sts2-rs-b8z17`: relic dispatchers fire owned relics only.
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::FestivePopper,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = start_combat(&mut state);
        let cultist_uid = state.enemy_uids[0];
        let _ = crate::damage::apply_damage(
            &mut state,
            crate::hooks::CreatureRef::Player,
            cultist_uid,
            6,
            crate::hooks::ValueProps::MOVE,
            None,
        );
        assert!(
            J87A_MODIFY_DAMAGE_ADDITIVE.load(Ordering::SeqCst) >= 1,
            "relic_effects::dispatch_modify_damage_additive did not fire \
             from damage::apply_damage — 6jk wiring regressed",
        );
    }

    /// `relic_effects::dispatch_modify_hp_lost` fires inside the
    /// `damage::apply_damage` pipeline (folded AFTER `hooks::modify_hp_lost`
    /// when `unblocked > 0`). The cultist target has 0 block in the fixture
    /// so a `raw_damage=6` hit clears block trivially and the fold runs.
    #[test]
    fn j87a_modify_hp_lost_fires_from_apply_damage() {
        J87A_MODIFY_HP_LOST.store(0, Ordering::SeqCst);
        re::register_modify_hp_lost(RelicId::Permafrost, j87a_h_modify_hp_lost as ModifyHpLostFn);
        let mut state = state_with_cultists(1);
        // Bead `sts2-rs-b8z17`: relic dispatchers fire owned relics only.
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::Permafrost,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = start_combat(&mut state);
        let cultist_uid = state.enemy_uids[0];
        let _ = crate::damage::apply_damage(
            &mut state,
            crate::hooks::CreatureRef::Player,
            cultist_uid,
            6,
            crate::hooks::ValueProps::MOVE,
            None,
        );
        assert!(
            J87A_MODIFY_HP_LOST.load(Ordering::SeqCst) >= 1,
            "relic_effects::dispatch_modify_hp_lost did not fire from \
             damage::apply_damage — 6jk wiring regressed",
        );
    }

    // -----------------------------------------------------------------
    // Bead sts2-rs-4nh5 — originating CardId threads into ModifyDamageCtx.card.
    //
    // Plays an enemy-targeted Strike (Ironclad) via `play_card`. The
    // dispatch helper `deal_damage_player_to_enemy` auto-derives the
    // originating CardId from `state.player.play.last()` and passes it
    // down to `damage::apply_damage`, which populates
    // `ModifyDamageCtx.card`. A sentinel `modify_damage_additive` listener
    // captures the ctx and asserts `ctx.card == Some(StrikeIronclad)`.
    // This unblocks `FakeStrikeDummy`'s card-tag gate in 46ms.
    // -----------------------------------------------------------------

    static NH5_CAPTURED_CARD: std::sync::Mutex<Option<crate::cards::CardId>> =
        std::sync::Mutex::new(None);

    fn nh5_h_modify_damage_additive(_s: &CombatState, ctx: &ModifyDamageCtx) -> i32 {
        // Only capture the FIRST non-None ctx.card we see — sentinel
        // listeners from other tests may also drive this channel with
        // `card == None` (parity_harness, intent paths). We retain
        // whatever first card-bearing dispatch the test triggers.
        if let Some(card) = ctx.card {
            let mut slot = NH5_CAPTURED_CARD.lock().expect("poisoned");
            if slot.is_none() {
                *slot = Some(card);
            }
        }
        0
    }

    /// Build a Strike (Ironclad) POD that targets one enemy and deals 6
    /// base damage — distinct from `j87_blank_strike`'s Self_-target
    /// zero-damage stub so the dispatched `deal_damage_player_to_enemy`
    /// path actually fires.
    fn nh5_strike_ironclad(uid_u32: u32) -> crate::cards::Card {
        crate::cards::Card {
            uid: crate::CardUid::new(uid_u32).expect("non-zero"),
            card_id: crate::cards::CardId::StrikeIronclad,
            cost: 1,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        }
    }

    #[test]
    #[serial_test::serial(global_registry)]
    fn nh5_modify_damage_ctx_carries_originating_card_id_on_player_strike() {
        // Reset capture slot.
        *NH5_CAPTURED_CARD.lock().expect("poisoned") = None;

        // Install the sentinel listener on a unique RelicId so the
        // process-global registry doesn't collide with other tests.
        re::register_modify_damage_additive(
            RelicId::HappyFlower,
            nh5_h_modify_damage_additive as ModifyDamageAdditiveFn,
        );

        // Make sure the content-side card registry is installed so
        // `dispatch(StrikeIronclad, ...)` resolves to the real handler
        // that calls `deal_damage_player_to_enemy`.
        crate::card_effects::install_default_registry();

        let mut state = state_with_cultists(1);
        // Bead `sts2-rs-b8z17`: relic dispatchers fire owned relics only, so the
        // sentinel listener's relic (HappyFlower) must be equipped.
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::HappyFlower,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        let _ = start_combat(&mut state);
        // Make sure the player has energy to spend (Strike costs 1).
        state.player.energy = 3;

        let card = nh5_strike_ironclad((state.cards.len() + 1) as u32);
        let new_uid = card.uid;
        state.cards.push(card);
        state.player.hand.push(new_uid);
        state.phase = CombatPhase::PlayerPlayPhase;
        let hand_idx = (state.player.hand.len() - 1) as u8;

        let enemy_idx: u8 = 0; // first (and only) cultist
        let _ = play_card(&mut state, hand_idx, Some(enemy_idx));

        let captured = *NH5_CAPTURED_CARD.lock().expect("poisoned");
        assert_eq!(
            captured,
            Some(crate::cards::CardId::StrikeIronclad),
            "ModifyDamageCtx.card should carry the originating CardId on \
             a player-attack path — 4nh5 plumbing regressed",
        );
    }

    // -----------------------------------------------------------------
    // sts2-rs-yva — AllEnemies target routing smoke test.
    //
    // Plays an `AllEnemies` Thunderclap via `play_card` on a 3-enemy
    // board; asserts every alive enemy takes damage in a single play.
    // Regression guard for the `TargetType::AllEnemies` short-circuit
    // that this bead removed.
    // -----------------------------------------------------------------

    /// Build a Thunderclap POD that targets `AllEnemies` (4 base damage,
    /// per the Python pin `sts2_env/cards/ironclad.py`).
    fn yva_thunderclap(uid_u32: u32) -> crate::cards::Card {
        crate::cards::Card {
            uid: crate::CardUid::new(uid_u32).expect("non-zero"),
            card_id: crate::cards::CardId::Thunderclap,
            cost: 1,
            card_type: crate::cards::CardType::Attack,
            target_type: crate::cards::TargetType::AllEnemies,
            upgraded: false,
            base_damage: 4,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        }
    }

    /// Test-only handler for the `Thunderclap` `CardId` registered into
    /// the global dispatch table. Fans `base_damage` from the resolving
    /// card across `state.enemy_uids` — a hand-rolled mirror of
    /// `sts2-content::cards::ironclad::play_thunderclap` so this test
    /// stays in-crate (sts2-core can't depend on sts2-content).
    fn yva_h_thunderclap_aoe(
        state: &mut CombatState,
        _t: Option<CreatureUid>,
    ) -> crate::card_effects::EffectResult {
        let card_uid = *state
            .player
            .play
            .last()
            .expect("dispatch contract: card on Play pile");
        let idx = card_uid.0.get() as usize - 1;
        let dmg = state.cards[idx].base_damage;
        let uids: smallvec::SmallVec<[CreatureUid; 8]> = state.enemy_uids.iter().copied().collect();
        for uid in uids {
            let alive = state
                .creatures
                .iter()
                .any(|c| c.id == uid.0 && c.current_hp > 0);
            if alive {
                let _ = crate::card_effects::deal_damage_player_to_enemy(state, uid, dmg);
            }
        }
        crate::card_effects::EffectResult::Resolved
    }

    #[test]
    #[serial_test::serial(global_registry)]
    fn yva_all_enemies_target_routing_thunderclap_hits_three_enemies() {
        // The default registry has Strike/Defend/Bash/AscendersBane only.
        // Install a Thunderclap entry here so `dispatch(Thunderclap, …)`
        // resolves to a real handler when `play_card` runs.
        crate::card_effects::install_default_registry();
        crate::card_effects::register(
            crate::cards::CardId::Thunderclap,
            yva_h_thunderclap_aoe as crate::card_effects::PlayCardFn,
        );

        let mut state = state_with_cultists(3);
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        let _ = start_combat(&mut state);
        state.player.energy = 3;

        let card = yva_thunderclap((state.cards.len() + 1) as u32);
        let new_uid = card.uid;
        state.cards.push(card);
        state.player.hand.push(new_uid);
        state.phase = CombatPhase::PlayerPlayPhase;
        let hand_idx = (state.player.hand.len() - 1) as u8;

        // Snapshot per-enemy HP pre-play.
        let hp_before: Vec<i32> = state
            .enemy_uids
            .iter()
            .map(|&uid| {
                state
                    .creatures
                    .iter()
                    .find(|c| c.id == uid.0)
                    .expect("enemy present")
                    .current_hp
            })
            .collect();

        // `AllEnemies` dispatch passes `target = None`; the engine arm
        // (post-yva) no longer short-circuits.
        let _ = play_card(&mut state, hand_idx, None);

        let hp_after: Vec<i32> = state
            .enemy_uids
            .iter()
            .map(|&uid| {
                state
                    .creatures
                    .iter()
                    .find(|c| c.id == uid.0)
                    .expect("enemy present")
                    .current_hp
            })
            .collect();

        assert_eq!(hp_before.len(), 3, "test setup: 3-enemy board");
        for (i, (h0, h1)) in hp_before.iter().zip(hp_after.iter()).enumerate() {
            assert!(
                h0 - h1 >= 4,
                "enemy {i} took {} damage from Thunderclap; expected ≥ 4 \
                 — `TargetType::AllEnemies` dispatch may be short-circuiting",
                h0 - h1,
            );
        }

        // Energy was spent (cost 1) and the card cycled Hand → Discard.
        assert_eq!(state.player.energy, 2, "Thunderclap cost 1 not deducted");
        assert!(
            state.player.hand.iter().all(|&u| u != new_uid),
            "Thunderclap should have left the hand",
        );
        assert!(
            state.player.discard.contains(&new_uid),
            "Thunderclap should have landed in the discard pile",
        );
    }

    // -----------------------------------------------------------------
    // Bead `sts2-rs-zlx.5` — modify_card_cost surface integration tests.
    //
    // These verify that `play_card` reads the resolved cost via
    // `hooks::effective_card_cost` and consumes the appropriate Free*
    // stack when a free play lands.
    // -----------------------------------------------------------------

    /// Helper: build a cost-2 Skill (DefendIronclad-shaped) with the
    /// given uid. Self_-targeted, no damage/block — the dispatch
    /// registry treats this as `NotImplemented` and only the energy +
    /// pile plumbing runs, which is exactly what we want to inspect.
    fn zlx5_cost2_skill(uid_u32: u32) -> crate::cards::Card {
        // `MayhemCard` is intentionally NOT in the default dispatch
        // registry (see `cards::dispatch::unimplemented_card_returns_not_implemented`).
        // Picking it here means the card-effect dispatch path returns
        // `NotImplemented` regardless of whether the global registry
        // has been installed by a sibling test — the test asserts on
        // energy / power-list mutation only, which `play_card` does
        // before invoking the handler.
        crate::cards::Card {
            uid: crate::CardUid::new(uid_u32).expect("non-zero"),
            card_id: crate::cards::CardId::MayhemCard,
            cost: 2,
            card_type: crate::cards::CardType::Skill,
            target_type: crate::cards::TargetType::Self_,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        }
    }

    fn zlx5_cost2_attack(uid_u32: u32) -> crate::cards::Card {
        let mut c = zlx5_cost2_skill(uid_u32);
        c.card_type = crate::cards::CardType::Attack;
        c
    }

    fn zlx5_cost1_power(uid_u32: u32) -> crate::cards::Card {
        let mut c = zlx5_cost2_skill(uid_u32);
        c.cost = 1;
        c.card_type = crate::cards::CardType::Power;
        c
    }

    /// Bead `sts2-rs-3k85q`: Powers (CardType::Power) are removed from
    /// combat on play (PileType.NONE in Python pin
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1245`,
    /// `_default_card_play_result`), not routed to discard. Regression
    /// guard for the parity_fuzz discard_count +1 drift surfaced by
    /// sub-bug D (sts2-rs-6vj51): every INFLAME play in the
    /// 100-trial fuzz showed Rust discard_count = C# + 1 because the
    /// played Power was being pushed to discard.
    #[test]
    fn power_card_play_removes_card_from_all_piles() {
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;
        state.player.energy = 3;

        let pre_hand = state.player.hand.len();
        let pre_discard = state.player.discard.len();
        let pre_exhaust = state.player.exhaust.len();

        let pow = zlx5_cost1_power((state.cards.len() + 1) as u32);
        let pow_uid = pow.uid;
        state.cards.push(pow);
        state.player.hand.push(pow_uid);
        let hand_idx = (state.player.hand.len() - 1) as u8;
        let _ = play_card(&mut state, hand_idx, None);

        assert_eq!(
            state.player.hand.len(),
            pre_hand,
            "Power leaves hand (back to pre-push baseline)",
        );
        assert_eq!(
            state.player.discard.len(),
            pre_discard,
            "Power does NOT enter discard pile",
        );
        assert_eq!(
            state.player.exhaust.len(),
            pre_exhaust,
            "Power does NOT enter exhaust pile",
        );
        assert!(
            state.player.play.iter().all(|&u| u != pow_uid),
            "Power not lingering on play pile",
        );
        assert!(
            !state.player.discard.contains(&pow_uid),
            "Power uid not in discard pile",
        );
    }

    /// `Corruption` zeroes a Skill's energy spend at play time. Without
    /// the `modify_card_cost` wiring, `state.player.energy` would
    /// decrement by the raw `card.cost = 2`; with the channel live,
    /// the Skill should cost 0 and energy stays at the pre-play value.
    #[test]
    fn zlx5_corruption_play_card_spends_zero_energy_on_skill() {
        use crate::{Power, PowerKind};

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;
        state.player.energy = 3;

        // Install Corruption.
        let pid = std::num::NonZeroU32::new(99).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Corruption,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let card = zlx5_cost2_skill((state.cards.len() + 1) as u32);
        let new_uid = card.uid;
        state.cards.push(card);
        state.player.hand.push(new_uid);
        let energy_before = state.player.energy;

        let hand_idx = (state.player.hand.len() - 1) as u8;
        let _ = play_card(&mut state, hand_idx, None);

        assert_eq!(
            state.player.energy, energy_before,
            "Corruption: Skill should cost 0 — energy must not decrement",
        );
    }

    /// `FreeAttack(1)` zeroes the next Attack's cost AND decrements
    /// the power stack to 0, which removes the power from the
    /// player's listener list (mirrors Python `owner.powers.pop` at
    /// `card_play_effects.py:545`).
    #[test]
    fn zlx5_free_attack_consumes_on_play() {
        use crate::{Power, PowerKind};

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;
        state.player.energy = 3;

        let pid = std::num::NonZeroU32::new(77).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::FreeAttack,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let card = zlx5_cost2_attack((state.cards.len() + 1) as u32);
        let new_uid = card.uid;
        state.cards.push(card);
        state.player.hand.push(new_uid);
        let energy_before = state.player.energy;

        let hand_idx = (state.player.hand.len() - 1) as u8;
        let _ = play_card(&mut state, hand_idx, None);

        assert_eq!(
            state.player.energy, energy_before,
            "FreeAttack: Attack should cost 0",
        );
        // The power was at amount=1; after the play it should be
        // gone from the player's power list AND the arena.
        assert!(
            !state.player.powers.iter().any(|u| u.0 == pid),
            "FreeAttack should drop from player.powers when amount hits 0",
        );
        assert!(
            !state.powers.iter().any(|p| p.id == pid),
            "FreeAttack should drop from state.powers when amount hits 0",
        );
    }

    /// Bead `sts2-rs-09x5n`: the HAND-PLAY NostalgiaPower redirect
    /// (`play_card`'s routing block) must `cards::pin_top` the card it
    /// sends to draw-top, exactly like the auto-play resolver — a
    /// definite-position insert the player just watched happen.
    #[test]
    fn nostalgia_hand_play_redirect_pins_draw_top() {
        use crate::{Power, PowerKind};

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;
        state.player.energy = 3;

        let pid = std::num::NonZeroU32::new(55).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Nostalgia,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        // A plain (non-exhaust) Attack in hand — the first Attack/Skill
        // played this turn, so it falls within Nostalgia's Amount(1).
        let card = zlx5_cost2_attack((state.cards.len() + 1) as u32);
        let new_uid = card.uid;
        state.cards.push(card);
        state.player.hand.push(new_uid);

        let hand_idx = (state.player.hand.len() - 1) as u8;
        let _ = play_card(&mut state, hand_idx, None);

        assert_eq!(
            state.player.draw.first().copied(),
            Some(new_uid),
            "NostalgiaPower redirects the first-Amount hand play to draw-top"
        );
        assert!(!state.player.discard.contains(&new_uid));
        assert_eq!(
            state.player.pinned_top.first().copied(),
            Some(new_uid),
            "the hand-play Nostalgia redirect must pin Top(0)"
        );
    }

    /// q6bix F-R: while `Corruption` is active, played Skills route to
    /// Exhaust, not Discard (`CorruptionPower.ModifyCardPlayResultPileType
    /// AndPosition` → `PileType.Exhaust` for owner Skills). The Rust
    /// cleanup only consulted the card's own EXHAUST keyword, so a played
    /// Defend went to Discard (sweep PILE_COUNT|card:CORRUPTION_CARD
    /// exhaust c2/r1 discard c0/r1).
    #[test]
    fn q6bix_corruption_routes_played_skill_to_exhaust() {
        use crate::{Power, PowerKind};

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;
        state.player.energy = 3;

        let pid = std::num::NonZeroU32::new(88).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Corruption,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        // A plain (non-exhaust) Skill in hand.
        let card = zlx5_cost2_skill((state.cards.len() + 1) as u32);
        let new_uid = card.uid;
        state.cards.push(card);
        state.player.hand.push(new_uid);

        let hand_idx = (state.player.hand.len() - 1) as u8;
        let _ = play_card(&mut state, hand_idx, None);

        assert!(
            state.player.exhaust.contains(&new_uid),
            "Corruption routes the played Skill to Exhaust"
        );
        assert!(
            !state.player.discard.contains(&new_uid),
            "Corruption-exhausted Skill must not land in Discard"
        );
    }

    /// q6bix F-Q: a `FreeAttack` applied through the card path lands on
    /// the player's mirror creature (`state.creatures[player].powers`),
    /// NOT legacy `state.player.powers`. `consume_free_card_power` must
    /// walk the mirror+legacy union or the stack is never consumed
    /// (Unrelenting left a stray FreeAttack: sweep PLAYER_STATE c1/r2).
    #[test]
    fn q6bix_free_attack_consumes_from_mirror_power() {
        use crate::cards::CardType;
        use crate::PowerKind;

        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let player_uid = state.init_player_creature();
        crate::card_effects::apply_power_to_creature(
            &mut state,
            player_uid,
            PowerKind::FreeAttack,
            1,
        );
        // Sanity: applied to the mirror, invisible to legacy player.powers.
        assert!(
            state.player.powers.is_empty(),
            "card-applied FreeAttack lives on the mirror, not legacy"
        );

        let consumed = consume_free_card_power(&mut state, CardType::Attack);
        assert_eq!(consumed, Some(PowerKind::FreeAttack));
        // Consumed to 0 → dropped from the union entirely.
        assert_eq!(
            state.player_power_amount(PowerKind::FreeAttack),
            0,
            "FreeAttack must be removed from the mirror+legacy union"
        );
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::FreeAttack),
            "FreeAttack power struct must be removed from the arena"
        );
    }

    /// Default (no cost-mod power) state: energy decrements by the
    /// raw card cost. Locks the no-regression baseline for the new
    /// channel.
    #[test]
    fn zlx5_no_modifier_spends_raw_cost() {
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;
        state.player.energy = 3;

        let card = zlx5_cost2_skill((state.cards.len() + 1) as u32);
        let new_uid = card.uid;
        state.cards.push(card);
        state.player.hand.push(new_uid);
        let energy_before = state.player.energy;

        let hand_idx = (state.player.hand.len() - 1) as u8;
        let _ = play_card(&mut state, hand_idx, None);

        assert_eq!(
            i32::from(energy_before) - i32::from(state.player.energy),
            2,
            "default: cost-2 Skill should spend 2 energy",
        );
    }

    // ── EchoForm (bead `sts2-rs-8ett`) ──────────────────────────────────
    //
    // Verifies the first-in-series-driven echo on
    // `PlayerCombatState.cards_first_in_series_this_turn`: with amount=1, the
    // first card each turn fires `after_card_played` twice (one initial play
    // + one echo replay); the second card fires it once (its first-in-series
    // index 1 is no longer `< amount`). The counter counts UP and resets to 0
    // each turn.

    /// EchoForm(amount=1): first card plays twice, second plays once.
    #[test]
    fn echo_form_doubles_first_card_each_turn() {
        use crate::{Power, PowerKind};

        // Reset the j87 after_card_played counter and register the
        // sentinel listener on the relic_effects bus — `play_card`
        // re-dispatches that bus on each echo replay, so each
        // replay bumps the counter once.
        J87_AFTER_CARD_PLAYED.store(0, Ordering::SeqCst);
        re::register_after_card_played(
            RelicId::DivineDestiny,
            j87_h_after_card_played as AfterCardPlayedFn,
        );

        let mut state = state_with_cultists(1);
        // Bead `sts2-rs-b8z17`: the after_card_played fire-counter is a sentinel
        // listener on DivineDestiny, which now must be equipped to fire.
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::DivineDestiny,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = start_combat(&mut state);

        // Install EchoForm(amount=1) on the player. `start_player_turn`
        // resets `cards_first_in_series_this_turn` to 0; the echo decision
        // reads the live EchoForm amount per play.
        let pid = std::num::NonZeroU32::new(123).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::EchoForm,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);
        assert_eq!(
            state.player.cards_first_in_series_this_turn, 0,
            "start_player_turn should reset cards_first_in_series_this_turn to 0",
        );

        // Play card #1 — should fire after_card_played twice (1 + echo).
        let card1 = j87_blank_strike((state.cards.len() + 1) as u32);
        let card1_uid = card1.uid;
        state.cards.push(card1);
        state.player.hand.push(card1_uid);
        let hand_idx = (state.player.hand.len() - 1) as u8;
        let before = J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst);
        let _ = play_card(&mut state, hand_idx, None);
        let after_first = J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst);
        assert_eq!(
            after_first - before,
            2,
            "EchoForm(1): first card should fire after_card_played twice (initial + echo replay)",
        );
        assert_eq!(
            state.player.cards_first_in_series_this_turn, 1,
            "EchoForm(1): one first-in-series play recorded after the first card",
        );

        // Play card #2 — first-in-series index 1 is not < amount 1, no echo.
        let card2 = j87_blank_strike((state.cards.len() + 1) as u32);
        let card2_uid = card2.uid;
        state.cards.push(card2);
        state.player.hand.push(card2_uid);
        let hand_idx = (state.player.hand.len() - 1) as u8;
        let before = J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst);
        let _ = play_card(&mut state, hand_idx, None);
        let after_second = J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst);
        assert_eq!(
            after_second - before,
            1,
            "EchoForm(1): second card should fire after_card_played once (counter exhausted)",
        );
    }

    /// GH #368.1: the EchoForm budget must survive a wire round trip.
    ///
    /// `cards_first_in_series_this_turn` is not serialized, so a state rebuilt
    /// from a wire — which is what the combat mirror does at EVERY graded
    /// decision — read 0 and re-opened the budget. With EchoForm(1) up, every
    /// card played after a reconcile was played twice: on the DLL arm, seed
    /// 1003's Cold Snap and Strike each hit TheObscura for 6 twice while the
    /// world hit once (the `+6` enemy-HP class this issue is about).
    ///
    /// Simulated here by zeroing the plain field, which is exactly what
    /// `state_from_dict` does. The `Power::counter` half is what must hold the
    /// line — and it does cross the wire, via the mirror's undisclosed-counter
    /// carry.
    #[test]
    fn echo_form_budget_is_not_reopened_by_a_wire_rebuild() {
        use crate::{Power, PowerKind};

        // Same sentinel wiring as `echo_form_doubles_first_card_each_turn`:
        // the fire-counter is a listener on DivineDestiny, so the relic has to
        // be both registered and equipped for a play to be counted at all.
        J87_AFTER_CARD_PLAYED.store(0, Ordering::SeqCst);
        re::register_after_card_played(
            RelicId::DivineDestiny,
            j87_h_after_card_played as AfterCardPlayedFn,
        );

        let mut state = state_with_cultists(1);
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::DivineDestiny,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = start_combat(&mut state);

        let pid = std::num::NonZeroU32::new(125).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::EchoForm,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));
        let _ = start_player_turn(&mut state);

        // Card #1 spends the budget: initial play + one echo.
        let card1 = j87_blank_strike((state.cards.len() + 1) as u32);
        let uid1 = card1.uid;
        state.cards.push(card1);
        state.player.hand.push(uid1);
        let idx = (state.player.hand.len() - 1) as u8;
        let before = J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst);
        let _ = play_card(&mut state, idx, None);
        assert_eq!(
            J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst) - before,
            2,
            "EchoForm(1) echoes the first card of the turn",
        );

        // THE RECONSTRUCTION. `state_from_dict` defaults this field to 0; the
        // power's counter is carried, so the budget must stay closed.
        state.player.cards_first_in_series_this_turn = 0;
        assert_eq!(
            echo_form_first_in_series_this_turn(&state),
            1,
            "the carried per-power counter must survive the field reset",
        );

        let card2 = j87_blank_strike((state.cards.len() + 1) as u32);
        let uid2 = card2.uid;
        state.cards.push(card2);
        state.player.hand.push(uid2);
        let idx = (state.player.hand.len() - 1) as u8;
        let before = J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst);
        let _ = play_card(&mut state, idx, None);
        assert_eq!(
            J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst) - before,
            1,
            "a card played after a wire rebuild must NOT be echoed again",
        );
    }

    /// The first-in-series counter resets to 0 at side-turn-start, so the
    /// EchoForm budget refreshes each turn (the first `Amount` cards are
    /// echoable again).
    #[test]
    fn echo_form_counter_resets_at_turn_start() {
        use crate::{Power, PowerKind};

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let pid = std::num::NonZeroU32::new(124).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::EchoForm,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);
        assert_eq!(
            state.player.cards_first_in_series_this_turn, 0,
            "turn-start should reset the first-in-series counter to 0",
        );

        // Simulate several first-in-series plays this turn.
        state.player.cards_first_in_series_this_turn = 5;

        // Move through the turn lifecycle back to PlayerTurnStart so a
        // second `start_player_turn` can run. We bypass the
        // enemy-turn execution path (which would need monster AI) by
        // resetting the phase + side directly to the turn-start
        // boundary the runtime would otherwise reach.
        state.phase = CombatPhase::PlayerTurnStart;
        state.round = 2;

        let _ = start_player_turn(&mut state);
        assert_eq!(
            state.player.cards_first_in_series_this_turn, 0,
            "the next turn-start should reset the counter back to 0",
        );
    }

    /// EchoForm(amount=2): the first TWO cards each get exactly one extra
    /// play; the third card is NOT echoed. Regression for bead
    /// `sts2-rs-8ett` — the prior `while echo_form_remaining > 0` loop dumped
    /// BOTH extra plays onto the first card (3 plays) and left the second
    /// card un-echoed (Defect sweep `PLAYER_STATE|THE_BOMB` seed 1510: two
    /// THE_BOMB cards = EchoForm(2), C# added 2 Bomb instances, Rust added 3).
    #[test]
    fn echo_form_amount_two_doubles_first_two_cards() {
        use crate::{Power, PowerKind};

        J87_AFTER_CARD_PLAYED.store(0, Ordering::SeqCst);
        re::register_after_card_played(
            RelicId::DivineDestiny,
            j87_h_after_card_played as AfterCardPlayedFn,
        );

        let mut state = state_with_cultists(1);
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::DivineDestiny,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = start_combat(&mut state);

        let pid = std::num::NonZeroU32::new(125).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::EchoForm,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));
        let _ = start_player_turn(&mut state);

        let fire_for_next_card = |state: &mut CombatState| {
            let card = j87_blank_strike((state.cards.len() + 1) as u32);
            let uid = card.uid;
            state.cards.push(card);
            state.player.hand.push(uid);
            let hand_idx = (state.player.hand.len() - 1) as u8;
            let before = J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst);
            let _ = play_card(state, hand_idx, None);
            J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst) - before
        };

        assert_eq!(
            fire_for_next_card(&mut state),
            2,
            "card #1 echoed (1 + echo)"
        );
        assert_eq!(
            fire_for_next_card(&mut state),
            2,
            "card #2 echoed (1 + echo)"
        );
        assert_eq!(
            fire_for_next_card(&mut state),
            1,
            "card #3 NOT echoed — budget of 2 distinct cards exhausted",
        );
    }

    /// EchoForm GAINED mid-turn echoes the cards played AFTER it. Regression
    /// for bead `sts2-rs-8ett` — the prior turn-start budget snapshot read 0
    /// (no EchoForm at turn start) so cards played after an Echo Form played
    /// that same turn never doubled (Defect sweep `ORB_STATE|CHILL` seed 3434
    /// / `ORB_STATE|HOTFIX` seed 819). The capture reads the LIVE amount per
    /// play, so a card played after the EchoForm grant is echoed.
    #[test]
    fn echo_form_gained_midturn_echoes_subsequent_cards() {
        use crate::{Power, PowerKind};

        J87_AFTER_CARD_PLAYED.store(0, Ordering::SeqCst);
        re::register_after_card_played(
            RelicId::DivineDestiny,
            j87_h_after_card_played as AfterCardPlayedFn,
        );

        let mut state = state_with_cultists(1);
        state.relics.push(crate::Relic {
            id: std::num::NonZeroU32::new(1).expect("nz"),
            kind: crate::RelicKind::DivineDestiny,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = start_combat(&mut state);
        // No EchoForm at turn start.
        let _ = start_player_turn(&mut state);

        // One first-in-series play happens BEFORE EchoForm exists.
        let card1 = j87_blank_strike((state.cards.len() + 1) as u32);
        let uid1 = card1.uid;
        state.cards.push(card1);
        state.player.hand.push(uid1);
        let idx1 = (state.player.hand.len() - 1) as u8;
        let _ = play_card(&mut state, idx1, None);
        assert_eq!(
            state.player.cards_first_in_series_this_turn, 1,
            "one first-in-series play recorded",
        );

        // Now EchoForm(3) is gained mid-turn (as a power card's OnPlay would).
        let pid = std::num::NonZeroU32::new(126).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::EchoForm,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let card2 = j87_blank_strike((state.cards.len() + 1) as u32);
        let uid2 = card2.uid;
        state.cards.push(card2);
        state.player.hand.push(uid2);
        let idx2 = (state.player.hand.len() - 1) as u8;
        let before = J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst);
        let _ = play_card(&mut state, idx2, None);
        assert_eq!(
            J87_AFTER_CARD_PLAYED.load(Ordering::SeqCst) - before,
            2,
            "card after a mid-turn EchoForm gain is echoed (first-in-series \
             index 1 < live amount 3)",
        );
    }

    // ──────────────────────────────────────────────────────────────────
    // Bead `sts2-rs-c68o` — FreeThisTurn cleanup wiring
    // ──────────────────────────────────────────────────────────────────

    /// `clear_per_turn_card_vars` resets `VarKey::FreeThisTurn` to 0 on
    /// every card in the arena (regardless of pile membership), so a
    /// card spawned by `InfernalBlade` that survives into the next turn
    /// reverts to its base cost. Bead `sts2-rs-c68o`.
    #[test]
    fn clear_per_turn_card_vars_clears_freethisturn_flag() {
        use crate::cards::{Card, CardId, CardType, TargetType, VarKey};
        use crate::CardUid;

        let mut state = CombatState::default();
        // Two cards: one with FreeThisTurn=1, one untouched.
        let uid_a = CardUid::new(1).expect("nz");
        let uid_b = CardUid::new(2).expect("nz");
        let mk = |uid: CardUid, free: i32| -> Card {
            let mut c = Card {
                uid,
                card_id: CardId::StrikeIronclad,
                cost: 1,
                card_type: CardType::Attack,
                target_type: TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..Card::pod_defaults()
            };
            if free != 0 {
                c.set_combat_var(VarKey::FreeThisTurn, free);
            }
            c
        };
        state.cards.push(mk(uid_a, 1));
        state.cards.push(mk(uid_b, 0));

        assert_eq!(state.cards[0].combat_var(VarKey::FreeThisTurn), 1);
        assert_eq!(state.cards[1].combat_var(VarKey::FreeThisTurn), 0);

        clear_per_turn_card_vars(&mut state);

        assert_eq!(
            state.cards[0].combat_var(VarKey::FreeThisTurn),
            0,
            "FreeThisTurn=1 card must be cleared to 0",
        );
        assert_eq!(
            state.cards[1].combat_var(VarKey::FreeThisTurn),
            0,
            "FreeThisTurn=0 card stays 0 (no-op)",
        );
    }

    /// Bead `sts2-rs-r6l9`: playing cards bumps the per-CardType
    /// bucket on `PlayerCombatState::cards_played_this_turn`, the
    /// `count_player_cards_played_this_turn` accessor returns the
    /// matching slot, and the buckets reset to zero at the start of
    /// the next player turn.
    ///
    /// Mirrors Python's `_played_cards_this_turn` append at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:931` plus the
    /// turn-boundary reset at `combat.py:678`. This is the surface
    /// `LunarBlast` (hits = skills played this turn) consumes.
    #[test]
    fn r6l9_cards_played_this_turn_increments_per_type_and_resets() {
        use crate::cards::CardType;

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;
        state.player.energy = 9;

        // Sanity: empty buckets at turn start.
        assert_eq!(state.count_player_cards_played_this_turn(None), 0);
        assert_eq!(
            state.count_player_cards_played_this_turn(Some(CardType::Skill)),
            0,
        );

        // Play one Skill (Mayhem, registry-unimplemented — we only
        // assert on the play-card side-effects, which run before card
        // dispatch).
        let skill = zlx5_cost2_skill((state.cards.len() + 1) as u32);
        let skill_uid = skill.uid;
        state.cards.push(skill);
        state.player.hand.push(skill_uid);
        let hand_idx = (state.player.hand.len() - 1) as u8;
        let _ = play_card(&mut state, hand_idx, None);

        // Play one Attack (cost-2 Mayhem-shaped Attack).
        let atk = zlx5_cost2_attack((state.cards.len() + 1) as u32);
        let atk_uid = atk.uid;
        state.cards.push(atk);
        state.player.hand.push(atk_uid);
        let hand_idx = (state.player.hand.len() - 1) as u8;
        let _ = play_card(&mut state, hand_idx, None);

        assert_eq!(
            state.count_player_cards_played_this_turn(Some(CardType::Skill)),
            1,
            "one Skill play this turn",
        );
        assert_eq!(
            state.count_player_cards_played_this_turn(Some(CardType::Attack)),
            1,
            "one Attack play this turn",
        );
        assert_eq!(
            state.count_player_cards_played_this_turn(Some(CardType::Power)),
            0,
            "no Power played this turn",
        );
        assert_eq!(
            state.count_player_cards_played_this_turn(None),
            2,
            "card_type=None sums all buckets",
        );

        // Drive a turn boundary: directly invoke start_player_turn —
        // this exercises the reset path without depending on the rest
        // of the end-turn pipeline (which can short-circuit on a dead
        // board / pending enemy moves and leave the buckets stale).
        state.phase = CombatPhase::PlayerTurnStart;
        state.is_over = false;
        let _ = start_player_turn(&mut state);

        assert_eq!(
            state.count_player_cards_played_this_turn(None),
            0,
            "buckets reset at the start of the next player turn",
        );
        assert_eq!(
            state.count_player_cards_played_this_turn(Some(CardType::Skill)),
            0,
        );
        assert_eq!(
            state.count_player_cards_played_this_turn(Some(CardType::Attack)),
            0,
        );
    }

    /// Bead `sts2-rs-x646`: the per-turn stars-gained tally on
    /// `PlayerCombatState::stars_gained_this_turn` increments via
    /// direct mutation, is summed by
    /// `count_player_stars_gained_this_turn`, and resets to zero at
    /// the start of the next player turn.
    ///
    /// Mirrors Python's `_stars_gained_this_turn` lifecycle at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py` (init line
    /// 132, reset lines 646/677, append inside `gain_stars` line
    /// 1453, summed by `count_stars_gained_this_turn` lines
    /// 1550-1551). At this Rust pin no `gain_stars` runtime exists
    /// in `sts2-core` (the helper lives in
    /// `sts2-content::cards::regent`); the field is also bumped
    /// inside `relic_effects::on_after_side_turn_start_divine_right`
    /// for the DivineRight short-circuit. This test is therefore a
    /// scaffolding sentinel — direct mutation + the
    /// turn-boundary reset path. `gain_stars` parity for content
    /// listeners is covered by the regent / DivineRight bead suites.
    #[test]
    fn x646_stars_gained_this_turn_increments_and_resets() {
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;

        // Sanity: zero at turn start.
        assert_eq!(state.count_player_stars_gained_this_turn(), 0);

        // Direct mutation (the increment path used by both
        // `cards::regent::gain_stars` and the DivineRight
        // short-circuit in `relic_effects`).
        state.player.stars_gained_this_turn = state.player.stars_gained_this_turn.saturating_add(2);
        state.player.stars_gained_this_turn = state.player.stars_gained_this_turn.saturating_add(3);

        assert_eq!(
            state.count_player_stars_gained_this_turn(),
            5,
            "two gains (2 + 3) sum to 5",
        );

        // Drive a turn boundary directly: invoke start_player_turn —
        // exercises the reset path without needing a full end-turn
        // pipeline (which can short-circuit on a dead board).
        state.phase = CombatPhase::PlayerTurnStart;
        state.is_over = false;
        let _ = start_player_turn(&mut state);

        assert_eq!(
            state.count_player_stars_gained_this_turn(),
            0,
            "stars_gained_this_turn resets at the start of the next player turn",
        );
    }

    /// Bead `sts2-rs-jm6e` (ozy.4-rider): each powered player→enemy
    /// hit dealt by `crate::damage::apply_damage` bumps
    /// `PlayerCombatState::powered_hits_this_turn`, the
    /// `count_player_powered_hits_this_turn` accessor returns the
    /// running total, an `UNPOWERED`-tagged hit (Poison-style tick)
    /// does NOT increment, and the counter resets to zero at the
    /// start of the next player turn.
    ///
    /// Mirrors Python's `_damage_events_this_turn` append at
    /// `sts2-rl-agent@01c094d:sts2_env/core/damage.py:161` plus the
    /// `props.is_powered()` filter in `combat.py:1519-1531` and the
    /// turn-boundary clear at `combat.py:678`. This is the surface
    /// `BeatIntoShape` (`sts2-rl-agent@01c094d:sts2_env/cards/
    /// regent.py:599-607`) consumes via
    /// `combat.count_powered_hits_this_turn(owner, target)`.
    #[test]
    fn jm6e_powered_hits_this_turn_increments_on_player_hit_and_resets() {
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        let cultist_uid = state.enemy_uids[0];

        // Sanity: empty counter at turn start.
        assert_eq!(state.count_player_powered_hits_this_turn(), 0);

        // First powered player→enemy hit: counter goes 0 → 1.
        let _ = crate::damage::apply_damage(
            &mut state,
            crate::hooks::CreatureRef::Player,
            cultist_uid,
            6,
            crate::hooks::ValueProps::MOVE,
            None,
        );
        assert_eq!(
            state.count_player_powered_hits_this_turn(),
            1,
            "one powered player hit this turn",
        );

        // Second powered player→enemy hit: counter goes 1 → 2.
        let _ = crate::damage::apply_damage(
            &mut state,
            crate::hooks::CreatureRef::Player,
            cultist_uid,
            6,
            crate::hooks::ValueProps::MOVE,
            None,
        );
        assert_eq!(
            state.count_player_powered_hits_this_turn(),
            2,
            "two powered player hits this turn",
        );

        // UNPOWERED hit (Poison-style tick): is_powered() is false
        // because the UNPOWERED bit is set, so the counter MUST
        // stay at 2. Python `combat.py:1525` filters on
        // `props.is_powered()` — the same predicate gates the
        // increment here.
        let _ = crate::damage::apply_damage(
            &mut state,
            crate::hooks::CreatureRef::Player,
            cultist_uid,
            3,
            crate::hooks::ValueProps::UNBLOCKABLE.union(crate::hooks::ValueProps::UNPOWERED),
            None,
        );
        assert_eq!(
            state.count_player_powered_hits_this_turn(),
            2,
            "UNPOWERED hit does not increment the powered-hits counter",
        );

        // Drive a turn boundary via `start_player_turn` (matches the
        // r6l9 sentinel — exercises just the reset path, no end-turn
        // pipeline interactions).
        state.phase = CombatPhase::PlayerTurnStart;
        state.is_over = false;
        let _ = start_player_turn(&mut state);

        assert_eq!(
            state.count_player_powered_hits_this_turn(),
            0,
            "powered-hits counter resets at the start of the next player turn",
        );
    }

    /// Issue #177: the per-target powered-hits tally
    /// (`PlayerCombatState::powered_hits_by_target_this_turn`, read via
    /// `count_player_powered_hits_on_target_this_turn`) keys the count by
    /// receiver — hits on enemy A do NOT show up under enemy B — and
    /// resets at the start of the next player turn alongside the
    /// combat-wide scalar. This is the surface `BeatIntoShape` consumes so
    /// its forge multiplier matches C# `BeatIntoShape.cs:27-31`'s
    /// `e.Receiver == target` filter in a multi-enemy fight.
    #[test]
    fn jm6e_per_target_powered_hits_are_independent_and_reset() {
        let mut state = state_with_cultists(2);
        let _ = start_combat(&mut state);
        let enemy_a = state.enemy_uids[0];
        let enemy_b = state.enemy_uids[1];

        // Two powered hits on A, one on B.
        for _ in 0..2 {
            let _ = crate::damage::apply_damage(
                &mut state,
                crate::hooks::CreatureRef::Player,
                enemy_a,
                4,
                crate::hooks::ValueProps::MOVE,
                None,
            );
        }
        let _ = crate::damage::apply_damage(
            &mut state,
            crate::hooks::CreatureRef::Player,
            enemy_b,
            4,
            crate::hooks::ValueProps::MOVE,
            None,
        );

        assert_eq!(
            state.count_player_powered_hits_on_target_this_turn(enemy_a),
            2,
            "two powered hits keyed to enemy A",
        );
        assert_eq!(
            state.count_player_powered_hits_on_target_this_turn(enemy_b),
            1,
            "one powered hit keyed to enemy B (independent of A)",
        );
        // Combat-wide scalar still totals all targets.
        assert_eq!(
            state.count_player_powered_hits_this_turn(),
            3,
            "combat-wide scalar is the sum across targets",
        );

        // UNPOWERED hit does not increment the per-target tally either.
        let _ = crate::damage::apply_damage(
            &mut state,
            crate::hooks::CreatureRef::Player,
            enemy_b,
            3,
            crate::hooks::ValueProps::UNBLOCKABLE.union(crate::hooks::ValueProps::UNPOWERED),
            None,
        );
        assert_eq!(
            state.count_player_powered_hits_on_target_this_turn(enemy_b),
            1,
            "UNPOWERED hit does not bump the per-target tally",
        );

        // Turn boundary clears the per-target tally.
        state.phase = CombatPhase::PlayerTurnStart;
        state.is_over = false;
        let _ = start_player_turn(&mut state);
        assert_eq!(
            state.count_player_powered_hits_on_target_this_turn(enemy_a),
            0,
            "per-target tally resets at the next player turn",
        );
        assert_eq!(
            state.count_player_powered_hits_on_target_this_turn(enemy_b),
            0,
            "per-target tally resets at the next player turn",
        );
    }

    // -----------------------------------------------------------------
    // Bead `sts2-rs-n4wd` — X-cost `energy_spent` surface.
    //
    // `play_card` records the available energy on the played
    // `CardInstance.energy_spent` and drains `state.player.energy`
    // to 0 *before* dispatching the card body. This mirrors Python's
    // X-cost path in `_execute_card_play` and is the canonical way
    // X-cost handlers (Skewer / Tempest / Whirlwind / MultiCast /
    // HeavenlyDrill) read the spend.
    // -----------------------------------------------------------------

    /// Helper: build a Self_-targeted X-cost Skill (cost = -1) with the
    /// given uid. The card has no registered handler so dispatch
    /// returns `NotImplemented` and only the energy / pile plumbing
    /// runs — exactly what we need to assert the X-cost surface.
    fn n4wd_x_cost_skill(uid_u32: u32) -> crate::cards::Card {
        crate::cards::Card {
            uid: crate::CardUid::new(uid_u32).expect("non-zero"),
            // `MayhemCard` is intentionally not in the default registry
            // (matches the `zlx5_cost2_skill` rationale above).
            card_id: crate::cards::CardId::MayhemCard,
            cost: -1,
            card_type: crate::cards::CardType::Skill,
            target_type: crate::cards::TargetType::Self_,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            combat_vars: crate::cards::CombatVars::new(),
            enchantments: crate::cards::CardEnchantments::new(),
            tags: crate::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        }
    }

    /// X-cost play with energy=3 → `energy_spent` set to 3, player
    /// energy drained to 0.
    #[test]
    fn n4wd_x_cost_records_energy_spent_and_drains_energy() {
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;
        state.player.energy = 3;

        let card = n4wd_x_cost_skill((state.cards.len() + 1) as u32);
        let new_uid = card.uid;
        state.cards.push(card);
        state.player.hand.push(new_uid);
        let hand_idx = (state.player.hand.len() - 1) as u8;

        let _ = play_card(&mut state, hand_idx, None);

        let idx = new_uid.0.get() as usize - 1;
        assert_eq!(
            state.cards[idx].energy_spent, 3,
            "X-cost: energy_spent should be set to the pre-play energy (3)",
        );
        assert_eq!(
            state.player.energy, 0,
            "X-cost: player energy should be drained to 0",
        );
    }

    /// X-cost with energy=0 (player drained from prior plays) → spend
    /// is 0, no underflow, card resolves cleanly.
    #[test]
    fn n4wd_x_cost_with_zero_energy_no_underflow() {
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;
        state.player.energy = 0;

        let card = n4wd_x_cost_skill((state.cards.len() + 1) as u32);
        let new_uid = card.uid;
        state.cards.push(card);
        state.player.hand.push(new_uid);
        let hand_idx = (state.player.hand.len() - 1) as u8;

        let _ = play_card(&mut state, hand_idx, None);

        let idx = new_uid.0.get() as usize - 1;
        assert_eq!(state.cards[idx].energy_spent, 0);
        assert_eq!(state.player.energy, 0);
    }

    /// GH #741: a non-X-cost card stamps the energy it paid into
    /// `energy_spent` (C# `PlayCardAction` sets `EnergyValue = EnergySpent =
    /// item` for every play), so late effects such as RightHandHand's see it.
    #[test]
    fn n4wd_non_x_cost_records_the_energy_paid() {
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.phase = CombatPhase::PlayerPlayPhase;
        state.player.energy = 3;

        let card = zlx5_cost2_skill((state.cards.len() + 1) as u32);
        let new_uid = card.uid;
        state.cards.push(card);
        state.player.hand.push(new_uid);
        let hand_idx = (state.player.hand.len() - 1) as u8;

        let _ = play_card(&mut state, hand_idx, None);

        let idx = new_uid.0.get() as usize - 1;
        assert_eq!(
            state.cards[idx].energy_spent, 2,
            "a cost-2 card records the 2 energy it paid",
        );
        assert_eq!(
            state.player.energy, 1,
            "non-X-cost cost-2 spend should leave 3-2 = 1 energy",
        );
    }

    // ── 6ynt.3.1 turn-end-in-hand effects ──────────────────────────────

    use crate::cards::{Card, CardId, CardType, TargetType};

    /// Build a curse `Card` with the given uid + id. Per-card scalars
    /// (cost, base_damage, ...) are intentionally zeroed: the
    /// turn-end-in-hand dispatcher reads only `card_id`. The Card fields
    /// match the status / curse specs in
    /// `crates/sts2-content/src/cards/{status,curse}_specs_generated.rs`
    /// closely enough for the predicate filter — exact scalar parity is
    /// outside the scope of this helper.
    fn mk_curse(uid: CardUid, id: CardId) -> Card {
        Card {
            uid,
            card_id: id,
            cost: 0,
            card_type: CardType::Curse,
            target_type: TargetType::None,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            ..Card::pod_defaults()
        }
    }

    /// Build a `CombatState` already in `PlayerEndTurnPhaseOne` with one
    /// curse card in hand. Player has 50/50 HP, no block, no powers.
    /// `state.cards` is populated so the predicate filter resolves the
    /// uid; `init_player_creature` is called so the player has a uid
    /// (required by `apply_damage_to_player` + `apply_power_to_creature`).
    fn state_with_curse_in_hand(curse: CardId) -> CombatState {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerEndTurnPhaseOne;
        state.round = 1;
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        let _ = state.init_player_creature();
        let uid = CardUid::new(1).expect("non-zero");
        state.cards.push(mk_curse(uid, curse));
        state.player.hand.push(uid);
        state
    }

    /// BURN: 2 self-damage at turn end, absorbable by block. Card moves
    /// Hand → Play → Discard. Mirrors Python `combat.py:2381-2389`.
    #[test]
    fn turn_end_burn_deals_two_self_damage() {
        let mut state = state_with_curse_in_hand(CardId::Burn);
        let hp_before = state.player.current_hp;

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.player.current_hp, hp_before - 2);
        // Card landed in discard (curse is non-ethereal) and play pile is empty.
        assert_eq!(state.player.hand.len(), 0);
        assert_eq!(state.player.play.len(), 0);
        assert_eq!(state.player.discard.len(), 1);
    }

    /// BURN with 5 block absorbs the 2 self-damage (UNPOWERED|MOVE, not
    /// UNBLOCKABLE).
    #[test]
    fn turn_end_burn_is_blockable() {
        let mut state = state_with_curse_in_hand(CardId::Burn);
        state.player.block = 5;
        let hp_before = state.player.current_hp;

        end_player_turn_phase_two(&mut state);

        // BURN's 2 damage absorbed by the 5 block; HP unchanged.
        assert_eq!(state.player.current_hp, hp_before);
    }

    /// DECAY: identical shape to BURN — 2 self-damage, blockable.
    #[test]
    fn turn_end_decay_deals_two_self_damage() {
        let mut state = state_with_curse_in_hand(CardId::Decay);
        let hp_before = state.player.current_hp;

        end_player_turn_phase_two(&mut state);

        assert_eq!(state.player.current_hp, hp_before - 2);
    }

    /// TOXIC: 5 self-damage, blockable (C# `Toxic.OnTurnEndInHand`,
    /// DamageVar(5m, Unpowered|Move)). Previously a no-op — the card's
    /// turn-end-in-hand effect was never wired.
    #[test]
    fn turn_end_toxic_deals_five_self_damage() {
        let mut state = state_with_curse_in_hand(CardId::Toxic);
        let hp_before = state.player.current_hp;

        end_player_turn_phase_two(&mut state);

        assert_eq!(state.player.current_hp, hp_before - 5);
    }

    /// TOXIC with 5 block absorbs the 5 self-damage (no UNBLOCKABLE).
    #[test]
    fn turn_end_toxic_is_blockable() {
        let mut state = state_with_curse_in_hand(CardId::Toxic);
        state.player.block = 5;
        let hp_before = state.player.current_hp;

        end_player_turn_phase_two(&mut state);

        assert_eq!(state.player.current_hp, hp_before);
    }

    /// INFECTION: 3 self-damage, blockable (C# `Infection.OnTurnEndInHand`,
    /// DamageVar(3m, Unpowered|Move)). Previously unwired.
    #[test]
    fn turn_end_infection_deals_three_self_damage() {
        let mut state = state_with_curse_in_hand(CardId::Infection);
        let hp_before = state.player.current_hp;

        end_player_turn_phase_two(&mut state);

        assert_eq!(state.player.current_hp, hp_before - 3);
    }

    /// BECKON: 6 UNBLOCKABLE HP loss — bypasses block entirely.
    #[test]
    fn turn_end_beckon_deals_six_unblockable() {
        let mut state = state_with_curse_in_hand(CardId::Beckon);
        state.player.block = 99;
        let hp_before = state.player.current_hp;

        end_player_turn_phase_two(&mut state);

        assert_eq!(state.player.current_hp, hp_before - 6);
        // Block remains intact — UNBLOCKABLE skips absorption.
        assert_eq!(state.player.block, 99);
    }

    /// BAD_LUCK: 13 UNBLOCKABLE HP loss.
    #[test]
    fn turn_end_bad_luck_deals_thirteen_unblockable() {
        let mut state = state_with_curse_in_hand(CardId::BadLuck);
        state.player.block = 50;
        let hp_before = state.player.current_hp;

        end_player_turn_phase_two(&mut state);

        assert_eq!(state.player.current_hp, hp_before - 13);
        assert_eq!(state.player.block, 50);
    }

    /// BAD_LUCK lethals a low-HP player and short-circuits the pipeline
    /// to `CombatLost`. Mirrors Python's `_check_combat_end()` inside
    /// the BAD_LUCK arm (`combat.py:2400`).
    #[test]
    fn turn_end_bad_luck_lethal_short_circuits() {
        let mut state = state_with_curse_in_hand(CardId::BadLuck);
        state.player.current_hp = 10;

        let result = end_player_turn_phase_two(&mut state);

        assert_eq!(result, StepResult::CombatLost);
        assert_eq!(state.player.current_hp, 0);
        assert!(state.is_over);
        assert!(!state.player_won);
    }

    /// DEBT: at turn-end-in-hand, loses `min(10, gold)` from the live
    /// combat gold immediately (C# `Debt.cs:26-30 OnTurnEndInHand`:
    /// `num = Mathf.Min(GoldVar(10), Owner.Gold); PlayerCmd.LoseGold(num)`,
    /// i.e. `Gold = Max(0, Gold - num)`). HP / block remain untouched
    /// (DEBT doesn't deal damage). The live `state.gold` propagates to
    /// `rs.gold` via the `exit_combat` writeback.
    #[test]
    fn turn_end_debt_loses_live_combat_gold() {
        // `state_with_curse_in_hand` defaults gold = 0; set > 10 to
        // observe the full loss.
        let mut state = state_with_curse_in_hand(CardId::Debt);
        state.gold = 50;
        let hp_before = state.player.current_hp;

        end_player_turn_phase_two(&mut state);

        assert_eq!(state.player.current_hp, hp_before);
        assert_eq!(state.player.discard.len(), 1);
        assert_eq!(
            state.gold, 40,
            "DEBT must lose 10 gold from live combat gold"
        );
    }

    /// DEBT clamp case: when gold < 10, lose only what's available
    /// (C# `Mathf.Min(10, Owner.Gold)` then `Max(0, Gold - num)`).
    #[test]
    fn turn_end_debt_clamps_gold_loss_to_available() {
        let mut state = state_with_curse_in_hand(CardId::Debt);
        state.gold = 5;

        end_player_turn_phase_two(&mut state);

        assert_eq!(state.gold, 0, "DEBT must clamp the loss to available gold");
    }

    /// DOUBT: applies WEAK 1 to the player.
    #[test]
    fn turn_end_doubt_applies_weak() {
        let mut state = state_with_curse_in_hand(CardId::Doubt);
        let player_uid = state.player.creature_uid.expect("creature uid set");

        end_player_turn_phase_two(&mut state);

        // Walk the player's powers and assert WEAK 1 is present.
        let creature = state
            .creatures
            .iter()
            .find(|c| c.id == player_uid.0)
            .expect("player creature");
        let weak = creature
            .powers
            .iter()
            .find_map(|p_uid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == p_uid.0 && matches!(p.kind, crate::PowerKind::Weak))
            })
            .expect("WEAK power applied");
        assert_eq!(weak.amount, 1);
    }

    /// SHAME: applies FRAIL 1 to the player.
    #[test]
    fn turn_end_shame_applies_frail() {
        let mut state = state_with_curse_in_hand(CardId::Shame);
        let player_uid = state.player.creature_uid.expect("creature uid set");

        end_player_turn_phase_two(&mut state);

        let creature = state
            .creatures
            .iter()
            .find(|c| c.id == player_uid.0)
            .expect("player creature");
        let frail = creature
            .powers
            .iter()
            .find_map(|p_uid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == p_uid.0 && matches!(p.kind, crate::PowerKind::Frail))
            })
            .expect("FRAIL power applied");
        assert_eq!(frail.amount, 1);
    }

    /// REGRET: HP loss equals the snapshot of hand size at turn-end.
    /// With a 3-card hand of REGRET / REGRET / REGRET the snapshot is
    /// 3, and each card deals 3 UNBLOCKABLE HP loss — total 9.
    #[test]
    fn turn_end_regret_uses_hand_snapshot() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerEndTurnPhaseOne;
        state.round = 1;
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        state.player.block = 99; // proves UNBLOCKABLE bypass.
        let _ = state.init_player_creature();
        for i in 1..=3u32 {
            let uid = CardUid::new(i).expect("non-zero");
            state.cards.push(mk_curse(uid, CardId::Regret));
            state.player.hand.push(uid);
        }

        end_player_turn_phase_two(&mut state);

        // Each REGRET applies 3 HP loss (snapshot = 3). Total: 9. Block
        // unchanged (UNBLOCKABLE).
        assert_eq!(state.player.current_hp, 50 - 9);
        assert_eq!(state.player.block, 99);
    }

    /// REGRET's hand snapshot is taken BEFORE any in-hand effect
    /// discards a card. With a hand of REGRET + BURN (size 2), both
    /// cards trigger and REGRET sees snapshot=2 — even though BURN
    /// would shrink the hand by the time REGRET runs if the snapshot
    /// were re-evaluated.
    #[test]
    fn turn_end_regret_snapshot_is_pre_loop() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerEndTurnPhaseOne;
        state.round = 1;
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        let _ = state.init_player_creature();
        let uid_regret = CardUid::new(1).expect("non-zero");
        let uid_burn = CardUid::new(2).expect("non-zero");
        state.cards.push(mk_curse(uid_regret, CardId::Regret));
        state.cards.push(mk_curse(uid_burn, CardId::Burn));
        state.player.hand.push(uid_regret);
        state.player.hand.push(uid_burn);

        end_player_turn_phase_two(&mut state);

        // REGRET reads snapshot=2 → 2 unblockable HP loss; BURN → 2
        // self-damage. Total: 4 HP loss.
        assert_eq!(state.player.current_hp, 50 - 4);
    }

    /// Non-turn-end-in-hand cards (e.g. a Strike) are NOT discarded by
    /// the step-3 loop — they go through the normal flush at step 5.
    /// The pile mutation observed at step 5 is identical regardless of
    /// step 3, so we just assert that no extra HP / block churn fires.
    #[test]
    fn turn_end_filter_skips_non_curses() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        state.phase = CombatPhase::PlayerEndTurnPhaseOne;
        state.round = 1;
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        let _ = state.init_player_creature();
        let uid = CardUid::new(1).expect("non-zero");
        state.cards.push(Card {
            uid,
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        });
        state.player.hand.push(uid);
        let hp_before = state.player.current_hp;

        end_player_turn_phase_two(&mut state);

        assert_eq!(state.player.current_hp, hp_before);
        // Strike still flushed to discard at step 5.
        assert_eq!(state.player.discard.len(), 1);
    }

    // ──────────────────────────────────────────────────────────────────
    // Bead `sts2-rs-76t5` — OneTwoPunch via card-play-count channel
    // ──────────────────────────────────────────────────────────────────
    //
    // Verifications use enemy HP delta from a Strike play (registered
    // via `card_effects::install_default_registry`) so we side-step
    // parallel-test races on the global j87 after-card-played counter.

    /// Build a Strike (Ironclad) instance attached to `state`. Returns
    /// the new card uid + hand index.
    fn push_strike_to_hand(state: &mut CombatState) -> (crate::CardUid, u8) {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let uid = crate::CardUid::new((state.cards.len() + 1) as u32).expect("nz");
        state.cards.push(Card {
            uid,
            card_id: CardId::StrikeIronclad,
            cost: 0,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        });
        state.player.hand.push(uid);
        let idx = (state.player.hand.len() - 1) as u8;
        (uid, idx)
    }

    /// `OneTwoPunch(1)` + a Strike played → 2 attack hits resolve
    /// (enemy HP drops by 12) and the power instance drops from the
    /// arena (amount 1 → 0 → popped). Mirrors Python pin behaviour at
    /// `sts2-rl-agent@01c094d:sts2_env/powers/card_play_effects.py:478-508`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn one_two_punch_doubles_first_attack() {
        use crate::{Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        // Install OneTwoPunch(amount=1) on the player.
        let pid = std::num::NonZeroU32::new(7601).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::OneTwoPunch,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);
        let enemy_uid = state.enemy_uids[0];
        let enemy_idx = state
            .creatures
            .iter()
            .position(|c| c.id == enemy_uid.0)
            .expect("enemy creature");
        let hp_before = state.creatures[enemy_idx].current_hp;

        // Play card #1 (Strike, base_damage=6) — should hit twice for
        // 12 damage total (initial + OneTwoPunch replay).
        let (_uid, hand_idx) = push_strike_to_hand(&mut state);
        let _ = play_card(&mut state, hand_idx, Some(0));
        let hp_after_first = state.creatures[enemy_idx].current_hp;
        assert_eq!(
            hp_before - hp_after_first,
            12,
            "OneTwoPunch(1): first Attack should hit twice (6 + 6 = 12 damage)",
        );

        // OneTwoPunch power instance should be popped (amount 1 → 0 →
        // RemovePower).
        assert!(
            !state.powers.iter().any(|p| p.id == pid),
            "OneTwoPunch(1): power instance should be removed after exhaustion",
        );
        assert!(
            !state.player.powers.iter().any(|u| u.0 == pid),
            "OneTwoPunch(1): player.powers should not retain the popped uid",
        );

        // Play card #2 (Strike) — power is gone, single hit (6 damage).
        let (_uid2, hand_idx) = push_strike_to_hand(&mut state);
        let _ = play_card(&mut state, hand_idx, Some(0));
        let hp_after_second = state.creatures[enemy_idx].current_hp;
        assert_eq!(
            hp_after_first - hp_after_second,
            6,
            "OneTwoPunch exhausted: second Attack should hit once (6 damage)",
        );
    }

    /// `OneTwoPunch(2)` decrements (not removes) on first Attack, so a
    /// second Attack also gets the bonus play.
    #[test]
    #[serial_test::serial(global_registry)]
    fn one_two_punch_decrements_when_amount_above_one() {
        use crate::{Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let pid = std::num::NonZeroU32::new(7602).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::OneTwoPunch,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);
        let enemy_uid = state.enemy_uids[0];
        let enemy_idx = state
            .creatures
            .iter()
            .position(|c| c.id == enemy_uid.0)
            .expect("enemy creature");
        let hp_before = state.creatures[enemy_idx].current_hp;

        // First Attack play (Strike → 12 damage from 2 hits).
        let (_uid1, hand_idx) = push_strike_to_hand(&mut state);
        let _ = play_card(&mut state, hand_idx, Some(0));
        let hp_after_first = state.creatures[enemy_idx].current_hp;
        assert_eq!(
            hp_before - hp_after_first,
            12,
            "OneTwoPunch(2): first Attack still gets the bonus play (2 hits)",
        );
        assert_eq!(
            state.powers.get_by_id(pid).map(|p| p.amount),
            Some(1),
            "OneTwoPunch(2): amount should decrement to 1 after first Attack",
        );

        // Second Attack play — bonus still applies (1 → 0 → popped).
        let (_uid2, hand_idx) = push_strike_to_hand(&mut state);
        let _ = play_card(&mut state, hand_idx, Some(0));
        let hp_after_second = state.creatures[enemy_idx].current_hp;
        assert_eq!(
            hp_after_first - hp_after_second,
            12,
            "OneTwoPunch(1 remaining): second Attack still gets the bonus play",
        );
        assert!(
            !state.powers.iter().any(|p| p.id == pid),
            "OneTwoPunch: power instance should be removed when amount hits 0",
        );
    }

    /// `OneTwoPunch` does NOT trigger on a Skill (only Attacks). The
    /// power instance is left untouched and the player's block reflects
    /// a single Defend play (5).
    #[test]
    #[serial_test::serial(global_registry)]
    fn one_two_punch_skips_non_attacks() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        use crate::{Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let pid = std::num::NonZeroU32::new(7603).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::OneTwoPunch,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);
        let block_before = state.player.block;

        // Defend (Self_, +5 block).
        let uid = crate::CardUid::new((state.cards.len() + 1) as u32).expect("nz");
        state.cards.push(Card {
            uid,
            card_id: CardId::DefendIronclad,
            cost: 0,
            card_type: CardType::Skill,
            target_type: TargetType::Self_,
            upgraded: false,
            base_damage: 0,
            base_block: 5,
            ..Card::pod_defaults()
        });
        state.player.hand.push(uid);
        let hand_idx = (state.player.hand.len() - 1) as u8;
        let _ = play_card(&mut state, hand_idx, None);

        assert_eq!(
            state.player.block - block_before,
            5,
            "OneTwoPunch should not double-apply Defend (+5 block, not +10)",
        );
        assert_eq!(
            state.powers.get_by_id(pid).map(|p| p.amount),
            Some(1),
            "OneTwoPunch amount should be unchanged after a Skill play",
        );
    }

    /// Bead `sts2-rs-qjw41` (f933s follow-up): ChainsOfBindingPower's
    /// `BeforeTurnEnd` clears every `Bound`-afflicted player card.
    /// Pre-seed 3 cards with Bound + the ChainsOfBinding power; after
    /// `end_player_turn_phase_one`, all 3 must be affliction-free.
    /// Mirrors C# `ChainsOfBindingPower.cs` (v0.103.2)
    /// `BeforeTurnEnd` foreach `AllCards` → `CardCmd.ClearAffliction`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn qjw41_chains_before_turn_end_clears_bound_from_all_player_cards() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        use crate::{card_afflictions, Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        // Pre-seed 3 Attack cards in hand, each with Bound.
        let first_uid = state.cards.len() + 1;
        for i in 0..3u32 {
            let raw = u32::try_from(first_uid + i as usize).expect("fits");
            let uid = crate::CardUid::new(raw).expect("nz");
            let mut card = Card {
                uid,
                card_id: CardId::StrikeIronclad,
                cost: 1,
                card_type: CardType::Attack,
                target_type: TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..Card::pod_defaults()
            };
            assert!(card.set_affliction(card_afflictions::CardAffliction::new(
                card_afflictions::CardAfflictionKind::Bound,
                1,
            )));
            state.cards.push(card);
            state.player.hand.push(uid);
        }

        // Apply ChainsOfBinding(3) to player so the BeforeTurnEnd
        // self-listener fires.
        let pid = std::num::NonZeroU32::new(99001).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::ChainsOfBinding,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        // Advance to PlayerPlayPhase (start_combat leaves the state at
        // PlayerTurnStart; end_player_turn_phase_one debug-asserts phase
        // == PlayerPlayPhase).
        let _ = start_player_turn(&mut state);

        assert_eq!(
            state
                .cards
                .iter()
                .filter(|c| c.affliction.is_some())
                .count(),
            3,
            "preseed: 3 Bound cards before turn end",
        );

        let _ = end_player_turn_phase_one(&mut state);

        assert_eq!(
            state
                .cards
                .iter()
                .filter(|c| c.affliction.is_some())
                .count(),
            0,
            "qjw41: all Bound afflictions cleared on player turn end",
        );
    }

    /// Bead `sts2-rs-f0own` (f933s follow-up): ChainsOfBindingPower's
    /// `ShouldPlay` blocks a second Bound-card play in the same turn.
    /// Two Bound StrikeIronclad cards in hand + ChainsOfBinding power:
    /// first play succeeds (energy spent, card moves Hand→Play); second
    /// play is rejected (Advanced, energy unchanged, card stays in hand).
    /// Mirrors C# `ChainsOfBindingPower.cs` (v0.103.2).
    #[test]
    #[serial_test::serial(global_registry)]
    fn f0own_chains_blocks_second_bound_card_play_in_same_turn() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        use crate::{card_afflictions, Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        // Pre-seed 2 Strike cards in hand, each with Bound.
        let first_uid = state.cards.len() + 1;
        let mut uids = Vec::new();
        for i in 0..2u32 {
            let raw = u32::try_from(first_uid + i as usize).expect("fits");
            let uid = crate::CardUid::new(raw).expect("nz");
            let mut card = Card {
                uid,
                card_id: CardId::StrikeIronclad,
                cost: 1,
                card_type: CardType::Attack,
                target_type: TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..Card::pod_defaults()
            };
            assert!(card.set_affliction(card_afflictions::CardAffliction::new(
                card_afflictions::CardAfflictionKind::Bound,
                1,
            )));
            state.cards.push(card);
            state.player.hand.push(uid);
            uids.push(uid);
        }

        let pid = std::num::NonZeroU32::new(99101).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::ChainsOfBinding,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);

        // Ensure enough energy for two attempts (default starts at 3).
        assert!(state.player.energy >= 2, "preseed energy >= 2");
        let energy_before = state.player.energy;
        let hand_len_before = state.player.hand.len();

        // First play: Bound card, should succeed.
        let _ = play_card(&mut state, 0, Some(0));
        assert!(
            state.combat_log.chains_bound_card_played_this_turn,
            "first Bound play must set chains_bound_card_played_this_turn",
        );
        assert_eq!(
            state.player.hand.len(),
            hand_len_before - 1,
            "first Bound play removes the card from hand",
        );
        let energy_after_first = state.player.energy;
        assert!(
            energy_after_first < energy_before,
            "first play spent energy"
        );

        // Second play: Bound card, should be rejected.
        // Find the remaining Bound card's hand index.
        let remaining_uid = uids[1];
        let hand_idx = state
            .player
            .hand
            .iter()
            .position(|u| *u == remaining_uid)
            .expect("second Bound card still in hand") as u8;
        let result = play_card(&mut state, hand_idx, Some(0));

        assert!(
            matches!(result, StepResult::Advanced),
            "second Bound play must be rejected as Advanced no-op",
        );
        assert_eq!(
            state.player.energy, energy_after_first,
            "rejected second play must NOT spend energy",
        );
        assert!(
            state.player.hand.contains(&remaining_uid),
            "rejected card must stay in hand",
        );
    }

    /// `sts2-rs-ybtmu`: the `legal_actions` surface must agree with the
    /// `play_card` ChainsOfBinding gate (C# `ChainsOfBindingPower.ShouldPlay`).
    /// Two Bound StrikeIronclad cards in hand + ChainsOfBinding power: before
    /// any play, BOTH Bound cards are listed as legal. After the first Bound
    /// card is played, `legal_actions` must STOP listing the second Bound card
    /// (it would be rejected by `play_card`). This closes the QUEEN_BOSS
    /// LEGAL_ACTION divergence — Queen's PUPPET_STRINGS applies
    /// ChainsOfBinding(3), which afflicts drawn cards with `Bound`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn ybtmu_chains_of_binding_filters_second_bound_card_from_legal_actions() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        use crate::{card_afflictions, Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let first_uid = state.cards.len() + 1;
        let mut uids = Vec::new();
        for i in 0..2u32 {
            let raw = u32::try_from(first_uid + i as usize).expect("fits");
            let uid = crate::CardUid::new(raw).expect("nz");
            let mut card = Card {
                uid,
                card_id: CardId::StrikeIronclad,
                cost: 1,
                card_type: CardType::Attack,
                target_type: TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..Card::pod_defaults()
            };
            assert!(card.set_affliction(card_afflictions::CardAffliction::new(
                card_afflictions::CardAfflictionKind::Bound,
                1,
            )));
            state.cards.push(card);
            state.player.hand.push(uid);
            uids.push(uid);
        }

        let pid = std::num::NonZeroU32::new(99301).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::ChainsOfBinding,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);
        assert!(state.player.energy >= 2, "preseed energy >= 2");

        // Before any Bound play, BOTH Bound cards are playable.
        let plays_before = crate::legal_actions(&state)
            .iter()
            .filter(|a| matches!(a, crate::Action::PlayCard { .. }))
            .count();
        assert!(
            plays_before >= 2,
            "both Bound cards must be legal before any Bound play (got {plays_before})",
        );

        // Play the first Bound card.
        let _ = play_card(&mut state, 0, Some(0));
        assert!(
            state.combat_log.chains_bound_card_played_this_turn,
            "first Bound play sets chains_bound_card_played_this_turn",
        );

        // The remaining Bound card must NOT be listed as legal anymore —
        // `play_card` would reject it, so `legal_actions` must agree.
        let remaining_uid = uids[1];
        let remaining_hand_idx = state
            .player
            .hand
            .iter()
            .position(|u| *u == remaining_uid)
            .expect("second Bound card still in hand") as u8;
        let still_listed = crate::legal_actions(&state).iter().any(|a| {
            matches!(
                a,
                crate::Action::PlayCard { hand_index, .. } if *hand_index == remaining_hand_idx
            )
        });
        assert!(
            !still_listed,
            "second Bound card must be filtered from legal_actions after the first Bound play",
        );
    }

    /// `sts2-rs-ybtmu`: SlothPower(1) (KnowledgeDemon curse, counter 1)
    /// limits the player to ONE card play per turn. The second play is a
    /// no-op `Advanced` that spends no energy and leaves the card in hand —
    /// and `legal_actions` stops listing plays once the cap is hit. Mirrors
    /// the ChainsOfBinding play-block test above.
    #[test]
    #[serial_test::serial(global_registry)]
    fn ybtmu_sloth_limits_player_to_amount_card_plays_per_turn() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        use crate::{Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let first_uid = state.cards.len() + 1;
        let mut uids = Vec::new();
        for i in 0..2u32 {
            let raw = u32::try_from(first_uid + i as usize).expect("fits");
            let uid = crate::CardUid::new(raw).expect("nz");
            state.cards.push(Card {
                uid,
                card_id: CardId::StrikeIronclad,
                cost: 1,
                card_type: CardType::Attack,
                target_type: TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..Card::pod_defaults()
            });
            state.player.hand.push(uid);
            uids.push(uid);
        }

        let pid = std::num::NonZeroU32::new(99201).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Sloth,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);
        assert!(state.player.energy >= 2, "preseed energy >= 2");

        // 0 cards played < cap 1 → plays are legal.
        assert!(
            crate::legal_actions(&state)
                .iter()
                .any(|a| matches!(a, crate::Action::PlayCard { .. })),
            "Sloth(1) with 0 plays this turn still allows a play",
        );

        // First play succeeds.
        let _ = play_card(&mut state, 0, Some(0));
        assert_eq!(
            state.count_player_cards_played_this_turn(None),
            1,
            "first play counts toward the per-turn total",
        );

        // 1 played >= cap 1 → no further plays are legal, and play_card no-ops.
        assert!(
            !crate::legal_actions(&state)
                .iter()
                .any(|a| matches!(a, crate::Action::PlayCard { .. })),
            "Sloth(1) blocks all plays after the cap is reached",
        );
        let energy_before = state.player.energy;
        let remaining_uid = uids[1];
        let hand_idx = state
            .player
            .hand
            .iter()
            .position(|u| *u == remaining_uid)
            .expect("second card still in hand") as u8;
        let result = play_card(&mut state, hand_idx, Some(0));
        assert!(
            matches!(result, StepResult::Advanced),
            "second play rejected as Advanced no-op",
        );
        assert_eq!(
            state.player.energy, energy_before,
            "rejected play spends no energy",
        );
        assert!(
            state.player.hand.contains(&remaining_uid),
            "rejected card stays in hand",
        );
    }

    // Synthetic stand-ins for KnowledgeDemon's CURSE_OF_KNOWLEDGE `effect_fn`
    // (the real one lives in `sts2-content`, which core cannot depend on).
    // Each stamps the pre-increment curse counter exactly as the content
    // effect does; `open_monster_curse_select` turns that into the screen.
    fn curse_stamp_0(state: &mut CombatState, _owner: CreatureUid) {
        state.pending_curse_choice = Some(0);
    }
    fn curse_stamp_1(state: &mut CombatState, _owner: CreatureUid) {
        state.pending_curse_choice = Some(1);
    }
    fn curse_stamp_2(state: &mut CombatState, _owner: CreatureUid) {
        state.pending_curse_choice = Some(2);
    }
    fn curse_noop(_state: &mut CombatState, _owner: CreatureUid) {}

    /// A 2-state moveset shaped like KnowledgeDemon's: a `DebuffIntent` curse
    /// move that opens the blocking screen, following up into a plain attack.
    fn curse_test_ai(counter: u8) -> crate::monster_ai::MonsterAi {
        use std::collections::BTreeMap;

        use crate::monster_ai::{MonsterAi, MonsterState, MoveState};

        let effect: crate::monster_ai::EffectFn = match counter {
            0 => curse_stamp_0,
            1 => curse_stamp_1,
            _ => curse_stamp_2,
        };
        let mut s: BTreeMap<&'static str, MonsterState> = BTreeMap::new();
        s.insert(
            "CURSE",
            MonsterState::Move(MoveState {
                id: "CURSE",
                effect,
                intents: smallvec::smallvec![Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 0
                }],
                follow_up: Some("SLAP"),
                must_perform_once: false,
            }),
        );
        s.insert(
            "SLAP",
            MonsterState::Move(MoveState {
                id: "SLAP",
                effect: curse_noop,
                intents: smallvec::smallvec![Intent::Attack { damage: 1 }],
                follow_up: Some("CURSE"),
                must_perform_once: false,
            }),
        );
        MonsterAi::new(s, "CURSE")
    }

    /// `sts2-rs-ybtmu` (retimed): the KnowledgeDemon curse choice is raised
    /// MID enemy turn — C# `CurseOfKnowledge` awaits
    /// `CardSelectCmd.FromChooseACardScreen` inside the move body. Driving the
    /// enemy-exec phase must therefore return `PendingChoice` with the phase
    /// still `EnemyTurnExec` and a `MonsterCurseSelect` of
    /// `[Disintegration, alt(counter)]` open. Resolving index 0 applies
    /// `DisintegrationPower(6/7/8)`; index 1 applies the alt curse power, and
    /// the parked enemy turn then runs to completion.
    fn drive_curse_choice(counter: u8, pick: u8) -> CombatState {
        crate::card_effects::ensure_installed();
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        // Arm the lone enemy with the curse-shaped moveset and telegraph it.
        let uid = state.enemy_uids[0];
        state.creatures[0].ai = Some(curse_test_ai(counter));
        if let Some(ai) = state.creatures[0].ai.as_mut() {
            ai.set_owner(uid);
        }
        state.creatures[0].intent = Some(Intent::Debuff {
            kind: PowerKind::Weak,
            amount: 0,
        });
        let round_before = state.round;
        state.side = Side::Enemy;
        state.phase = CombatPhase::EnemyTurnExec;

        let result = enemy_turn_exec(&mut state);
        assert_eq!(
            result,
            StepResult::PendingChoice,
            "the curse move must suspend the enemy turn on its blocking screen",
        );
        assert_eq!(
            state.phase,
            CombatPhase::EnemyTurnExec,
            "the suspended state is still mid-enemy-turn (C# pauses in place)",
        );
        assert_eq!(
            state.round, round_before,
            "the round does NOT advance while the screen is open",
        );
        assert!(
            state.pending_enemy_exec_resume.is_some(),
            "the rest of the enemy turn is parked",
        );
        let pc = state.pending_choice.as_ref().expect("curse choice opened");
        assert_eq!(pc.kind, crate::ChoiceKind::MonsterCurseSelect);
        assert_eq!(pc.options.len(), 2);
        assert_eq!(pc.options[0].card_id, crate::cards::CardId::Disintegration);

        let mut sel: smallvec::SmallVec<[u8; 4]> = smallvec::SmallVec::new();
        sel.push(pick);
        let _ = crate::apply(&mut state, crate::Action::ResolveChoice { selection: sel });
        assert!(
            state.pending_choice.is_none(),
            "resolution clears the choice",
        );
        assert!(
            state.pending_curse_choice.is_none(),
            "resolution consumes the curse marker",
        );
        assert!(
            state.pending_enemy_exec_resume.is_none(),
            "resolution consumes the parked enemy turn",
        );
        assert_eq!(
            state.round,
            round_before + 1,
            "the parked enemy turn completes on resolution (round advances)",
        );
        state
    }

    /// The suspended enemy turn resumes at the monster that had not yet acted:
    /// with the curse monster in slot 0 and a plain attacker in slot 1, the
    /// attacker's damage lands only AFTER the curse screen is answered.
    #[test]
    #[serial_test::serial(global_registry)]
    fn curse_screen_suspends_the_rest_of_the_enemy_turn() {
        crate::card_effects::ensure_installed();
        let mut state = state_with_cultists(2);
        let _ = start_combat(&mut state);
        let curse_uid = state.enemy_uids[0];
        state.creatures[0].ai = Some(curse_test_ai(0));
        if let Some(ai) = state.creatures[0].ai.as_mut() {
            ai.set_owner(curse_uid);
        }
        state.creatures[0].intent = Some(Intent::Debuff {
            kind: PowerKind::Weak,
            amount: 0,
        });
        state.creatures[1].intent = Some(Intent::Attack { damage: 7 });
        let hp_before = state.player.current_hp;
        state.side = Side::Enemy;
        state.phase = CombatPhase::EnemyTurnExec;

        assert_eq!(enemy_turn_exec(&mut state), StepResult::PendingChoice);
        assert_eq!(
            state.player.current_hp, hp_before,
            "the second monster has NOT acted while the screen is open",
        );
        assert_eq!(
            state
                .pending_enemy_exec_resume
                .as_ref()
                .map(|r| r.next_index),
            Some(1),
            "the walk is parked on the not-yet-acted monster",
        );

        let mut sel: smallvec::SmallVec<[u8; 4]> = smallvec::SmallVec::new();
        sel.push(0);
        let _ = crate::apply(&mut state, crate::Action::ResolveChoice { selection: sel });
        assert_eq!(
            state.player.current_hp,
            hp_before - 7,
            "the parked monster acts once the screen is answered",
        );
    }

    fn player_power_amt(state: &CombatState, kind: PowerKind) -> i32 {
        state
            .player
            .powers
            .iter()
            .find_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == kind)
                    .map(|p| p.amount)
            })
            .unwrap_or(0)
    }

    #[test]
    #[serial_test::serial(global_registry)]
    fn ybtmu_curse_choice_index0_applies_disintegration_by_counter() {
        use crate::PowerKind;
        // Option 0 = Disintegration; amount = {6,7,8} by counter.
        let s0 = drive_curse_choice(0, 0);
        assert_eq!(player_power_amt(&s0, PowerKind::Disintegration), 6);
        let s1 = drive_curse_choice(1, 0);
        assert_eq!(player_power_amt(&s1, PowerKind::Disintegration), 7);
        let s2 = drive_curse_choice(2, 0);
        assert_eq!(player_power_amt(&s2, PowerKind::Disintegration), 8);
    }

    #[test]
    #[serial_test::serial(global_registry)]
    fn ybtmu_curse_choice_index1_applies_counter_alt_power() {
        use crate::PowerKind;
        // Option 1 = the counter-keyed alt: MindRot(1) / Sloth(3) / WasteAway(1).
        let s0 = drive_curse_choice(0, 1);
        assert_eq!(player_power_amt(&s0, PowerKind::MindRot), 1);
        assert_eq!(player_power_amt(&s0, PowerKind::Disintegration), 0);
        let s1 = drive_curse_choice(1, 1);
        assert_eq!(player_power_amt(&s1, PowerKind::Sloth), 3);
        let s2 = drive_curse_choice(2, 1);
        assert_eq!(player_power_amt(&s2, PowerKind::WasteAway), 1);
    }

    /// `OneTwoPunch` is removed at owner-side turn end. Mirrors Python
    /// `OneTwoPunchPower.after_turn_end` at
    /// `sts2-rl-agent@01c094d:sts2_env/powers/card_play_effects.py:504-508`.
    /// Without this, a `OneTwoPunch(1)` left on the player by a play
    /// with no follow-up Attack would leak into the next turn's first
    /// Attack — observed in
    /// `fight-replays-mistakes/00030217..._floor021_thieving_hopper_weak.html`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn one_two_punch_removed_on_player_turn_end() {
        use crate::hooks::{self, TurnSideCtx};
        use crate::{Power, PowerKind, Side};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let pid = std::num::NonZeroU32::new(7604).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::OneTwoPunch,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);
        hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });

        assert!(
            !state.powers.iter().any(|p| p.id == pid),
            "OneTwoPunch should be removed on owner-side turn end",
        );
        assert!(
            !state.player.powers.iter().any(|u| u.0 == pid),
            "player.powers should not retain the popped uid",
        );
    }

    /// `OneTwoPunch` is NOT removed on the enemy-side turn-end pass —
    /// the owner-side guard mirrors Python `if side == owner.side`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn one_two_punch_persists_through_enemy_turn_end() {
        use crate::hooks::{self, TurnSideCtx};
        use crate::{Power, PowerKind, Side};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let pid = std::num::NonZeroU32::new(7605).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::OneTwoPunch,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);
        hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });

        assert!(
            state.powers.iter().any(|p| p.id == pid),
            "OneTwoPunch should NOT be removed by enemy-side after_turn_end",
        );
    }

    /// `RagePower` is removed at owner-side turn end. Mirrors Python
    /// `RagePower.after_turn_end` at
    /// `sts2-rl-agent@01c094d:sts2_env/powers/card_play_effects.py:168-172`.
    /// Without removal, Rage's "+block on attack played" body fires
    /// across all subsequent turns.
    #[test]
    #[serial_test::serial(global_registry)]
    fn rage_removed_on_player_turn_end() {
        use crate::hooks::{self, TurnSideCtx};
        use crate::{Power, PowerKind, Side};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let pid = std::num::NonZeroU32::new(7610).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Rage,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);
        hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });

        assert!(
            !state.powers.iter().any(|p| p.id == pid),
            "Rage should be removed on owner-side turn end",
        );
        assert!(
            !state.player.powers.iter().any(|u| u.0 == pid),
            "player.powers should not retain the popped Rage uid",
        );
    }

    /// `FlameBarrierPower` is removed at the *opposite*-side turn end —
    /// for a player-applied FlameBarrier, that's the end of the enemy
    /// turn. Mirrors Python
    /// `FlameBarrierPower.after_turn_end` at
    /// `sts2-rl-agent@01c094d:sts2_env/powers/damage_reactions.py:105-111`
    /// (`if owner.side != side: pop`).
    #[test]
    #[serial_test::serial(global_registry)]
    fn flame_barrier_removed_on_enemy_turn_end() {
        use crate::hooks::{self, TurnSideCtx};
        use crate::{Power, PowerKind, Side};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);

        let pid = std::num::NonZeroU32::new(7611).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::FlameBarrier,
            amount: 4,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let _ = start_player_turn(&mut state);

        // Player-side turn-end pass: FlameBarrier must NOT be removed yet.
        hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
        assert!(
            state.powers.iter().any(|p| p.id == pid),
            "FlameBarrier must survive owner-side turn end (opposite-side guard)",
        );

        // Enemy-side turn-end pass: now the opposite-side guard fires.
        hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
        assert!(
            !state.powers.iter().any(|p| p.id == pid),
            "FlameBarrier should be removed at the end of the enemy turn",
        );
    }

    /// `RupturePower`: when the owner loses HP on their own turn (and
    /// damage > 0), gain `amount` Strength. Mirrors Python
    /// `RupturePower.after_damage_received` at
    /// `sts2-rl-agent@01c094d:sts2_env/powers/card_play_effects.py:217-229`.
    /// Wired inline in `dispatch_after_damage_received` because the post-
    /// mitigation `damage` value isn't visible in the per-listener fn.
    #[test]
    #[serial_test::serial(global_registry)]
    fn rupture_grants_strength_on_player_damage() {
        use crate::combat_loop::{CreatureRef, ValueProps};
        use crate::hooks::{self, DamageCtx};
        use crate::{Power, PowerKind, Side};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        state.init_player_creature();
        state.side = Side::Player;

        let pid = std::num::NonZeroU32::new(7620).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Rupture,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        // Simulate a powered hit on the player (damage = 5).
        let ctx = DamageCtx {
            dealer: CreatureRef::Enemy(state.enemy_uids[0]),
            target: CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        hooks::dispatch_after_damage_received(&mut state, &ctx, 5);

        let str_amount = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Strength)
            .map_or(0, |p| p.amount);
        assert_eq!(
            str_amount, 2,
            "Rupture(2) on own-turn HP loss should grant +2 Strength",
        );
    }

    /// `RupturePower` does NOT fire when `damage == 0` (Python gate
    /// `damage > 0`), even if the player is otherwise targeted.
    #[test]
    #[serial_test::serial(global_registry)]
    fn rupture_does_not_fire_on_zero_damage() {
        use crate::combat_loop::{CreatureRef, ValueProps};
        use crate::hooks::{self, DamageCtx};
        use crate::{Power, PowerKind, Side};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        state.init_player_creature();
        state.side = Side::Player;

        let pid = std::num::NonZeroU32::new(7621).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Rupture,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let ctx = DamageCtx {
            dealer: CreatureRef::Enemy(state.enemy_uids[0]),
            target: CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        hooks::dispatch_after_damage_received(&mut state, &ctx, 0);

        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Strength),
            "Rupture must not fire on damage == 0",
        );
    }

    /// `JuggernautPower`: when the player gains block, deal `amount`
    /// unpowered damage to a random alive enemy. Mirrors Python
    /// `JuggernautPower.after_block_gained` at
    /// `sts2-rl-agent@01c094d:sts2_env/powers/card_play_effects.py:249-265`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn juggernaut_deals_damage_on_player_block_gain() {
        use crate::{Power, PowerKind};

        crate::card_effects::ensure_installed();

        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);

        let pid = std::num::NonZeroU32::new(7622).unwrap();
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Juggernaut,
            amount: 5,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(crate::PowerUid(pid));

        let enemy_uid = state.enemy_uids[0];
        let enemy_idx = state
            .creatures
            .iter()
            .position(|c| c.id == enemy_uid.0)
            .expect("enemy creature");
        let hp_before = state.creatures[enemy_idx].current_hp;

        crate::card_effects::gain_block_player(&mut state, 8);

        let hp_after = state.creatures[enemy_idx].current_hp;
        assert_eq!(
            hp_before - hp_after,
            5,
            "Juggernaut(5) should deal 5 unpowered damage to the random enemy on block gain",
        );
        assert!(
            state.player.block >= 8,
            "Player should still have gained the block (Juggernaut is a side effect)",
        );
    }

    // ── sts2-rs-3guy: request_end_turn_after_play rider ────────────────────

    /// A handler that raises `request_end_turn_after_play` mid-play
    /// causes `play_card` to run the FULL end-turn pipeline (player
    /// end-turn → enemy turn → next player turn) and clear the flag,
    /// mirroring C#'s `PlayerCmd.EndTurn(owner, canBackOut: false)` in
    /// `VoidForm.OnPlay` (VoidForm.cs:26). The earlier behavior — only
    /// bumping `phase` to `PlayerEndTurnPhaseOne` and returning — parked
    /// the engine with no pump, so the enemy turn never ran (Regent fuzz
    /// seed 130: VoidForm made the player immortal). Port-pin
    /// `sts2-rl-agent@01c094d:sts2_env/cards/regent.py:842-846`, consumer
    /// at `combat.py:909-911`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn request_end_turn_after_play_advances_phase() {
        crate::card_effects::ensure_installed();
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
        let round_before = state.round;

        // Simulate a VoidForm-style handler by raising the flag before
        // the play resolves. (The real VoidForm body raises this from
        // inside `card_effects::dispatch`; the test bypasses by setting
        // it pre-play to keep the fixture self-contained.)
        state.player.request_end_turn_after_play = true;

        let (_uid, hand_idx) = push_strike_to_hand(&mut state);
        let res = play_card(&mut state, hand_idx, Some(0));

        // The full chain ran: the enemy took its turn and the player is
        // back in the play phase of the NEXT round — NOT parked in
        // `PlayerEndTurnPhaseOne` with the enemy turn skipped.
        assert_eq!(
            state.phase,
            CombatPhase::PlayerPlayPhase,
            "request_end_turn_after_play: should land back in the next player's play phase",
        );
        assert_eq!(
            state.round,
            round_before + 1,
            "request_end_turn_after_play: the round must advance (enemy turn ran)",
        );
        assert!(
            !state.player.request_end_turn_after_play,
            "request_end_turn_after_play: flag should be cleared after consumption",
        );
        assert!(
            matches!(res, StepResult::Advanced),
            "request_end_turn_after_play: should signal Advanced on rider trigger",
        );
    }

    /// With the flag low, `play_card` leaves the phase unchanged (still
    /// `PlayerPlayPhase`) so the player can keep playing cards. Locks
    /// in the "default off" contract — the rider is opt-in per
    /// handler, not a global side-effect.
    #[test]
    #[serial_test::serial(global_registry)]
    fn request_end_turn_after_play_does_not_fire_when_low() {
        crate::card_effects::ensure_installed();
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);
        assert!(!state.player.request_end_turn_after_play);

        let (_uid, hand_idx) = push_strike_to_hand(&mut state);
        let _ = play_card(&mut state, hand_idx, Some(0));

        assert_eq!(
            state.phase,
            CombatPhase::PlayerPlayPhase,
            "no rider: phase should stay at PlayerPlayPhase",
        );
    }

    /// `start_player_turn` clears any stale `request_end_turn_after_play`
    /// at the top of the player's turn. Guards against a save/load
    /// round-trip that captured the state mid-play before the
    /// bottom-of-`play_card` consumer ran.
    #[test]
    #[serial_test::serial(global_registry)]
    fn request_end_turn_after_play_resets_at_turn_start() {
        crate::card_effects::ensure_installed();
        let mut state = state_with_cultists(1);
        let _ = start_combat(&mut state);
        state.player.request_end_turn_after_play = true;

        let _ = start_player_turn(&mut state);

        assert!(
            !state.player.request_end_turn_after_play,
            "start_player_turn should clear any stale end-turn-after-play flag",
        );
    }

    /// Bead `sts2-rs-79tv9` (dvqa1.2): the cross-creature AfterDeath
    /// dispatch fires every registered listener once per
    /// `(dead, witness)` pair, drains
    /// `combat_log.newly_dead_this_turn`, and skips the dead
    /// creature itself + any creature without a listener. Empty
    /// carrier → no-op.
    #[test]
    fn after_death_dispatch_fires_once_per_witness_and_drains() {
        use crate::monster_ai::{MonsterAi, MonsterState, MoveState};
        use smallvec::smallvec as sv;
        use std::collections::BTreeMap;
        use std::sync::atomic::{AtomicI32, Ordering};

        // Shared listener counter. `fn` pointers can't capture
        // closures, so use a static atomic the listener increments
        // and the test reads.
        static FIRED: AtomicI32 = AtomicI32::new(0);
        fn listener(_state: &mut CombatState, _dead: CreatureUid, _owner: CreatureUid) {
            FIRED.fetch_add(1, Ordering::SeqCst);
        }
        fn noop_effect(_: &mut CombatState, _: CreatureUid) {}

        fn make_ai_with_listener() -> MonsterAi {
            let mut states = BTreeMap::new();
            states.insert(
                "M",
                MonsterState::Move(MoveState {
                    id: "M",
                    effect: noop_effect,
                    intents: sv![Intent::Defend { block: 0 }],
                    follow_up: Some("M"),
                    must_perform_once: false,
                }),
            );
            let mut ai = MonsterAi::new(states, "M");
            ai.after_death = Some(listener);
            ai
        }

        FIRED.store(0, Ordering::SeqCst);

        // Build a CombatState with three creatures:
        //   - witness_a:  alive, listener registered (should fire)
        //   - witness_b:  alive, NO listener (should NOT fire)
        //   - dead:       hp=0, listener registered (must be skipped
        //                 because we exclude the dead uid itself even
        //                 when its AI has a listener)
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        let mk = |id_u32: u32, hp: i32, ai: Option<MonsterAi>| Creature {
            id: NonZeroU32::new(id_u32).expect("non-zero"),
            current_hp: hp,
            max_hp: 50,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::ShrinkerBeetle),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("AfterDeathTest"),
            ai,
        };
        state
            .creatures
            .push(mk(1, 30, Some(make_ai_with_listener()))); // witness_a
        state.creatures.push(mk(2, 30, None)); // witness_b
        state
            .creatures
            .push(mk(3, 0, Some(make_ai_with_listener()))); // dead

        let dead_uid = CreatureUid(NonZeroU32::new(3).expect("non-zero"));
        state.combat_log.newly_dead_this_turn.push(dead_uid);

        super::dispatch_after_death_listeners(&mut state);

        // Listener fires exactly once: witness_a observed dead.
        // witness_b has no listener; dead is excluded as itself.
        assert_eq!(FIRED.load(Ordering::SeqCst), 1);
        assert!(state.combat_log.newly_dead_this_turn.is_empty());

        // Empty carrier → no-op (idempotent re-call).
        super::dispatch_after_death_listeners(&mut state);
        assert_eq!(FIRED.load(Ordering::SeqCst), 1);
    }

    /// Default `after_death = None` means a perform_intent that
    /// leaves no deaths is a complete no-op of this dispatch lane —
    /// guards against accidental firing on the empty path.
    #[test]
    fn after_death_dispatch_noop_on_empty_carrier() {
        let mut state = CombatState {
            rng: crate::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        assert!(state.combat_log.newly_dead_this_turn.is_empty());
        // Should not panic, allocate, or fire anything.
        super::dispatch_after_death_listeners(&mut state);
        assert!(state.combat_log.newly_dead_this_turn.is_empty());
    }
}
