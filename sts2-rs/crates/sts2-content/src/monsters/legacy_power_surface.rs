//! The single canonical "apply a power to the player through the legacy
//! `state.player.powers` surface" helper shared by every monster module.
//!
//! Monster `effect_fn`s that debuff the PLAYER route through here (SELF-buffs
//! go through `card_effects::apply_power_to_creature`) so wire-side observers
//! — `state_to_dict`, the Python parity harness — see the power. Hook routing
//! (modify chains / Artifact negation) is intentionally bypassed, matching the
//! historical behaviour of the per-module copies this module replaces.
//!
//! Each of `act1_overgrowth` / `act1_underdocks` / `act2_hive` / `act3_glory` /
//! `shared` used to carry its own byte-copy of this helper. The mirror+legacy
//! union fix (below) was applied to the `act3_glory` copy only, so the other
//! four kept the legacy-only scan and its duplicate-power bug — bead
//! `sts2-rs-60kxh`. One implementation, imported five times, is the fix.

use sts2_core::{CombatState, PowerKind};

/// Search the mirror+legacy UNION for a live player power of `kind`.
///
/// A card-applied player power lives on the player's mirror Creature
/// (`apply_power_to_creature` writes there), so a raw `state.player.powers`
/// scan misses it and pushes a DUPLICATE same-kind power instead of merging.
/// Fuzz `THE_LOST_AND_FORGOTTEN` (~30 subjects): TheForgotten's Dexterity(-2)
/// steal failed to fold into a card-granted Dexterity(+1) → 2 Dexterity powers
/// (cs `powers.len` 4 vs rs 5).
fn find_player_power(state: &CombatState, kind: PowerKind) -> Option<std::num::NonZeroU32> {
    state.player_power_uids().into_iter().find_map(|puid| {
        state
            .powers
            .iter()
            .find(|p| p.id == puid.0 && p.kind == kind)
            .map(|p| p.id)
    })
}

/// Push a fresh player power onto the legacy surface and return nothing.
///
/// `skip_next_tick` mirrors `apply_power_to_creature` (card_effects.rs) so a
/// freshly-applied Vulnerable / Weak / Frail skips its first turn-end
/// decrement tick.
fn push_new_player_power(state: &mut CombatState, kind: PowerKind, amount: i32) {
    let next_id_raw = state
        .powers
        .iter()
        .map(|p| p.id.get())
        .max()
        .map_or(1, |m| m.saturating_add(1));
    let new_id = std::num::NonZeroU32::new(next_id_raw).expect("power id non-zero");
    state.powers.push(sts2_core::Power {
        id: new_id,
        kind,
        amount,
        counter: 0,
        applier: None,
        skip_next_tick: matches!(
            kind,
            PowerKind::Vulnerable | PowerKind::Weak | PowerKind::Frail
        ),
    });
    state.player.powers.push(sts2_core::PowerUid(new_id));
}

/// Apply `amount` stacks of `kind` to the player via the legacy
/// `state.player.powers` push path, merging into an existing instance of the
/// same kind when one is live on either the mirror Creature or the legacy
/// surface.
///
/// Guards `amount <= 0`: this is the positive-only path used by debuffs like
/// Weak / Vulnerable / Frail. For signed *stat* powers use
/// [`apply_signed_power_to_player`]. Bead `sts2-rs-fad0f`.
pub(crate) fn apply_power_to_player_via_legacy_surface(
    state: &mut CombatState,
    kind: PowerKind,
    amount: i32,
) {
    if amount <= 0 {
        return;
    }
    if let Some(id) = find_player_power(state, kind) {
        if let Some(pow) = state.powers.get_mut_by_id(id) {
            pow.amount = pow.amount.saturating_add(amount);
        }
        record_player_power_row(state, kind, amount);
        return;
    }
    push_new_player_power(state, kind, amount);
    record_player_power_row(state, kind, amount);
    if kind == PowerKind::Ringing {
        sts2_core::monster_power_effects::ringing_after_applied(state);
    }
}

/// Log a player-side power apply that came through THIS legacy surface.
///
/// The core chokepoint (`card_effects::apply_power_to_creature_inner`) records
/// its own row, but monster debuffs never reach it — they push straight onto
/// `state.player.powers` here — so without this a fight's log would show
/// "Twig Slime used Sticky Shot" and never the Weak it applied. No applier is
/// threaded through this surface, so the row renders C#'s applier-less phrasing
/// ("X received N WEAK"); the move row immediately above it names the monster.
fn record_player_power_row(state: &mut CombatState, kind: PowerKind, amount: i32) {
    if !sts2_core::event_log::armed(state) {
        return;
    }
    let receiver = sts2_core::event_log::LogCreature::Player(state.player.character);
    sts2_core::event_log::record(
        state,
        sts2_core::event_log::EventLogEntry::PowerReceived {
            applier: None,
            receiver,
            power: kind,
            amount,
        },
    );
}

/// Apply a SIGNED `amount` of `kind` to the player via the legacy
/// `state.player.powers` surface.
///
/// Unlike [`apply_power_to_player_via_legacy_surface`] this permits negative
/// amounts for signed *stat* powers — e.g. TheForgotten's MIASMA stealing 2
/// Dexterity (`DexterityPower(-2)` on the player in C#). Stacks additively
/// onto an existing power, and REMOVES the instance when it nets to exactly 0
/// (C# `ShouldRemoveDueToAmount`, mirroring
/// `card_effects::apply_signed_attr_power`) — the older merge left a dead
/// 0-amount power behind, so the player carried one MORE power than C#
/// (`powers.len` cs N vs rs N+1). Bead `sts2-rs-96y80`.
pub(crate) fn apply_signed_power_to_player(state: &mut CombatState, kind: PowerKind, amount: i32) {
    if amount == 0 {
        return;
    }
    if let Some(id) = find_player_power(state, kind) {
        let mut remove = false;
        if let Some(pow) = state.powers.get_mut_by_id(id) {
            pow.amount = pow.amount.saturating_add(amount);
            remove = pow.amount == 0;
        }
        if remove {
            state.powers.retain(|p| p.id != id);
            state.player.powers.retain(|u| u.0 != id);
            for c in &mut state.creatures {
                c.powers.retain(|u| u.0 != id);
            }
        }
        record_player_power_row(state, kind, amount);
        return;
    }
    // Signed attr powers (Strength / Dexterity) carry no turn-end decrement,
    // so the `skip_next_tick` flag `push_new_player_power` sets for the
    // Vulnerable / Weak / Frail trio never applies to them.
    push_new_player_power(state, kind, amount);
    record_player_power_row(state, kind, amount);
}

#[cfg(test)]
mod tests {
    use super::{apply_power_to_player_via_legacy_surface, apply_signed_power_to_player};
    use sts2_core::card_effects::apply_power_to_creature;
    use sts2_core::{CombatState, PowerKind};

    /// A state whose player has its mirror Creature wired up, matching what
    /// `combat_start` builds — the shape in which the legacy-only scan bug
    /// reproduces.
    fn state_with_player_mirror() -> CombatState {
        let mut state = CombatState::default();
        state.player.max_hp = 80;
        state.player.current_hp = 80;
        state.init_player_creature();
        state
    }

    fn instances_of(state: &CombatState, kind: PowerKind) -> usize {
        state.powers.iter().filter(|p| p.kind == kind).count()
    }

    fn amount_of(state: &CombatState, kind: PowerKind) -> i32 {
        state
            .powers
            .iter()
            .filter(|p| p.kind == kind)
            .map(|p| p.amount)
            .sum()
    }

    /// Bead `sts2-rs-60kxh`. A card-applied player power lives on the mirror
    /// Creature, NOT on `state.player.powers`. A monster debuff of the same
    /// kind must merge into it. The pre-fix legacy-only scan missed the mirror
    /// instance and pushed a duplicate — this asserts the single-instance
    /// merge that every monster module now shares.
    #[test]
    fn merges_into_a_mirror_applied_power_instead_of_duplicating() {
        let mut state = state_with_player_mirror();
        let player = state.player.creature_uid.expect("mirror creature");
        apply_power_to_creature(&mut state, player, PowerKind::Weak, 1);

        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Weak, 2);

        assert_eq!(
            instances_of(&state, PowerKind::Weak),
            1,
            "monster debuff must fold into the mirror-applied Weak, not spawn a second instance",
        );
        assert_eq!(
            amount_of(&state, PowerKind::Weak),
            3,
            "1 (card) + 2 (monster)"
        );
    }

    /// The signed path has the same union requirement — this is the
    /// THE_LOST_AND_FORGOTTEN fuzz case: TheForgotten's Dexterity(-2) steal
    /// folding into a card-granted Dexterity(+1).
    #[test]
    fn signed_apply_merges_into_a_mirror_applied_power() {
        let mut state = state_with_player_mirror();
        let player = state.player.creature_uid.expect("mirror creature");
        apply_power_to_creature(&mut state, player, PowerKind::Dexterity, 1);

        apply_signed_power_to_player(&mut state, PowerKind::Dexterity, -2);

        assert_eq!(
            instances_of(&state, PowerKind::Dexterity),
            1,
            "Dexterity steal must fold into the card-granted instance",
        );
        assert_eq!(amount_of(&state, PowerKind::Dexterity), -1, "1 + (-2)");
    }

    /// A signed power netting to exactly 0 is REMOVED (C#
    /// `ShouldRemoveDueToAmount`) — including from the mirror Creature's uid
    /// list, which is where a mirror-applied instance is referenced.
    #[test]
    fn signed_apply_removes_a_mirror_power_that_nets_to_zero() {
        let mut state = state_with_player_mirror();
        let player = state.player.creature_uid.expect("mirror creature");
        apply_power_to_creature(&mut state, player, PowerKind::Strength, 2);

        apply_signed_power_to_player(&mut state, PowerKind::Strength, -2);

        assert_eq!(instances_of(&state, PowerKind::Strength), 0);
        assert!(
            state.player_power_uids().is_empty(),
            "the dead power must be unlinked from the mirror Creature too",
        );
    }

    /// Repeated applications still stack into one instance on the legacy-only
    /// shape (no mirror Creature) — the pre-existing contract kept intact.
    #[test]
    fn stacks_into_existing_instance_without_a_mirror_creature() {
        let mut state = CombatState::default();
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Frail, 2);
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Frail, 3);

        assert_eq!(instances_of(&state, PowerKind::Frail), 1);
        assert_eq!(amount_of(&state, PowerKind::Frail), 5);
        assert_eq!(state.player.powers.len(), 1);
    }

    /// The unsigned path is positive-only: it is the Weak / Vulnerable / Frail
    /// surface, and a non-positive amount must be a no-op rather than a
    /// negative stack.
    #[test]
    fn unsigned_apply_ignores_non_positive_amounts() {
        let mut state = state_with_player_mirror();
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Vulnerable, 0);
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Vulnerable, -3);
        assert_eq!(instances_of(&state, PowerKind::Vulnerable), 0);
    }
}
