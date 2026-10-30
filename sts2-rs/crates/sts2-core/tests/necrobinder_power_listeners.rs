//! Necrobinder power-listener integration tests (bead `sts2-rs-4wppb`).
//!
//! Each of these Necrobinder powers was previously *inert*: the card played and
//! stamped the `PowerKind`, but no hook arm read it at runtime. These tests pin
//! the now-wired listener bodies end to end through the public `dispatch_*`
//! aggregators (which apply the effects each per-listener body returns), every
//! one mirroring its C# `PowerModel` in
//! `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Powers/`.
//!
//! These exercise pure-core surfaces (no content registry needed): powers are
//! pushed directly onto the player / a spawned enemy and the hook dispatched.

use sts2_core::card_effects;
use sts2_core::cards::{Card, CardId, CardTags, CardType, KeywordOverride};
use sts2_core::hooks::{
    self, CardCostCtx, CardCtx, CombatLifecycleCtx, CreatureRef, DamageCtx, DamageMul, ListenerRef,
    MaxEnergyCtx, PowerAppliedCtx, TurnSideCtx, ValueProps,
};
use sts2_core::{CardUid, CombatState, CreatureUid, MonsterKind, Power, PowerKind, PowerUid, Side};

/// Push a fresh `kind(amount)` power onto the player's legacy power list. The
/// uid is derived from the current `powers` length so it stays unique + nonzero.
/// Drive the folded `AfterPlayerTurnStart` powers (SummonNextTurn et al.) the
/// way `combat_loop` does — over a power snapshot, applying each instance's
/// effects. These kinds are skipped by the `after_side_turn_start` walk (bead
/// `sts2-rs-r2ixj`), so a test of their turn-start effect must drive this path.
fn dispatch_folded_after_player_turn_start(state: &mut CombatState, ctx: &TurnSideCtx) {
    let uids = state.player_power_uids();
    for uid in uids {
        if let Some(eff) = hooks::collect_folded_after_player_turn_start_one(state, uid, ctx) {
            hooks::apply_effects(state, &eff);
        }
    }
}

fn push_player_power(state: &mut CombatState, kind: PowerKind, amount: i32) -> PowerUid {
    let pid = PowerUid::new(state.powers.len() as u32 + 1).expect("nonzero power uid");
    state.powers.push(Power {
        id: pid.0,
        kind,
        amount,
        counter: 0,
        applier: None,
        skip_next_tick: false,
    });
    state.player.powers.push(pid);
    pid
}

/// Attach a fresh `kind(amount)` power to an already-spawned enemy creature.
fn attach_enemy_power(
    state: &mut CombatState,
    enemy: CreatureUid,
    kind: PowerKind,
    amount: i32,
) -> PowerUid {
    let pid = PowerUid::new(state.powers.len() as u32 + 1).expect("nonzero power uid");
    state.powers.push(Power {
        id: pid.0,
        kind,
        amount,
        counter: 0,
        applier: None,
        skip_next_tick: false,
    });
    let c = state
        .creatures
        .iter_mut()
        .find(|c| c.id == enemy.0)
        .expect("spawned enemy creature present");
    c.powers.push(pid);
    pid
}

/// Sum the `Doom` stacks currently attached to the creature `uid`.
fn doom_on_creature(state: &CombatState, uid: CreatureUid) -> i32 {
    let c = state
        .creatures
        .iter()
        .find(|c| c.id == uid.0)
        .expect("creature present");
    c.powers
        .iter()
        .filter_map(|pu| {
            state
                .powers
                .iter()
                .find(|p| p.id == pu.0 && p.kind == PowerKind::Doom)
        })
        .map(|p| p.amount)
        .sum()
}

/// C# `FriendshipPower.ModifyMaxEnergy`: `amount + base.Amount` for the owning
/// player — a permanent +Amount max-energy buff while the Counter Buff exists.
#[test]
fn friendship_folds_plus_amount_into_max_energy() {
    let mut state = CombatState::default();
    push_player_power(&mut state, PowerKind::Friendship, 2);
    let delta = hooks::dispatch_modify_max_energy(&state, &MaxEnergyCtx::default());
    assert_eq!(
        delta, 2,
        "Friendship(2) folds +2 max energy for the owning player"
    );
}

/// C# `EnergyNextTurnPower.AfterEnergyReset`: for the owning player,
/// `GainEnergy(Amount)` then `PowerCmd.Remove(this)` — a one-shot gain that
/// fully removes itself (NOT a per-turn decrement).
#[test]
fn energy_next_turn_grants_energy_then_self_removes() {
    let mut state = CombatState::default();
    state.gain_energy(3);
    let pid = push_player_power(&mut state, PowerKind::EnergyNextTurn, 2);

    hooks::dispatch_after_energy_reset(&mut state, &CombatLifecycleCtx::default());

    assert_eq!(state.player.energy, 5, "3 base + EnergyNextTurn(2) = 5");
    assert!(
        !state.powers.iter().any(|p| p.id == pid.0),
        "EnergyNextTurn removes itself entirely after firing"
    );
    assert!(
        !state.player.powers.contains(&pid),
        "EnergyNextTurn uid unlinked from the player power list"
    );
}

/// C# `StarNextTurnPower.AfterEnergyReset`: for the owning player,
/// `GainStars(Amount)` then `PowerCmd.Remove(this)` — the stars-side analogue
/// of `EnergyNextTurn`. A one-shot gain that fully removes itself, so the
/// power never lingers as a stale stack (the parity divergence this wiring
/// fixes — Regent's Convergence / HiddenCache apply it and C# converts it at
/// the next energy reset).
#[test]
fn star_next_turn_grants_stars_then_self_removes() {
    let mut state = CombatState::default();
    let pid = push_player_power(&mut state, PowerKind::StarNextTurn, 2);

    hooks::dispatch_after_energy_reset(&mut state, &CombatLifecycleCtx::default());

    assert_eq!(state.player.stars, 2, "StarNextTurn(2) grants 2 stars");
    assert!(
        !state.powers.iter().any(|p| p.id == pid.0),
        "StarNextTurn removes itself entirely after firing"
    );
    assert!(
        !state.player.powers.contains(&pid),
        "StarNextTurn uid unlinked from the player power list"
    );
}

/// C# `NeurosurgePower.AfterSideTurnStart`: on the owner's side turn start,
/// `PowerCmd.Apply<DoomPower>(Owner, Amount, Owner, null)` — the owner
/// doom-marks itself by Amount each of its own turns (no tick-down).
#[test]
fn neurosurge_dooms_self_on_owner_turn_start() {
    let mut state = CombatState::default();
    state.player.current_hp = 60;
    state.player.max_hp = 60;
    let player_uid = state.init_player_creature();
    push_player_power(&mut state, PowerKind::Neurosurge, 3);

    hooks::dispatch_after_side_turn_start(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(
        doom_on_creature(&state, player_uid),
        3,
        "Neurosurge(3) self-applies Doom(3) on the player's turn start"
    );

    // It does not tick down — a second owner turn start stacks Doom to 6.
    hooks::dispatch_after_side_turn_start(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(
        doom_on_creature(&state, player_uid),
        6,
        "Neurosurge keeps stacking Doom each owner turn (no decrement)"
    );

    // Wrong side (enemy turn start) is a no-op for a player-owned Neurosurge.
    hooks::dispatch_after_side_turn_start(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(
        doom_on_creature(&state, player_uid),
        6,
        "enemy-side turn start does not fire a player-owned Neurosurge"
    );
}

/// C# `OblivionPower.Before/AfterCardPlayed` + `AfterTurnEnd`: a player-applied
/// Debuff living on an enemy. For every card the player plays *after* Oblivion
/// exists, apply `Amount` Doom to the owner enemy; the power removes itself at
/// the player's turn end. The `BeforeCardPlayed` arm
/// (`dispatch_before_card_played_powers`) records the card so `AfterCardPlayed`
/// fires — the card that APPLIED Oblivion is never armed (its before-play arm
/// ran before the power existed), so it does NOT self-Doom.
#[test]
fn oblivion_dooms_owner_each_card_and_self_removes_at_player_turn_end() {
    let mut state = CombatState::default();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    let pid = attach_enemy_power(&mut state, enemy, PowerKind::Oblivion, 3);

    // The applying card: AfterCardPlayed WITHOUT a preceding arm must NOT Doom
    // (C# `amountsForPlayedCards` never recorded the Oblivion card itself).
    hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(
        doom_on_creature(&state, enemy),
        0,
        "the card that applied Oblivion does not self-Doom the owner"
    );

    // A real subsequent card play: arm in BeforeCardPlayed, then fire.
    hooks::dispatch_before_card_played_powers(&mut state, /*resolved_cost=*/ 1, false);
    hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(
        doom_on_creature(&state, enemy),
        3,
        "first armed player card applies Oblivion's Amount (3) Doom to the owner enemy"
    );

    hooks::dispatch_before_card_played_powers(&mut state, 1, false);
    hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(
        doom_on_creature(&state, enemy),
        6,
        "each subsequent armed player card stacks another 3 Doom"
    );

    // End of the PLAYER's turn removes Oblivion (regardless of owner side).
    hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert!(
        !state.powers.iter().any(|p| p.id == pid.0),
        "Oblivion removes itself at the player's turn end"
    );
}

/// Regression (`sts2-rs-oblivion-snapshot`): a 2nd Oblivion played while the
/// first is armed stacks the live `Amount` during its OnPlay (3 → 6), but the
/// Doom granted for THAT play must be the pre-OnPlay SNAPSHOT (3), not the
/// since-stacked live amount (6). C# `OblivionPower.BeforeCardPlayed` records
/// `base.Amount` BEFORE OnPlay; `AfterCardPlayed` grants that recorded value.
/// The bug granted Doom off the live amount (parity fuzz: a monster's Doom
/// cs=42 rs=45 — Rust over-granted by the +3 stack from the same play).
#[test]
fn oblivion_grants_pre_onplay_amount_snapshot_not_live_stacked_amount() {
    let mut state = CombatState::default();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    let pid = attach_enemy_power(&mut state, enemy, PowerKind::Oblivion, 3);

    // Before-play arm: snapshots the current Amount (3) into the counter.
    hooks::dispatch_before_card_played_powers(&mut state, /*resolved_cost=*/ 1, false);

    // A 2nd Oblivion's OnPlay stacks the live amount 3 → 6 mid-resolution
    // (before AfterCardPlayed fires for this same card play).
    state
        .powers
        .iter_mut()
        .find(|p| p.id == pid.0)
        .expect("Oblivion power present")
        .amount = 6;

    hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(
        doom_on_creature(&state, enemy),
        3,
        "Oblivion grants the pre-OnPlay snapshot (3), not the since-stacked live amount (6)"
    );
}

/// The Oblivion debuff survives the *enemy's* turn end — it only clears on the
/// player's turn end (C# `side == CombatSide.Player`).
#[test]
fn oblivion_survives_enemy_turn_end() {
    let mut state = CombatState::default();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    let pid = attach_enemy_power(&mut state, enemy, PowerKind::Oblivion, 3);

    hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert!(
        state.powers.iter().any(|p| p.id == pid.0),
        "Oblivion is NOT removed at the enemy's turn end"
    );
}

/// C# `ShroudPower.AfterPowerAmountChanged`: `if applier == Owner && power
/// is DoomPower { GainBlock(Owner, Amount, Unpowered) }`. When the owning
/// player applies Doom to any creature, the player gains `Amount` block.
#[test]
fn shroud_gains_block_when_player_applies_doom() {
    let mut state = CombatState::default();
    state.player.current_hp = 60;
    state.player.max_hp = 60;
    let _ = state.init_player_creature();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    push_player_power(&mut state, PowerKind::Shroud, 5);

    // Simulate the player applying Doom(3) to the enemy.
    let ctx = PowerAppliedCtx {
        target: CreatureRef::Enemy(enemy),
        power_kind: PowerKind::Doom,
        delta: 3,
        applier: Some(CreatureRef::Player),
    };
    hooks::dispatch_after_power_applied(&mut state, &ctx);
    assert_eq!(
        state.player.block, 5,
        "Shroud(5) grants 5 block when the player applies Doom"
    );

    // A second Doom application grants another 5.
    hooks::dispatch_after_power_applied(&mut state, &ctx);
    assert_eq!(
        state.player.block, 10,
        "each player Doom application re-fires Shroud's block grant"
    );
}

/// Shroud does not fire when the applier is not the owner, nor on a
/// non-Doom power application.
#[test]
fn shroud_ignores_non_player_applier_and_non_doom() {
    let mut state = CombatState::default();
    state.player.current_hp = 60;
    state.player.max_hp = 60;
    let _ = state.init_player_creature();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    push_player_power(&mut state, PowerKind::Shroud, 5);

    // Enemy-applied Doom: no Shroud block.
    hooks::dispatch_after_power_applied(
        &mut state,
        &PowerAppliedCtx {
            target: CreatureRef::Player,
            power_kind: PowerKind::Doom,
            delta: 3,
            applier: Some(CreatureRef::Enemy(enemy)),
        },
    );
    assert_eq!(
        state.player.block, 0,
        "enemy-applied Doom does not arm Shroud"
    );

    // Player-applied non-Doom (Vulnerable) without Vicious: no Shroud block.
    hooks::dispatch_after_power_applied(
        &mut state,
        &PowerAppliedCtx {
            target: CreatureRef::Enemy(enemy),
            power_kind: PowerKind::Vulnerable,
            delta: 2,
            applier: Some(CreatureRef::Player),
        },
    );
    assert_eq!(
        state.player.block, 0,
        "a non-Doom player application does not arm Shroud"
    );
}

/// Regression (Necrobinder fuzz seeds 192/340/495): Shroud must fire through
/// the APPLIER apply surface, not only the direct `dispatch_after_power_applied`
/// call. NEUROSURGE self-Dooms the player at turn start via
/// `Effect::ApplyPower { applier: player }` →
/// `apply_power_to_creature_with_applier`, which previously fired only the relic
/// observer channels and skipped the player reactors — so Shroud never granted
/// its block off NEUROSURGE's self-Doom (C# block 2, Rust 0). The applier path
/// now fires `dispatch_after_power_applied` with the resolved applier.
#[test]
fn shroud_fires_through_the_applier_apply_surface() {
    let mut state = CombatState::default();
    state.player.current_hp = 60;
    state.player.max_hp = 60;
    let player_uid = state.init_player_creature();
    push_player_power(&mut state, PowerKind::Shroud, 2);

    // NEUROSURGE-style self-Doom: the player applies Doom to its own mirror
    // creature, recording itself as the applier.
    card_effects::apply_power_to_creature_with_applier(
        &mut state,
        player_uid,
        PowerKind::Doom,
        3,
        Some(player_uid),
    );
    assert_eq!(
        state.player.block, 2,
        "Shroud(2) grants 2 block when the player self-Dooms via the applier path"
    );
}

/// The applier surface must NOT arm Shroud when an ENEMY is the applier
/// (e.g. a monster dooming the player), matching the `applier == Owner` gate.
#[test]
fn shroud_ignores_enemy_applier_through_the_applier_surface() {
    let mut state = CombatState::default();
    state.player.current_hp = 60;
    state.player.max_hp = 60;
    let player_uid = state.init_player_creature();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    push_player_power(&mut state, PowerKind::Shroud, 2);

    // Enemy applies Doom to the player via the applier surface: no Shroud block.
    card_effects::apply_power_to_creature_with_applier(
        &mut state,
        player_uid,
        PowerKind::Doom,
        3,
        Some(enemy),
    );
    assert_eq!(
        state.player.block, 0,
        "an enemy-applied Doom through the applier surface does not arm Shroud"
    );
}

/// C# `SummonNextTurnPower.AfterPlayerTurnStart`: on the owning player's
/// turn start, `OstyCmd.Summon(player, Amount, this)` then
/// `PowerCmd.Remove(this)` — summon the Osty by `Amount` and self-remove.
#[test]
fn summon_next_turn_summons_osty_then_self_removes() {
    let mut state = CombatState::default();
    state.player.current_hp = 60;
    state.player.max_hp = 60;
    let _ = state.init_player_creature();
    let pid = push_player_power(&mut state, PowerKind::SummonNextTurn, 8);
    assert!(state.player.pet_uid.is_none(), "no Osty before turn start");

    dispatch_folded_after_player_turn_start(&mut state, &TurnSideCtx { side: Side::Player });

    let pet = state
        .player
        .pet_uid
        .expect("Osty summoned at player turn start");
    let osty = state
        .creatures
        .iter()
        .find(|c| c.id == pet.0)
        .expect("Osty creature present");
    assert_eq!(osty.max_hp, 8, "SummonNextTurn(8) summons an 8-HP Osty");
    assert!(
        !state.powers.iter().any(|p| p.id == pid.0),
        "SummonNextTurn removes itself after firing"
    );
}

/// SummonNextTurn fires only at the *player's* side turn start — the
/// enemy's turn start is a no-op (no Osty, power persists).
#[test]
fn summon_next_turn_does_not_fire_on_enemy_turn() {
    let mut state = CombatState::default();
    state.player.current_hp = 60;
    state.player.max_hp = 60;
    let _ = state.init_player_creature();
    let pid = push_player_power(&mut state, PowerKind::SummonNextTurn, 8);

    hooks::dispatch_after_side_turn_start(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert!(
        state.player.pet_uid.is_none(),
        "enemy-side turn start does not summon the player's Osty"
    );
    assert!(
        state.powers.iter().any(|p| p.id == pid.0),
        "SummonNextTurn persists through the enemy's turn start"
    );
}

/// C# `SpiritOfAshPower.BeforeCardPlayed`: `if Card.Owner == Owner.Player &&
/// Card.Keywords.Contains(Ethereal) { GainBlock(Owner, Amount, Unpowered) }`.
/// The block fires when the played card is Ethereal, independent of its cost
/// (so it fires even on a sub-2-cost card, unlike DanseMacabre).
#[test]
fn spirit_of_ash_gains_block_when_an_ethereal_card_is_played() {
    let mut state = CombatState::default();
    push_player_power(&mut state, PowerKind::SpiritOfAsh, 6);

    // Non-Ethereal card (cost 1): no block.
    hooks::dispatch_before_card_played_powers(&mut state, 1, false);
    assert_eq!(
        state.player.block, 0,
        "a non-Ethereal play does not arm SpiritOfAsh"
    );

    // Ethereal card at cost 1 (< DanseMacabre's threshold): still +6 block.
    hooks::dispatch_before_card_played_powers(&mut state, 1, true);
    assert_eq!(
        state.player.block, 6,
        "SpiritOfAsh(6) grants 6 block on an Ethereal play regardless of cost"
    );

    // A second Ethereal play stacks another 6.
    hooks::dispatch_before_card_played_powers(&mut state, 1, true);
    assert_eq!(
        state.player.block, 12,
        "SpiritOfAsh re-fires per Ethereal play"
    );
}

/// Build an Ethereal card POD (uid 1) and push it into the arena.
fn push_ethereal_card(state: &mut CombatState) -> CardUid {
    let uid = CardUid::new(state.cards.len() as u32 + 1).expect("nonzero card uid");
    let card = Card {
        uid,
        keyword_override: Some(KeywordOverride::ETHEREAL),
        ..Card::pod_defaults()
    };
    state.cards.push(card);
    uid
}

/// C# `VeilpiercerPower.TryModifyEnergyCostInCombat`: an owned Ethereal card
/// costs 0 while Veilpiercer has charges; a non-Ethereal card is unaffected.
#[test]
fn veilpiercer_makes_owned_ethereal_card_cost_zero() {
    let mut state = CombatState::default();
    let eth = push_ethereal_card(&mut state);
    // The discount holds in hand (C# `Pile.Type` Hand or Play).
    state.player.hand.push(eth);
    push_player_power(&mut state, PowerKind::Veilpiercer, 2);

    let eth_ctx = CardCostCtx {
        owner: CreatureRef::Player,
        card_id: Some(CardId::StrikeIronclad),
        card_uid: Some(eth),
        card_type: Some(CardType::Attack),
        card_tags: CardTags::NONE,
        base_cost: 3,
    };
    assert_eq!(
        hooks::dispatch_modify_card_cost(&state, &eth_ctx),
        0,
        "Veilpiercer drops an owned Ethereal card to cost 0"
    );

    // A non-Ethereal card (no keyword override → not Ethereal by id) keeps its
    // base cost.
    let plain_uid = CardUid::new(state.cards.len() as u32 + 1).expect("nonzero");
    state.cards.push(Card {
        uid: plain_uid,
        ..Card::pod_defaults()
    });
    state.player.hand.push(plain_uid);
    let plain_ctx = CardCostCtx {
        card_uid: Some(plain_uid),
        ..eth_ctx
    };
    assert_eq!(
        hooks::dispatch_modify_card_cost(&state, &plain_ctx),
        3,
        "Veilpiercer does not discount a non-Ethereal card"
    );

    // An Ethereal card in the discard pile keeps its cost (sts2-agent fork).
    let discarded = push_ethereal_card(&mut state);
    state.player.discard.push(discarded);
    let discarded_ctx = CardCostCtx {
        card_uid: Some(discarded),
        ..eth_ctx
    };
    assert_eq!(
        hooks::dispatch_modify_card_cost(&state, &discarded_ctx),
        3,
        "Veilpiercer does not discount an Ethereal card outside hand and play"
    );
}

/// C# `VeilpiercerPower.BeforeCardPlayed`: each Ethereal play consumes one
/// charge; the power is removed when it reaches 0.
#[test]
fn veilpiercer_decrements_on_ethereal_play_and_removes_at_zero() {
    let mut state = CombatState::default();
    let pid = push_player_power(&mut state, PowerKind::Veilpiercer, 2);

    // First Ethereal play: 2 → 1 (still present).
    hooks::dispatch_before_card_played_powers(&mut state, 1, true);
    let amt = state
        .powers
        .iter()
        .find(|p| p.id == pid.0)
        .map(|p| p.amount);
    assert_eq!(amt, Some(1), "first Ethereal play consumes one charge");

    // A non-Ethereal play does not consume a charge.
    hooks::dispatch_before_card_played_powers(&mut state, 1, false);
    assert_eq!(
        state
            .powers
            .iter()
            .find(|p| p.id == pid.0)
            .map(|p| p.amount),
        Some(1),
        "a non-Ethereal play leaves Veilpiercer untouched"
    );

    // Second Ethereal play: 1 → 0 → removed.
    hooks::dispatch_before_card_played_powers(&mut state, 1, true);
    assert!(
        !state.powers.iter().any(|p| p.id == pid.0),
        "Veilpiercer is removed when its last charge is consumed"
    );
}

/// Push an Ethereal card POD and append its uid to the draw pile (front-drawn).
fn push_ethereal_into_draw(state: &mut CombatState) -> CardUid {
    let uid = push_ethereal_card(state);
    state.player.draw.push(uid);
    uid
}

/// C# `PagestormPower.AfterCardDrawn`: drawing an owned Ethereal card draws
/// `Amount` more. The extra draws recurse through the on-draw surface, so a
/// pile of Ethereal cards cascades: drawing one pulls the whole chain.
#[test]
fn pagestorm_cascades_ethereal_draws() {
    let mut state = CombatState::default();
    push_player_power(&mut state, PowerKind::Pagestorm, 1);
    // Three Ethereal cards in the draw pile (uids 1..=3, front-drawn).
    for _ in 0..3 {
        push_ethereal_into_draw(&mut state);
    }

    let moved = state.draw_into_hand(1);
    // Drew 1 directly; Pagestorm(1) fires per Ethereal, recursively pulling the
    // remaining two until the pile is empty.
    assert_eq!(
        state.player.hand.len(),
        3,
        "Pagestorm cascades all 3 Ethereal"
    );
    assert_eq!(moved, 1, "draw_into_hand reports only its own direct draw");
}

/// Without Pagestorm, drawing one Ethereal card draws exactly one — no rider.
#[test]
fn no_pagestorm_means_no_extra_draw() {
    let mut state = CombatState::default();
    for _ in 0..3 {
        push_ethereal_into_draw(&mut state);
    }
    state.draw_into_hand(1);
    assert_eq!(
        state.player.hand.len(),
        1,
        "no Pagestorm power → drawing an Ethereal draws nothing extra"
    );
}

/// Pagestorm only reacts to Ethereal draws — a non-Ethereal card draws nothing
/// extra even while the power is active.
#[test]
fn pagestorm_ignores_non_ethereal_draws() {
    let mut state = CombatState::default();
    push_player_power(&mut state, PowerKind::Pagestorm, 1);
    // Three plain (non-Ethereal) cards in the draw pile.
    for _ in 0..3 {
        let uid = CardUid::new(state.cards.len() as u32 + 1).expect("nonzero");
        state.cards.push(Card {
            uid,
            ..Card::pod_defaults()
        });
        state.player.draw.push(uid);
    }
    state.draw_into_hand(1);
    assert_eq!(
        state.player.hand.len(),
        1,
        "Pagestorm does not fire on a non-Ethereal draw"
    );
}

/// C# `HangPower.ModifyDamageMultiplicative`: damage the Hang card deals to a
/// Hang-afflicted target is multiplied by the existing Hang stacks; non-Hang
/// sources (and absent Hang) leave the damage unscaled.
#[test]
fn hang_card_multiplies_its_damage_on_a_hang_afflicted_target() {
    let mut state = CombatState::default();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    // Target already carries Hang(3) from a prior play.
    let hang_pid = attach_enemy_power(&mut state, enemy, PowerKind::Hang, 3);

    // Put a Hang card on top of the play pile — the mid-resolution source the
    // listener reads (mirrors C# `cardSource`).
    let cuid = CardUid::new(state.cards.len() as u32 + 1).expect("nonzero");
    state.cards.push(Card {
        uid: cuid,
        card_id: CardId::Hang,
        ..Card::pod_defaults()
    });
    state.player.play.push(cuid);

    let ctx = DamageCtx {
        dealer: CreatureRef::Player,
        target: CreatureRef::Enemy(enemy),
        props: ValueProps::MOVE,
    };
    assert_eq!(
        hooks::modify_damage_multiplicative(&state, ListenerRef::Power(hang_pid), &ctx),
        DamageMul::new(3, 1),
        "Hang card damage on a Hang(3) target is ×3"
    );

    // With a non-Hang card on top of play (e.g. another attack), no multiplier.
    state.player.play.clear();
    let other = CardUid::new(state.cards.len() as u32 + 1).expect("nonzero");
    state.cards.push(Card {
        uid: other,
        card_id: CardId::StrikeIronclad,
        ..Card::pod_defaults()
    });
    state.player.play.push(other);
    assert_eq!(
        hooks::modify_damage_multiplicative(&state, ListenerRef::Power(hang_pid), &ctx),
        DamageMul::ONE,
        "non-Hang source does not scale on a Hang target"
    );
}

/// C# `ReaperFormPower.AfterDamageGiven`: a powered attack by the player (or
/// the player's Osty) applies Doom = `TotalDamage * Amount` to the target,
/// gating on TotalDamage (blocked + unblocked), not the unblocked residual.
#[test]
fn reaper_form_dooms_target_by_total_damage_times_amount() {
    let mut state = CombatState::default();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    push_player_power(&mut state, PowerKind::ReaperForm, 2);

    let ctx = DamageCtx {
        dealer: CreatureRef::Player,
        target: CreatureRef::Enemy(enemy),
        props: ValueProps::MOVE,
    };
    // total_damage = 10, of which 6 got through block.
    hooks::dispatch_after_damage_given(&mut state, &ctx, 6, 10, false);
    assert_eq!(
        doom_on_creature(&state, enemy),
        20,
        "ReaperForm(2) applies Doom = TotalDamage(10) * 2"
    );
}

/// ReaperForm gates on TotalDamage, so a fully-blocked powered hit (unblocked
/// = 0) still doom-marks; an unpowered hit does not fire at all.
#[test]
fn reaper_form_fires_on_fully_blocked_but_not_unpowered() {
    let mut state = CombatState::default();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    push_player_power(&mut state, PowerKind::ReaperForm, 3);

    let powered = DamageCtx {
        dealer: CreatureRef::Player,
        target: CreatureRef::Enemy(enemy),
        props: ValueProps::MOVE,
    };
    // Fully blocked: unblocked = 0 but TotalDamage = 4 → still dooms (4*3).
    hooks::dispatch_after_damage_given(&mut state, &powered, 0, 4, true);
    assert_eq!(
        doom_on_creature(&state, enemy),
        12,
        "a fully-blocked powered hit still triggers ReaperForm (gates on TotalDamage)"
    );

    // Unpowered hit (e.g. reflected/HP-loss style): no ReaperForm.
    let unpowered = DamageCtx {
        dealer: CreatureRef::Player,
        target: CreatureRef::Enemy(enemy),
        props: ValueProps::UNPOWERED,
    };
    hooks::dispatch_after_damage_given(&mut state, &unpowered, 5, 5, false);
    assert_eq!(
        doom_on_creature(&state, enemy),
        12,
        "an unpowered hit does not trigger ReaperForm"
    );
}

/// A minimal card factory for the SentryMode test: returns a POD card carrying
/// the requested id (SweepingGaze) so the core lane can materialize the token
/// without the content crate's real factory.
// `Option<Card>` is mandated by the `CardFactoryFn` pointer type this is
// registered as — the wrap is structural, not removable.
#[allow(clippy::unnecessary_wraps)]
fn stub_card_factory(id: CardId, _upgraded: bool) -> Option<Card> {
    Some(Card {
        card_id: id,
        ..Card::pod_defaults()
    })
}

/// C# `SentryModePower.BeforeHandDraw`: for the owning player, `Amount` times
/// create a SweepingGaze and add it to hand.
#[test]
fn sentry_mode_adds_sweeping_gaze_cards_to_hand() {
    sts2_core::register_card_factory(stub_card_factory);
    let mut state = CombatState::default();
    push_player_power(&mut state, PowerKind::SentryMode, 2);
    assert_eq!(state.player.hand.len(), 0, "hand empty before SentryMode");

    hooks::dispatch_before_hand_draw_powers(&mut state);

    assert_eq!(
        state.player.hand.len(),
        2,
        "SentryMode(2) adds 2 SweepingGaze cards to hand"
    );
    for &uid in &state.player.hand {
        let card = state
            .cards
            .iter()
            .find(|c| c.uid == uid)
            .expect("card present");
        assert_eq!(
            card.card_id,
            CardId::SweepingGaze,
            "SentryMode generates SweepingGaze tokens"
        );
    }
}

/// Current HP of the enemy creature `uid`.
fn enemy_hp(state: &CombatState, uid: CreatureUid) -> i32 {
    state
        .creatures
        .iter()
        .find(|c| c.id == uid.0)
        .expect("enemy present")
        .current_hp
}

/// Regression (`sts2-rs-deaths-door-doom`): `apply_power_to_enemy` arms
/// `doom_applied_by_player_this_turn` (DeathsDoor's `WasDoomAppliedThisTurn`)
/// ONLY when the Doom actually lands. C# scans for a real `PowerReceivedEntry`,
/// which a Doom applied to an already-dead target (e.g. BlightStrike's
/// damage-equal Doom on the killing hit) never produces — so the flag must stay
/// false (parity fuzz: player.block cs=11 rs=23, Rust wrongly armed the 3x off a
/// kill-shot Doom).
#[test]
fn deaths_door_flag_only_arms_on_a_landed_player_doom() {
    let mut state = CombatState::default();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);

    // Doom applied to a DEAD enemy records no PowerReceivedEntry → no arm.
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == enemy.0) {
        c.current_hp = 0;
    }
    card_effects::apply_power_to_enemy(&mut state, enemy, PowerKind::Doom, 3);
    assert!(
        !state.combat_log.doom_applied_by_player_this_turn,
        "Doom on an already-dead enemy does not arm DeathsDoor"
    );

    // A zero-stack Doom apply records nothing either.
    let live = state.spawn_enemy(MonsterKind::Nibbit);
    card_effects::apply_power_to_enemy(&mut state, live, PowerKind::Doom, 0);
    assert!(
        !state.combat_log.doom_applied_by_player_this_turn,
        "a 0-stack Doom apply does not arm DeathsDoor"
    );

    // A real landed Doom on a live enemy arms the flag.
    card_effects::apply_power_to_enemy(&mut state, live, PowerKind::Doom, 3);
    assert!(
        state.combat_log.doom_applied_by_player_this_turn,
        "a Doom that lands on a live enemy arms DeathsDoor"
    );
}

/// C# `SleightOfFleshPower.AfterPowerAmountChanged`: when the player applies a
/// non-temporary debuff to an enemy, deal `Amount` Unpowered damage to it.
#[test]
fn sleight_of_flesh_damages_enemy_when_player_applies_a_debuff() {
    let mut state = CombatState::default();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    let hp0 = enemy_hp(&state, enemy);
    push_player_power(&mut state, PowerKind::SleightOfFlesh, 4);

    // Player applies Doom (a debuff) to the enemy.
    hooks::dispatch_after_power_applied(
        &mut state,
        &PowerAppliedCtx {
            target: CreatureRef::Enemy(enemy),
            power_kind: PowerKind::Doom,
            delta: 3,
            applier: Some(CreatureRef::Player),
        },
    );
    assert_eq!(
        enemy_hp(&state, enemy),
        hp0 - 4,
        "SleightOfFlesh(4) deals 4 Unpowered damage to the debuffed enemy"
    );
}

/// SleightOfFlesh does not fire on a buff applied to an enemy, nor on a debuff
/// applied to the player.
#[test]
fn sleight_of_flesh_ignores_buffs_and_self_debuffs() {
    let mut state = CombatState::default();
    state.player.current_hp = 60;
    state.player.max_hp = 60;
    let _ = state.init_player_creature();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    let hp0 = enemy_hp(&state, enemy);
    push_player_power(&mut state, PowerKind::SleightOfFlesh, 4);

    // Applying a buff (Strength) to the enemy: not a debuff → no damage.
    hooks::dispatch_after_power_applied(
        &mut state,
        &PowerAppliedCtx {
            target: CreatureRef::Enemy(enemy),
            power_kind: PowerKind::Strength,
            delta: 2,
            applier: Some(CreatureRef::Player),
        },
    );
    assert_eq!(
        enemy_hp(&state, enemy),
        hp0,
        "a buff application does not trigger SleightOfFlesh"
    );

    // Applying a debuff to the PLAYER (not an enemy): no SleightOfFlesh damage.
    let php0 = state.player.current_hp;
    hooks::dispatch_after_power_applied(
        &mut state,
        &PowerAppliedCtx {
            target: CreatureRef::Player,
            power_kind: PowerKind::Vulnerable,
            delta: 2,
            applier: Some(CreatureRef::Player),
        },
    );
    assert_eq!(
        enemy_hp(&state, enemy),
        hp0,
        "enemy untouched by a self-debuff"
    );
    assert_eq!(
        state.player.current_hp, php0,
        "SleightOfFlesh deals no damage when the debuff target is the player"
    );
}

/// Regression (Necrobinder fuzz seed 443): a DIRECT Osty kill via
/// [`CombatState::kill_osty`] (SACRIFICE / BONE_SHARDS) must fire
/// `Hook.AfterCurrentHpChanged(Osty, -currentHp)` just like the damage-redirect
/// path, so `NecroMasteryPower` reflects `lostHp * Amount` Unblockable|Unpowered
/// damage to every hittable enemy. C# `CreatureCmd.KillWithoutCheckingWinCondition`
/// (CreatureCmd.cs:349-353) fires this hook after `LoseHpInternal`; the prior
/// Rust `kill_osty` zeroed HP silently, so SACRIFICE dealt 0 reflected damage
/// while C# dealt 8 (monsters[0].hp +8 in Rust).
#[test]
fn killing_osty_directly_reflects_its_hp_via_necro_mastery() {
    let mut state = CombatState::default();
    state.player.current_hp = 60;
    state.player.max_hp = 60;
    let _ = state.init_player_creature();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    let hp0 = enemy_hp(&state, enemy);

    // Summon an 8-HP Osty and put NecroMastery(1) on the player.
    let pet = state.summon_osty(8).expect("Osty summoned");
    assert_eq!(state.player.pet_uid, Some(pet), "pet uid tracks the Osty");
    push_player_power(&mut state, PowerKind::NecroMastery, 1);

    // SACRIFICE-style direct kill.
    state.kill_osty();

    assert_eq!(
        enemy_hp(&state, enemy),
        hp0 - 8,
        "killing the 8-HP Osty reflects 8 (=lostHp*1) Unblockable damage to the enemy"
    );
    assert!(
        state
            .creatures
            .iter()
            .find(|c| c.id == pet.0)
            .is_some_and(|c| c.current_hp == 0),
        "the Osty is left as a 0-HP corpse"
    );
}

/// NecroMastery scales the reflected damage by its stack amount.
#[test]
fn killing_osty_reflection_scales_with_necro_mastery_amount() {
    let mut state = CombatState::default();
    state.player.current_hp = 60;
    state.player.max_hp = 60;
    let _ = state.init_player_creature();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    let hp0 = enemy_hp(&state, enemy);

    let _pet = state.summon_osty(5).expect("Osty summoned");
    push_player_power(&mut state, PowerKind::NecroMastery, 3);

    state.kill_osty();

    assert_eq!(
        enemy_hp(&state, enemy),
        hp0 - 15,
        "NecroMastery(3) reflects 5*3 = 15 damage when the 5-HP Osty is killed"
    );
}

/// Without NecroMastery the direct Osty kill reflects nothing.
#[test]
fn killing_osty_without_necro_mastery_reflects_nothing() {
    let mut state = CombatState::default();
    state.player.current_hp = 60;
    state.player.max_hp = 60;
    let _ = state.init_player_creature();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    let hp0 = enemy_hp(&state, enemy);

    let _pet = state.summon_osty(8).expect("Osty summoned");
    state.kill_osty();

    assert_eq!(
        enemy_hp(&state, enemy),
        hp0,
        "no NecroMastery means a direct Osty kill deals no reflected damage"
    );
}
