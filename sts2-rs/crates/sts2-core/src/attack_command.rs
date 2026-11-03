//! The player's attack commands: identity, admission, results and lifecycle.
//!
//! C# runs every card attack as one `AttackCommand`
//! (`Commands.Builders/AttackCommand.cs`, `Execute`): it is refused outright
//! (no hooks at all) when combat is over or ending or its attacker is dead;
//! an admitted command awaits `Hook.BeforeAttack`, then its hits (each hit's
//! `CreatureCmd.Damage` results appended to `command.Results`; the loop stops
//! when the attacker dies or no valid target is left, so a command can finish
//! with zero hits), then `Hook.AfterAttack`, and only then returns to the
//! card's `OnPlay`. Every Attack-typed card issues exactly one command
//! (Pact's End only when it can deal damage; an Osty card only when Osty is
//! there, with Osty as the attacker; Omnislice / Echoing Slash through an
//! `AttackContext`, which has the same Before/After pair), and no other card
//! does.
//!
//! The command's own listeners at this pin:
//! * `VigorPower.BeforeAttack` latches the first powered command of its owner
//!   (`commandToModify`, `amountWhenAttackStarted`); its additive bonus then
//!   only reaches hits whose card is that command's card, and its
//!   `AfterAttack` for that same command subtracts the starting amount
//!   (removed at 0). The latch is never cleared otherwise: it lives as long as
//!   the power instance.
//! * `GigantificationPower.BeforeAttack` latches the first powered command of
//!   an Attack card its owner owns; ×3 applies to that command's card (or to
//!   any of the owner's powered card damage while nothing is latched); its
//!   `AfterAttack` for that command clears the latch and decrements.
//! * `SkittishPower.AfterAttack` (an enemy's): once per turn, for a
//!   `Move` command whose source is a card, the FIRST result in the command's
//!   results for the owner decides: nonzero unblocked damage grants `Amount`
//!   block. A blocked first hit grants nothing even if a later hit lands.
//!
//! The engine's card handlers write their own hit loops, so a command is
//! carried through the existing dispatch: [`crate::card_effects::dispatch_for_card`]
//! brackets every `OnPlay`. A command opens lazily before the card's first
//! `Move` hit from the player (or the player's Osty) on an enemy
//! ([`before_hit`], called from the one enemy-damage choke point), collects
//! that card's hit results, and closes (`AfterAttack`) either where a handler
//! ends it explicitly ([`end_card_command`], right after its attack, for the
//! cards with work after the attack) or when the `OnPlay` dispatch returns.
//! An Attack card whose dispatch opened none (zero hits: X=0 Whirlwind,
//! Flak Cannon without Statuses, a target that was already gone) runs an
//! empty admitted command there. Commands are a stack, so a play nested in
//! another card's `OnPlay` owns its own command. A command never stays open
//! across a decision: every dispatch closes what it opened before it returns.

use crate::hooks::{CreatureRef, ValueProps};
use crate::{CardUid, CombatState, CreatureUid, PowerKind};

/// One open attack command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttackCommandFrame {
    /// Command identity (unique within the combat).
    pub id: u32,
    /// `command.Attacker`: the player, or the player's Osty.
    pub attacker: CreatureRef,
    /// `command.ModelSource` when it is a card.
    pub source: Option<CardUid>,
    /// `command.DamageProps`.
    pub props: ValueProps,
    /// `command.Results`, flattened in order: (receiver, unblocked damage).
    pub results: smallvec::SmallVec<[(CreatureUid, i32); 6]>,
    /// False for a command a handler opened explicitly that was refused (C#
    /// `Execute` returned before `BeforeAttack`): it runs no hooks, and every
    /// hit it attempts is stopped.
    pub admitted: bool,
}

/// A power's command latch (`commandToModify` and, for Vigor,
/// `amountWhenAttackStarted`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttackPowerLatch {
    /// The latched command's id.
    pub command: u32,
    /// That command's card.
    pub source: Option<CardUid>,
    /// The power's amount when the command started (Vigor).
    pub amount_at_start: i32,
}

/// The attack-command state of a combat.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttackCommandState {
    /// Open commands, innermost last.
    pub frames: Vec<AttackCommandFrame>,
    /// The next command id.
    pub next_id: u32,
    /// The player's `VigorPower` latch.
    pub vigor: Option<AttackPowerLatch>,
    /// The player's `GigantificationPower` latch.
    pub gigantification: Option<AttackPowerLatch>,
    /// Whether the `OnPlay` dispatch now running has opened a command for its
    /// card (saved and restored around nested dispatches).
    pub dispatch_opened: bool,
    /// How many damage calls are running inside the current `OnPlay` dispatch
    /// (saved and reset around nested dispatches). A hit dealt while another
    /// is running (a power's or relic's reaction to it: Juggernaut after
    /// Shroud after Reaper Form's Doom) is that callback's own damage, never
    /// a hit of the card's command.
    pub damage_depth: u32,
    /// While one hit fans out over several targets (an all-enemies attack:
    /// one C# `CreatureCmd.Damage` over its target list), what its first target
    /// decided for the whole hit (`Some(None)`: no target reached yet). `None`
    /// when no fan-out runs. The attacker's death on an earlier target doesn't
    /// abandon the hit's later targets; only the next hit is stopped.
    pub fanout: Option<Option<HitOwner>>,
}

/// Saved by [`fanout_begin`].
#[derive(Clone, Copy, Debug)]
pub struct FanOutMark(Option<Option<HitOwner>>);

/// One hit over several targets begins (pair with [`fanout_end`]): admission
/// and continuation are decided once, at its first target.
pub fn fanout_begin(state: &mut CombatState) -> FanOutMark {
    let mark = FanOutMark(state.attack_commands.fanout);
    state.attack_commands.fanout = Some(None);
    mark
}

/// The fan-out hit is over.
pub fn fanout_end(state: &mut CombatState, mark: FanOutMark) {
    state.attack_commands.fanout = mark.0;
}

/// What one hit on an enemy is to the running card's attack command
/// ([`before_hit`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitOwner {
    /// Not a command hit (no Attack card resolving, not a `Move` hit of the
    /// player or Osty, a hit nested in another damage call, or a hit after the
    /// card's command already closed): plain damage, not recorded.
    Plain,
    /// A hit of the innermost command: it lands and its result is recorded.
    Command,
    /// The command was refused, or its attacker died or combat is ending: C#
    /// `AttackCommand.Execute` runs no (further) hit.
    Stopped,
}

fn attacker_alive(state: &CombatState, attacker: CreatureRef) -> bool {
    match attacker {
        CreatureRef::Player => state.player.current_hp > 0,
        CreatureRef::Enemy(uid) => state
            .creatures
            .iter()
            .any(|c| c.id == uid.0 && c.current_hp > 0),
        CreatureRef::Ally(_) | CreatureRef::Nobody => false,
    }
}

fn card_is_attack(state: &CombatState, card: CardUid) -> bool {
    state
        .cards
        .get(card.0.get() as usize - 1)
        .is_some_and(|c| c.card_type == crate::cards::CardType::Attack)
}

fn player_power_uid(state: &CombatState, kind: PowerKind) -> Option<crate::PowerUid> {
    state.player_power_uids().into_iter().find(|uid| {
        state
            .powers
            .get_by_id(uid.0)
            .is_some_and(|p| p.kind == kind)
    })
}

fn remove_power(state: &mut CombatState, pid: std::num::NonZeroU32) {
    state.powers.retain(|p| p.id != pid);
    state.player.powers.retain(|u| u.0 != pid);
    for c in &mut state.creatures {
        c.powers.retain(|u| u.0 != pid);
    }
}

/// Open a command (C# `AttackCommand.Execute` up to and including
/// `Hook.BeforeAttack`). Returns `false` when it is refused: combat over or
/// ending, or the attacker dead (C# returns before any hook).
pub fn begin(
    state: &mut CombatState,
    attacker: CreatureRef,
    props: ValueProps,
    source: Option<CardUid>,
) -> bool {
    if crate::combat_loop::combat_is_ending(state) || !attacker_alive(state, attacker) {
        return false;
    }
    let id = state.attack_commands.next_id;
    state.attack_commands.next_id = id.wrapping_add(1);
    state.attack_commands.frames.push(AttackCommandFrame {
        id,
        attacker,
        source,
        props,
        results: smallvec::SmallVec::new(),
        admitted: true,
    });
    if source.is_some() && source == state.resolving_card {
        state.attack_commands.dispatch_opened = true;
    }
    // ── BeforeAttack ──────────────────────────────────────────────────────
    // A latch outlives nothing but its power instance: a power that is gone
    // takes its `Data` with it.
    let vigor = player_power_uid(state, PowerKind::Vigor);
    if vigor.is_none() {
        state.attack_commands.vigor = None;
    }
    if player_power_uid(state, PowerKind::Gigantification).is_none() {
        state.attack_commands.gigantification = None;
    }
    // C# `VigorPower.BeforeAttack`: Attacker == Owner, powered, nothing latched,
    // ModelSource null or a card.
    if let Some(vigor) = vigor {
        if attacker == CreatureRef::Player
            && props.is_powered()
            && state.attack_commands.vigor.is_none()
        {
            let amount = state.powers.get_by_id(vigor.0).map_or(0, |p| p.amount);
            state.attack_commands.vigor = Some(AttackPowerLatch {
                command: id,
                source,
                amount_at_start: amount,
            });
        }
    }
    // C# `GigantificationPower.BeforeAttack`: ModelSource is an Attack card the
    // owner owns, powered, nothing latched. An Osty card's command is unpowered
    // in C# (the engine routes its hits as `Move` from the Osty), so the
    // player-attacker gate stands in for `IsPoweredAttack()` there.
    if state.attack_commands.gigantification.is_none()
        && attacker == CreatureRef::Player
        && props.is_powered()
        && source.is_some_and(|c| card_is_attack(state, c))
        && player_power_uid(state, PowerKind::Gigantification).is_some()
    {
        state.attack_commands.gigantification = Some(AttackPowerLatch {
            command: id,
            source,
            amount_at_start: 0,
        });
    }
    true
}

/// Close the innermost command (C# `Hook.AfterAttack` for it). No-op when no
/// command is open.
pub fn end(state: &mut CombatState) {
    let Some(frame) = state.attack_commands.frames.pop() else {
        return;
    };
    if !frame.admitted {
        return;
    }
    // ── AfterAttack ───────────────────────────────────────────────────────
    // C# `VigorPower.AfterAttack`: `ModifyAmount(-amountWhenAttackStarted)` for
    // its latched command; removed at 0 (`ShouldRemoveDueToAmount`).
    if let Some(latch) = state.attack_commands.vigor {
        if latch.command == frame.id {
            if let Some(pid) = player_power_uid(state, PowerKind::Vigor) {
                let mut gone = false;
                if let Some(p) = state.powers.get_mut_by_id(pid.0) {
                    p.amount = p.amount.saturating_sub(latch.amount_at_start);
                    gone = p.amount <= 0;
                }
                if gone {
                    remove_power(state, pid.0);
                    state.attack_commands.vigor = None;
                }
            } else {
                state.attack_commands.vigor = None;
            }
        }
    }
    // C# `GigantificationPower.AfterAttack`: clear the latch, `Decrement`.
    if let Some(latch) = state.attack_commands.gigantification {
        if latch.command == frame.id {
            state.attack_commands.gigantification = None;
            if let Some(pid) = player_power_uid(state, PowerKind::Gigantification) {
                let mut gone = false;
                if let Some(p) = state.powers.get_mut_by_id(pid.0) {
                    p.amount = p.amount.saturating_sub(1);
                    gone = p.amount <= 0;
                }
                if gone {
                    remove_power(state, pid.0);
                }
            }
        }
    }
    crate::hooks::skittish_after_attack(state, &frame);
}

/// The card's command is over: close it if it is the innermost open command
/// and belongs to the card whose `OnPlay` is running. Handlers call this right
/// after their attack when work follows it in C# (a selection, an auto-play,
/// a draw, a power, block, …), so that work sees the command's `AfterAttack`
/// already done.
pub fn end_card_command(state: &mut CombatState) {
    let card = state.resolving_card;
    if card.is_some()
        && state
            .attack_commands
            .frames
            .last()
            .is_some_and(|f| f.source == card)
    {
        end(state);
    }
}

/// Open a new command for the card whose `OnPlay` is running, dealt by the
/// player: for a handler that issues more than one command in one `OnPlay`
/// (Mad Science's attack hits, each its own `Execute`). Pair with
/// [`end_card_command`].
///
/// Returns whether it was admitted. A refused command still stands as a frame
/// until `end_card_command` (no hooks): its attempted hits are stopped, never
/// ordinary damage.
pub fn begin_card_command(state: &mut CombatState) -> bool {
    let card = state.resolving_card;
    if begin(state, CreatureRef::Player, ValueProps::MOVE, card) {
        return true;
    }
    state.attack_commands.frames.push(AttackCommandFrame {
        id: state.attack_commands.next_id,
        attacker: CreatureRef::Player,
        source: card,
        props: ValueProps::MOVE,
        results: smallvec::SmallVec::new(),
        admitted: false,
    });
    state.attack_commands.next_id = state.attack_commands.next_id.wrapping_add(1);
    if card.is_some() {
        state.attack_commands.dispatch_opened = true;
    }
    false
}

fn card_attacker(state: &CombatState, dealer: CreatureRef) -> bool {
    match dealer {
        CreatureRef::Player => true,
        CreatureRef::Enemy(uid) => state.player.pet_uid == Some(uid),
        CreatureRef::Ally(_) | CreatureRef::Nobody => false,
    }
}

/// Before one hit on an enemy: what the hit is to the running card's command,
/// opening the command if this is its first hit (a `Move` hit dealt by the
/// player or the player's Osty while an Attack card's `OnPlay` runs, not nested
/// in another damage call, with no open command of that card and attacker
/// innermost). C# `AttackCommand.Execute`: a refused command runs no hit, and
/// its hit loop stops once the attacker is dead or combat is ending (a hit
/// already started finishes; the admitted command still completes).
pub(crate) fn before_hit(
    state: &mut CombatState,
    dealer: CreatureRef,
    props: ValueProps,
) -> HitOwner {
    if state.attack_commands.damage_depth > 0 {
        return HitOwner::Plain;
    }
    if let Some(Some(decided)) = state.attack_commands.fanout {
        return decided;
    }
    let owner = hit_owner(state, dealer, props);
    if let Some(slot) = state.attack_commands.fanout.as_mut() {
        *slot = Some(owner);
    }
    owner
}

fn hit_owner(state: &mut CombatState, dealer: CreatureRef, props: ValueProps) -> HitOwner {
    let Some(card) = state.resolving_card else {
        return HitOwner::Plain;
    };
    if !props.contains(ValueProps::MOVE) || !card_attacker(state, dealer) {
        return HitOwner::Plain;
    }
    if state
        .attack_commands
        .frames
        .last()
        .is_some_and(|f| f.source == Some(card) && f.attacker == dealer)
    {
        let admitted = state
            .attack_commands
            .frames
            .last()
            .is_some_and(|f| f.admitted);
        return if !admitted
            || crate::combat_loop::combat_is_ending(state)
            || !attacker_alive(state, dealer)
        {
            HitOwner::Stopped
        } else {
            HitOwner::Command
        };
    }
    if !card_is_attack(state, card) {
        return HitOwner::Plain;
    }
    // A command already ran for this card in this dispatch (closed by its
    // handler): a later hit is not part of a new command unless the handler
    // opens one. Hits outside any command are plain damage.
    if state.attack_commands.dispatch_opened {
        return HitOwner::Plain;
    }
    if begin(state, dealer, props, Some(card)) {
        HitOwner::Command
    } else {
        HitOwner::Stopped
    }
}

/// After one command hit produced a result (C# `AttackCommand.AddResultsInternal`
/// with the hit's own `CreatureCmd.Damage` results): append it to the innermost
/// command's results.
pub(crate) fn record_result(state: &mut CombatState, receiver: CreatureUid, unblocked: i32) {
    if let Some(frame) = state.attack_commands.frames.last_mut() {
        frame.results.push((receiver, unblocked));
    }
}

/// Saved around one `OnPlay` dispatch.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DispatchMark {
    depth: usize,
    opened: bool,
    damage_depth: u32,
    fanout: Option<Option<HitOwner>>,
}

pub(crate) fn dispatch_enter(state: &mut CombatState) -> DispatchMark {
    let mark = DispatchMark {
        depth: state.attack_commands.frames.len(),
        opened: state.attack_commands.dispatch_opened,
        damage_depth: state.attack_commands.damage_depth,
        fanout: state.attack_commands.fanout,
    };
    state.attack_commands.fanout = None;
    state.attack_commands.dispatch_opened = false;
    // A card played from inside a damage call (an auto-play a reaction
    // triggers) runs its own command from the top.
    state.attack_commands.damage_depth = 0;
    mark
}

/// After one `OnPlay` dispatch (while `resolving_card` still names the card):
/// close what it opened, and run the empty admitted command of an Attack card
/// whose `OnPlay` reached no hit.
pub(crate) fn dispatch_exit(state: &mut CombatState, mark: DispatchMark, card: CardUid) {
    while state.attack_commands.frames.len() > mark.depth {
        end(state);
    }
    if !state.attack_commands.dispatch_opened
        && card_is_attack(state, card)
        && !state.player.attack_command_suppressed_this_play
    {
        let card_id = state
            .cards
            .get(card.0.get() as usize - 1)
            .map(|c| c.card_id);
        let attacker = if card_id.is_some_and(crate::cards::CardId::is_osty_attack) {
            state
                .player
                .pet_uid
                .map_or(CreatureRef::Nobody, CreatureRef::Enemy)
        } else {
            CreatureRef::Player
        };
        if begin(state, attacker, ValueProps::MOVE, Some(card)) {
            end(state);
        }
    }
    state.attack_commands.dispatch_opened = mark.opened;
    state.attack_commands.damage_depth = mark.damage_depth;
    state.attack_commands.fanout = mark.fanout;
}

/// Vigor's additive bonus for a hit from `card` (`None`: not a card's damage).
/// C# `VigorPower.ModifyDamageAdditive`: nothing for another card's damage while
/// a command is latched.
#[must_use]
pub fn vigor_applies(state: &CombatState, card: Option<CardUid>) -> bool {
    match state.attack_commands.vigor {
        Some(latch) => card.is_none() || card == latch.source,
        None => true,
    }
}

/// Gigantification's ×3 for a hit from `card`. C#
/// `GigantificationPower.ModifyDamageMultiplicative`: while latched, only the
/// latched command's card. (`None`: no card is resolving, e.g. a hand preview,
/// which C# computes with the previewed card as source.)
#[must_use]
pub fn gigantification_applies(state: &CombatState, card: Option<CardUid>) -> bool {
    match state.attack_commands.gigantification {
        Some(latch) => card.is_none() || card == latch.source,
        None => true,
    }
}
