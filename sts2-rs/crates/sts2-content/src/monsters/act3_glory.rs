// Doc comments use upper-snake-case DSL state names (e.g. `FAB_BRANCH`,
// `DRAMATIC_OPEN`) that clippy flags as missing-backticks identifiers.
// They are state-id strings, not Rust paths.
#![allow(clippy::doc_markdown)]

//! Act-3 monster ports (bead sts2-rs-m7f).
//!
//! Ported from sts2-rl-agent@01c094d:sts2_env/monsters/act3.py
//!
//! Ports the 28 monsters from
//! `sts2-rl-agent@01c094d:sts2_env/monsters/act3.py` (826 LOC) to
//! data-driven `MonsterAi` instances using the DSL skeleton from P5-1.
//! Conventions match [`super::act1_overgrowth`] and [`super::act2_hive`]:
//!
//! - Effect bodies are no-ops; intent metadata is preserved.
//! - Constructors return `MonsterAi` only.
//! - `Intent::Summon` landed via bead `sts2-rs-47p5` and is wired here
//!   (bead `sts2-rs-r9pl`) for Fabricator FABRICATE / FABRICATING_STRIKE,
//!   SoulNexus SUMMON, and Queen SUMMON_GUARD.
//! - `Intent::Heal` landed via bead `sts2-rs-va5r` and is wired here
//!   for SoulNexus `SOUL_DRAIN` (Attack + Heal multi-intent slot at
//!   `act3.py:671`).
//! - `ConditionalBranchState`s whose Python predicates capture
//!   per-instance state (FrogKnight CHARGE_CHECK, TestSubject
//!   PHASE_CHECK) collapse to the always-true fallback per the contract
//!   documented in [`super::act2_hive`].
//! - Door reintroduces a `ConditionalBranchState` (`REVIVE_CHECK`) that
//!   isn't in the Python source — the C# game gates revival on the
//!   `DOOR_REVIVAL` power; we model the gate now using the
//!   combat-global proxy (`combat.round`) so downstream beads can wire
//!   it to a real per-creature power read without re-shaping the state
//!   graph.
//! - `MysteriousKnight` is duplicated locally for now. When the
//!   `shared.rs` sub-bead lands (sts2-rs-uzn) the canonical ctor moves
//!   there and this re-exports it.
//!
//! See [`super::act2_hive`] for the full predicate-evaluation contract.

use std::collections::BTreeMap;

use smallvec::smallvec;
use sts2_core::monster_ai::{
    ConditionFn, ConditionalBranch, ConditionalBranchState, MonsterAi, MonsterState,
    MoveRepeatType, MoveState, RandomBranchState,
};
use sts2_core::monsters::Intent;
use sts2_core::powers::PowerKind;
use sts2_core::{CombatState, CreatureUid};

use super::dsl::{always_true, br, cb, mv, mv_eff, noop_effect, proto_ai, proto_ais, ProtoAis};
use super::{apply_power_to_player_via_legacy_surface, apply_signed_power_to_player};

/// `Noisebot` NOISE_MOVE (`NoiseMove`, `Noisebot.cs:45,51-64`):
/// `StatusIntent(2)` → creates 2 Dazed for the player — one into the
/// Discard pile and one into the Draw pile at a random position
/// (`CardPileCmd.AddGeneratedCardToCombat`). Bead `sts2-rs-bm1u5`
/// (previously a telegraph-only `mv()` noop). The draw-pile Dazed is
/// inserted at a RANDOM index drawn from the run-level `shuffle` stream,
/// mirroring C#'s `CardPilePosition.Random` (`Rng.Shuffle.NextInt(Count+1)`,
/// `CardPileCmd.cs:515`) — the earlier top-of-draw simplification under-read
/// the `shuffle` stream by one per Noisebot move, desyncing every later
/// reshuffle (gh #181 DRAW_ORDER/RNG_DESYNC/PILE_COUNT|end_turn family).
/// Bead `sts2-rs-noisebot-shuffle`.
fn noisebot_noise_effect(state: &mut CombatState, _owner: CreatureUid) {
    use sts2_core::card_effects::PileKind;
    let _ = crate::cards::status::spawn_dazed(state, PileKind::Discard);
    let _ = crate::cards::status::spawn_dazed_into_draw_random(state);
}

/// Guardbot GUARD_MOVE: give 15 block to every alive Fabricator on the
/// monster side. Mirrors C# `Guardbot.cs:46-54` `GuardMove`:
/// `foreach (var fab in Enemies.Where(c => c.Monster is Fabricator))
/// await CreatureCmd.GainBlock(fab, 15m, ValueProp.Unpowered, null)`.
/// The `Intent::Defend` on the move is a telegraph-only `DefendIntent()`
/// in C# (no payload); the block is applied here. Previously the Rust
/// port carried `Intent::Defend { block: 8 }`, which the Defend-intent
/// fast path in `combat_loop::perform_intent` (combat_loop.rs:2067)
/// silently routed into a self-GainBlock for the Guardbot — wrong target
/// AND wrong amount. Bead `sts2-rs-la22z`.
fn guardbot_guard_effect(state: &mut CombatState, _owner: CreatureUid) {
    let fabricator_uids: smallvec::SmallVec<[CreatureUid; 2]> = state
        .creatures
        .iter()
        .filter(|c| c.species == Some(sts2_core::MonsterKind::Fabricator) && c.current_hp > 0)
        .filter_map(|c| CreatureUid::new(c.id.get()))
        .collect();
    for uid in fabricator_uids {
        sts2_core::hooks::apply_effects(
            state,
            &[sts2_core::hooks::Effect::GainBlockCreature {
                target: sts2_core::hooks::CreatureRef::Enemy(uid),
                amount: 15,
            }],
        );
    }
}

/// `MechaKnight` FLAMETHROWER_MOVE (`FlamethrowerMove`,
/// `MechaKnight.cs:82,131-135`): `StatusIntent(4)` →
/// `CardPileCmd.AddToCombatAndPreview<Burn>(targets, PileType.Hand, 4)`.
/// Adds 4 Burn to the player's hand. Bead `sts2-rs-6uunc` (previously a
/// telegraph-only `mv()` noop). Hand-cap overflow is a documented
/// simplification: `spawn_burn` unconditionally pushes into hand rather
/// than routing overflow to discard; the card count (4 Burn) is faithful.
fn mecha_knight_flamethrower_effect(state: &mut CombatState, _owner: CreatureUid) {
    use sts2_core::card_effects::PileKind;
    for _ in 0..4 {
        let _ = crate::cards::status::spawn_burn(state, PileKind::Hand);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Telegraph-only Buff/Debuff effect_fns (bead sts2-rs-pdsdl)
//
// `perform_intent` runs ONLY the per-move `effect_fn` for non-attack
// intents — `Intent::Buff`/`Intent::Debuff` are telegraph-only and apply
// no power on their own. Each effect below mirrors the matching
// `PowerCmd.Apply<…>` call in the authoritative C# decomp
// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/`).
// SELF-buffs route through `apply_power_to_creature`; PLAYER-debuffs
// route through `apply_power_to_player_via_legacy_surface` so wire-side
// observers (state_to_dict / Python parity harness) see them.
// ────────────────────────────────────────────────────────────────────────────

/// DevotedSculptor FORBIDDEN_INCANTATION applies `Ritual(9)` to self.
/// Mirrors C# `DevotedSculptor.cs:49` `ForbiddenIncantationMove`:
/// `await PowerCmd.Apply<RitualPower>(base.Creature, _ritualGain, null, null)`
/// with `_ritualGain = 9` (DevotedSculptor.cs:20).
fn devoted_sculptor_forbidden_incantation_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Ritual, 9);
}

/// Axebot BOOT_UP (v0.107.1). The 10 Block is applied by the primary
/// `Intent::Defend`. C# `v0.107.1/…/Axebot.cs:139-140` `BootUpMove` grants
/// `BootUpStrGain * (2 - StockAmount)` Strength, with
/// `BootUpStrGain = GetValueIfAscension(DeadlyEnemies, 4, 3)` (Axebot.cs:68)
/// and `StockAmount = _stockOverrideAmount.GetValueOrDefault(2)`
/// (Axebot.cs:78-89). A fresh, non-respawned bot (StockAmount = 2) gets
/// `3 * 0 = 0` Strength — a no-op. A respawned bot opens on BOOT_UP
/// (`_stockOverrideAmount.HasValue` → the move machine starts at BOOT_UP,
/// Axebot.cs:120-123) with `StockAmount = dead_amount - 1`: the first respawn
/// (stock 1) gains `3 * (2 - 1) = 3`, the second (stock 0) gains `3 * 2 = 6`.
/// `axebot_stock_after_death` overwrites the respawn's Stock to `dead - 1`, so
/// reading the owner's live Stock amount here matches C#'s `StockAmount`.
fn axebot_boot_up_effect(state: &mut CombatState, owner: CreatureUid) {
    use sts2_core::ascension::{get_value_if_ascension, AscensionLevel::DeadlyEnemies};
    // BootUpStrGain (Axebot.cs:68): 4 at A9 DeadlyEnemies, else 3.
    let str_gain = get_value_if_ascension(state.ascension, DeadlyEnemies, 4, 3);
    // StockAmount = the respawned Axebot's stock override. BOOT_UP is only ever
    // performed by a RESPAWNED bot — the move machine starts a fresh (stock-2)
    // bot at HAMMER_UPPERCUT and never returns to BOOT_UP, but a bot with a
    // `_stockOverrideAmount` (set on respawn to `dead_amount - 1`) opens on
    // BOOT_UP (Axebot.cs:95-99). So when this fires, `_stockOverrideAmount`
    // ALWAYS HasValue and `StockAmount = _stockOverrideAmount` ∈ {1, 0}.
    //
    // Rust models the override via the live `StockPower` amount — but a stock-0
    // respawn carries NO `StockPower` (C# `AfterAddedToRoom` applies it only
    // when `StockAmount > 0`, and `axebot_stock_after_death` drops it). So an
    // ABSENT StockPower here means stock 0, NOT the fresh-bot default of 2:
    // defaulting to 2 gave the 2nd respawn `3 * (2 - 2) = 0` Strength where C#'s
    // `_stockOverrideAmount = 0` yields `3 * (2 - 0) = 6` (Ironclad AXEBOTS_NORMAL
    // seed 224, `MONSTER_STATE`). Default to 0.
    let stock_amount: i32 = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().find_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::Stock)
                    .map(|p| p.amount)
            })
        })
        .unwrap_or(0);
    let amount = str_gain * (2 - stock_amount);
    if amount != 0 {
        sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, amount);
    }
}

/// `StockPower.AfterDeath` — respawn a fresh Axebot when a stocked Axebot
/// dies. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Powers/StockPower.cs`:
/// [decompiled C# omitted — see the cited path]
///
/// The fresh Axebot is added at the dead bot's slot (`base.Owner.SlotName`),
/// so it occupies the same position in turn/display order. C# `Axebot`'s
/// `StockAmount` override starts its move machine at the BOOT_UP state
/// (`Axebot.cs:104-108`: `if (_stockOverrideAmount.HasValue) return new
/// MonsterMoveStateMachine(list, moveState /* BOOT_UP */);`), so the respawn
/// always opens with BOOT_UP — modeled via `create_axebot_ai(true)`.
///
/// The respawned bot's own `StockPower` is `dead_amount - 1`: `spawn_enemy`
/// auto-applies the static `on_spawn_self_buffs` `Stock(2)`, which we then
/// overwrite to `dead_amount - 1` to match `axebot.StockAmount = base.Amount
/// - 1`. When that reaches 0, the bot's `AfterAddedToRoom` guard
/// (`StockAmount > 0`) means no `StockPower` is applied, and this handler's
/// `amount > 0` guard (via `fire_for_owner`'s `amount != 0` filter) means a
/// 0-stock bot never respawns — capping the chain at three Axebots per slot.
///
/// Registered into [`sts2_core::enemy_death_effects`] (keyed by
/// `PowerKind::Stock`) and fired per-death from the `just_died` chokepoint
/// (`damage.rs`) via [`sts2_core::enemy_death_effects::fire_for_owner`].
/// Bead `sts2-rs-zo6fk`.
pub(crate) fn axebot_stock_after_death(state: &mut CombatState, owner: CreatureUid) {
    // Read the dying bot's Stock amount.
    let dead_stock: i32 = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().find_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::Stock)
                    .map(|p| p.amount)
            })
        })
        .unwrap_or(0);
    if dead_stock <= 0 {
        return;
    }
    let new_stock = dead_stock - 1;

    // Spawn the replacement at the dead bot's slot index (C# adds at
    // `base.Owner.SlotName`). The dead bot's corpse is retained in
    // `enemy_uids` (Rust keeps corpses), so insert just before it to land
    // in the same display position; if the corpse is absent (defensive),
    // append.
    let dead_index = state
        .enemy_uids
        .iter()
        .position(|u| u.0 == owner.0)
        .unwrap_or(state.enemy_uids.len());
    // C# `CreateCreature` rolls the respawn's unique HP against `_enemies`
    // (minus the new bot). At this point — inside the dying bot's
    // `StockPower.AfterDeath`, before C#'s post-AfterDeath `RemoveCreature` —
    // `_enemies` holds the ALIVE siblings plus THIS dying owner, but NOT
    // corpses from PRIOR deaths (those were already removed). Rust retains all
    // corpses (`sts2-rs-c8why`), so we must curate the exclusion set: alive
    // enemies ∪ the dying owner. Passing the full corpse set instead pollutes
    // the candidate range with stale MaxHp and drifts the Niche `NextItem`
    // index vs C# on 2nd+ respawns. Bead `sts2-rs-etqsw`.
    let taken: std::collections::HashSet<i32> = state
        .enemy_uids
        .iter()
        .filter_map(|u| state.creatures.iter().find(|c| c.id == u.0))
        .filter(|c| c.current_hp > 0 || c.id == owner.0)
        .map(|c| c.max_hp)
        .collect();
    let new_uid =
        state.spawn_enemy_at_index_with_taken(sts2_core::MonsterKind::Axebot, dead_index, &taken);
    // C# `Axebot` with a `StockAmount` override opens on BOOT_UP.
    let ai = crate::ascension_scaling::summoned_ai(
        state,
        sts2_core::MonsterKind::Axebot,
        create_axebot_ai(true),
    );
    // Telegraph BOOT_UP's intent — but ONLY when the respawn happens during
    // the PLAYER's turn. This mirrors C# `CombatManager.AfterCreatureAdded`
    // (CombatManager.cs:689-696): a mid-combat-added enemy has its move
    // rolled (`RollMove`) *only* `if CurrentSide == CombatSide.Player`.
    //
    // Two death timings reach this `StockPower.AfterDeath` respawn:
    //   * Player-turn death (player attack kills the bot) — `side == Player`.
    //     C# rolls the move now, so the bot telegraphs BOOT_UP and performs it
    //     on the upcoming enemy turn. We set the intent so the next enemy
    //     turn's `acting_enemies` snapshot (which filters on `creature.intent`)
    //     includes it.
    //   * Enemy-turn-start death (a poison/DoT tick kills the bot during
    //     `enemy_turn_start`, before the `enemy_turn_exec` acting snapshot) —
    //     `side == Enemy`. C# does NOT roll (CurrentSide == Enemy) and, on top
    //     of that, `MonsterModel.SpawnedThisTurn` makes `Creature.TakeTurn`
    //     skip `PerformMove` for any monster added this turn
    //     (Creature.cs:603-606). So the respawned bot sits out the very turn
    //     it spawned and gains its BOOT_UP block+Strength only on the FOLLOWING
    //     enemy turn. We model both by leaving `intent = None`: the natural
    //     `intent.is_none()` filter in `enemy_turn_exec` excludes it this turn,
    //     and `enemy_turn_end::roll_next_enemy_intents` rolls BOOT_UP for it
    //     (the equivalent of C#'s player-turn-start `PrepareForNextTurn`), so
    //     it performs BOOT_UP next enemy turn. Setting the intent here instead
    //     made Rust perform BOOT_UP a full enemy turn early (parity seed 5881:
    //     poison-respawned AxeBot `block` cs=0 rs=10). Bead `sts2-rs-vgc81`.
    let initial_intent = if state.side == sts2_core::Side::Player {
        ai.current_move().intents.first().copied()
    } else {
        None
    };
    state.set_creature_ai(new_uid, ai);
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == new_uid.0) {
        c.intent = initial_intent;
    }

    // `spawn_enemy_at_index` already auto-applied the static `Stock(2)`
    // on-spawn buff; overwrite it to `dead_amount - 1`
    // (`axebot.StockAmount = base.Amount - 1`). If `new_stock` is 0, drop
    // the StockPower entirely (C# `AfterAddedToRoom` applies StockPower only
    // when `StockAmount > 0`).
    let stock_pid: Option<std::num::NonZeroU32> = state
        .creatures
        .iter()
        .find(|c| c.id == new_uid.0)
        .and_then(|c| {
            c.powers.iter().find_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::Stock)
                    .map(|p| p.id)
            })
        });
    if let Some(pid) = stock_pid {
        if new_stock > 0 {
            if let Some(p) = state.powers.get_mut_by_id(pid) {
                p.amount = new_stock;
            }
        } else {
            // Remove the power record and its uid from the creature.
            state.powers.retain(|p| p.id != pid);
            if let Some(c) = state.creatures.iter_mut().find(|c| c.id == new_uid.0) {
                c.powers.retain(|puid| puid.0 != pid);
            }
        }
    }
}

/// `TurretOperator.cs:49-54` `ReloadMove`:
/// `await PowerCmd.Apply<StrengthPower>(base.Creature, 1m, …)` to self. The
/// telegraph `BuffIntent` applies nothing, so the Strength lands here.
fn turret_operator_reload_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 1);
}

/// `SpectralKnight.cs:60-68` `HexMove`:
/// `await PowerCmd.Apply<HexPower>(target, 2m, …)` to the player. The
/// telegraph `DebuffIntent` applies nothing, so the Hex stack lands here.
/// NOTE: `HexPower`'s card-affliction behavior (afflict all cards →
/// Hexed/Ethereal on play) is NOT yet wired in the Rust engine — only the
/// `PowerKind::Hex` stack is tracked here. Behavioral wiring is tracked by
/// `sts2-rs-l7k3d`.
fn spectral_knight_hex_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Hex, 2);
    // C# `HexPower.AfterDeath` (`HexPower.cs:37-43`) removes the power when
    // its APPLIER dies — stamp the applier so the death path
    // (`remove_hex_from_dead_applier`) can drop it (sweep seed 4170:
    // killing the SpectralKnight left the player permanently Hexed in Rust).
    if let Some(p) = state
        .powers
        .iter_mut()
        .filter(|p| p.kind == PowerKind::Hex)
        .last()
    {
        p.applier = Some(owner);
    }
}

/// Axebot HAMMER_UPPERCUT applies `Weak(2)` AND `Frail(2)` to the player
/// after its attack. Mirrors C# `v0.107.1/…/Axebot.cs:127-128`
/// `HammerUppercutMove`: `PowerCmd.Apply<WeakPower>(targets, 2m, …) ;
/// PowerCmd.Apply<FrailPower>(targets, 2m, …)`. `…wb8lk.11`: v0.107.1 bumped
/// both debuffs 1→2. The telegraph carries `Intent::Debuff{Weak,2}`; the
/// Frail half lands here.
fn axebot_hammer_uppercut_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 2);
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 2);
}

/// FrogKnight FOR_THE_QUEEN applies `Strength(5)` to self. Mirrors C#
/// `FrogKnight.cs:88-92` `ForTheQueenMove`:
/// `await PowerCmd.Apply<StrengthPower>(base.Creature, 5m, …)`.
fn frog_knight_for_the_queen_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 5);
}

/// FrogKnight TONGUE_LASH applies `Frail(2)` to the player after its
/// attack. Mirrors C# `FrogKnight.cs:103-109` `TongueLashMove`:
/// `DamageCmd.Attack(TongueLashDamage) ; await
/// PowerCmd.Apply<FrailPower>(targets, 2m, …)`.
fn frog_knight_tongue_lash_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 2);
}

/// GlobeHead SHOCKING_SLAP applies `Frail(2)` to the player after its
/// attack. Mirrors C# `GlobeHead.cs:75` `ShockingSlap`:
/// `await PowerCmd.Apply<FrailPower>(targets, 2m, …)`.
fn globe_head_shocking_slap_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 2);
}

/// GlobeHead GALVANIC_BURST applies `Strength(2)` to self after its
/// attack. Mirrors C# `GlobeHead.cs:78-83` `GalvanicBurstMove`:
/// `await PowerCmd.Apply<StrengthPower>(base.Creature, 2m, …)`.
fn globe_head_galvanic_burst_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// SlimedBerserker LEECHING_HUG applies `Weak(3)` to the player AND
/// `Strength(3)` to self. Mirrors C# `SlimedBerserker.cs:70-75`
/// `LeechingHugMove`: `await PowerCmd.Apply<WeakPower>(targets, 3m, null, null)
/// ; await PowerCmd.Apply<StrengthPower>(base.Creature, 3m, …)`.
fn slimed_berserker_leeching_hug_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 3);
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 3);
}

/// SlimedBerserker VOMIT_ICHOR adds 10 Slimed cards to the player
/// Discard pile. Mirrors C# `SlimedBerserker.cs:67-72` `VomitIchorMove`:
/// `await CardPileCmd.AddToCombatAndPreview<Slimed>(targets,
/// PileType.Discard, 10, addedByPlayer: false)` (`_vomitSlimeInDiscard`
/// constant = 10). The `Intent::StatusCard` on the move is
/// telegraph-only (`perform_intent` only runs the effect_fn for
/// non-attack intents); the card add must live in the effect body.
/// Bead `sts2-rs-mq5im`.
fn slimed_berserker_vomit_ichor_effect(state: &mut CombatState, _owner: CreatureUid) {
    use sts2_core::card_effects::PileKind;
    for _ in 0..10 {
        let _ = crate::cards::status::spawn_slimed(state, PileKind::Discard);
    }
}

/// Stabbot STAB applies `Frail(1)` to the player after its attack.
/// Mirrors C# `Stabbot.cs:48-53` `StabMove`:
/// `DamageCmd.Attack(StabDamage) ; await PowerCmd.Apply<FrailPower>(targets, 1m, …)`.
fn stabbot_stab_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 1);
}

/// FlailKnight / MysteriousKnight WAR_CHANT applies `Strength(3)` to
/// self. Mirrors C# `FlailKnight.cs:60-64` `WarChantMove`:
/// `await PowerCmd.Apply<StrengthPower>(base.Creature, 3m, …)`.
fn knight_war_chant_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 3);
}

/// LivingShield SMASH applies `Strength(3)` (EnrageStr) to self after
/// its attack. Mirrors C# `LivingShield.cs:61-67` `SmashMove`:
/// `DamageCmd.Attack(SmashDamage) ; await
/// PowerCmd.Apply<StrengthPower>(base.Creature, EnrageStr, …)` with
/// `EnrageStr => 3` (LivingShield.cs:29).
fn living_shield_smash_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 3);
}

/// MechaKnight WINDUP applies `Strength(5)` to self (after the GainBlock
/// 15 telegraphed by `Intent::Defend`). Mirrors C# `MechaKnight.cs:122-128`
/// `WindupMove`: `GainBlock(15) ; await
/// PowerCmd.Apply<StrengthPower>(base.Creature, 5m, …)`.
fn mecha_knight_windup_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 5);
}

/// OwlMagistrate VERDICT applies `Vulnerable(4)` to the player after its
/// attack. Mirrors C# `OwlMagistrate.cs:136-143` `VerdictMove`:
/// `DamageCmd.Attack(VerdictDamage) ; await
/// PowerCmd.Apply<VulnerablePower>(targets, 4m, …)`.
fn owl_magistrate_verdict_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Vulnerable, 4);
    // C# `OwlMagistrate.cs::VerdictMove` ends with
    // `PowerCmd.Remove<SoarPower>(base.Creature)` — the owl lands, dropping
    // the JUDICIAL_FLIGHT defensive Soar buff. Bead `sts2-rs-0qo0b`.
    remove_power_from_creature(state, owner, PowerKind::Soar);
}

/// OwlMagistrate JUDICIAL_FLIGHT applies Soar(1) to self (the "flying" turn
/// halves incoming powered damage; dropped by the following VERDICT).
/// Mirrors C# `OwlMagistrate.cs::JudicialFlightMove`
/// `PowerCmd.Apply<SoarPower>(base.Creature, 1m, …)`. Bead `sts2-rs-0qo0b`.
fn owl_magistrate_judicial_flight_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Soar, 1);
}

/// Remove every instance of `kind` owned by `creature` (mirrors C#
/// `PowerCmd.Remove<TPower>`). No-op if the creature has no such power.
fn remove_power_from_creature(state: &mut CombatState, creature: CreatureUid, kind: PowerKind) {
    let remove_ids: Vec<std::num::NonZeroU32> = state
        .creatures
        .iter()
        .find(|c| c.id == creature.0)
        .map(|c| {
            c.powers
                .iter()
                .filter(|pu| state.powers.iter().any(|p| p.id == pu.0 && p.kind == kind))
                .map(|pu| pu.0)
                .collect()
        })
        .unwrap_or_default();
    for id in remove_ids {
        state.powers.retain(|p| p.id != id);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == creature.0) {
            c.powers.retain(|pu| pu.0 != id);
        }
    }
}

/// Find the Aeonglass creature — the (single) monster carrying a
/// `WitheringPresence` power (`Aeonglass.cs:78-84` — owner == the boss).
fn aeonglass_creature_id(state: &CombatState) -> Option<std::num::NonZeroU32> {
    state
        .creatures
        .iter()
        .find(|c| {
            c.powers.iter().any(|pu| {
                state
                    .powers
                    .iter()
                    .any(|p| p.id == pu.0 && p.kind == PowerKind::WitheringPresence)
            })
        })
        .map(|c| c.id)
}

/// Read the Aeonglass `WitherUpgradeCount`. In C# this is an `Aeonglass`
/// monster field (not the power), so we store it on the boss's `MonsterAi`
/// `counter`. Returns 0 if the boss / its AI is absent (isolated fixture).
/// Bead `…wb8lk.2`.
fn aeonglass_wither_upgrade_count(state: &CombatState) -> i32 {
    aeonglass_creature_id(state)
        .and_then(|cid| state.creatures.iter().find(|c| c.id == cid))
        .and_then(|c| c.ai.as_ref())
        .map_or(0, sts2_core::monster_ai::MonsterAi::counter)
}

/// Set the Aeonglass `WitherUpgradeCount` on the boss's `MonsterAi` counter.
/// No-op if the boss / its AI is absent.
fn aeonglass_set_wither_upgrade_count(state: &mut CombatState, value: i32) {
    let Some(cid) = aeonglass_creature_id(state) else {
        return;
    };
    if let Some(ai) = state
        .creatures
        .iter_mut()
        .find(|c| c.id == cid)
        .and_then(|c| c.ai.as_mut())
    {
        ai.set_counter(value);
    }
}

/// `FakeUpgrade` every `Wither` card currently in the combat (any pile) by
/// `+1` level. Mirrors C# `Aeonglass.IncreasingIntensityMove` iterating
/// `target.Player.PlayerCombatState.AllCards` and calling `wither.FakeUpgrade()`.
fn aeonglass_bump_existing_withers(state: &mut CombatState) {
    use sts2_core::cards::{CardId, VarKey};
    let wither_uids: smallvec::SmallVec<[usize; 8]> = state
        .cards
        .iter()
        .enumerate()
        .filter(|(_, c)| c.card_id == CardId::Wither)
        .map(|(i, _)| i)
        .collect();
    for i in wither_uids {
        if let Some(card) = state.cards.get_mut(i) {
            let next = card.combat_var(VarKey::WitherFakeUpgradeLevel) + 1;
            card.set_combat_var(VarKey::WitherFakeUpgradeLevel, next);
        }
    }
}

/// Aeonglass EBB — primary `SingleAttackIntent(EbbDamage)` (applied by
/// `perform_intent`) + secondary `DefendIntent` (applied here). Mirrors C#
/// `Aeonglass.EbbMove`: `DamageCmd.Attack(EbbDamage) ; CreatureCmd.GainBlock(
/// base.Creature, EbbBlock=33, Move)`. The 33 block is constant (no
/// ascension). Bead `…wb8lk.2`.
fn aeonglass_ebb_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::hooks::apply_effects(
        state,
        &[sts2_core::hooks::Effect::GainBlockCreature {
            target: sts2_core::hooks::CreatureRef::Enemy(owner),
            amount: 33,
        }],
    );
}

/// Aeonglass INCREASING_INTENSITY — primary `StatusIntent(WitherAmount)` +
/// secondary `BuffIntent`. Mirrors C# `Aeonglass.IncreasingIntensityMove`:
///   1. `FakeUpgrade` every existing Wither in the player's piles (+1 level).
///   2. `WitherUpgradeCount++`.
///   3. Add `WitherAmount` Withers to the Discard pile, each matched to the
///      new `WitherUpgradeCount`.
///   4. Grant self `StrengthPower(IncreasingIntensityBaseStrength +
///      AdditionalStrength)`, then `AdditionalStrength++`.
///
/// `AdditionalStrength` and `WitherUpgradeCount` are always equal at rest and
/// both increment once per move, so this models them with a single counter
/// (`WitherUpgradeCount`, stored on the boss's `MonsterAi` counter):
/// the strength granted on the Nth move is `base + (count_before_increment)`,
/// and the new Withers are upgraded to `count_after_increment`. Bead `…wb8lk.2`.
fn aeonglass_increasing_intensity_effect(state: &mut CombatState, owner: CreatureUid) {
    use sts2_core::ascension::{get_value_if_ascension, AscensionLevel::DeadlyEnemies};
    use sts2_core::card_effects::PileKind;
    use sts2_core::cards::VarKey;

    let base_strength = get_value_if_ascension(state.ascension, DeadlyEnemies, 4, 3);
    let wither_amount = get_value_if_ascension(state.ascension, DeadlyEnemies, 2, 1);

    // AdditionalStrength == WitherUpgradeCount BEFORE this move's increment.
    let count_before = aeonglass_wither_upgrade_count(state);

    // 1. Bump every existing Wither (+1 level).
    aeonglass_bump_existing_withers(state);
    // 2. WitherUpgradeCount++.
    let count_after = count_before + 1;
    aeonglass_set_wither_upgrade_count(state, count_after);
    // 3. Add `wither_amount` Withers, each matched to the new upgrade count.
    for _ in 0..wither_amount {
        let card_uid = crate::cards::status::spawn_wither(state, PileKind::Discard);
        for _ in 0..count_after {
            if let Some(card) = state.cards.get_mut(card_uid.0.get() as usize - 1) {
                let next = card.combat_var(VarKey::WitherFakeUpgradeLevel) + 1;
                card.set_combat_var(VarKey::WitherFakeUpgradeLevel, next);
            }
        }
    }
    // 4. Grant self Strength = base + AdditionalStrength(count_before).
    sts2_core::card_effects::apply_power_to_creature(
        state,
        owner,
        PowerKind::Strength,
        base_strength + count_before,
    );
}

/// SoulNexus DRAIN_LIFE applies `Vulnerable(2)` AND `Weak(2)` to the
/// player after its attack. Mirrors C# `SoulNexus.cs:95-101`
/// `DrainLifeMove`: `DamageCmd.Attack(DrainLifeDamage) ; await
/// PowerCmd.Apply<VulnerablePower>(targets, 2m, …) ; await
/// PowerCmd.Apply<WeakPower>(targets, 2m, …)`. The `Intent::DebuffStrong`
/// marker is the wire-side telegraph (C# `DebuffIntent(strong: true)`);
/// the actual power application lives here.
fn soul_nexus_drain_life_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Vulnerable, 2);
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 2);
}

/// FrogKnight CHARGE_CHECK predicate (bead `sts2-rs-k27e`). Mirrors
/// Python `act3.py:294`:
/// `not _state["beetle_charged"] and creature.current_hp < creature.max_hp // 2`.
/// Counter == 0 ↔ `beetle_charged == False`; the BEETLE_CHARGE effect
/// flips it to 1 so the predicate fires at most once per combat.
fn frog_knight_should_charge(
    combat: &CombatState,
    owner: sts2_core::CreatureUid,
    counter: i32,
) -> bool {
    if counter != 0 {
        return false;
    }
    let Some(creature) = combat.creatures.iter().find(|c| c.id == owner.0) else {
        return false;
    };
    creature.current_hp < creature.max_hp / 2
}

/// BEETLE_CHARGE effect (bead `sts2-rs-k27e`). Sets the FrogKnight's
/// AI counter to 1 — Rust analogue of Python's
/// `_state["beetle_charged"] = True` at `act3.py:289`. Damage delivery
/// is handled by the `Intent::Attack { damage: 35 }` slot upstream;
/// this effect only flips the once-per-combat gate.
fn frog_knight_beetle_charge_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    if let Some(creature) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
        if let Some(ai) = creature.ai.as_mut() {
            ai.set_counter(1);
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Weak encounters
// ────────────────────────────────────────────────────────────────────────────

/// `DevotedSculptor` — FORBIDDEN_INCANTATION → SAVAGE (self-loop).
pub fn create_devoted_sculptor_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "FORBIDDEN_INCANTATION",
        MonsterState::Move(MoveState {
            id: "FORBIDDEN_INCANTATION",
            effect: devoted_sculptor_forbidden_incantation_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Ritual,
                amount: 9
            }],
            follow_up: Some("SAVAGE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "SAVAGE",
        mv("SAVAGE", smallvec![Intent::Attack { damage: 12 }], "SAVAGE"),
    );
    MonsterAi::new(s, "FORBIDDEN_INCANTATION")
}

/// `ScrollOfBiting` MORE_TEETH (`MoreTeethMove`,
/// `ScrollOfBiting.cs`): `PowerCmd.Apply<StrengthPower>(self, 2m)`
/// self-buff. Bead `sts2-rs-pd3j4`. Hook routing intentionally
/// bypassed (matches the legacy-surface convention).
fn scroll_of_biting_more_teeth_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// `ScrollOfBiting` — bead `sts2-rs-pd3j4`. C# v0.103.2
/// `ScrollOfBiting.cs::GenerateMoveStateMachine`:
///
/// - CHOMP `SingleAttackIntent(ChompDamage=14)`, follow → MORE_TEETH
/// - CHEW `MultiAttackIntent(ChewDamage=5, hits=2)`, follow → rand
/// - MORE_TEETH `BuffIntent()` (`StrengthPower(2)` self), follow → CHEW
/// - rand: `CHOMP` (CannotRepeat, weight 1) | `CHEW`
///   (CanRepeatXTimes(2), weight 1)
///
/// C# initial state is `StarterMoveIdx % 3` (0=CHOMP, 1=CHEW,
/// 2=MORE_TEETH). `starter_idx` is threaded from the encounter roster
/// via `MonsterSpawnArgs::ScrollOfBiting { starter_idx }` so the four
/// (Normal) / three (Weak) scrolls open on the C# rotated moves
/// (`ScrollsOfBitingNormal.cs` / `ScrollsOfBitingWeak.cs`
/// `GenerateMonsters`: `idx, (idx+1)%3, (idx+2)%3`, plus a fixed idx-2
/// 4th scroll in Normal). Before bead `sts2-rs-ybtmu` the factory took
/// no arg and every scroll defaulted to CHOMP (MONSTER_STATE
/// divergence). The prior Rust port shipped a fabricated `BITE(7)`
/// self-loop with no C# basis.
pub fn create_scroll_of_biting_ai(starter_idx: u8) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "CHOMP",
        mv(
            "CHOMP",
            smallvec![Intent::Attack { damage: 14 }],
            "MORE_TEETH",
        ),
    );
    s.insert(
        "CHEW",
        mv(
            "CHEW",
            smallvec![Intent::MultiAttack { damage: 5, hits: 2 }],
            "rand",
        ),
    );
    s.insert(
        "MORE_TEETH",
        MonsterState::Move(MoveState {
            id: "MORE_TEETH",
            effect: scroll_of_biting_more_teeth_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2,
            }],
            follow_up: Some("CHEW"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "rand",
        // C# `ScrollOfBiting.cs::GenerateMoveStateMachine`:
        //   randomBranchState.AddBranch(CHOMP, MoveRepeatType.CannotRepeat);
        //   randomBranchState.AddBranch(CHEW, 2);
        // The `2` on the CHEW branch is the `maxRepeats` overload
        // (`AddBranch(state, int maxRepeats)` → `CanRepeatXTimes`, weight
        // defaults to 1f), NOT a weight. Both branches therefore carry
        // weight 1; CHEW may repeat up to 2× consecutively. A prior port
        // misread it as `CanRepeatForever` weight 2.0, which made the
        // summed `NextFloat(total)` draw off `total=3` instead of `total=2`
        // — same MonsterAi tick, different scaling AND thresholds, so the
        // `rand` branch picked CHEW where C# picked CHOMP (and vice-versa)
        // for the same RNG value. Bead `sts2-rs-u3wjj`.
        MonsterState::RandomBranch(RandomBranchState {
            id: "rand",
            branches: smallvec![
                br("CHOMP", MoveRepeatType::CannotRepeat, 0, 1.0),
                br("CHEW", MoveRepeatType::CanRepeatXTimes, 2, 1.0),
            ],
        }),
    );
    // C# `ScrollOfBiting.cs::GenerateMoveStateMachine`: the initial
    // state is `StarterMoveIdx % 3` (0=CHOMP, 1=CHEW, 2=MORE_TEETH).
    let initial = match starter_idx % 3 {
        0 => "CHOMP",
        1 => "CHEW",
        _ => "MORE_TEETH",
    };
    MonsterAi::new(s, initial)
}

/// `TurretOperator` (`TurretOperator.cs:35-46`) — UNLOAD_MOVE_1
/// (MultiAttack `FireDamage`×5) → UNLOAD_MOVE_2 (same) → RELOAD_MOVE
/// (Buff: StrengthPower 1 to self) → loop back to UNLOAD_MOVE_1. Initial
/// state is UNLOAD_MOVE_1. `FireDamage` = 3 at asc-0 (4 on
/// `AscensionLevel.DeadlyEnemies`; ascension is identity at the pin).
pub fn create_turret_operator_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "UNLOAD_MOVE_1",
        mv(
            "UNLOAD_MOVE_1",
            smallvec![Intent::MultiAttack { damage: 3, hits: 5 }],
            "UNLOAD_MOVE_2",
        ),
    );
    s.insert(
        "UNLOAD_MOVE_2",
        mv(
            "UNLOAD_MOVE_2",
            smallvec![Intent::MultiAttack { damage: 3, hits: 5 }],
            "RELOAD_MOVE",
        ),
    );
    s.insert(
        "RELOAD_MOVE",
        mv_eff(
            "RELOAD_MOVE",
            smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 1
            }],
            "UNLOAD_MOVE_1",
            turret_operator_reload_effect,
        ),
    );
    MonsterAi::new(s, "UNLOAD_MOVE_1")
}

// ────────────────────────────────────────────────────────────────────────────
// Normal encounters
// ────────────────────────────────────────────────────────────────────────────

/// `Axebot` (v0.107.1) — BOOT_UP → HAMMER_UPPERCUT ↔ ONE_TWO.
///
/// `…wb8lk.11`: v0.107.1 rework (decomp `v0.107.1/…/Axebot.cs:84-99`). SHARPEN
/// and the `RAND` RandomBranch were removed; the machine is now deterministic:
///   - BOOT_UP_MOVE: Defend (BootUpBlock 10/15) + Buff. → HAMMER_UPPERCUT.
///     C# grants `BootUpStrGain * (2 - StockAmount)` Strength; with the default
///     StockAmount = 2 that is 0, so BOOT_UP grants only Block. (A stock-
///     respawned Axebot has a lower StockAmount and does gain Strength; that
///     stock-scaled Strength is not modelled — see fidelity notes.)
///   - HAMMER_UPPERCUT_MOVE: Attack (12/14) + Debuff → Weak 2 + Frail 2 to the
///     player. → ONE_TWO.
///   - ONE_TWO_MOVE: MultiAttack (9/10) × 2. → HAMMER_UPPERCUT.
/// Per the decomp, a normal Axebot (no stock override) opens on
/// HAMMER_UPPERCUT; only a stock-overridden / respawned Axebot opens on
/// BOOT_UP. `start_with_boot_up` selects the BOOT_UP opener.
///
/// A StockPower respawn builds one mid-combat, so each is a clone of a
/// per-thread prototype (`proto_ai`).
pub fn create_axebot_ai(start_with_boot_up: bool) -> MonsterAi {
    thread_local! {
        static PROTO: ProtoAis<2> = const { proto_ais() };
    }
    proto_ai(&PROTO, usize::from(start_with_boot_up), || {
        build_axebot_ai(start_with_boot_up)
    })
}

pub(crate) fn build_axebot_ai(start_with_boot_up: bool) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "BOOT_UP",
        MonsterState::Move(MoveState {
            id: "BOOT_UP",
            effect: axebot_boot_up_effect,
            intents: smallvec![
                Intent::Defend { block: 10 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 0
                }
            ],
            follow_up: Some("HAMMER_UPPERCUT"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "ONE_TWO",
        mv(
            "ONE_TWO",
            smallvec![Intent::MultiAttack { damage: 9, hits: 2 }],
            "HAMMER_UPPERCUT",
        ),
    );
    s.insert(
        "HAMMER_UPPERCUT",
        MonsterState::Move(MoveState {
            id: "HAMMER_UPPERCUT",
            effect: axebot_hammer_uppercut_effect,
            intents: smallvec![
                Intent::Attack { damage: 12 },
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 2
                }
            ],
            follow_up: Some("ONE_TWO"),
            must_perform_once: false,
        }),
    );
    let initial = if start_with_boot_up {
        "BOOT_UP"
    } else {
        "HAMMER_UPPERCUT"
    };
    MonsterAi::new(s, initial)
}

/// `Zapbot` — ZAP self-loop. Fabricator-spawned ally.
pub fn create_zapbot_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "ZAP",
        mv("ZAP", smallvec![Intent::Attack { damage: 14 }], "ZAP"),
    );
    MonsterAi::new(s, "ZAP")
}

/// `Stabbot` — STAB self-loop (SingleAttack 11 + Frail 1). Mirrors C#
/// decomp `MegaCrit.Sts2.Core.Models.Monsters/Stabbot.cs` v0.103.2
/// (bead `sts2-rs-znyy`, parent `sts2-rs-kgwj`). Python pin `01c094d`
/// ships a stale MultiAttack 3 ×3 variant we deliberately diverge
/// from per the monster-stat audit (Pattern C — same +28% HP
/// overstatement as Zapbot/Guardbot/Noisebot plus a wrong attack
/// shape on Stab). Fabricator-spawned ally.
pub fn create_stabbot_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "STAB",
        MonsterState::Move(MoveState {
            id: "STAB",
            effect: stabbot_stab_effect,
            intents: smallvec![
                Intent::Attack { damage: 11 },
                Intent::Debuff {
                    kind: PowerKind::Frail,
                    amount: 1,
                },
            ],
            follow_up: Some("STAB"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "STAB")
}

/// `Guardbot` — GUARD self-loop. Fabricator-spawned ally; each turn
/// gives 15 block to every alive Fabricator (C# `Guardbot.cs:46-54`).
/// Telegraph-only `Intent::Defend { block: 0 }` mirrors C#
/// `DefendIntent()` with no payload; the actual block is applied in
/// `guardbot_guard_effect`. Bead `sts2-rs-la22z`.
pub fn create_guardbot_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "GUARD",
        mv_eff(
            "GUARD",
            smallvec![Intent::Defend { block: 0 }],
            "GUARD",
            guardbot_guard_effect,
        ),
    );
    MonsterAi::new(s, "GUARD")
}

/// `Noisebot` — NOISE (status-card) self-loop. Fabricator-spawned ally.
pub fn create_noisebot_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "NOISE",
        mv_eff(
            "NOISE",
            smallvec![Intent::StatusCard],
            "NOISE",
            noisebot_noise_effect,
        ),
    );
    MonsterAi::new(s, "NOISE")
}

/// Fabricator aggro spawn pool — C# `Fabricator.aggroSpawns`
/// (`{Zapbot, Stabbot}`). Bead `sts2-rs-dygv1`.
const FABRICATOR_AGGRO: [sts2_core::MonsterKind; 2] = [
    sts2_core::MonsterKind::Zapbot,
    sts2_core::MonsterKind::Stabbot,
];

/// Fabricator defensive spawn pool — C# `Fabricator.defenseSpawns`
/// (`{Guardbot, Noisebot}`). Bead `sts2-rs-dygv1`.
const FABRICATOR_DEFENSE: [sts2_core::MonsterKind; 2] = [
    sts2_core::MonsterKind::Guardbot,
    sts2_core::MonsterKind::Noisebot,
];

/// Encode a Fabricator bot kind as a small non-zero int for storage in the
/// Fabricator's `MonsterAi.counter` (the per-instance `_lastSpawned` slot);
/// 0 means "nothing spawned yet". Only the four bot kinds are mapped.
///
/// `pub` because wire reconstruction (`sts2-py::state_dict`) re-infers
/// `_lastSpawned` from the roster (the wire does not carry it) and must
/// write the SAME encoding this module's `fabricator_spawn_bot` reads.
pub fn fabricator_bot_code(kind: sts2_core::MonsterKind) -> i32 {
    use sts2_core::MonsterKind::{Guardbot, Noisebot, Stabbot, Zapbot};
    match kind {
        Zapbot => 1,
        Stabbot => 2,
        Guardbot => 3,
        Noisebot => 4,
        _ => 0,
    }
}

/// Spawn one bot from `pool` (excluding the Fabricator's last-spawned bot)
/// and apply `Minion(1)`. Mirrors C# `Fabricator.SpawnBot`:
/// `RunRng.MonsterAi.NextItem(options.Where(m => m != _lastSpawned))` then
/// `PowerCmd.Apply<MinionPower>(spawned, 1m)`, and records `_lastSpawned`.
/// We store `_lastSpawned` in the owner's per-instance `MonsterAi.counter`
/// (bead `sts2-rs-9vs6y`). With the two-element pools this makes consecutive
/// same-pool spawns alternate deterministically, exactly as C#'s exclusion
/// does.
///
/// Bead `sts2-rs-z4jsg` (coverage `encounter:FABRICATOR_NORMAL`):
/// * The pick consumes `RunRng.MonsterAi` — `Rng.NextItem` =
///   `NextInt(0, count)`, one counter tick EVEN for a 1-element
///   filtered pool (the `_lastSpawned` exclusion leaves a singleton on
///   consecutive same-pool spawns) — previously this drew from the
///   wrong stream (`state.rng`), desyncing `monster_ai` cs=3 rs=2 on
///   the first FABRICATING_STRIKE. `HashSet` iteration preserves
///   insertion order for these small sets, so the pool slices match
///   C#'s `ElementAt` indexing.
/// * Spawn POSITION follows the encounter slots
///   `[bot1, bot2, fabricator, bot3, bot4]` + `GetNextSlot` (FIRST
///   unused slot in declared order) + `SortEnemiesBySlotName`: bots in
///   the two slots before the Fabricator sort ahead of it, the two
///   after sort behind. Each bot's slot is tracked on its `MonsterAi`
///   and the slot-sort is reproduced directly (bead `sts2-rs-07sfm`,
///   superseding the earlier live-count front-insert which mis-ordered
///   the case where a bot dies and the next spawn reuses its freed
///   lower slot — Regent fuzz seed 162). Mirrors the Ovicopter LAY_EGGS
///   port (`ovicopter_lay_eggs_effect`, `sts2-rs-c8why`).
fn fabricator_spawn_bot(
    state: &mut CombatState,
    owner: CreatureUid,
    pool: &[sts2_core::MonsterKind],
) {
    let last_code = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| c.ai.as_ref().map(sts2_core::monster_ai::MonsterAi::counter))
        .unwrap_or(0);
    // Candidates = pool minus the last-spawned bot. Falls back to the full
    // pool if exclusion would empty it (cannot happen for the 2-elem pools).
    let candidates: smallvec::SmallVec<[sts2_core::MonsterKind; 4]> = pool
        .iter()
        .copied()
        .filter(|&k| fabricator_bot_code(k) != last_code)
        .collect();
    let candidates: &[sts2_core::MonsterKind] = if candidates.is_empty() {
        pool
    } else {
        &candidates
    };
    let idx = state
        .run_rng
        .monster_ai
        .next_int_exclusive(0, candidates.len() as i32) as usize;
    let kind = candidates[idx];
    // C# `Fabricator.SpawnBot` →
    // `Encounter.GetNextSlot` = the FIRST encounter slot
    // (`[bot1, bot2, fabricator, bot3, bot4]`, declared order) not held by a
    // living enemy, then `CombatManager.AddCreature` →
    // `SortEnemiesBySlotName` re-sorts the active `Enemies` by slot index on
    // every add. The Fabricator sits in the MIDDLE slot (index 2); bots fill
    // the four surrounding slots {0, 1, 3, 4}. A bot death frees its slot, so
    // the next spawn can reuse a slot BELOW a surviving bot — the old
    // live-count front-insert mis-ordered that case (Regent fuzz seed 162:
    // ZAPBOT in bot1 dies, so the refilled NOISEBOT belongs at bot1/index 0,
    // not appended at the end). Track each bot's slot on its `MonsterAi`
    // (excluded from `Creature` equality) and reproduce the slot-sort
    // directly, mirroring the Ovicopter LAY_EGGS port
    // (`ovicopter_lay_eggs_effect`, bead `sts2-rs-c8why`) — but picking the
    // FIRST free slot (C# `FirstOrDefault`) rather than the last. Corpses are
    // retained in the `enemy_uids` substrate, so liveness is filtered on
    // `current_hp > 0`. Bead `sts2-rs-07sfm`.
    // `FabricatorNormal.Slots = [bot1, bot2, fabricator, bot3, bot4]`; the
    // Fabricator's own slot 2 is stamped at combat entry, so the shared
    // first-free search naturally skips it and hands back bot1, bot2, bot3,
    // bot4 in C#'s declared order.
    //
    // This used to be ~35 lines of hand-rolled search and insert scan with a
    // `species == Fabricator` special case standing in for the missing slot
    // record. The slot model (GH #552) is that rule, once, for every
    // encounter.
    const FAB_SLOT_COUNT: u8 = 5;
    const LAST_BOT_SLOT: u8 = 4;
    // If every slot is held — an extreme corner where a FabricateMove
    // double-spawn overshoots the 4-bot cap — fall back to the last bot slot.
    // C# hands out `string.Empty` there, which sorts first; preserved as-is
    // rather than "fixed", because it is unreachable and the old port chose
    // this. Bead `sts2-rs-07sfm`.
    let slot = state
        .first_free_enemy_slot(FAB_SLOT_COUNT)
        .unwrap_or(LAST_BOT_SLOT);
    let ai = crate::ascension_scaling::summoned_ai(state, kind, create_bot_ai(kind));
    let uid = state.spawn_enemy_in_slot(kind, slot, ai);
    sts2_core::card_effects::apply_power_to_creature(state, uid, PowerKind::Minion, 1);
    // Record _lastSpawned on the Fabricator's own AI.
    if let Some(idx) = state.creatures.iter().position(|c| c.id == owner.0) {
        if let Some(ai) = state.creatures[idx].ai.as_mut() {
            ai.set_counter(fabricator_bot_code(kind));
        }
    }
}

/// Resolve a bot kind to its moveset ctor — the summon-site AI
/// attachment (`sts2-rs-cuocf` contract: AI attachment is the spawn
/// caller's responsibility). Bead `sts2-rs-z4jsg`.
///
/// The Fabricator summons inside the search, so each bot is a clone of a
/// per-thread prototype ([`proto_ai`]).
pub(crate) fn create_bot_ai(kind: sts2_core::MonsterKind) -> MonsterAi {
    use sts2_core::MonsterKind::{Guardbot, Noisebot, Stabbot};
    thread_local! {
        static PROTO: ProtoAis<4> = const { proto_ais() };
    }
    let (variant, build): (usize, fn() -> MonsterAi) = match kind {
        Stabbot => (0, create_stabbot_ai),
        Guardbot => (1, create_guardbot_ai),
        Noisebot => (2, create_noisebot_ai),
        _ => (3, create_zapbot_ai),
    };
    proto_ai(&PROTO, variant, build)
}

/// FABRICATE: spawn one defensive bot then one aggro bot, each a Minion.
/// C# `Fabricator.FabricateMove` (`SpawnDefensiveBot(); SpawnAggroBot();`).
fn fabricator_fabricate_effect(state: &mut CombatState, owner: CreatureUid) {
    fabricator_spawn_bot(state, owner, &FABRICATOR_DEFENSE);
    fabricator_spawn_bot(state, owner, &FABRICATOR_AGGRO);
}

/// FABRICATING_STRIKE: spawn one aggro bot (Minion) alongside the 18-damage
/// attack telegraph. C# `Fabricator.FabricatingStrikeMove` (`Attack(18); …;
/// SpawnAggroBot();`). The attack lands via `Intent::Attack`; this effect
/// fires post-damage and adds the spawn.
fn fabricator_fabricating_strike_effect(state: &mut CombatState, owner: CreatureUid) {
    fabricator_spawn_bot(state, owner, &FABRICATOR_AGGRO);
}

/// C# `Fabricator.CanFabricate` —
/// `GetTeammatesOf(Creature).Count(c => c.IsAlive) < 4`.
/// `CombatState.GetTeammatesOf` returns `GetCreaturesOnSide(side)` —
/// the ENTIRE enemy side INCLUDING the Fabricator itself
/// (CombatState.cs:509-512; no self-exclusion — same misread as the
/// Ovicopter `CanLay` gate, bead `sts2-rs-z4jsg`). So the Fabricator
/// stops at 3 live bots (side count 4 → DISINTEGRATE); the prior
/// `< 5` gate let it overfill to 4 bots. Alive-filtered because Rust
/// keeps corpses in `enemy_uids` (`sts2-rs-c8why`). Bead
/// `sts2-rs-dygv1` / `sts2-rs-z4jsg`.
fn fabricator_can_fabricate(combat: &CombatState) -> bool {
    let alive_side = combat
        .enemy_uids
        .iter()
        .filter(|uid| {
            combat
                .creatures
                .iter()
                .any(|c| c.id == uid.0 && c.current_hp > 0)
        })
        .count();
    alive_side < 4
}

/// `Fabricator` — FAB_BRANCH (`CanFabricate` → FAB_RAND, else DISINTEGRATE).
/// FAB_RAND randomly picks FABRICATE or FABRICATING_STRIKE, then returns to
/// FAB_BRANCH. Rebased from the prior Python-pin port (which stubbed
/// `_can_fabricate` to always-true and left both SUMMON moves as silent
/// noops) to C# v0.103.2 `Fabricator.cs` (source of truth): FABRICATE spawns
/// a defensive + an aggro Minion bot, FABRICATING_STRIKE attacks for 18 and
/// spawns an aggro Minion bot, and the `CanFabricate` ally-cap gate routes to
/// the (previously unreachable) DISINTEGRATE move once 4 enemies are alive.
/// Bead `sts2-rs-dygv1`.
pub fn create_fabricator_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "FAB_BRANCH",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "FAB_BRANCH",
            branches: smallvec![
                cb(fabricator_can_fabricate, "FAB_RAND"),
                cb(always_true, "DISINTEGRATE"),
            ],
        }),
    );
    s.insert(
        "FAB_RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "FAB_RAND",
            branches: smallvec![
                br("FABRICATE", MoveRepeatType::CanRepeatForever, 0, 1.0),
                br(
                    "FABRICATING_STRIKE",
                    MoveRepeatType::CanRepeatForever,
                    0,
                    1.0
                ),
            ],
        }),
    );
    s.insert(
        "FABRICATE",
        mv_eff(
            "FABRICATE",
            smallvec![Intent::Summon],
            "FAB_BRANCH",
            fabricator_fabricate_effect,
        ),
    );
    s.insert(
        "FABRICATING_STRIKE",
        mv_eff(
            "FABRICATING_STRIKE",
            smallvec![Intent::Attack { damage: 18 }, Intent::Summon],
            "FAB_BRANCH",
            fabricator_fabricating_strike_effect,
        ),
    );
    s.insert(
        "DISINTEGRATE",
        mv(
            "DISINTEGRATE",
            smallvec![Intent::Attack { damage: 11 }],
            "FAB_BRANCH",
        ),
    );
    // FAB_BRANCH is itself the entry point; new_with_combat would be the
    // ideal ctor but new() resolves through ConditionalBranch using the
    // always-true branch, which yields FAB_RAND → first move.
    let combat = CombatState::default();
    MonsterAi::new_with_combat(s, "FAB_BRANCH", &combat)
}

/// `FrogKnight` — initial TONGUE_LASH (Attack 13 + Frail 2 to player) →
/// STRIKE_DOWN_EVIL → FOR_THE_QUEEN (Strength +5 self) → CHARGE_CHECK →
/// {BEETLE_CHARGE | TONGUE_LASH}. CHARGE_CHECK collapses to TONGUE_LASH
/// after the beetle-charge counter flips (HP-threshold predicate captures
/// per-instance state). Mirrors C# `FrogKnight.cs:69-85`
/// `GenerateMoveStateMachine` which returns `new MonsterMoveStateMachine(
/// list, moveState3)` where `moveState3 = TONGUE_LASH` (decomp line 85).
///
/// Bead `sts2-rs-ybtmu` (Act-2/3 parity vein): reverts the `sts2-rs-8v6he`
/// regression, which misread `moveState3` (TONGUE_LASH) as `moveState`
/// (FOR_THE_QUEEN) and opened on the self-Buff — shifting the cadence and
/// adding a phantom turn-1 Strength while dropping the turn-1 player Frail.
pub fn create_frog_knight_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "TONGUE_LASH",
        MonsterState::Move(MoveState {
            id: "TONGUE_LASH",
            effect: frog_knight_tongue_lash_effect,
            intents: smallvec![
                Intent::Attack { damage: 13 },
                Intent::Debuff {
                    kind: PowerKind::Frail,
                    amount: 2
                }
            ],
            follow_up: Some("STRIKE_DOWN_EVIL"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "STRIKE_DOWN_EVIL",
        mv(
            "STRIKE_DOWN_EVIL",
            smallvec![Intent::Attack { damage: 21 }],
            "FOR_THE_QUEEN",
        ),
    );
    s.insert(
        "FOR_THE_QUEEN",
        MonsterState::Move(MoveState {
            id: "FOR_THE_QUEEN",
            effect: frog_knight_for_the_queen_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 5
            }],
            follow_up: Some("CHARGE_CHECK"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "CHARGE_CHECK",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "CHARGE_CHECK",
            branches: smallvec![
                ConditionalBranch {
                    condition: ConditionFn::CounterAware(frog_knight_should_charge),
                    state_id: "BEETLE_CHARGE",
                },
                cb(always_true, "TONGUE_LASH"),
            ],
        }),
    );
    s.insert(
        "BEETLE_CHARGE",
        MonsterState::Move(MoveState {
            id: "BEETLE_CHARGE",
            effect: frog_knight_beetle_charge_effect,
            intents: smallvec![Intent::Attack { damage: 35 }],
            follow_up: Some("TONGUE_LASH"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "TONGUE_LASH")
}

/// `GlobeHead` — SHOCKING_SLAP → THUNDER_STRIKE → GALVANIC_BURST →
/// SHOCKING_SLAP.
pub fn create_globe_head_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SHOCKING_SLAP",
        MonsterState::Move(MoveState {
            id: "SHOCKING_SLAP",
            effect: globe_head_shocking_slap_effect,
            intents: smallvec![
                Intent::Attack { damage: 13 },
                Intent::Debuff {
                    kind: PowerKind::Frail,
                    amount: 2
                }
            ],
            follow_up: Some("THUNDER_STRIKE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "THUNDER_STRIKE",
        mv(
            "THUNDER_STRIKE",
            smallvec![Intent::MultiAttack { damage: 6, hits: 3 }],
            "GALVANIC_BURST",
        ),
    );
    s.insert(
        "GALVANIC_BURST",
        MonsterState::Move(MoveState {
            id: "GALVANIC_BURST",
            effect: globe_head_galvanic_burst_effect,
            intents: smallvec![
                Intent::Attack { damage: 16 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2
                }
            ],
            follow_up: Some("SHOCKING_SLAP"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "SHOCKING_SLAP")
}

/// `OwlMagistrate` — SCRUTINY → PECK_ASSAULT → JUDICIAL_FLIGHT → VERDICT → cycle.
///
/// Rebased off Python pin `01c094d` to C# v0.103.2 canonical
/// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/OwlMagistrate.cs`).
/// Bead `sts2-rs-yxvi`. JUDICIAL_FLIGHT in C# applies `SoarPower` (unported);
/// modelled here as an unspecified self-Buff sentinel until a follow-up bead
/// ports Soar. VERDICT applies Vulnerable 4 in C#.
pub fn create_owl_magistrate_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "MAGISTRATE_SCRUTINY",
        mv(
            "MAGISTRATE_SCRUTINY",
            smallvec![Intent::Attack { damage: 16 }],
            "PECK_ASSAULT",
        ),
    );
    s.insert(
        "PECK_ASSAULT",
        mv(
            "PECK_ASSAULT",
            smallvec![Intent::MultiAttack { damage: 4, hits: 6 }],
            "JUDICIAL_FLIGHT",
        ),
    );
    s.insert(
        // Bead `sts2-rs-0qo0b`: JUDICIAL_FLIGHT applies Soar(1) to self per
        // C# OwlMagistrate.cs::JudicialFlightMove (was telegraph-only with a
        // placeholder Strength/0 buff, so the defensive Soar never landed).
        "JUDICIAL_FLIGHT",
        MonsterState::Move(MoveState {
            id: "JUDICIAL_FLIGHT",
            effect: owl_magistrate_judicial_flight_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Soar,
                amount: 1
            }],
            follow_up: Some("VERDICT"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "VERDICT",
        MonsterState::Move(MoveState {
            id: "VERDICT",
            effect: owl_magistrate_verdict_effect,
            intents: smallvec![
                Intent::Attack { damage: 33 },
                Intent::Debuff {
                    kind: PowerKind::Vulnerable,
                    amount: 4
                }
            ],
            follow_up: Some("MAGISTRATE_SCRUTINY"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "MAGISTRATE_SCRUTINY")
}

/// `SlimedBerserker` — deterministic 4-move chain
/// VOMIT_ICHOR → FURIOUS_PUMMELING → LEECHING_HUG → SMOTHER → VOMIT
/// (loop) per C# decomp
/// `MegaCrit.Sts2.Core.Models.Monsters/SlimedBerserker.cs` v0.103.2
/// (`GenerateMoveStateMachine`). Bead `sts2-rs-6v3s`, parent
/// `sts2-rs-kgwj` (Pattern A — Python pin `01c094d` ships a stale
/// 3-move SLASH/SLAM/ENRAGE variant we deliberately diverge from
/// per the monster-stat audit).
///
/// Move payloads track the C# constants verbatim:
/// - VOMIT_ICHOR → `StatusIntent(10)` (telegraphs 10 Slimed cards
///   added to player discard). The Rust port emits
///   `Intent::StatusCard` (telegraph only); the actual Slimed card
///   injection lives behind the per-move dispatcher (status-card
///   spawn surface — covered by `Intent::StatusCard` consumers
///   elsewhere in the codebase).
/// - FURIOUS_PUMMELING → `MultiAttackIntent(PummelingDamage=4, hits=4)`.
/// - LEECHING_HUG → `DebuffIntent` (Weak 3 to player) + `BuffIntent`
///   (Strength 3 self). Rust enriches both with the C# constants.
///   Multi-intent moves emit two ordered intents per the
///   `act2`/`act3` pattern (Debuff first, then self-Buff).
/// - SMOTHER → `SingleAttackIntent(SmotherDamage=30)`.
pub fn create_slimed_berserker_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "VOMIT_ICHOR_MOVE",
        mv_eff(
            "VOMIT_ICHOR_MOVE",
            smallvec![Intent::StatusCard],
            "FURIOUS_PUMMELING_MOVE",
            slimed_berserker_vomit_ichor_effect,
        ),
    );
    s.insert(
        "FURIOUS_PUMMELING_MOVE",
        mv(
            "FURIOUS_PUMMELING_MOVE",
            smallvec![Intent::MultiAttack { damage: 4, hits: 4 }],
            "LEECHING_HUG_MOVE",
        ),
    );
    s.insert(
        "LEECHING_HUG_MOVE",
        MonsterState::Move(MoveState {
            id: "LEECHING_HUG_MOVE",
            effect: slimed_berserker_leeching_hug_effect,
            intents: smallvec![
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 3,
                },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 3,
                },
            ],
            follow_up: Some("SMOTHER_MOVE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "SMOTHER_MOVE",
        mv(
            "SMOTHER_MOVE",
            smallvec![Intent::Attack { damage: 30 }],
            "VOMIT_ICHOR_MOVE",
        ),
    );
    MonsterAi::new(s, "VOMIT_ICHOR_MOVE")
}

/// `TheLost` DEBILITATING_SMOG — `TheLost.cs::DebilitatingSmogMove`:
/// `PowerCmd.Apply<StrengthPower>(targets, -2m, …)` (player) **and**
/// `PowerCmd.Apply<StrengthPower>(base.Creature, 2m, …)` (self).
/// `DebilitatingSmogStrengthStealAmount = 2` non-asc (identity at pin).
/// Bead `sts2-rs-kh6d0`: a Strength-steal move; the prior Rust port
/// fabricated a `DARK_SLASH(9) ↔ SHADOW_STRIKE(13)` cycle with no C#
/// basis. Hook routing intentionally bypassed (matches the
/// `apply_signed_power_to_player` convention).
fn the_lost_debilitating_smog_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_signed_power_to_player(state, PowerKind::Strength, -2);
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 2);
    // Record the steal so `PossessStrengthPower.AfterDeath` can give it back
    // when TheLost dies (bead `sts2-rs-jwb4a`).
    record_possess_steal(state, owner, PowerKind::PossessStrength, 2);
}

/// Accumulate `stolen` stat points the monster just took from the player into
/// its `possess_kind` power's `counter`, so the matching `*_after_death`
/// handler can restore the running total when the monster dies. Mirrors C#
/// `PossessStrengthPower`/`PossessSpeedPower`'s `Data.stolenStrength` dict,
/// which `AfterPowerAmountChanged` grows by every negative stat the owner
/// applies to the player (`PossessStrengthPower.cs:31-55`). The on-spawn
/// `Possess*` stack (amount 1) is just the marker; the real stolen total lives
/// in `counter`.
fn record_possess_steal(
    state: &mut CombatState,
    owner: CreatureUid,
    possess_kind: PowerKind,
    stolen: i32,
) {
    let pid = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().find_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == possess_kind)
                    .map(|p| p.id)
            })
        });
    if let Some(pid) = pid {
        if let Some(p) = state.powers.get_mut_by_id(pid) {
            p.counter = p.counter.saturating_add(stolen);
        }
    }
}

/// Read `owner`'s accumulated possess-steal total (the `counter` on its
/// `possess_kind` power; 0 if absent).
fn possess_stolen_total(state: &CombatState, owner: CreatureUid, possess_kind: PowerKind) -> i32 {
    state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().find_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == possess_kind)
                    .map(|p| p.counter)
            })
        })
        .unwrap_or(0)
}

/// `PossessStrengthPower.AfterDeath` — when TheLost dies, give the player back
/// the Strength it stole (C# `PossessStrengthPower.cs:60-67`:
/// `Apply<StrengthPower>(player, -stolen)`, i.e. +stolen since the recorded
/// value is the negative debuff). A net-zero Strength instance is then dropped
/// (`ShouldRemoveDueToAmount`), so a player whose only Strength was the steal
/// returns to no-Strength. Fired from the enemy-death chokepoint
/// (`fire_for_owner` mid-combat / `fire_pending` at combat end); restoring no
/// creature, it never keeps combat open. Bead `sts2-rs-jwb4a`.
pub(crate) fn possess_strength_after_death(state: &mut CombatState, owner: CreatureUid) {
    let stolen = possess_stolen_total(state, owner, PowerKind::PossessStrength);
    if stolen != 0 {
        apply_signed_power_to_player(state, PowerKind::Strength, stolen);
    }
}

/// `PossessSpeedPower.AfterDeath` — the Dexterity analogue for TheForgotten
/// (MIASMA steals Dexterity). Bead `sts2-rs-jwb4a`.
pub(crate) fn possess_speed_after_death(state: &mut CombatState, owner: CreatureUid) {
    let stolen = possess_stolen_total(state, owner, PowerKind::PossessSpeed);
    if stolen != 0 {
        apply_signed_power_to_player(state, PowerKind::Dexterity, stolen);
    }
}

/// `TheLost` — DEBILITATING_SMOG ↔ EYE_LASERS. Bead `sts2-rs-kh6d0`:
/// rebased onto C# `TheLost.cs::GenerateMoveStateMachine` v0.103.2.
/// Initial state = DEBILITATING_SMOG (first move added). EYE_LASERS
/// is `MultiAttackIntent(EyeLasersDamage=4, hits=2)`. On-spawn
/// `PossessStrengthPower(1)` is already wired via
/// `MonsterKind::TheLost => &[(PossessStrength, 1)]` in
/// `crates/sts2-core/src/monsters.rs::on_spawn_self_buffs`.
pub fn create_the_lost_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "DEBILITATING_SMOG",
        // C# DebuffIntent + BuffIntent: -2 Str (player) and +2 Str (self).
        MonsterState::Move(MoveState {
            id: "DEBILITATING_SMOG",
            effect: the_lost_debilitating_smog_effect,
            intents: smallvec![
                Intent::Debuff {
                    kind: PowerKind::Strength,
                    amount: 2,
                },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2,
                },
            ],
            follow_up: Some("EYE_LASERS"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "EYE_LASERS",
        mv(
            "EYE_LASERS",
            smallvec![Intent::MultiAttack { damage: 4, hits: 2 }],
            "DEBILITATING_SMOG",
        ),
    );
    MonsterAi::new(s, "DEBILITATING_SMOG")
}

/// Read `owner`'s current Dexterity stacks (0 if none / not found). Used to
/// compute TheForgotten's DREAD damage (C# `DreadDamage = 13 + Dexterity`).
fn the_forgotten_dexterity(state: &CombatState, owner: CreatureUid) -> i32 {
    state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .map_or(0, |c| {
            c.powers
                .iter()
                .find_map(|pu| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == pu.0 && p.kind == PowerKind::Dexterity)
                        .map(|p| p.amount)
                })
                .unwrap_or(0)
        })
}

/// MIASMA: C# `MiasmaMove` — steal 2 Dexterity from the player
/// (`DexterityPower(-2)` on player), gain 8 block, then `DexterityPower(+2)`
/// on self. After self-Dex changes, recompute DREAD's stored attack damage
/// to `13 + self Dexterity` so the next DREAD telegraph/hit reflects it
/// (C# `DreadDamage` is `() => 13 + GetPowerAmount<DexterityPower>()`; we
/// mirror the dynamic value via the stored intent — Rust monster attacks are
/// not auto-boosted by the attacker's Dexterity, so no double-count). The
/// block-8 is applied here, not via the `Intent::Defend` telegraph, because
/// MIASMA's primary intent is the Debuff and `perform_intent` only resolves
/// a move's first intent. Bead `sts2-rs-96y80`.
fn the_forgotten_miasma_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_signed_power_to_player(state, PowerKind::Dexterity, -2);
    // Record the steal for `PossessSpeedPower.AfterDeath` (bead `sts2-rs-jwb4a`).
    record_possess_steal(state, owner, PowerKind::PossessSpeed, 2);
    sts2_core::hooks::apply_effects(
        state,
        &[sts2_core::hooks::Effect::GainBlockCreature {
            target: sts2_core::hooks::CreatureRef::Enemy(owner),
            amount: 8,
        }],
    );
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Dexterity, 2);
    let dread = 13 + the_forgotten_dexterity(state, owner);
    if let Some(idx) = state.creatures.iter().position(|c| c.id == owner.0) {
        if let Some(ai) = state.creatures[idx].ai.as_mut() {
            if let Some(MonsterState::Move(m)) =
                std::sync::Arc::make_mut(&mut ai.states).get_mut("DREAD")
            {
                for intent in &mut m.intents {
                    if let Intent::Attack { damage } = intent {
                        *damage = dread;
                    }
                }
            }
        }
    }
}

/// `TheForgotten` — MIASMA (init) ↔ DREAD.
///
/// Rebased to C# v0.103.2 `TheForgotten.cs` (source of truth). The prior
/// HAUNT/WAIL_OF_SORROW Rust moveset (Attack 7 + Weak / Attack 11) was a
/// fabrication. C# (non-ascension):
/// - MIASMA: DebilitatingSmog — Dexterity -2 to player, gain 8 block,
///   Dexterity +2 to self (telegraph `Debuff + Defend + Buff`).
/// - DREAD: `SingleAttack(DreadDamage)` where `DreadDamage = 13 + self
///   Dexterity`. MIASMA's effect ramps the stored DREAD damage as self Dex
///   grows (starts 13; 15 after the first MIASMA, 17 after the second, …).
///
/// NOTE: C# `AfterAddedToRoom` also applies `PossessSpeedPower(1)` to self
/// and HP is 106 (non-asc). Both live in `monsters.rs`
/// (`on_spawn_self_buffs` + the HP table) — deferred here to avoid colliding
/// with the in-flight `on_spawn` sweep (sts2-rs-jj9gv); tracked for a
/// monsters.rs follow-up. Bead `sts2-rs-96y80`.
pub fn create_the_forgotten_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "MIASMA",
        mv_eff(
            "MIASMA",
            smallvec![
                Intent::Debuff {
                    kind: PowerKind::Dexterity,
                    amount: -2
                },
                Intent::Defend { block: 8 },
                Intent::Buff {
                    kind: PowerKind::Dexterity,
                    amount: 2
                }
            ],
            "DREAD",
            the_forgotten_miasma_effect,
        ),
    );
    s.insert(
        "DREAD",
        mv("DREAD", smallvec![Intent::Attack { damage: 13 }], "MIASMA"),
    );
    MonsterAi::new(s, "MIASMA")
}

// ────────────────────────────────────────────────────────────────────────────
// Elite encounters
// ────────────────────────────────────────────────────────────────────────────

fn knight_states() -> BTreeMap<&'static str, MonsterState> {
    let mut s = BTreeMap::new();
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            // C# `FlailKnight.cs`: AddBranch(WAR_CHANT, CannotRepeat) +
            // AddBranch(FLAIL, 2) + AddBranch(RAM, 2). The `2`s are the
            // maxRepeats overload (CanRepeatXTimes(2), weight 1f), NOT
            // weight 2 — all three weights are 1 (total 3, not 5). Bead
            // `sts2-rs-pp5ld`.
            branches: smallvec![
                br("WAR_CHANT", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("FLAIL", MoveRepeatType::CanRepeatXTimes, 2, 1.0),
                br("RAM", MoveRepeatType::CanRepeatXTimes, 2, 1.0),
            ],
        }),
    );
    s.insert(
        "WAR_CHANT",
        MonsterState::Move(MoveState {
            id: "WAR_CHANT",
            effect: knight_war_chant_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 3
            }],
            follow_up: Some("RAND"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "FLAIL",
        mv(
            "FLAIL",
            smallvec![Intent::MultiAttack { damage: 9, hits: 2 }],
            "RAND",
        ),
    );
    s.insert(
        "RAM",
        mv("RAM", smallvec![Intent::Attack { damage: 15 }], "RAND"),
    );
    s
}

/// `FlailKnight` — RAND(WAR_CHANT CannotRepeat | FLAIL CanRepeatXTimes(2)
/// | RAM CanRepeatXTimes(2); all weight 1); initial=RAM.
pub fn create_flail_knight_ai() -> MonsterAi {
    MonsterAi::new(knight_states(), "RAM")
}

/// `MysteriousKnight` — same state machine as `FlailKnight`; the
/// per-creature Strength+6 / Plating(6) seeds move to the dispatcher
/// bead. When `shared.rs` (sts2-rs-uzn) lands this re-exports from
/// there.
pub fn create_mysterious_knight_ai() -> MonsterAi {
    MonsterAi::new(knight_states(), "RAM")
}

/// `LivingShield` — SHIELD_SLAM_BRANCH(allies>0 ? SHIELD_SLAM : SMASH)
/// → SHIELD_SLAM (loops back to branch) or SMASH (self-loop with
/// Strength buff).
///
/// Python takes a `get_ally_count` callback. We adopt the combat-global
/// proxy: `combat.enemy_uids.len() > 1` is the boolean "has allies"
/// signal (the boss itself counts as one entry). Tests drive both
/// branches by inserting / clearing extra enemy uids.
/// Count enemies that are still ALIVE. C# `LivingShield.GetAllyCount`
/// (`LivingShield.cs:70-73`) is `GetTeammatesOf(self).Count(c => c.IsAlive
/// && c != self)`; the boss is always alive while taking its turn, so
/// `alive_count > 1` ⟺ at least one *other* alive ally (`GetAllyCount > 0`).
/// The prior raw `enemy_uids.len()` counted corpses too — mirror the
/// Ovicopter corpse-filter idiom (`act2_hive.rs`). Sweep 8644df (seeds
/// 3691/4834).
fn living_shield_alive_enemy_count(combat: &CombatState) -> usize {
    combat
        .enemy_uids
        .iter()
        .filter(|&&uid| {
            combat
                .creatures
                .iter()
                .any(|c| c.id == uid.0 && c.current_hp > 0)
        })
        .count()
}

fn living_shield_has_allies(combat: &CombatState) -> bool {
    living_shield_alive_enemy_count(combat) > 1
}

fn living_shield_alone(combat: &CombatState) -> bool {
    living_shield_alive_enemy_count(combat) <= 1
}

pub fn create_living_shield_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SHIELD_SLAM_BRANCH",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "SHIELD_SLAM_BRANCH",
            branches: smallvec![
                cb(living_shield_has_allies, "SHIELD_SLAM"),
                cb(living_shield_alone, "SMASH"),
            ],
        }),
    );
    s.insert(
        "SHIELD_SLAM",
        mv(
            "SHIELD_SLAM",
            smallvec![Intent::Attack { damage: 6 }],
            "SHIELD_SLAM_BRANCH",
        ),
    );
    s.insert(
        "SMASH",
        MonsterState::Move(MoveState {
            id: "SMASH",
            effect: living_shield_smash_effect,
            intents: smallvec![
                Intent::Attack { damage: 16 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 3
                }
            ],
            follow_up: Some("SMASH"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "SHIELD_SLAM")
}

/// `MagiKnight` (`MagiKnight.cs:55-74`) — a DETERMINISTIC move chain, NOT a
/// random branch. The prior Rust port (RAND of MAGIC_MISSILE / ARCANE_BLAST /
/// ARCANE_SIEGE) was a Python-pin fabrication: none of those moves exist in
/// the C# decomp. Bead `sts2-rs-ybtmu` (KNIGHTS_ELITE parity).
///
/// Asc-0 values: `PowerShieldDamage` 6, `PowerShieldBlock` 5, `SpearDamage`
/// 10, `BombDamage` 35 (ascension is identity at the pin).
///
/// Chain (each move's `FollowUpState`):
///   FIRST_POWER_SHIELD → Attack 6 + GainBlock 5 (self) → DAMPEN
///   DAMPEN            → apply DampenPower(1) to player    → RAM
///   RAM (Spear)       → Attack 10                          → PREP
///   PREP             → GainBlock 5 (self)                 → BOMB
///   BOMB             → Attack 35                          → RAM
/// so after the one-time intro (PowerShield→Dampen→Ram→Prep→Bomb) it loops
/// RAM→PREP→BOMB forever. Start state is FIRST_POWER_SHIELD.
///
/// Block handling differs between the two shield moves because the engine
/// only performs a move's FIRST intent (`intents.first()`):
///   - PREP's first/only intent is `Intent::Defend { block: 5 }`, which the
///     Defend fast path in `combat_loop::perform_intent` resolves as a
///     self-GainBlock — correct as-is.
///   - FIRST_POWER_SHIELD's first intent is the Attack, so its `Defend` is
///     telegraph-only; the 5 self-block is applied by
///     `magi_knight_power_shield_effect` (fires after the attack).
///
/// DAMPEN's card-downgrade behavior: `DampenPower.AfterApplied` (C#
/// `DampenPower.cs:30-43`) iterates all upgraded player cards, downgrades them
/// (tracking which ones for later re-upgrade), and applies the Ethereal keyword
/// if the player also has Hex. When DAMPEN is removed, `AfterRemoved` restores
/// the original upgrade levels. This behavior is implemented via
/// `install_dampen_power_handlers()` (bead `sts2-rs-mw4fy`), which registers
/// hook bodies that fire on power-apply and power-remove.
pub fn create_magi_knight_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "FIRST_POWER_SHIELD",
        mv_eff(
            "FIRST_POWER_SHIELD",
            smallvec![Intent::Attack { damage: 6 }, Intent::Defend { block: 5 }],
            "DAMPEN",
            magi_knight_power_shield_effect,
        ),
    );
    s.insert(
        "DAMPEN",
        mv_eff(
            "DAMPEN",
            smallvec![Intent::Debuff {
                kind: PowerKind::Dampen,
                amount: 1
            }],
            "RAM",
            magi_knight_dampen_effect,
        ),
    );
    s.insert(
        "RAM",
        mv("RAM", smallvec![Intent::Attack { damage: 10 }], "PREP"),
    );
    s.insert(
        "PREP",
        mv("PREP", smallvec![Intent::Defend { block: 5 }], "BOMB"),
    );
    s.insert(
        "BOMB",
        mv("BOMB", smallvec![Intent::Attack { damage: 35 }], "RAM"),
    );
    MonsterAi::new(s, "FIRST_POWER_SHIELD")
}

/// MagiKnight FIRST_POWER_SHIELD (`MagiKnight.cs:96-103` `PowerShieldMove`):
/// after the SingleAttack(6), `CreatureCmd.GainBlock(base.Creature,
/// PowerShieldBlock=5)`. The engine only performs a move's FIRST intent
/// (`intents.first()` — the Attack here), so the secondary `Intent::Defend`
/// is telegraph-only and the block must come from this effect fn, which fires
/// after the attack lands (bead `sts2-rs-k27e` attack-effect wiring).
fn magi_knight_power_shield_effect(state: &mut CombatState, owner: CreatureUid) {
    let amount = crate::ascension_scaling::tough_block(
        state.ascension,
        sts2_core::MonsterKind::MagiKnight,
        "FIRST_POWER_SHIELD",
        5,
    );
    sts2_core::hooks::apply_effects(
        state,
        &[sts2_core::hooks::Effect::GainBlockCreature {
            target: sts2_core::hooks::CreatureRef::Enemy(owner),
            amount,
        }],
    );
}

/// MagiKnight DAMPEN_MOVE (`MagiKnight.cs:76-94`): applies `DampenPower` (1) to
/// the player. The power is applied via the legacy player-power surface, and
/// the registered `after_applied_for_power` hook fires immediately, downgrading
/// all upgraded cards and marking them for re-upgrade on power removal.
fn magi_knight_dampen_effect(state: &mut CombatState, owner: CreatureUid) {
    // Ensure the dampen power handlers are registered (idempotent via Once guard).
    static INSTALL: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    let () = INSTALL.get_or_init(|| {
        sts2_core::monster_power_effects::install_dampen_power_handlers();
    });

    // Apply the power via the legacy surface, which triggers the apply path.
    apply_power_to_player_via_legacy_surface(state, PowerKind::Dampen, 1);

    // C# `DampenPower.AddCaster(creature)` records the applying creature; the
    // power removes itself when all its casters die (`DampenPower.AfterDeath`
    // → `casters.Count == 0` → `PowerCmd.Remove`). Stamp the caster on the
    // instance via the per-power `applier` field so the death hook can drop
    // Dampen (and re-upgrade the downgraded cards) when this MagiKnight dies.
    // MagiKnight is the ONLY source of Dampen and KNIGHTS_ELITE spawns exactly
    // one, so the single-applier model is exact (sweep seed 615: FIGHT_ME
    // killed the MagiKnight, C# dropped DAMPEN, Rust kept it).
    if let Some(pid) = state.player_power_uids().into_iter().find_map(|puid| {
        state
            .powers
            .iter()
            .find(|p| p.id == puid.0 && p.kind == PowerKind::Dampen)
            .map(|p| p.id)
    }) {
        if let Some(pow) = state.powers.get_mut_by_id(pid) {
            pow.applier = Some(owner);
        }
    }

    // Manually dispatch the after_applied hook for Dampen targeting the player.
    // (The regular apply path doesn't trigger the after_applied dispatch for
    // player-debuffs via legacy surface, so we dispatch it here.)
    let player_uid = state
        .player
        .creature_uid
        .expect("player should have a creature_uid by this phase");
    let _ = sts2_core::monster_power_effects::dispatch_after_applied_for_power(
        state,
        player_uid,
        PowerKind::Dampen,
        Some(owner),
    );
}

/// `SpectralKnight` (`SpectralKnight.cs:44-58`) — HEX (Debuff: HexPower 2
/// to player) → SOUL_SLASH (SingleAttack `SoulSlashDamage`) → RAND.
/// RAND branches: SOUL_SLASH (CanRepeatXTimes(2), weight 1) | SOUL_FLAME
/// (MultiAttack `SoulFlameDamage`×3, CannotRepeat) → RAND. Initial state
/// is HEX. `SoulSlashDamage` = 15, `SoulFlameDamage` = 3 at asc-0
/// (ascension is identity at the pin).
pub fn create_spectral_knight_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "HEX",
        mv_eff(
            "HEX",
            smallvec![Intent::Debuff {
                kind: PowerKind::Hex,
                amount: 2
            }],
            "SOUL_SLASH",
            spectral_knight_hex_effect,
        ),
    );
    s.insert(
        "SOUL_SLASH",
        mv(
            "SOUL_SLASH",
            smallvec![Intent::Attack { damage: 15 }],
            "RAND",
        ),
    );
    s.insert(
        "SOUL_FLAME",
        mv(
            "SOUL_FLAME",
            smallvec![Intent::MultiAttack { damage: 3, hits: 3 }],
            "RAND",
        ),
    );
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            // C# `SpectralKnight.cs`: AddBranch(SOUL_SLASH, 2) +
            // AddBranch(SOUL_FLAME, CannotRepeat). The `2` is the
            // maxRepeats overload (CanRepeatXTimes(2), weight 1f), NOT
            // weight 2 — both weights are 1 (total 2, not 3). Bead
            // `sts2-rs-pp5ld`.
            branches: smallvec![
                br("SOUL_SLASH", MoveRepeatType::CanRepeatXTimes, 2, 1.0),
                br("SOUL_FLAME", MoveRepeatType::CannotRepeat, 1, 1.0),
            ],
        }),
    );
    MonsterAi::new(s, "HEX")
}

/// `MechaKnight` — CHARGE → FLAMETHROWER → WINDUP → HEAVY_CLEAVE →
/// FLAMETHROWER → WINDUP → HEAVY_CLEAVE → … (CHARGE fires only at
/// start, then the FLAMETHROWER/WINDUP/HEAVY_CLEAVE triple loops).
/// Bead `sts2-rs-dpov` rebaseline per C# decomp `MechaKnight.cs`.
/// Previous pin (Python 01c094d) had STEEL_SLASH/MISSILE_SALVO/
/// SHIELD_UP/OVERDRIVE_STRIKE — different move names and weaker
/// stats (CHARGE 14 vs 25, HEAVY_CLEAVE 24 vs 35) consistent with
/// the audit's Pattern-A (Python pin pre-rebalance).
///
/// Move payloads (C# decomp):
/// - CHARGE_MOVE       — Attack 25 (ChargeDamage; DeadlyAsc 30)
/// - FLAMETHROWER_MOVE — adds 4 Burn status cards to player hand
///   (StatusIntent(4)). Modeled as a single `Intent::StatusCard`
///   marker — the engine has no count-aware status intent at this
///   pin; the 4-count is encoded only in the C# move-side code.
/// - WINDUP_MOVE       — Defend 15 + Buff Strength +5 (WindupMove
///   body in C# applies StrengthPower 5m + GainBlock 15m)
/// - HEAVY_CLEAVE_MOVE — Attack 35 (HeavyCleaveDamage; DeadlyAsc 40)
pub fn create_mecha_knight_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "CHARGE",
        mv(
            "CHARGE",
            smallvec![Intent::Attack { damage: 25 }],
            "FLAMETHROWER",
        ),
    );
    s.insert(
        "FLAMETHROWER",
        mv_eff(
            "FLAMETHROWER",
            smallvec![Intent::StatusCard],
            "WINDUP",
            mecha_knight_flamethrower_effect,
        ),
    );
    s.insert(
        "WINDUP",
        MonsterState::Move(MoveState {
            id: "WINDUP",
            effect: mecha_knight_windup_effect,
            intents: smallvec![
                Intent::Defend { block: 15 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 5
                }
            ],
            follow_up: Some("HEAVY_CLEAVE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "HEAVY_CLEAVE",
        mv(
            "HEAVY_CLEAVE",
            smallvec![Intent::Attack { damage: 35 }],
            "FLAMETHROWER",
        ),
    );
    MonsterAi::new(s, "CHARGE")
}

/// `Osty` — HAUNT self-loop. SoulNexus-spawned pet/ally (HP 1).
///
/// C# `Osty.cs:GenerateMoveStateMachine` ships a single `NOTHING_MOVE`
/// whose body is `(IReadOnlyList<Creature> _) => Task.CompletedTask` —
/// Osty does nothing each turn. Self-acting damage attribution flows
/// via `FromOsty` card-hand-offs (Unleash / Protector etc., tagged
/// `CardTag.OstyAttack`), not via an Osty intent. Bead `sts2-rs-0meqd`.
///
/// The prior Rust port leaned on a fabricated 4-damage HAUNT attack
/// to source Osty-as-dealer events for the BoneFlute relic. That
/// surface is not C#-faithful; the canonical wiring (route
/// `play_unleash` / `play_protector` damage with Osty as dealer when
/// the player owns Osty) is left for a follow-up bead — BoneFlute's
/// `on_after_damage_given` listener falls silent in the interim,
/// which is closer to C# behaviour than the fake attack was.
pub fn create_osty_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert("HAUNT", mv("HAUNT", smallvec![Intent::Sleep], "HAUNT"));
    MonsterAi::new(s, "HAUNT")
}

/// `SoulNexus` — SOUL_BURN_MOVE → RAND{SOUL_BURN | MAELSTROM |
/// DRAIN_LIFE} with `MoveRepeatType::CannotRepeat`. Mirrors C# decomp
/// `MegaCrit.Sts2.Core.Models.Monsters/SoulNexus.cs` v0.103.2 (bead
/// `sts2-rs-blwi`, parent `sts2-rs-kgwj`). Python pin `01c094d` ships
/// a stale 155 HP / 3-state SUMMON-SOUL_DRAIN-SPIRIT_BARRAGE variant
/// (with a 10-HP self-heal on SOUL_DRAIN) we deliberately diverge
/// from per the monster-stat audit
/// (`docs/perf/2026-05-18-monster-stat-audit.md`, Pattern A -34% HP).
///
/// Move payloads track C# constants:
/// - SOUL_BURN_MOVE → `SingleAttackIntent(29)`.
/// - MAELSTROM_MOVE → `MultiAttackIntent(6, 4)`.
/// - DRAIN_LIFE_MOVE → `SingleAttackIntent(18)` + `DebuffIntent(strong: true)`.
///   The C# strong-debuff modal is approximated as the `Intent::DebuffStrong`
///   telegraph marker; the actual powers it applies — `Vulnerable(2)` +
///   `Weak(2)` to the player per `SoulNexus.cs:95-101` `DrainLifeMove`
///   (`PowerCmd.Apply<VulnerablePower>(targets, 2m) ;
///   PowerCmd.Apply<WeakPower>(targets, 2m)`) — are applied by
///   [`soul_nexus_drain_life_effect`] (bead `sts2-rs-pdsdl`, which
///   corrected the prior comment that wrongly asserted `DrainLifeMove`
///   never calls `PowerCmd.Apply`).
///
/// `Intent::Heal` no longer appears in the SoulNexus telegraph — the
/// Python-pin self-heal mechanic from beads `sts2-rs-va5r` /
/// `sts2-rs-yzak` is not present in C# v0.103.2.
pub fn create_soul_nexus_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SOUL_BURN_MOVE",
        mv(
            "SOUL_BURN_MOVE",
            smallvec![Intent::Attack { damage: 29 }],
            "RAND",
        ),
    );
    s.insert(
        "MAELSTROM_MOVE",
        mv(
            "MAELSTROM_MOVE",
            smallvec![Intent::MultiAttack { damage: 6, hits: 4 }],
            "RAND",
        ),
    );
    s.insert(
        "DRAIN_LIFE_MOVE",
        MonsterState::Move(MoveState {
            id: "DRAIN_LIFE_MOVE",
            effect: soul_nexus_drain_life_effect,
            intents: smallvec![Intent::Attack { damage: 18 }, Intent::DebuffStrong],
            follow_up: Some("RAND"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            branches: smallvec![
                br("SOUL_BURN_MOVE", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("MAELSTROM_MOVE", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("DRAIN_LIFE_MOVE", MoveRepeatType::CannotRepeat, 1, 1.0),
            ],
        }),
    );
    MonsterAi::new(s, "SOUL_BURN_MOVE")
}

// ────────────────────────────────────────────────────────────────────────────
// Boss encounters
// ────────────────────────────────────────────────────────────────────────────

/// `Aeonglass` — Act-3 boss (v0.107.1, replaces Doormaker). Three-move cycle
/// (no one-shot intro): EBB_MOVE → EYE_LASERS_MOVE → INCREASING_INTENSITY_MOVE
/// → EBB_MOVE → … Mirrors C#
/// `MegaCrit.Sts2.Core.Models.Monsters/Aeonglass.cs::GenerateMoveStateMachine`
/// (`moveState.FollowUpState = moveState2`, etc.; initial state EBB_MOVE).
///
/// - EBB_MOVE: `SingleAttackIntent(EbbDamage)` + `DefendIntent`. EbbDamage =
///   `GetValueIfAscension(DeadlyEnemies, 32, 26)` ⇒ A0 base 26 (ascension
///   table provides the A9 32). Block 33 (constant) applied in
///   [`aeonglass_ebb_effect`].
/// - EYE_LASERS_MOVE: `MultiAttackIntent(EyeLasersDamage, 2)`. EyeLasersDamage
///   = `GetValueIfAscension(DeadlyEnemies, 12, 11)` ⇒ A0 base 11 × 2 hits.
/// - INCREASING_INTENSITY_MOVE: `StatusIntent(WitherAmount)` + `BuffIntent`.
///   Adds `WitherAmount` (A0 1 / A9 2) Wither cards, bumps every existing
///   Wither's FakeUpgrade level, and grants self scaling Strength — see
///   [`aeonglass_increasing_intensity_effect`].
///
/// On-spawn `WitheringPresence(6)` (player) + `Artifact(3)` (self) are applied
/// via [`sts2_core::monsters::MonsterKind::on_spawn_opponent_debuffs`] /
/// `on_spawn_self_buffs`, not here. Bead `sts2-rs-v0107-content-port-wb8lk.2`.
pub fn create_aeonglass_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "EBB_MOVE",
        MonsterState::Move(MoveState {
            id: "EBB_MOVE",
            effect: aeonglass_ebb_effect,
            intents: smallvec![Intent::Attack { damage: 26 }, Intent::Defend { block: 33 },],
            follow_up: Some("EYE_LASERS_MOVE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "EYE_LASERS_MOVE",
        mv(
            "EYE_LASERS_MOVE",
            smallvec![Intent::MultiAttack {
                damage: 11,
                hits: 2
            }],
            "INCREASING_INTENSITY_MOVE",
        ),
    );
    s.insert(
        "INCREASING_INTENSITY_MOVE",
        MonsterState::Move(MoveState {
            id: "INCREASING_INTENSITY_MOVE",
            effect: aeonglass_increasing_intensity_effect,
            intents: smallvec![
                Intent::StatusCard,
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 3,
                },
            ],
            follow_up: Some("EBB_MOVE"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "EBB_MOVE")
}

/// YOURE_MINE_MOVE applies Frail(99), Weak(99), Vulnerable(99) to the
/// player. C# `Queen.cs:163-172` `YoureMineMove`:
/// `PowerCmd.Apply<{FrailPower,WeakPower,VulnerablePower}>(targets, 99m, …)`.
fn queen_youre_mine_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 99);
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 99);
    apply_power_to_player_via_legacy_surface(state, PowerKind::Vulnerable, 99);
}

/// BURN_BRIGHT_FOR_ME applies Strength(+1) to every teammate (excluding
/// self) and gains 20 block on self. C# `Queen.cs:174-185`
/// `BurnBrightForMeMove`: enumerates `GetTeammatesOf(self)`,
/// `PowerCmd.Apply<StrengthPower>(item, 1m, …)` for each teammate that
/// isn't self, then `CreatureCmd.GainBlock(self, 20m, Move, …)`.
fn queen_burn_bright_effect(state: &mut CombatState, owner: CreatureUid) {
    use sts2_core::CreatureKind;
    let teammates: Vec<CreatureUid> = state
        .creatures
        .iter()
        .filter(|c| c.current_hp > 0)
        .map(|c| CreatureUid(c.id))
        .filter(|&uid| uid != owner && state.creature_kind(uid) == CreatureKind::Monster)
        .collect();
    for uid in teammates {
        sts2_core::card_effects::apply_power_to_creature(state, uid, PowerKind::Strength, 1);
    }
    // Via the shared `CreatureCmd.GainBlock` funnel (modify chain +
    // `after_block_gained` buses + the combat-log row), not a raw field write
    // — see `slithering_strangler_twack_effect` for why.
    sts2_core::hooks::apply_effects(
        state,
        &[sts2_core::hooks::Effect::GainBlockCreature {
            target: sts2_core::hooks::CreatureRef::Enemy(owner),
            amount: 20,
        }],
    );
}

/// ENRAGE_MOVE applies Strength(+2) to self. C# `Queen.cs:206-211`
/// `EnrageMove`: `PowerCmd.Apply<StrengthPower>(self, 2m, …)`.
fn queen_enrage_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// Conditional predicate for the YOURE_MINE_NOW / BURN_BRIGHT_FOR_ME
/// conditional branches. Reads the Queen AI's per-instance counter as
/// `_hasAmalgamDied` (0=false, 1=true). The flip is performed by
/// [`queen_after_death`] (bead `sts2-rs-6g4v4`, m70xg.3) via the
/// cross-creature `AfterDeath` dispatch in `combat_loop.rs` (bead
/// `sts2-rs-79tv9`): when TorchHeadAmalgam dies, the listener sets
/// the counter and `SetMoveImmediate(EnragedState)` if NextMove ==
/// BurnBrightForMeState. While both monsters are alive the counter
/// stays 0 and the branch picks BURN_BRIGHT.
fn queen_has_amalgam_died(_state: &CombatState, _owner: CreatureUid, counter: i32) -> bool {
    counter > 0
}

/// PUPPET_STRINGS_MOVE effect — bead `sts2-rs-igdhp` (f933s sub-3).
///
/// C# `Queen.cs:156-161 PuppetStringsMove`:
///
///   await PowerCmd.Apply<ChainsOfBindingPower>(targets, 3m, Creature, null);
///
/// Applies 3 stacks of `ChainsOfBinding` to the player. The
/// power-side listener (sub-1, [`sts2-rs-kese5`]) then afflicts each
/// newly-drawn player card with `Bound` up to 3 per turn while the
/// power is active. The per-turn cap-check budget lives on
/// `TurnEvents::bound_cards_afflicted_by_chains_this_turn` (sub-2,
/// `sts2-rs-hmxmc`).
fn queen_puppet_strings_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::ChainsOfBinding, 3);
}

/// `AfterDeath` listener for the Queen — bead `sts2-rs-6g4v4` (m70xg.3).
///
/// C# `Queen.cs:213-225` `AfterDeath`: when `creature.Monster is
/// TorchHeadAmalgam && base.Creature.IsAlive`, set `HasAmalgamDied =
/// true` and, if `NextMove == BurnBrightForMeState`, call
/// `SetMoveImmediate(EnragedState)`. We mirror that here:
///
/// 1. The cross-creature dispatch loop in `combat_loop.rs` only fires
///    on living owners (`current_hp > 0`), so the C# `IsAlive` guard
///    is structurally enforced upstream.
/// 2. We bail unless the dead creature's `species` is
///    `TorchHeadAmalgam` — Queen ignores deaths of anything else.
/// 3. We set the per-instance counter to 1 so every subsequent
///    `ConditionalBranch` evaluation of [`queen_has_amalgam_died`]
///    returns `true` (flipping both `YOURE_MINE_NOW_BRANCH` and
///    `BURN_BRIGHT_FOR_ME_BRANCH` to the
///    OFF_WITH_YOUR_HEAD / EXECUTION / ENRAGE chain).
/// 4. If Queen's currently-resolved next move (post-perform follow-up
///    resolution) is `BURN_BRIGHT_FOR_ME_MOVE`, we
///    [`force_next_state`](sts2_core::monster_ai::MonsterAi::force_next_state)
///    to `ENRAGE_MOVE` — that is C# `SetMoveImmediate(EnragedState)`.
///    Otherwise we let the normal graph + flipped counter carry her
///    into the OFF_WITH_YOUR_HEAD chain on the next roll.
fn queen_after_death(
    state: &mut sts2_core::CombatState,
    dead: sts2_core::CreatureUid,
    owner: sts2_core::CreatureUid,
) {
    let dead_species = state
        .creatures
        .iter()
        .find(|c| c.id == dead.0)
        .and_then(|c| c.species);
    if dead_species != Some(sts2_core::MonsterKind::TorchHeadAmalgam) {
        return;
    }
    let Some(owner_idx) = state.creatures.iter().position(|c| c.id == owner.0) else {
        return;
    };
    let Some(ai) = state.creatures[owner_idx].ai.as_mut() else {
        return;
    };
    ai.set_counter(1);
    if ai.current_state_id() == "BURN_BRIGHT_FOR_ME_MOVE" {
        ai.force_next_state("ENRAGE_MOVE");
    }
}

/// `Queen` — C# `Queen.cs` v0.103.2 7-state machine. Initial is
/// PUPPET_STRINGS_MOVE.
///
/// PUPPET_STRINGS_MOVE  → YOURE_MINE_MOVE
/// YOURE_MINE_MOVE      → YOURE_MINE_NOW_BRANCH
/// YOURE_MINE_NOW_BRANCH → ConditionalBranch
///     - HasAmalgamDied  → OFF_WITH_YOUR_HEAD_MOVE
///     - else            → BURN_BRIGHT_FOR_ME_MOVE
/// BURN_BRIGHT_FOR_ME_MOVE → BURN_BRIGHT_FOR_ME_BRANCH
/// BURN_BRIGHT_FOR_ME_BRANCH → ConditionalBranch (same condition)
///     - HasAmalgamDied  → OFF_WITH_YOUR_HEAD_MOVE
///     - else            → BURN_BRIGHT_FOR_ME_MOVE
/// OFF_WITH_YOUR_HEAD_MOVE → EXECUTION_MOVE
/// EXECUTION_MOVE          → ENRAGE_MOVE
/// ENRAGE_MOVE             → OFF_WITH_YOUR_HEAD_MOVE
///
/// The prior fabricated SUMMON_GUARD → ROYAL_STRIKE → GUILLOTINE chain
/// (bead `sts2-rs-r9pl`) doesn't exist in the C# pin — Queen does not
/// summon, doesn't have a 30-damage Guillotine, and doesn't apply Weak
/// via her attacks. Bead `sts2-rs-cd77n` (parent umbrella `sts2-rs-m70xg`).
///
/// PUPPET_STRINGS_MOVE applies `ChainsOfBindingPower(3)` to the
/// player via [`queen_puppet_strings_effect`] (bead `sts2-rs-igdhp`,
/// f933s sub-3). The power-side `AfterCardDrawn` afflicts each
/// newly-drawn player card with `Bound` up to 3 per turn (bead
/// `sts2-rs-kese5`, f933s sub-1; per-turn cap-check budget on
/// [`TurnEvents::bound_cards_afflicted_by_chains_this_turn`], bead
/// `sts2-rs-hmxmc`, f933s sub-2). The companion
/// `BeforeCardPlayed` / `ShouldPlay` / `BeforeTurnEnd` hooks
/// (gate further plays of any Bound card once one has been played;
/// clear all `Bound` afflictions at turn end) remain as future
/// follow-ups under [`sts2-rs-f933s`].
///
/// DEFERRED:
/// - HasAmalgamDied flag flip on TorchHeadAmalgam death is wired via
///   the [`queen_after_death`] listener (bead `sts2-rs-6g4v4`,
///   m70xg.3): when TorchHeadAmalgam enters `newly_dead_this_turn`
///   the cross-creature `AfterDeath` dispatch (bead `sts2-rs-79tv9`)
///   flips Queen's counter to 1 and force-jumps her to ENRAGE if her
///   resolved next move is BURN_BRIGHT_FOR_ME_MOVE.
pub fn create_queen_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "PUPPET_STRINGS_MOVE",
        MonsterState::Move(MoveState {
            id: "PUPPET_STRINGS_MOVE",
            effect: queen_puppet_strings_effect,
            intents: smallvec![Intent::Debuff {
                kind: PowerKind::ChainsOfBinding,
                amount: 3,
            }],
            follow_up: Some("YOURE_MINE_MOVE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "YOURE_MINE_MOVE",
        MonsterState::Move(MoveState {
            id: "YOURE_MINE_MOVE",
            effect: queen_youre_mine_effect,
            intents: smallvec![
                Intent::Debuff {
                    kind: PowerKind::Frail,
                    amount: 99,
                },
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 99,
                },
                Intent::Debuff {
                    kind: PowerKind::Vulnerable,
                    amount: 99,
                },
            ],
            follow_up: Some("YOURE_MINE_NOW_BRANCH"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "YOURE_MINE_NOW_BRANCH",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "YOURE_MINE_NOW_BRANCH",
            branches: smallvec![
                ConditionalBranch {
                    condition: ConditionFn::CounterAware(queen_has_amalgam_died),
                    state_id: "OFF_WITH_YOUR_HEAD_MOVE",
                },
                ConditionalBranch {
                    condition: ConditionFn::Stateless(|_| true),
                    state_id: "BURN_BRIGHT_FOR_ME_MOVE",
                },
            ],
        }),
    );
    s.insert(
        "BURN_BRIGHT_FOR_ME_MOVE",
        MonsterState::Move(MoveState {
            id: "BURN_BRIGHT_FOR_ME_MOVE",
            effect: queen_burn_bright_effect,
            intents: smallvec![
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 1,
                },
                Intent::Defend { block: 20 },
            ],
            follow_up: Some("BURN_BRIGHT_FOR_ME_BRANCH"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "BURN_BRIGHT_FOR_ME_BRANCH",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "BURN_BRIGHT_FOR_ME_BRANCH",
            branches: smallvec![
                ConditionalBranch {
                    condition: ConditionFn::CounterAware(queen_has_amalgam_died),
                    state_id: "OFF_WITH_YOUR_HEAD_MOVE",
                },
                ConditionalBranch {
                    condition: ConditionFn::Stateless(|_| true),
                    state_id: "BURN_BRIGHT_FOR_ME_MOVE",
                },
            ],
        }),
    );
    s.insert(
        "OFF_WITH_YOUR_HEAD_MOVE",
        mv(
            "OFF_WITH_YOUR_HEAD_MOVE",
            smallvec![Intent::MultiAttack { damage: 3, hits: 5 }],
            "EXECUTION_MOVE",
        ),
    );
    s.insert(
        "EXECUTION_MOVE",
        mv(
            "EXECUTION_MOVE",
            smallvec![Intent::Attack { damage: 15 }],
            "ENRAGE_MOVE",
        ),
    );
    s.insert(
        "ENRAGE_MOVE",
        MonsterState::Move(MoveState {
            id: "ENRAGE_MOVE",
            effect: queen_enrage_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2,
            }],
            follow_up: Some("OFF_WITH_YOUR_HEAD_MOVE"),
            must_perform_once: false,
        }),
    );
    let mut ai = MonsterAi::new(s, "PUPPET_STRINGS_MOVE");
    // Bead `sts2-rs-6g4v4` (m70xg.3): TorchHeadAmalgam AfterDeath flips
    // Queen's `HasAmalgamDied` counter and short-circuits BURN_BRIGHT →
    // ENRAGE per C# `Queen.cs:213-225`.
    ai.after_death = Some(queen_after_death);
    ai
}

/// SKULL_BASH applies Vulnerable(1) to the player (C# `SkullBashMove`
/// `PowerCmd.Apply<VulnerablePower>(targets, 1m)`), alongside the 14-damage
/// attack. Bead `sts2-rs-jz9eo`.
fn test_subject_skull_bash_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Vulnerable, 1);
}

/// BURNING_GROWL applies `StrengthPower(BurningGrowlStrengthGain)` to self
/// (C# `BurningGrowlMove` `PowerCmd.Apply<StrengthPower>(base.Creature,
/// BurningGrowlStrengthGain, …)`) and adds `BurningGrowlBurnCount` Burn
/// cards to the player's hand (C# `CardPileCmd.AddToCombatAndPreview<Burn>(
/// player.PlayerCombatState, PileType.Hand, BurningGrowlBurnCount)`).
/// Non-ascension: Strength = 2, Burn = 3 (ascension is identity at pin
/// `01c094d`). Bead `sts2-rs-jnf4a` slice — the BURNING_GROWL_MOVE state
/// itself is unreachable until the AdaptablePower.AfterDeath → DeadState
/// → REVIVE_BRANCH pipeline lands; this effect_fn is wired now so the
/// state-machine scaffold is ready when that infra arrives.
fn test_subject_burning_growl_effect(state: &mut CombatState, owner: CreatureUid) {
    use sts2_core::ascension::{get_value_if_ascension, AscensionLevel::DeadlyEnemies};
    use sts2_core::card_effects::PileKind;
    use sts2_core::monsters::test_subject;
    // BurningGrowlStrengthGain / BurningGrowlBurnCount — TestSubject.cs:102,104,
    // both `GetValueIfAscension(DeadlyEnemies, …)`.
    let strength = get_value_if_ascension(
        state.ascension,
        DeadlyEnemies,
        test_subject::BURNING_GROWL_STRENGTH_DEADLY,
        test_subject::BURNING_GROWL_STRENGTH,
    );
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, strength);
    let burns = get_value_if_ascension(
        state.ascension,
        DeadlyEnemies,
        test_subject::BURNING_GROWL_BURN_COUNT_DEADLY,
        test_subject::BURNING_GROWL_BURN_COUNT,
    );
    for _ in 0..burns {
        let _ = crate::cards::status::spawn_burn(state, PileKind::Hand);
    }
}

/// `TestSubject.MultiClawMove` — `ExtraMultiClawCount++` AFTER the attack
/// resolves (C# TestSubject.cs:310), so the NEXT MULTI_CLAW telegraph shows
/// one more hit (`MultiClawTotalCount => BaseMultiClawCount +
/// ExtraMultiClawCount`, TestSubject.cs:163: 3→4→5→…).
///
/// The extra count lives in the owner's PainfulStabs power counter: the
/// power's lifetime (applied on the 1st revive, removed on the 2nd —
/// [`adaptable_after_death`]) exactly brackets the 2nd form, the only phase
/// where MULTI_CLAW is reachable. `combat_loop::roll_next_enemy_intents`
/// adds the counter to the state machine's static base hits when
/// telegraphing. 2026-07-17 matched-seed gap audit (seeds 1012/1016/1026/
/// 1028): the static 3-hit model under-predicted late-2nd-form damage 2×.
fn test_subject_multi_claw_effect(state: &mut CombatState, owner: CreatureUid) {
    let painful_stabs_pid = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().find_map(|pu| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == pu.0 && p.kind == PowerKind::PainfulStabs)
                    .map(|p| p.id)
            })
        });
    if let Some(pid) = painful_stabs_pid {
        if let Some(p) = state.powers.get_mut_by_id(pid) {
            p.counter = p.counter.saturating_add(1);
        }
    }
}

/// `AdaptablePower.AfterDeath` — revive the TestSubject on each of its first
/// two deaths.
///
/// C# `AdaptablePower.cs` (v0.103.2):
///   - `ShouldStopCombatFromEnding()` → `true` (always; until the power is
///     removed on the second revive).
///   - `ShouldCreatureBeRemovedFromCombatAfterDeath()` → `false` for owner.
///   - `AfterDeath` → sets `isReviving = true`, calls `testSubject.TriggerDeadState()`.
///
/// C# `TestSubject.cs:252-278` `RespawnMove` (called via `TriggerDeadState` →
/// `SetMoveImmediate(DeadState)` → the state machine performs the RESPAWN_MOVE):
///   - Respawns == 1: `Revive(SecondFormHp = 200)` + `Apply<PainfulStabsPower>(1)`.
///   - Respawns == 2: `Revive(ThirdFormHp = 300)` + `Apply<NemesisPower>(1)`
///     + `Remove<AdaptablePower>` + `Remove<PainfulStabsPower>`.
///
/// We encode the respawn count in the `Adaptable` power's `counter` field
/// (starts at 0 at spawn, incremented here on each revive) and mirror it onto
/// the AI `counter` for the REVIVE_BRANCH predicate. On the 2nd revive
/// (`counter` reaches 2) the power is removed so
/// `combat_loop::should_stop_combat_ending` no longer keeps the combat open;
/// the third death resolves normally.
///
/// **Timing (bead `sts2-rs-jnf4a`).** C# does NOT revive at death: the corpse
/// stays at hp 0 until the TestSubject's NEXT enemy turn, when `RESPAWN_MOVE`
/// performs the heal-to-next-form. This handler is therefore invoked from the
/// enemy-turn revive pass in `combat_loop::enemy_turn_exec` (mirroring the
/// IllusionPower Step-2.5 revive), NOT from the `enemy_death_effects` death
/// chokepoint — `fire_for_owner`/`fire_pending` skip `PowerKind::Adaptable`.
/// It stays registered in `enemy_death_effects` only so the revive pass can
/// `lookup` it. Originally bead `sts2-rs-hjpnj` (death-time revive); retimed
/// under `sts2-rs-jnf4a`.
pub(crate) fn adaptable_after_death(state: &mut CombatState, owner: CreatureUid) {
    use sts2_core::card_effects::apply_power_to_creature;
    use sts2_core::monsters::test_subject;

    // Snapshot the power-uid list for the owner so we can look up power data
    // without holding a borrow on `state.creatures` while we mutate `state`.
    let owner_power_uids: smallvec::SmallVec<[std::num::NonZeroU32; 8]> = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .map(|c| c.powers.iter().map(|pu| pu.0).collect())
        .unwrap_or_default();

    // Find the Adaptable power's (id, counter) on this owner.
    let adaptable_entry: Option<(std::num::NonZeroU32, i32)> =
        owner_power_uids.iter().find_map(|&pid| {
            state
                .powers
                .iter()
                .find(|p| p.id == pid && p.kind == PowerKind::Adaptable)
                .map(|p| (p.id, p.counter))
        });

    let Some((adaptable_pid, respawns_before)) = adaptable_entry else {
        return; // no Adaptable power — nothing to do
    };

    let respawns_after = respawns_before + 1;

    // Increment the respawn counter on the Adaptable power.
    if let Some(p) = state.powers.get_mut_by_id(adaptable_pid) {
        p.counter = respawns_after;
    }

    // Determine form HP for this revive.
    let form_hp = if respawns_after == 1 {
        test_subject::SECOND_FORM_HP
    } else {
        test_subject::THIRD_FORM_HP
    };

    // Revive: set max_hp and current_hp to the new form HP, clear block.
    // C# `Revive(baseRespawnHp)`:
    //   `scaledHp = baseRespawnHp * Players.Count` (single-player → ×1).
    //   `CreatureCmd.SetMaxHp(base.Creature, scaledHp)` → `CreatureCmd.Heal(…)`.
    // Direct field writes — `creature_gain_max_hp` / `creature_heal` gate on
    // `current_hp > 0`, which is false for a dead creature.
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
        c.max_hp = form_hp;
        c.current_hp = form_hp;
        c.block = 0;
    }

    // Apply form-transition powers.
    if respawns_after == 1 {
        // First revive: Apply PainfulStabs(1). C# TestSubject.cs:267-268.
        apply_power_to_creature(state, owner, PowerKind::PainfulStabs, 1);
    } else {
        // Second revive: Apply Nemesis(1), remove AdaptablePower + PainfulStabsPower.
        // C# TestSubject.cs:271-274.
        apply_power_to_creature(state, owner, PowerKind::Nemesis, 1);

        // Re-snapshot power uids after Nemesis was applied (apply may push new power).
        let updated_power_uids: smallvec::SmallVec<[std::num::NonZeroU32; 8]> = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .map(|c| c.powers.iter().map(|pu| pu.0).collect())
            .unwrap_or_default();

        // Remove PainfulStabs from owner. C# TestSubject.cs:274.
        let painful_stabs_id: Option<std::num::NonZeroU32> =
            updated_power_uids.iter().find_map(|&pid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == pid && p.kind == PowerKind::PainfulStabs)
                    .map(|p| p.id)
            });
        if let Some(pid) = painful_stabs_id {
            state.powers.retain(|p| p.id != pid);
            for c in &mut state.creatures {
                c.powers.retain(|u| u.0 != pid);
            }
            state.player.powers.retain(|u| u.0 != pid);
        }

        // Remove AdaptablePower from owner. C# TestSubject.cs:273.
        // After removal the corpse no longer counts as "pending revive"
        // (`combat_loop::should_stop_combat_ending`), so the third death
        // resolves combat normally.
        state.powers.retain(|p| p.id != adaptable_pid);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != adaptable_pid);
        }
        state.player.powers.retain(|u| u.0 != adaptable_pid);
    }

    // Advance the move state machine to the post-revive branch. C#
    // `TriggerDeadState` → `SetMoveImmediate(DeadState=RESPAWN)`; the
    // RESPAWN move's perform (this revive pass) then transitions
    // RESPAWN → REVIVE_BRANCH → MULTI_CLAW (Respawns < 2) / PHASE3_LACERATE.
    // We mirror that by forcing the AI onto RESPAWN, marking it performed,
    // and seeding the AI `counter` with the respawn count so the
    // REVIVE_BRANCH `CounterAware` predicate routes correctly. The next
    // intent roll (`enemy_turn_end::roll_next_enemy_intents`, now that the
    // creature is alive) resolves the branch and telegraphs the post-revive
    // move. Bead `sts2-rs-jnf4a`.
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
        if let Some(ai) = c.ai.as_mut() {
            ai.set_counter(respawns_after);
            ai.force_current_state("RESPAWN");
            ai.on_move_performed();
        }
    }
}

/// `TestSubject` — phase 1: BITE ↔ SKULL_BASH.
///
/// Rebased to C# v0.103.2 `TestSubject.cs` (source of truth). The prior
/// CHARGE_UP/DISCHARGE/OVERLOAD + fabricated P2 chain (CHAIN_LIGHTNING etc.)
/// does not exist in the decomp. Non-ascension: BiteDamage 20,
/// SkullBashDamage 14 (+ Vulnerable 1). Initial is BITE (C#
/// `MonsterMoveStateMachine(list, BITE_MOVE)`).
///
/// Phase 1 (BITE ↔ SKULL_BASH) is faithful to C# `TestSubject.cs`. Phase
/// 2 and 3 (post-respawn moves: MULTI_CLAW, PHASE3_LACERATE, BIG_POUNCE,
/// BURNING_GROWL) are scaffolded below; the DeadState state-machine
/// transition is deferred to bead `sts2-rs-jz9eo`. The
/// `adaptable_after_death` handler (bead `sts2-rs-hjpnj`) correctly revives
/// the TestSubject at SecondFormHp/ThirdFormHp, so post-revive the boss
/// continues in BITE ↔ SKULL_BASH until the full phase-3 pipeline lands.
pub fn create_test_subject_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "BITE",
        mv(
            "BITE",
            smallvec![Intent::Attack { damage: 20 }],
            "SKULL_BASH",
        ),
    );
    s.insert(
        "SKULL_BASH",
        mv_eff(
            "SKULL_BASH",
            smallvec![
                Intent::Attack { damage: 14 },
                Intent::Debuff {
                    kind: PowerKind::Vulnerable,
                    amount: 1
                }
            ],
            "BITE",
            test_subject_skull_bash_effect,
        ),
    );
    // ── Phase-3 scaffold (sts2-rs-jnf4a) ────────────────────────────────
    // C# `TestSubject.cs:GenerateMoveStateMachine` (the post-DeadState
    // REVIVE_BRANCH side) builds two distinct continuations:
    //   * Respawns < 2: MULTI_CLAW → MULTI_CLAW (self-loop with
    //     `ExtraMultiClawCount++` per perform — hit count grows from
    //     3 by one per repeat). The self-loop matches
    //     `moveState3.FollowUpState = moveState3` at TestSubject.cs:226.
    //   * Respawns >= 2: PHASE3_LACERATE → BIG_POUNCE → BURNING_GROWL
    //     → PHASE3_LACERATE (cycle). Matches the three FollowUpState
    //     assignments at TestSubject.cs:227-229.
    // MULTI_CLAW's `ExtraMultiClawCount++` growth is modeled via
    // [`test_subject_multi_claw_effect`] (increment stored in the owner's
    // PainfulStabs power counter) + the telegraph-time hit-count add in
    // `combat_loop::roll_next_enemy_intents`; the static intent below
    // carries only the BASE hit count.
    s.insert(
        "MULTI_CLAW",
        mv_eff(
            "MULTI_CLAW",
            smallvec![Intent::MultiAttack {
                damage: sts2_core::monsters::test_subject::MULTI_CLAW_DAMAGE,
                hits: sts2_core::monsters::test_subject::BASE_MULTI_CLAW_COUNT as u8,
            }],
            "MULTI_CLAW",
            test_subject_multi_claw_effect,
        ),
    );
    s.insert(
        "PHASE3_LACERATE",
        mv(
            "PHASE3_LACERATE",
            smallvec![Intent::MultiAttack {
                damage: sts2_core::monsters::test_subject::PHASE3_LACERATE_DAMAGE,
                hits: 3u8,
            }],
            "BIG_POUNCE",
        ),
    );
    s.insert(
        "BIG_POUNCE",
        mv(
            "BIG_POUNCE",
            smallvec![Intent::Attack {
                damage: sts2_core::monsters::test_subject::BIG_POUNCE_DAMAGE
            }],
            "BURNING_GROWL",
        ),
    );
    s.insert(
        "BURNING_GROWL",
        mv_eff(
            "BURNING_GROWL",
            smallvec![
                Intent::StatusCard,
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2,
                },
            ],
            "PHASE3_LACERATE",
            test_subject_burning_growl_effect,
        ),
    );
    // ── RESPAWN / REVIVE_BRANCH (sts2-rs-jnf4a) ─────────────────────────
    // C# `TestSubject.cs:187` `DeadState = MoveState("RESPAWN_MOVE",
    // RespawnMove, HealIntent, BuffIntent) { MustPerformOnceBeforeTransitioning
    // = true }`, with `DeadState.FollowUpState = REVIVE_BRANCH` and
    // `REVIVE_BRANCH.AddState(MULTI_CLAW, Respawns < 2)` /
    // `.AddState(PHASE3_LACERATE, Respawns >= 2)`. The actual heal/power
    // application is driven by [`adaptable_after_death`] from the enemy-turn
    // revive pass in `combat_loop::enemy_turn_exec` (NOT a death-time revive);
    // this state exists so that, post-revive, `force_current_state("RESPAWN")`
    // + `on_move_performed()` makes the next intent roll resolve
    // RESPAWN → REVIVE_BRANCH → the correct post-revive moveset. The move's
    // own `effect` is a no-op (the revive ran in the pass); its Heal/Buff
    // intents are the telegraph C# shows on the reviving corpse's turn.
    s.insert(
        "RESPAWN",
        MonsterState::Move(MoveState {
            id: "RESPAWN",
            effect: noop_effect,
            intents: smallvec![
                Intent::Heal,
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 0,
                },
            ],
            follow_up: Some("REVIVE_BRANCH"),
            must_perform_once: true,
        }),
    );
    s.insert(
        "REVIVE_BRANCH",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "REVIVE_BRANCH",
            branches: smallvec![
                ConditionalBranch {
                    condition: ConditionFn::CounterAware(test_subject_respawns_lt_two),
                    state_id: "MULTI_CLAW",
                },
                ConditionalBranch {
                    condition: ConditionFn::Stateless(always_true),
                    state_id: "PHASE3_LACERATE",
                },
            ],
        }),
    );
    MonsterAi::new(s, "BITE")
}

/// REVIVE_BRANCH predicate: route to the phase-2 MULTI_CLAW self-loop while
/// the TestSubject still has revives left (`Respawns < 2`), else to the
/// phase-3 cycle (C# `TestSubject.cs:205-206`). `counter` is the AI's
/// respawn count, set by [`adaptable_after_death`] on each revive.
fn test_subject_respawns_lt_two(_: &CombatState, _: CreatureUid, counter: i32) -> bool {
    counter < 2
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: every ctor produces an AI whose initial move resolves
    /// without panicking.
    #[test]
    fn every_ctor_has_resolvable_initial_move() {
        let smokes: Vec<MonsterAi> = vec![
            create_devoted_sculptor_ai(),
            create_scroll_of_biting_ai(0),
            create_turret_operator_ai(),
            create_axebot_ai(false),
            create_axebot_ai(true),
            create_zapbot_ai(),
            create_stabbot_ai(),
            create_guardbot_ai(),
            create_noisebot_ai(),
            create_fabricator_ai(),
            create_frog_knight_ai(),
            create_globe_head_ai(),
            create_owl_magistrate_ai(),
            create_slimed_berserker_ai(),
            create_the_lost_ai(),
            create_the_forgotten_ai(),
            create_flail_knight_ai(),
            create_mysterious_knight_ai(),
            create_living_shield_ai(),
            create_magi_knight_ai(),
            create_spectral_knight_ai(),
            create_mecha_knight_ai(),
            create_osty_ai(),
            create_soul_nexus_ai(),
            create_aeonglass_ai(),
            create_queen_ai(),
            create_test_subject_ai(),
        ];
        for ai in smokes {
            let _ = ai.current_move();
        }
    }

    /// C# `ScrollOfBiting.cs::GenerateMoveStateMachine` selects the
    /// initial state off `StarterMoveIdx % 3`: 0 => CHOMP, 1 => CHEW,
    /// 2 => MORE_TEETH. The rotated encounter rosters (bead
    /// `sts2-rs-ybtmu`) thread `starter_idx` so each scroll opens on a
    /// distinct move. Before the fix the factory took no arg and every
    /// scroll defaulted to CHOMP (MONSTER_STATE divergence).
    #[test]
    fn scroll_of_biting_starter_idx_rotates_initial_state() {
        for (idx, expected) in [(0u8, "CHOMP"), (1, "CHEW"), (2, "MORE_TEETH")] {
            let ai = create_scroll_of_biting_ai(idx);
            assert_eq!(
                ai.current_state_id(),
                expected,
                "StarterMoveIdx({idx}) should open in {expected}"
            );
        }
        // C# uses `% 3`, so idx 3 wraps back to CHOMP (idx 0).
        assert_eq!(create_scroll_of_biting_ai(3).current_state_id(), "CHOMP");
    }

    fn run_three_moves(ai: &mut MonsterAi) -> Vec<&'static str> {
        let mut out = Vec::new();
        out.push(ai.current_move().id);
        for seed in 0..2 {
            ai.on_move_performed();
            out.push(ai.roll_move_seeded(seed).id);
        }
        out
    }

    #[test]
    fn noisebot_noise_adds_one_dazed_to_discard_and_draw() {
        // `Noisebot.NoiseMove` = `StatusIntent(2)` → 1 Dazed into Discard
        // + 1 Dazed into Draw. Bead `sts2-rs-bm1u5`.
        use sts2_core::cards::CardId;
        let mut state = CombatState::default();
        let pre_discard = state.player.discard.len();
        let pre_draw = state.player.draw.len();
        let pre_shuffle = state.run_rng.shuffle.counter();

        noisebot_noise_effect(&mut state, CreatureUid::new(1).expect("nz"));

        assert_eq!(
            state.player.discard.len(),
            pre_discard + 1,
            "1 Dazed added to discard"
        );
        assert_eq!(
            state.player.draw.len(),
            pre_draw + 1,
            "1 Dazed added to draw"
        );
        // gh #181: the draw-pile Dazed is a `CardPilePosition.Random` insert
        // (`Noisebot.cs:64`), so it draws exactly ONE read off the run-level
        // `shuffle` stream; the discard Dazed (default position) draws none.
        assert_eq!(
            state.run_rng.shuffle.counter(),
            pre_shuffle + 1,
            "exactly one shuffle read (random-position draw insert only)"
        );
        for &uid in state
            .player
            .discard
            .iter()
            .skip(pre_discard)
            .chain(state.player.draw.iter())
            .take(2)
        {
            assert_eq!(
                state.cards[uid.0.get() as usize - 1].card_id,
                CardId::Dazed,
                "spawned cards are Dazed"
            );
        }
    }

    #[test]
    fn mecha_knight_flamethrower_adds_four_burn_to_hand() {
        // `MechaKnight.FlamethrowerMove` = `StatusIntent(4)` → 4 Burn to
        // the player's hand. Bead `sts2-rs-6uunc`.
        use sts2_core::cards::CardId;
        let mut state = CombatState::default();
        let pre = state.player.hand.len();

        mecha_knight_flamethrower_effect(&mut state, CreatureUid::new(1).expect("nz"));

        assert_eq!(
            state.player.hand.len(),
            pre + 4,
            "FLAMETHROWER adds 4 Burn to hand"
        );
        for &uid in state.player.hand.iter().skip(pre) {
            assert_eq!(
                state.cards[uid.0.get() as usize - 1].card_id,
                CardId::Burn,
                "spawned cards are Burn"
            );
        }
    }

    /// Bead `sts2-rs-jnf4a` slice: pin the phase-3 state-machine
    /// adjacency shape (unreachable until DeadState/REVIVE_BRANCH is
    /// wired). Mirrors C# `TestSubject.cs:225-229`:
    /// * MULTI_CLAW self-loop.
    /// * LACERATE → POUNCE → GROWL → LACERATE chain.
    /// Also verifies BURNING_GROWL's intent payload and effect_fn
    /// (Strength 2 + 3 Burn cards into hand).
    #[test]
    fn test_subject_phase3_state_machine_scaffold_shape() {
        use sts2_core::cards::CardId;
        use sts2_core::monster_ai::MonsterState;
        let ai = create_test_subject_ai();

        // Adjacency from C# FollowUpState wiring.
        for (id, expected_follow) in [
            ("MULTI_CLAW", "MULTI_CLAW"),
            ("PHASE3_LACERATE", "BIG_POUNCE"),
            ("BIG_POUNCE", "BURNING_GROWL"),
            ("BURNING_GROWL", "PHASE3_LACERATE"),
        ] {
            let st = ai.states.get(id).expect("phase-3 state present");
            let MonsterState::Move(mv) = st else {
                panic!("{id} must be Move");
            };
            assert_eq!(
                mv.follow_up,
                Some(expected_follow),
                "{id} follow_up mismatch"
            );
        }

        // Intent shapes (non-ascension).
        let multi_claw = ai.states.get("MULTI_CLAW").expect("present");
        if let MonsterState::Move(mv) = multi_claw {
            assert_eq!(
                mv.intents.as_slice(),
                &[Intent::MultiAttack {
                    damage: 10,
                    hits: 3
                }][..]
            );
        }
        let lacerate = ai.states.get("PHASE3_LACERATE").expect("present");
        if let MonsterState::Move(mv) = lacerate {
            assert_eq!(
                mv.intents.as_slice(),
                &[Intent::MultiAttack {
                    damage: 10,
                    hits: 3
                }][..]
            );
        }
        let pounce = ai.states.get("BIG_POUNCE").expect("present");
        if let MonsterState::Move(mv) = pounce {
            assert_eq!(mv.intents.as_slice(), &[Intent::Attack { damage: 45 }][..]);
        }
        let growl = ai.states.get("BURNING_GROWL").expect("present");
        if let MonsterState::Move(mv) = growl {
            assert_eq!(
                mv.intents.as_slice(),
                &[
                    Intent::StatusCard,
                    Intent::Buff {
                        kind: PowerKind::Strength,
                        amount: 2,
                    },
                ][..]
            );
        }

        // BURNING_GROWL effect: +2 Strength to self, 3 Burn into hand.
        let mut combat = combat_with_monster(sts2_core::MonsterKind::TestSubject, "TestSubject");
        let owner = sts2_core::CreatureUid::new(1).expect("nz");
        let pre_hand = combat.player.hand.len();
        test_subject_burning_growl_effect(&mut combat, owner);
        assert_eq!(self_power(&combat, PowerKind::Strength), 2);
        assert_eq!(combat.player.hand.len(), pre_hand + 3);
        for &uid in combat.player.hand.iter().skip(pre_hand) {
            assert_eq!(
                combat.cards[uid.0.get() as usize - 1].card_id,
                CardId::Burn,
                "spawned cards are Burn"
            );
        }
    }

    /// `test_subject_multi_claw_effect`: each performance increments the
    /// owner's PainfulStabs power counter (the Rust home of C#'s
    /// `ExtraMultiClawCount`, TestSubject.cs:310), and a missing
    /// PainfulStabs power (phase 1/3 states — unreachable for MULTI_CLAW,
    /// but the effect must not panic) is a no-op.
    #[test]
    fn test_subject_multi_claw_effect_increments_painful_stabs_counter() {
        use std::num::NonZeroU32;

        use sts2_core::{Power, PowerUid};

        let mut combat = combat_with_monster(sts2_core::MonsterKind::TestSubject, "TestSubject");
        let owner = sts2_core::CreatureUid::new(1).expect("nz");

        // No PainfulStabs yet → no-op, no panic.
        test_subject_multi_claw_effect(&mut combat, owner);
        assert!(combat.powers.iter().all(|p| p.counter == 0));

        // Attach PainfulStabs(1) as `adaptable_after_death` would.
        let pid = NonZeroU32::new(7).expect("nz");
        combat
            .powers
            .push(Power::new(pid, PowerKind::PainfulStabs, 1));
        combat.creatures[0].powers.push(PowerUid(pid));

        test_subject_multi_claw_effect(&mut combat, owner);
        test_subject_multi_claw_effect(&mut combat, owner);
        let ctr = combat
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::PainfulStabs)
            .map_or(-1, |p| p.counter);
        assert_eq!(ctr, 2, "one increment per MULTI_CLAW performance");
    }

    #[test]
    fn test_subject_phase1_bite_skull_bash_loop() {
        // Bead sts2-rs-jz9eo: C# TestSubject phase 1 is BITE ↔ SKULL_BASH
        // (initial BITE). The prior CHARGE_UP/DISCHARGE/OVERLOAD + fabricated
        // P2 chain was a Python-pin invention; respawn/phase-3 are deferred.
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{
            CombatState, Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, PowerUid,
        };

        let mut combat = CombatState::default();
        let owner_uid_raw = NonZeroU32::new(1).expect("nz");
        let test_subject_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        combat.creatures.push(Creature {
            id: owner_uid_raw,
            current_hp: 50, // below half (max 200)
            max_hp: 200,
            block: 0,
            powers: test_subject_powers,
            species: Some(MonsterKind::TestSubject),
            monster_id: MonsterId("TestSubject"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(create_test_subject_ai()),
        });
        let owner = CreatureUid(owner_uid_raw);

        let mut ai = combat.creatures[0].ai.take().expect("ai");
        ai.set_owner(owner);
        // C# TestSubject phase 1: BITE ↔ SKULL_BASH, initial BITE. Bead jz9eo.
        let mut trace = Vec::new();
        for turn in 0u64..4 {
            trace.push(ai.current_move().id);
            ai.on_move_performed();
            ai.roll_move_with_combat_seeded(turn, &combat);
        }
        assert_eq!(trace, vec!["BITE", "SKULL_BASH", "BITE", "SKULL_BASH"]);
    }

    #[test]
    fn frog_knight_charge_gate_fires_once_below_half_hp() {
        // Bead sts2-rs-k27e: CHARGE_CHECK fires BEETLE_CHARGE exactly
        // once when (counter == 0 && current_hp < max_hp / 2). After
        // the BEETLE_CHARGE move performs, counter flips to 1 and the
        // predicate must degrade to false, returning the rotation to
        // TONGUE_LASH.
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{
            CombatState, Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, PowerUid,
        };

        let mut combat = CombatState::default();
        let owner_uid_raw = NonZeroU32::new(1).expect("nz");
        let mut frog_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        frog_powers.clear();
        combat.creatures.push(Creature {
            id: owner_uid_raw,
            current_hp: 80, // below max/2 = 95
            max_hp: 191,
            block: 0,
            powers: frog_powers,
            species: Some(MonsterKind::FrogKnight),
            monster_id: MonsterId("FrogKnight"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(create_frog_knight_ai()),
        });
        let owner = CreatureUid(owner_uid_raw);

        // Walk the rotation: TONGUE_LASH (initial) → STRIKE_DOWN_EVIL →
        // FOR_THE_QUEEN → CHARGE_CHECK → BEETLE_CHARGE (counter==0,
        // hp<max/2 satisfied). Initial state is TONGUE_LASH per C#
        // `FrogKnight.cs:85` (`moveState3`); bead `sts2-rs-ybtmu`
        // reverted the `sts2-rs-8v6he` FOR_THE_QUEEN regression.
        let mut ai = combat.creatures[0].ai.take().expect("ai");
        ai.set_owner(owner);
        assert_eq!(ai.current_move().id, "TONGUE_LASH");
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_with_combat_seeded(0, &combat).id,
            "STRIKE_DOWN_EVIL"
        );
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_with_combat_seeded(0, &combat).id,
            "FOR_THE_QUEEN"
        );
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_with_combat_seeded(0, &combat).id,
            "BEETLE_CHARGE",
            "CHARGE_CHECK must fire BEETLE_CHARGE when counter==0 and hp<max/2"
        );

        // Manually flip the counter to simulate the BEETLE_CHARGE
        // effect_fn firing (the combat-loop dispatcher invokes it
        // after the Attack intent — covered by perform_intent tests).
        ai.set_counter(1);
        ai.on_move_performed();
        // Next rotation: BEETLE_CHARGE → TONGUE_LASH → STRIKE_DOWN_EVIL
        // → FOR_THE_QUEEN → CHARGE_CHECK; this time CounterAware
        // returns false (counter==1), so we fall through to TONGUE_LASH.
        for expected in ["TONGUE_LASH", "STRIKE_DOWN_EVIL", "FOR_THE_QUEEN"] {
            assert_eq!(ai.roll_move_with_combat_seeded(0, &combat).id, expected);
            ai.on_move_performed();
        }
        assert_eq!(
            ai.roll_move_with_combat_seeded(0, &combat).id,
            "TONGUE_LASH",
            "CHARGE_CHECK must skip BEETLE_CHARGE after counter flips to 1"
        );
    }

    #[test]
    fn frog_knight_charge_gate_skips_above_half_hp() {
        // Counter is 0 but HP is above max/2 → predicate returns false,
        // CHARGE_CHECK falls through to TONGUE_LASH.
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{
            CombatState, Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, PowerUid,
        };

        let mut combat = CombatState::default();
        let owner_uid_raw = NonZeroU32::new(1).expect("nz");
        let frog_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        combat.creatures.push(Creature {
            id: owner_uid_raw,
            current_hp: 191, // full HP
            max_hp: 191,
            block: 0,
            powers: frog_powers,
            species: Some(MonsterKind::FrogKnight),
            monster_id: MonsterId("FrogKnight"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(create_frog_knight_ai()),
        });

        let mut ai = combat.creatures[0].ai.take().expect("ai");
        ai.set_owner(CreatureUid(owner_uid_raw));
        // Initial state is TONGUE_LASH per C# `FrogKnight.cs:85`
        // (`moveState3`); bead `sts2-rs-ybtmu` reverted the
        // `sts2-rs-8v6he` FOR_THE_QUEEN regression. Walk to CHARGE_CHECK.
        assert_eq!(ai.current_move().id, "TONGUE_LASH");
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_with_combat_seeded(0, &combat).id,
            "STRIKE_DOWN_EVIL"
        );
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_with_combat_seeded(0, &combat).id,
            "FOR_THE_QUEEN"
        );
        ai.on_move_performed();
        let _ = ai.roll_move_with_combat_seeded(0, &combat);
        assert_eq!(
            ai.current_move().id,
            "TONGUE_LASH",
            "CHARGE_CHECK must skip BEETLE_CHARGE when hp >= max/2"
        );
    }

    #[test]
    fn devoted_sculptor_then_savage_self_loop() {
        let mut ai = create_devoted_sculptor_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["FORBIDDEN_INCANTATION", "SAVAGE", "SAVAGE"]
        );
    }

    #[test]
    fn turret_operator_unload_unload_reload_loop() {
        // `TurretOperator.cs:35-46`: UNLOAD_MOVE_1 → UNLOAD_MOVE_2 →
        // RELOAD_MOVE → UNLOAD_MOVE_1 (loop). Initial = UNLOAD_MOVE_1.
        let mut ai = create_turret_operator_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["UNLOAD_MOVE_1", "UNLOAD_MOVE_2", "RELOAD_MOVE"]
        );
        // Fourth move loops back to UNLOAD_MOVE_1.
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(2).id, "UNLOAD_MOVE_1");
    }

    #[test]
    fn axebot_boot_up_flag() {
        assert_eq!(
            create_axebot_ai(false).current_move().id,
            "HAMMER_UPPERCUT",
            "start_with_boot_up=false opens on HAMMER_UPPERCUT (v0.107.1)"
        );
        assert_eq!(create_axebot_ai(true).current_move().id, "BOOT_UP");
    }

    #[test]
    fn fabricator_emits_summon_intent_on_fabricate() {
        // Fabricator entry point is FAB_BRANCH → FAB_RAND → {FABRICATE,
        // FABRICATING_STRIKE}. Both moves carry `Intent::Summon` per
        // Python `IntentType.SUMMON` (bead `sts2-rs-r9pl`; variant
        // landed by `sts2-rs-47p5`). Walk the AI for several seeds
        // and confirm every resolved move is one of the expected
        // SUMMON-bearing leaves.
        let combat = CombatState::default();
        for seed in 0..40 {
            let mut ai = create_fabricator_ai();
            let m = ai.roll_move_with_combat_seeded(seed, &combat);
            assert!(
                matches!(m.id, "FABRICATE" | "FABRICATING_STRIKE"),
                "expected SUMMON leaf, got {} (seed {seed})",
                m.id,
            );
            assert!(
                m.intents.contains(&Intent::Summon),
                "{} should carry Intent::Summon",
                m.id,
            );
        }
    }

    #[test]
    fn frog_knight_charge_check_collapses_to_tongue_lash() {
        let mut ai = create_frog_knight_ai();
        let combat = CombatState::default();
        // Initial state TONGUE_LASH → STRIKE_DOWN_EVIL → FOR_THE_QUEEN →
        // CHARGE_CHECK → TONGUE_LASH (degraded always-true fallback; HP
        // unavailable here because the AI was constructed without an
        // owner-bound combat — the CounterAware predicate returns false).
        // Bead `sts2-rs-ybtmu` reverted the `sts2-rs-8v6he` FOR_THE_QUEEN
        // regression; initial state is TONGUE_LASH per C# `FrogKnight.cs:85`.
        assert_eq!(ai.current_move().id, "TONGUE_LASH");
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_with_combat_seeded(0, &combat).id,
            "STRIKE_DOWN_EVIL"
        );
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_with_combat_seeded(0, &combat).id,
            "FOR_THE_QUEEN"
        );
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_with_combat_seeded(0, &combat).id,
            "TONGUE_LASH",
            "CHARGE_CHECK should collapse to TONGUE_LASH"
        );
    }

    #[test]
    fn globe_head_three_chain() {
        let mut ai = create_globe_head_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["SHOCKING_SLAP", "THUNDER_STRIKE", "GALVANIC_BURST"]
        );
    }

    #[test]
    fn owl_magistrate_four_move_cycle() {
        // C# v0.103.2 cycle: SCRUTINY -> PECK_ASSAULT -> JUDICIAL_FLIGHT -> VERDICT -> ...
        // Bead `sts2-rs-yxvi`.
        let mut ai = create_owl_magistrate_ai();
        let mut got = Vec::new();
        for _ in 0..5 {
            got.push(ai.roll_move_seeded(0).id);
            ai.on_move_performed();
        }
        assert_eq!(
            got,
            vec![
                "MAGISTRATE_SCRUTINY",
                "PECK_ASSAULT",
                "JUDICIAL_FLIGHT",
                "VERDICT",
                "MAGISTRATE_SCRUTINY",
            ]
        );
    }

    #[test]
    fn slimed_berserker_four_move_chain_loops() {
        // Bead sts2-rs-6v3s: chain is
        // VOMIT_ICHOR → FURIOUS_PUMMELING → LEECHING_HUG → SMOTHER → VOMIT
        // per C# `SlimedBerserker.cs::GenerateMoveStateMachine`.
        let mut ai = create_slimed_berserker_ai();
        let expected = [
            "VOMIT_ICHOR_MOVE",
            "FURIOUS_PUMMELING_MOVE",
            "LEECHING_HUG_MOVE",
            "SMOTHER_MOVE",
        ];
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(ai.current_move().id, *want, "step {i}");
            ai.on_move_performed();
            ai.roll_move_seeded(0);
        }
        // After SMOTHER, the chain loops back to VOMIT_ICHOR.
        assert_eq!(ai.current_move().id, "VOMIT_ICHOR_MOVE");
    }

    #[test]
    fn the_lost_alternates() {
        // C# TheLost.cs: DEBILITATING_SMOG (init) ↔ EYE_LASERS. Prior
        // DARK_SLASH/SHADOW_STRIKE Rust moveset was fabricated. Bead
        // sts2-rs-kh6d0.
        let mut ai = create_the_lost_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["DEBILITATING_SMOG", "EYE_LASERS", "DEBILITATING_SMOG"]
        );
    }

    #[test]
    fn the_forgotten_miasma_dread_loop() {
        // C# TheForgotten.cs: MIASMA (init) ↔ DREAD. Prior HAUNT/WAIL_OF_SORROW
        // was fabricated. Bead 96y80.
        let mut ai = create_the_forgotten_ai();
        assert_eq!(run_three_moves(&mut ai), vec!["MIASMA", "DREAD", "MIASMA"]);
    }

    #[test]
    fn the_forgotten_miasma_steals_dex_blocks_and_ramps_dread() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 106,
            max_hp: 106,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::TheForgotten),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("TheForgotten"),
            ai: Some(create_the_forgotten_ai()),
        });
        state.enemy_uids.push(owner);

        let dread_dmg = |state: &CombatState| -> i32 {
            state.creatures[0]
                .ai
                .as_ref()
                .and_then(|ai| match ai.states.get("DREAD") {
                    Some(MonsterState::Move(m)) => m.intents.iter().find_map(|i| match i {
                        Intent::Attack { damage } => Some(*damage),
                        _ => None,
                    }),
                    _ => None,
                })
                .expect("DREAD attack intent")
        };
        let player_dex = |state: &CombatState| -> i32 {
            state
                .player
                .powers
                .iter()
                .find_map(|pu| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == pu.0 && p.kind == PowerKind::Dexterity)
                        .map(|p| p.amount)
                })
                .unwrap_or(0)
        };

        assert_eq!(dread_dmg(&state), 13, "base DreadDamage is 13 (+0 Dex)");
        the_forgotten_miasma_effect(&mut state, owner);
        assert_eq!(
            player_dex(&state),
            -2,
            "MIASMA steals 2 Dexterity from player"
        );
        assert_eq!(state.creatures[0].block, 8, "MIASMA gains 8 block");
        assert_eq!(
            dread_dmg(&state),
            15,
            "DREAD ramps to 13 + self Dex(2) = 15"
        );
        // Second MIASMA: self Dex 2->4, DREAD -> 17.
        state.creatures[0].block = 0;
        the_forgotten_miasma_effect(&mut state, owner);
        assert_eq!(
            dread_dmg(&state),
            17,
            "DREAD ramps to 13 + self Dex(4) = 17"
        );
    }

    #[test]
    fn flail_knight_war_chant_never_consecutive() {
        let mut ai = create_flail_knight_ai();
        let mut last = "";
        let mut wc_streak = 0u32;
        for seed in 0..200 {
            let id = ai.roll_move_seeded(seed).id;
            if id == "WAR_CHANT" && id == last {
                wc_streak += 1;
                assert!(
                    wc_streak == 0,
                    "WAR_CHANT repeated at seed {seed} (CannotRepeat broken)"
                );
            }
            last = id;
            ai.on_move_performed();
        }
    }

    #[test]
    fn mysterious_knight_starts_on_ram() {
        // Same state machine as FlailKnight; both share `RAM` initial.
        assert_eq!(create_mysterious_knight_ai().current_move().id, "RAM");
        assert_eq!(create_flail_knight_ai().current_move().id, "RAM");
    }

    #[test]
    fn living_shield_branch_swaps_on_ally_count() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        // SHIELD_SLAM_BRANCH(allies>0 → SHIELD_SLAM ; else → SMASH).
        // Walk one full cycle in each combat state.
        let mut combat = CombatState::default();
        let u1 = CreatureUid::new(1).expect("non-zero");
        let u2 = CreatureUid::new(2).expect("non-zero");
        // `living_shield_alive_enemy_count` filters `enemy_uids` to creatures
        // with `current_hp > 0` (C# `GetAllyCount` counts only IsAlive
        // teammates), so the test must back each uid with a live Creature.
        let live = |id: CreatureUid| Creature {
            id: id.0,
            current_hp: 10,
            max_hp: 10,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::LivingShield),
            monster_id: MonsterId("LivingShield"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        };
        // Allies present: one Living Shield + one extra (alive) enemy.
        combat.enemy_uids.push(u1);
        combat.enemy_uids.push(u2);
        combat.creatures.push(live(u1));
        combat.creatures.push(live(u2));
        let mut ai = create_living_shield_ai();
        assert_eq!(ai.current_move().id, "SHIELD_SLAM");
        ai.on_move_performed();
        let nxt = ai.roll_move_with_combat_seeded(0, &combat).id;
        assert_eq!(
            nxt, "SHIELD_SLAM",
            "with allies present the branch returns to SHIELD_SLAM"
        );

        // No allies: only the Living Shield itself alive.
        combat.enemy_uids.clear();
        combat.enemy_uids.push(u1);
        let mut ai2 = create_living_shield_ai();
        ai2.on_move_performed();
        let nxt2 = ai2.roll_move_with_combat_seeded(0, &combat).id;
        assert_eq!(
            nxt2, "SMASH",
            "with no allies, the branch should fall through to SMASH"
        );
    }

    #[test]
    fn magi_knight_initial_is_first_power_shield() {
        // C# MagiKnight starts at FIRST_POWER_SHIELD: Attack 6 + GainBlock 5.
        let ai = create_magi_knight_ai();
        let mv = ai.current_move();
        assert_eq!(mv.id, "FIRST_POWER_SHIELD");
        assert_eq!(
            mv.intents.as_slice(),
            &[Intent::Attack { damage: 6 }, Intent::Defend { block: 5 }],
        );
    }

    #[test]
    fn fabricator_summon_effects_spawn_minion_bots() {
        use sts2_core::MonsterKind;
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();

        // FABRICATE: one defensive bot + one aggro bot, each a Minion.
        fabricator_fabricate_effect(&mut state, owner);
        assert_eq!(state.creatures.len(), 2, "FABRICATE spawns two bots");
        let kinds: Vec<MonsterKind> = state.creatures.iter().filter_map(|c| c.species).collect();
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, MonsterKind::Guardbot | MonsterKind::Noisebot)),
            "FABRICATE spawns a defensive bot, got {kinds:?}"
        );
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, MonsterKind::Zapbot | MonsterKind::Stabbot)),
            "FABRICATE spawns an aggro bot, got {kinds:?}"
        );
        let all_minions = state.creatures.iter().all(|c| {
            c.powers.iter().any(|pu| {
                state
                    .powers
                    .iter()
                    .any(|p| p.id == pu.0 && p.kind == PowerKind::Minion && p.amount == 1)
            })
        });
        assert!(all_minions, "each spawned bot carries Minion(1)");

        // FABRICATING_STRIKE: one more aggro bot.
        fabricator_fabricating_strike_effect(&mut state, owner);
        assert_eq!(
            state.creatures.len(),
            3,
            "FABRICATING_STRIKE spawns one aggro bot"
        );
    }

    #[test]
    fn fabricator_lastspawned_excludes_consecutive_same_pool() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        // A Fabricator WITH its AI attached tracks _lastSpawned in the AI
        // counter; two consecutive aggro spawns must alternate (the second
        // excludes the first), matching C# Fabricator.SpawnBot. Bead 9vs6y.
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 150,
            max_hp: 150,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::Fabricator),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Fabricator"),
            ai: Some(create_fabricator_ai()),
        });
        state.enemy_uids.push(owner);
        // The Fabricator sits in slot 2 of
        // `[bot1, bot2, fabricator, bot3, bot4]`; combat entry stamps that, and
        // the slot model needs it to place bots either side (GH #552).
        state.assign_encounter_slots(&[2]);

        let aggro = |k: MonsterKind| matches!(k, MonsterKind::Zapbot | MonsterKind::Stabbot);
        fabricator_fabricating_strike_effect(&mut state, owner);
        fabricator_fabricating_strike_effect(&mut state, owner);
        let spawned: Vec<MonsterKind> = state
            .creatures
            .iter()
            .filter_map(|c| c.species)
            .filter(|&k| aggro(k))
            .collect();
        assert_eq!(spawned.len(), 2, "two aggro bots spawned");
        assert_ne!(
            spawned[0], spawned[1],
            "_lastSpawned exclusion alternates consecutive aggro bots, got {spawned:?}"
        );
    }

    #[test]
    fn fabricator_refills_freed_lower_slot_ahead_of_surviving_bot() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        // Regression for bead `sts2-rs-07sfm` (Regent fuzz seed 162): when a
        // bot in a LOW slot dies, C# `GetNextSlot` (FirstOrDefault free slot)
        // reuses that freed slot, and `SortEnemiesBySlotName` keeps the new
        // bot AHEAD of a surviving higher-slot bot. The old live-count
        // front-insert appended it instead.
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 150,
            max_hp: 150,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::Fabricator),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Fabricator"),
            ai: Some(create_fabricator_ai()),
        });
        state.enemy_uids.push(owner);
        // The Fabricator sits in slot 2 of
        // `[bot1, bot2, fabricator, bot3, bot4]`; combat entry stamps that, and
        // the slot model needs it to place bots either side (GH #552).
        state.assign_encounter_slots(&[2]);

        // FabricateMove: one defensive bot (slot 0) + one aggro bot (slot 1),
        // both ahead of the Fabricator (slot 2). Living order:
        // [bot@slot0, bot@slot1, FABRICATOR].
        fabricator_fabricate_effect(&mut state, owner);
        let living: Vec<CreatureUid> = state
            .enemy_uids
            .iter()
            .copied()
            .filter(|u| {
                state
                    .creatures
                    .iter()
                    .any(|c| c.id == u.0 && c.current_hp > 0)
            })
            .collect();
        assert_eq!(living.len(), 3, "two bots + Fabricator alive");
        let (bot0, bot1) = (living[0], living[1]);
        assert_eq!(state.enemy_slot(bot0), Some(0));
        assert_eq!(state.enemy_slot(bot1), Some(1));

        // Kill the bot occupying the LOWER slot (slot 0).
        let idx0 = state
            .creatures
            .iter()
            .position(|c| c.id == bot0.0)
            .expect("bot0 present");
        state.creatures[idx0].current_hp = 0;

        // Spawn another bot; the freed slot 0 must be reused.
        let before: std::collections::HashSet<u32> =
            state.creatures.iter().map(|c| c.id.get()).collect();
        fabricator_spawn_bot(&mut state, owner, &FABRICATOR_DEFENSE);
        let new_uid = state
            .creatures
            .iter()
            .find(|c| !before.contains(&c.id.get()))
            .map(|c| CreatureUid(c.id))
            .expect("new bot spawned");

        assert_eq!(
            state.enemy_slot(new_uid),
            Some(0),
            "refilled bot reuses the freed lower slot 0"
        );
        let living_after: Vec<CreatureUid> = state
            .enemy_uids
            .iter()
            .copied()
            .filter(|u| {
                state
                    .creatures
                    .iter()
                    .any(|c| c.id == u.0 && c.current_hp > 0)
            })
            .collect();
        let pos_new = living_after
            .iter()
            .position(|&u| u == new_uid)
            .expect("new bot in living order");
        let pos_bot1 = living_after
            .iter()
            .position(|&u| u == bot1)
            .expect("surviving bot in living order");
        assert!(
            pos_new < pos_bot1,
            "refilled slot-0 bot must sort AHEAD of the surviving slot-1 bot, got {living_after:?}"
        );
    }

    #[test]
    fn fabricator_can_fabricate_gate_routes_to_disintegrate_when_full() {
        use sts2_core::MonsterKind;
        let mut combat = CombatState::default();
        // 1 alive enemy (the Fabricator stand-in) → can fabricate.
        combat.spawn_enemy(MonsterKind::Fabricator);
        assert!(
            fabricator_can_fabricate(&combat),
            "with <5 alive enemies the boss fabricates"
        );
        // Fill to 5 alive enemies → ally cap reached → DISINTEGRATE.
        for _ in 0..4 {
            combat.spawn_enemy(MonsterKind::Zapbot);
        }
        assert_eq!(combat.enemy_uids.len(), 5);
        assert!(
            !fabricator_can_fabricate(&combat),
            "with 5 alive enemies the boss must DISINTEGRATE instead"
        );
    }

    #[test]
    fn spectral_knight_hex_then_soul_slash_then_rand() {
        // `SpectralKnight.cs:44-58`: initial HEX → SOUL_SLASH → RAND
        // {SOUL_SLASH w2 | SOUL_FLAME CannotRepeat}. Bead sts2-rs-29kfv.
        let mut ai = create_spectral_knight_ai();
        assert_eq!(ai.current_move().id, "HEX");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "SOUL_SLASH");
        ai.on_move_performed();
        // From RAND, both branches are valid SOUL_SLASH/SOUL_FLAME moves.
        let third = ai.roll_move_seeded(1).id;
        assert!(
            third == "SOUL_SLASH" || third == "SOUL_FLAME",
            "RAND must roll SOUL_SLASH or SOUL_FLAME, got {third}"
        );
    }

    /// Bead `sts2-rs-dpov`: post-rebaseline MechaKnight runs
    /// CHARGE once then loops FLAMETHROWER → WINDUP → HEAVY_CLEAVE
    /// → FLAMETHROWER → … (HEAVY_CLEAVE follows back to
    /// FLAMETHROWER, NOT to CHARGE). Replaces the pre-rebaseline
    /// STEEL_SLASH/MISSILE_SALVO/SHIELD_UP/OVERDRIVE_STRIKE chain.
    #[test]
    fn mecha_knight_charge_then_flamethrower_loop() {
        let mut ai = create_mecha_knight_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["CHARGE", "FLAMETHROWER", "WINDUP"]
        );
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "HEAVY_CLEAVE");
        // HEAVY_CLEAVE loops back to FLAMETHROWER (not CHARGE).
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "FLAMETHROWER");
        // Continue the loop.
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "WINDUP");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "HEAVY_CLEAVE");
    }

    /// Bead `sts2-rs-dpov`: per-move intent payloads match
    /// C# decomp `MechaKnight.cs`.
    #[test]
    fn mecha_knight_intent_payloads_match_csharp() {
        let mut ai = create_mecha_knight_ai();
        assert_eq!(
            ai.current_move().intents.as_slice(),
            &[Intent::Attack { damage: 25 }]
        );
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_seeded(0).intents.as_slice(),
            &[Intent::StatusCard]
        );
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_seeded(0).intents.as_slice(),
            &[
                Intent::Defend { block: 15 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 5
                }
            ]
        );
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_seeded(0).intents.as_slice(),
            &[Intent::Attack { damage: 35 }]
        );
    }

    #[test]
    fn osty_self_loop() {
        let mut ai = create_osty_ai();
        for _ in 0..5 {
            assert_eq!(ai.current_move().id, "HAUNT");
            ai.on_move_performed();
            ai.roll_move_seeded(0);
        }
    }

    #[test]
    fn soul_nexus_initial_soul_burn_then_rand_cannot_repeat() {
        // Per C# decomp v0.103.2: initial move SOUL_BURN_MOVE, then
        // RAND{SOUL_BURN_MOVE | MAELSTROM_MOVE | DRAIN_LIFE_MOVE} with
        // CannotRepeat. After the initial SOUL_BURN, no branch repeats
        // on consecutive turns across a 300-seed sweep.
        let mut ai = create_soul_nexus_ai();
        assert_eq!(ai.current_move().id, "SOUL_BURN_MOVE");
        ai.on_move_performed();
        let mut last = "";
        for seed in 0..300 {
            let id = ai.roll_move_seeded(seed).id;
            assert_ne!(id, last, "CannotRepeat violation at seed {seed}");
            last = id;
            ai.on_move_performed();
        }
    }

    #[test]
    fn aeonglass_cycles_ebb_eye_lasers_increasing_intensity() {
        // C# v0.107.1 `Aeonglass.GenerateMoveStateMachine`: initial EBB_MOVE,
        // then EBB → EYE_LASERS → INCREASING_INTENSITY → EBB → … Bead
        // `sts2-rs-v0107-content-port-wb8lk.2`.
        let mut ai = create_aeonglass_ai();
        let mut got = Vec::new();
        for _ in 0..4 {
            got.push(ai.roll_move_seeded(0).id);
            ai.on_move_performed();
        }
        assert_eq!(
            got,
            vec![
                "EBB_MOVE",
                "EYE_LASERS_MOVE",
                "INCREASING_INTENSITY_MOVE",
                "EBB_MOVE",
            ]
        );
    }

    #[test]
    fn aeonglass_move_intents_match_csharp_values() {
        // A0 base values: Ebb SingleAttack(26) + Defend(33); EyeLasers
        // MultiAttack(11, 2); IncreasingIntensity StatusCard + Buff(Strength).
        let ai = create_aeonglass_ai();
        let intents = |id: &str| -> smallvec::SmallVec<[Intent; 4]> {
            match ai.states.get(id) {
                Some(MonsterState::Move(m)) => m.intents.clone(),
                _ => panic!("no move {id}"),
            }
        };
        assert_eq!(
            &intents("EBB_MOVE")[..],
            &[Intent::Attack { damage: 26 }, Intent::Defend { block: 33 }]
        );
        assert_eq!(
            &intents("EYE_LASERS_MOVE")[..],
            &[Intent::MultiAttack {
                damage: 11,
                hits: 2
            }]
        );
        assert!(matches!(
            intents("INCREASING_INTENSITY_MOVE")[0],
            Intent::StatusCard
        ));
    }

    #[test]
    fn aeonglass_ebb_gains_thirtythree_block_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::Aeonglass, "Aeonglass");
        aeonglass_ebb_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        let block = combat
            .creatures
            .iter()
            .find(|c| c.id.get() == 1)
            .map_or(0, |c| c.block);
        assert_eq!(block, 33, "Ebb grants 33 block to self");
    }

    #[test]
    fn aeonglass_increasing_intensity_grows_strength_and_wither_level() {
        use sts2_core::cards::{CardId, VarKey};
        use sts2_core::{CombatState, MonsterKind};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _player = state.init_player_creature();
        let owner = state.spawn_enemy(MonsterKind::Aeonglass);
        // WitherUpgradeCount lives on the boss's MonsterAi counter (C# Aeonglass
        // field), so the AI must be attached as it is in real combat.
        state.set_creature_ai(owner, create_aeonglass_ai());

        let self_strength = |state: &CombatState| -> i32 {
            state
                .creatures
                .iter()
                .find(|c| c.id == owner.0)
                .map_or(0, |c| {
                    c.powers
                        .iter()
                        .filter_map(|p| state.powers.get_by_id(p.0))
                        .filter(|p| p.kind == PowerKind::Strength)
                        .map(|p| p.amount)
                        .sum()
                })
        };
        let wither_levels = |state: &CombatState| -> Vec<i32> {
            state
                .cards
                .iter()
                .filter(|c| c.card_id == CardId::Wither)
                .map(|c| c.combat_var(VarKey::WitherFakeUpgradeLevel))
                .collect()
        };

        // 1st INCREASING_INTENSITY: +Strength 3 (base 3 + AdditionalStrength 0);
        // 1 Wither (A0) added at FakeUpgrade level 1.
        aeonglass_increasing_intensity_effect(&mut state, owner);
        assert_eq!(self_strength(&state), 3);
        assert_eq!(wither_levels(&state), vec![1]);

        // 2nd INCREASING_INTENSITY: +Strength 4 (base 3 + AdditionalStrength 1)
        // ⇒ total 7. Existing Wither bumped to level 2; new Wither at level 2.
        aeonglass_increasing_intensity_effect(&mut state, owner);
        assert_eq!(self_strength(&state), 7);
        let mut levels = wither_levels(&state);
        levels.sort_unstable();
        assert_eq!(levels, vec![2, 2], "both Withers synced to upgrade count 2");
    }

    #[test]
    fn queen_opens_with_puppet_strings_then_youre_mine_then_burn_bright() {
        // Counter==0 (HasAmalgamDied=false), so YOURE_MINE_NOW_BRANCH
        // picks BURN_BRIGHT_FOR_ME. C# Queen.cs:124-141 initial state.
        let mut ai = create_queen_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec![
                "PUPPET_STRINGS_MOVE",
                "YOURE_MINE_MOVE",
                "BURN_BRIGHT_FOR_ME_MOVE",
            ]
        );
    }

    /// Bead `sts2-rs-igdhp` (f933s sub-3): the PUPPET_STRINGS_MOVE
    /// effect_fn applies 3 stacks of `ChainsOfBinding` to the player
    /// per C# `Queen.cs:156-161 PuppetStringsMove`. Verifies the
    /// power record lands at the legacy player.powers surface with
    /// amount=3.
    #[test]
    fn igdhp_queen_puppet_strings_applies_chains_of_binding_three_to_player() {
        let mut state = CombatState::default();
        let owner = CreatureUid::new(1).expect("nz");
        super::queen_puppet_strings_effect(&mut state, owner);
        let amt = state
            .player
            .powers
            .iter()
            .filter_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::ChainsOfBinding)
                    .map(|p| p.amount)
            })
            .sum::<i32>();
        assert_eq!(
            amt, 3,
            "PUPPET_STRINGS grants player ChainsOfBinding(3) — C# Queen.cs:156-161",
        );
    }

    /// Bead `sts2-rs-6g4v4` (m70xg.3): the Queen's `AfterDeath` listener
    /// flips her per-instance `HasAmalgamDied` counter when a
    /// `TorchHeadAmalgam` enters `newly_dead_this_turn`, and — if her
    /// resolved next move is `BURN_BRIGHT_FOR_ME_MOVE` — short-circuits
    /// the graph to `ENRAGE_MOVE` via `force_next_state`. Mirrors C#
    /// `Queen.cs:213-225 AfterDeath` (`SetMoveImmediate(EnragedState)`).
    #[test]
    fn queen_after_death_flips_counter_and_forces_enrage_when_in_burn_bright() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, PowerUid};

        let mut combat = CombatState::default();
        let queen_uid = CreatureUid(NonZeroU32::new(1).expect("nz"));
        let amalgam_uid = CreatureUid(NonZeroU32::new(2).expect("nz"));

        let mut queen_ai = create_queen_ai();
        // Walk the AI to BURN_BRIGHT_FOR_ME_MOVE so the listener's
        // `SetMoveImmediate` branch fires.
        assert_eq!(
            run_three_moves(&mut queen_ai),
            vec![
                "PUPPET_STRINGS_MOVE",
                "YOURE_MINE_MOVE",
                "BURN_BRIGHT_FOR_ME_MOVE",
            ]
        );
        assert_eq!(queen_ai.current_state_id(), "BURN_BRIGHT_FOR_ME_MOVE");
        assert_eq!(queen_ai.counter(), 0);

        combat.creatures.push(Creature {
            id: queen_uid.0,
            current_hp: 200,
            max_hp: 200,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: Some(sts2_core::MonsterKind::Queen),
            monster_id: MonsterId("Queen"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(queen_ai),
        });
        combat.creatures.push(Creature {
            id: amalgam_uid.0,
            current_hp: 0,
            max_hp: 199,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: Some(sts2_core::MonsterKind::TorchHeadAmalgam),
            monster_id: MonsterId("TorchHeadAmalgam"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        combat.enemy_uids.push(queen_uid);
        combat.enemy_uids.push(amalgam_uid);

        super::queen_after_death(&mut combat, amalgam_uid, queen_uid);

        let queen = combat
            .creatures
            .iter()
            .find(|c| c.id == queen_uid.0)
            .expect("queen present");
        let ai = queen.ai.as_ref().expect("queen has ai");
        assert_eq!(ai.counter(), 1, "HasAmalgamDied counter flips to 1");
        assert_eq!(
            ai.forced_next_state(),
            Some("ENRAGE_MOVE"),
            "BURN_BRIGHT → ENRAGE short-circuit",
        );
    }

    /// Negative path for `sts2-rs-6g4v4`: the listener ignores deaths of
    /// non-Amalgam creatures (e.g. a sibling enemy or summoned ally).
    /// C# `Queen.cs:215` guards on `creature.Monster is TorchHeadAmalgam`.
    #[test]
    fn queen_after_death_ignores_non_amalgam_deaths() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, PowerUid};

        let mut combat = CombatState::default();
        let queen_uid = CreatureUid(NonZeroU32::new(1).expect("nz"));
        let other_uid = CreatureUid(NonZeroU32::new(2).expect("nz"));

        let queen_ai = create_queen_ai();
        combat.creatures.push(Creature {
            id: queen_uid.0,
            current_hp: 200,
            max_hp: 200,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: Some(sts2_core::MonsterKind::Queen),
            monster_id: MonsterId("Queen"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(queen_ai),
        });
        combat.creatures.push(Creature {
            id: other_uid.0,
            current_hp: 0,
            max_hp: 30,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: Some(sts2_core::MonsterKind::FrogKnight),
            monster_id: MonsterId("FrogKnight"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });

        super::queen_after_death(&mut combat, other_uid, queen_uid);

        let queen = combat
            .creatures
            .iter()
            .find(|c| c.id == queen_uid.0)
            .expect("queen present");
        let ai = queen.ai.as_ref().expect("queen has ai");
        assert_eq!(ai.counter(), 0, "counter unchanged for non-Amalgam death");
        assert_eq!(ai.forced_next_state(), None, "no forced override");
    }

    /// Per C# decomp v0.103.2: SOUL_BURN deals 29, MAELSTROM is a 6 ×4
    /// MultiAttack, DRAIN_LIFE deals 18 + strong-debuff telegraph.
    /// The Python-pin SOUL_DRAIN `[Attack 10, Heal]` slot from beads
    /// `sts2-rs-va5r` / `sts2-rs-yzak` was removed; SoulNexus has no
    /// self-heal in v0.103.2.
    #[test]
    fn soul_nexus_intents_match_c_sharp_payloads() {
        let ai = create_soul_nexus_ai();
        let cases = [
            ("SOUL_BURN_MOVE", &[Intent::Attack { damage: 29 }][..]),
            (
                "MAELSTROM_MOVE",
                &[Intent::MultiAttack { damage: 6, hits: 4 }][..],
            ),
            (
                "DRAIN_LIFE_MOVE",
                &[Intent::Attack { damage: 18 }, Intent::DebuffStrong][..],
            ),
        ];
        for (id, expected) in cases {
            let m = ai.states.get(id).expect("state present");
            match m {
                sts2_core::monster_ai::MonsterState::Move(mv) => {
                    assert_eq!(mv.intents.as_slice(), expected, "{id} intents");
                }
                _ => panic!("{id} must be a MoveState"),
            }
        }
    }

    // ────────────────────────────────────────────────────────────────────
    // Telegraph-only Buff/Debuff effect_fn tests (bead sts2-rs-pdsdl)
    // ────────────────────────────────────────────────────────────────────

    /// Builds a single-monster combat with the monster at uid=1 and
    /// returns it for self-buff assertions.
    fn combat_with_monster(kind: sts2_core::MonsterKind, id: &'static str) -> CombatState {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, PowerUid};

        let mut combat = CombatState::default();
        let owner_raw = NonZeroU32::new(1).expect("nz");
        combat.creatures.push(Creature {
            id: owner_raw,
            current_hp: 100,
            max_hp: 100,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: Some(kind),
            monster_id: MonsterId(id),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        combat.enemy_uids.push(CreatureUid(owner_raw));
        combat
    }

    fn self_power(state: &CombatState, kind: PowerKind) -> i32 {
        let owner_raw = std::num::NonZeroU32::new(1).expect("nz");
        let creature = state
            .creatures
            .iter()
            .find(|c| c.id == owner_raw)
            .expect("monster present");
        creature
            .powers
            .iter()
            .filter_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == kind)
            })
            .map(|p| p.amount)
            .sum()
    }

    fn player_power(state: &CombatState, kind: PowerKind) -> i32 {
        state
            .player
            .powers
            .iter()
            .filter_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == kind)
                    .map(|p| p.amount)
            })
            .sum()
    }

    /// Bead `sts2-rs-zo6fk`: `StockPower.AfterDeath` respawns a fresh Axebot
    /// at the dead bot's slot, with `StockAmount = dead - 1`, opening on
    /// BOOT_UP. Mirrors C# `StockPower.cs`. Here a single stocked Axebot
    /// (Stock 2) dies → one live replacement appears in its slot with
    /// Stock(1) and a telegraphed BOOT_UP (Defend 10) intent.
    fn stock_amount(state: &CombatState, uid: sts2_core::CreatureUid) -> i32 {
        state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .and_then(|c| {
                c.powers.iter().find_map(|puid| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == puid.0 && p.kind == PowerKind::Stock)
                        .map(|p| p.amount)
                })
            })
            .unwrap_or(0)
    }

    #[test]
    fn axebot_stock_death_respawns_fresh_bot_with_decremented_stock() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::{CreatureUid, MonsterKind};

        let mut combat = combat_with_monster(MonsterKind::Axebot, "Axebot");
        let dead = CreatureUid::new(1).expect("nz");
        // Stock(2) on the live bot (C# `AfterAddedToRoom` default), then kill.
        apply_power_to_creature(&mut combat, dead, PowerKind::Stock, 2);
        assert_eq!(stock_amount(&combat, dead), 2);
        if let Some(c) = combat.creatures.iter_mut().find(|c| c.id == dead.0) {
            c.current_hp = 0;
        }

        axebot_stock_after_death(&mut combat, dead);

        // The dead bot's Stock is consumed (zeroed by the per-death fire path
        // is the engine's job; the handler itself leaves the dead bot a
        // corpse) and a NEW live Axebot now exists.
        let live: Vec<_> = combat
            .enemy_uids
            .iter()
            .filter(|u| {
                combat.creatures.iter().any(|c| {
                    c.id == u.0 && c.current_hp > 0 && c.species == Some(MonsterKind::Axebot)
                })
            })
            .copied()
            .collect();
        assert_eq!(live.len(), 1, "exactly one live Axebot respawned");
        let new_uid = live[0];
        assert_ne!(
            new_uid.0, dead.0,
            "respawn is a NEW creature, not the corpse"
        );
        // Respawn carries Stock(dead-1) = Stock(1).
        assert_eq!(
            stock_amount(&combat, new_uid),
            1,
            "respawn's Stock is decremented (2 -> 1)"
        );
        // Respawn is full-HP (fresh roll in [70,78] — `…wb8lk.11` v0.107.1 HP).
        let new_c = combat
            .creatures
            .iter()
            .find(|c| c.id == new_uid.0)
            .expect("respawn present");
        assert!(
            (70..=78).contains(&new_c.current_hp) && new_c.current_hp == new_c.max_hp,
            "respawn is at full fresh HP in the Axebot range, got {}",
            new_c.current_hp
        );
        // Respawn opens on BOOT_UP (telegraphed Defend 10).
        assert_eq!(
            new_c.intent,
            Some(sts2_core::monsters::Intent::Defend { block: 10 }),
            "respawn telegraphs BOOT_UP's Defend(10)"
        );
        let ai = new_c.ai.as_ref().expect("respawn has an AI");
        assert_eq!(ai.current_move().id, "BOOT_UP", "respawn opens on BOOT_UP");
        // Respawn occupies the dead bot's slot index (0).
        assert_eq!(
            combat.enemy_uids.iter().position(|u| u.0 == new_uid.0),
            Some(0),
            "respawn lands at the dead bot's slot"
        );
    }

    /// Bead `sts2-rs-jwb4a`: `PossessStrengthPower.AfterDeath` returns the
    /// Strength TheLost stole (DEBILITATING_SMOG) to the player when it dies.
    /// Two smog casts steal 4 Strength (counter accumulates); on death the
    /// handler restores +4, netting the player's -4 to exactly 0 — which is
    /// then removed (`ShouldRemoveDueToAmount`). Mirrors C#
    /// `PossessStrengthPower.cs` `Data.stolenStrength` + `AfterDeath`. Repro:
    /// v2 fuzz seed 2042 (HOWL_FROM_BEYOND kills TheLost; Rust used to keep the
    /// player's -2 Strength).
    #[test]
    fn the_lost_possess_strength_restored_to_player_on_death() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::{CreatureUid, MonsterKind};

        let mut combat = combat_with_monster(MonsterKind::TheLost, "TheLost");
        let lost = CreatureUid::new(1).expect("nz");
        // On-spawn PossessStrength(1) marker (C# `AfterAddedToRoom`).
        apply_power_to_creature(&mut combat, lost, PowerKind::PossessStrength, 1);

        the_lost_debilitating_smog_effect(&mut combat, lost);
        the_lost_debilitating_smog_effect(&mut combat, lost);
        assert_eq!(
            player_power(&combat, PowerKind::Strength),
            -4,
            "two DEBILITATING_SMOG casts steal 4 Strength from the player"
        );
        assert_eq!(
            possess_stolen_total(&combat, lost, PowerKind::PossessStrength),
            4,
            "the steal accumulates in PossessStrength's counter"
        );

        if let Some(c) = combat.creatures.iter_mut().find(|c| c.id == lost.0) {
            c.current_hp = 0;
        }
        possess_strength_after_death(&mut combat, lost);

        assert_eq!(
            player_power(&combat, PowerKind::Strength),
            0,
            "death restores the stolen Strength (net 0)"
        );
        assert!(
            !combat.player.powers.iter().any(|u| combat
                .powers
                .iter()
                .any(|p| p.id == u.0 && p.kind == PowerKind::Strength)),
            "the net-zero Strength instance is removed (ShouldRemoveDueToAmount)"
        );
    }

    /// Enemy-turn-start death (e.g. a poison/DoT tick kills the bot during
    /// `enemy_turn_start`, before the `enemy_turn_exec` acting snapshot). C#
    /// `AfterCreatureAdded` does NOT roll the move when `CurrentSide == Enemy`,
    /// and `MonsterModel.SpawnedThisTurn` makes `Creature.TakeTurn` skip
    /// `PerformMove` for the turn it spawned. So the respawn must sit out the
    /// turn it spawned: `intent` stays `None` (excluded from that enemy turn's
    /// acting snapshot) even though the AI opens on BOOT_UP and will telegraph
    /// it at the next end-of-turn roll. Parity seed 5881: setting the intent
    /// here made Rust perform BOOT_UP a turn early (block cs=0 rs=10).
    #[test]
    fn axebot_stock_death_during_enemy_turn_leaves_intent_unset() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::{CreatureUid, MonsterKind, Side};

        let mut combat = combat_with_monster(MonsterKind::Axebot, "Axebot");
        // Simulate the poison death happening DURING the enemy turn.
        combat.side = Side::Enemy;
        let dead = CreatureUid::new(1).expect("nz");
        apply_power_to_creature(&mut combat, dead, PowerKind::Stock, 2);
        if let Some(c) = combat.creatures.iter_mut().find(|c| c.id == dead.0) {
            c.current_hp = 0;
        }

        axebot_stock_after_death(&mut combat, dead);

        let new_uid = combat
            .enemy_uids
            .iter()
            .find(|u| {
                combat.creatures.iter().any(|c| {
                    c.id == u.0 && c.current_hp > 0 && c.species == Some(MonsterKind::Axebot)
                })
            })
            .copied()
            .expect("a fresh Axebot respawned");
        let new_c = combat
            .creatures
            .iter()
            .find(|c| c.id == new_uid.0)
            .expect("respawn present");
        // Suppressed this turn: no intent, so `enemy_turn_exec`'s
        // `creature.intent`-filter excludes it (mirrors SpawnedThisTurn).
        assert_eq!(
            new_c.intent, None,
            "enemy-turn respawn sits out this turn — intent stays None"
        );
        // But the AI is wired and opens on BOOT_UP, so the end-of-turn re-roll
        // will telegraph it for the FOLLOWING enemy turn.
        let ai = new_c.ai.as_ref().expect("respawn has an AI");
        assert_eq!(
            ai.current_move().id,
            "BOOT_UP",
            "respawn's AI still opens on BOOT_UP for the next-turn roll"
        );
    }

    #[test]
    fn axebot_stock_zero_does_not_respawn() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::{CreatureUid, MonsterKind};

        // A bot whose Stock is already 0 must not respawn (C# guard
        // `base.Amount > 0`). We model amount 0 by never applying Stock.
        let mut combat = combat_with_monster(MonsterKind::Axebot, "Axebot");
        let dead = CreatureUid::new(1).expect("nz");
        // No Stock applied at all → amount 0.
        let _ = apply_power_to_creature; // keep import used
        if let Some(c) = combat.creatures.iter_mut().find(|c| c.id == dead.0) {
            c.current_hp = 0;
        }
        let before = combat.enemy_uids.len();
        axebot_stock_after_death(&mut combat, dead);
        assert_eq!(
            combat.enemy_uids.len(),
            before,
            "no respawn when the dead bot has no Stock"
        );
    }

    /// Push an Axebot with independent `current_hp` / `max_hp` (the shared
    /// `push_enemy` clamps `max_hp` to `current_hp.max(1)`, which can't model
    /// a corpse whose `max_hp` lands in the respawn HP range). Used by the
    /// `axebot_stock_after_death` taken-set tests below.
    fn push_axebot_hp(
        combat: &mut CombatState,
        id: u32,
        current_hp: i32,
        max_hp: i32,
    ) -> sts2_core::CreatureUid {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, PowerUid};

        let raw = NonZeroU32::new(id).expect("nz");
        combat.creatures.push(Creature {
            id: raw,
            current_hp,
            max_hp,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: Some(MonsterKind::Axebot),
            monster_id: MonsterId("Axebot"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        let uid = CreatureUid(raw);
        combat.enemy_uids.push(uid);
        uid
    }

    fn creature_max_hp(state: &CombatState, uid: sts2_core::CreatureUid) -> i32 {
        state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .map_or(0, |c| c.max_hp)
    }

    /// `axebot_stock_after_death` builds the unique-HP exclusion `taken` set
    /// from ONLY {alive same-side enemies} ∪ {the dying owner} (lines 397-398).
    /// Corpses from PRIOR deaths must NOT pollute the candidate range (bead
    /// `sts2-rs-etqsw`). Axebot HP rolls in `[40, 44]`.
    ///
    /// Fixture (CombatState seed 0, niche cursor at 0):
    ///   * owner  uid1: dying Axebot, `max_hp = 100` (out of the [40,44] range,
    ///     so its presence in `taken` never shrinks the candidate set).
    ///   * alive  uid2..5: max_hp 40, 41, 42, 43 (current_hp 50).
    ///   * corpse uid6: `current_hp = 0`, `max_hp = 44`, id ≠ owner.
    ///
    /// Correct logic keeps the four alive siblings (40,41,42,43) plus the
    /// owner (100); it EXCLUDES the corpse. The candidate range `[40,44]`
    /// minus `{40,41,42,43}` is the singleton `{44}`, so the respawn rolls
    /// exactly 44 (a 1-element set always yields its only member).
    ///
    /// Every mutation of lines 397-398 changes which siblings land in `taken`,
    /// re-opening the candidate set so the deterministic niche roll lands ≠ 44:
    ///   * `>` → `==` (line 398:34): keeps only `hp == 0` (the corpse) + owner →
    ///     `taken∩[40,44] = {44}` → remaining `{40,41,42,43}` (4 cands) → roll
    ///     index 2 → 42.
    ///   * `>` → `>=`: keeps the corpse too → `taken = {40..44}` (full) →
    ///     empty fallback `NextInt(40,44)` → 43.
    ///   * `>` → `<`: keeps nothing in range (no hp<0) → full range → roll 43.
    ///   * `||` → `&&`: keeps only `hp>0 && id==owner` (owner is dead) → empty
    ///     `taken` → full range → 43.
    ///   * `==` → `!=` (398:46): keeps everything except the dying owner →
    ///     corpse included → full `taken` → fallback 43.
    ///   * `==` → `!=` (397:62, the `find`): scrambles the uid→creature
    ///     mapping, changing the collected `max_hp`s away from `{40,41,42,43}`.
    /// In every case the respawn's `max_hp` is NOT 44, so `assert_eq!(.., 44)`
    /// fails under the mutant and passes clean.
    #[test]
    fn axebot_respawn_taken_set_excludes_corpses_and_keeps_alive_plus_owner() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::{CreatureUid, MonsterKind};

        let mut combat = combat_with_monster(MonsterKind::Axebot, "Axebot");
        let owner = CreatureUid::new(1).expect("nz");
        // Owner is the dying bot: out-of-range max_hp, Stock(2) so it respawns.
        // Apply Stock while still ALIVE (apply_power no-ops on 0-hp), THEN kill.
        if let Some(c) = combat.creatures.iter_mut().find(|c| c.id == owner.0) {
            c.max_hp = 100;
        }
        apply_power_to_creature(&mut combat, owner, PowerKind::Stock, 2);
        if let Some(c) = combat.creatures.iter_mut().find(|c| c.id == owner.0) {
            c.current_hp = 0;
        }
        // `…wb8lk.11`: v0.107.1 Axebot HP range is [70,78]. Eight ALIVE
        // siblings fill 70..77, and a CORPSE sits at 78.
        push_axebot_hp(&mut combat, 2, 50, 70);
        push_axebot_hp(&mut combat, 3, 50, 71);
        push_axebot_hp(&mut combat, 4, 50, 72);
        push_axebot_hp(&mut combat, 5, 50, 73);
        push_axebot_hp(&mut combat, 6, 50, 74);
        push_axebot_hp(&mut combat, 7, 50, 75);
        push_axebot_hp(&mut combat, 8, 50, 76);
        push_axebot_hp(&mut combat, 9, 50, 77);
        let corpse = push_axebot_hp(&mut combat, 10, 0, 78);
        assert_eq!(creature_max_hp(&combat, corpse), 78);

        let before: std::collections::HashSet<u32> =
            combat.creatures.iter().map(|c| c.id.get()).collect();
        axebot_stock_after_death(&mut combat, owner);
        let respawn = combat
            .creatures
            .iter()
            .find(|c| !before.contains(&c.id.get()))
            .map(|c| CreatureUid(c.id))
            .expect("a respawn was created");

        assert_eq!(
            creature_max_hp(&combat, respawn),
            78,
            "respawn rolls the only candidate (78) left after excluding the eight \
             alive siblings; corpse max_hp 78 must NOT enter `taken`"
        );
    }

    /// `axebot_stock_after_death` reads the DYING bot's own Stock amount via
    /// `find(|p| p.id == puid.0 && p.kind == PowerKind::Stock)` (line 365). The
    /// `&&` is load-bearing: with `||` the lookup matches the FIRST power whose
    /// id matches the puid OR whose kind is Stock, so a non-Stock power listed
    /// ahead of the real Stock record is mis-read.
    ///
    /// Fixture: apply Strength(7) FIRST, then Stock(2). The owner's power list
    /// is `[strength_puid, stock_puid]`; `state.powers` is `[Strength(7),
    /// Stock(2)]`. Correct reads Stock(2) → `new_stock = 1` → respawn Stock(1).
    /// Under `||`, the first puid (Strength) matches `id == strength_puid`
    /// immediately, returning amount 7 → `new_stock = 6` → respawn Stock(6).
    #[test]
    fn axebot_respawn_reads_stock_not_other_powers() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::{CreatureUid, MonsterKind};

        let mut combat = combat_with_monster(MonsterKind::Axebot, "Axebot");
        let owner = CreatureUid::new(1).expect("nz");
        // Strength FIRST so it precedes the Stock record in `state.powers`.
        apply_power_to_creature(&mut combat, owner, PowerKind::Strength, 7);
        apply_power_to_creature(&mut combat, owner, PowerKind::Stock, 2);
        if let Some(c) = combat.creatures.iter_mut().find(|c| c.id == owner.0) {
            c.current_hp = 0;
        }

        let before: std::collections::HashSet<u32> =
            combat.creatures.iter().map(|c| c.id.get()).collect();
        axebot_stock_after_death(&mut combat, owner);
        let respawn = combat
            .creatures
            .iter()
            .find(|c| {
                !before.contains(&c.id.get())
                    && c.current_hp > 0
                    && c.species == Some(MonsterKind::Axebot)
            })
            .map(|c| CreatureUid(c.id))
            .expect("a respawn was created");

        assert_eq!(
            stock_amount(&combat, respawn),
            1,
            "respawn Stock = dead Stock(2) - 1; reading Strength(7) would yield 6"
        );
    }

    /// When the dying bot's Stock decrements to 0, `axebot_stock_after_death`
    /// must DROP the respawn's auto-applied `StockPower` entirely (lines
    /// 459-467): C# `AfterAddedToRoom` applies `StockPower` only when
    /// `StockAmount > 0`. This exercises the removal block that four mutants
    /// touch:
    ///   * `new_stock > 0` → `>=` (459:22): keeps Stock(0) on the respawn.
    ///   * `p.id != pid` → `==` (465:42): retains ONLY the Stock record in
    ///     `state.powers`, leaving it instead of removing it.
    ///   * `c.id == new_uid.0` → `!=` (466:71): strips the puid from the WRONG
    ///     creature, so the respawn keeps a dangling Stock puid.
    ///   * `puid.0 != pid` → `==` (467:47): retains ONLY the Stock puid on the
    ///     respawn instead of dropping it.
    ///
    /// Assertions: the respawn's `powers` list is EMPTY (kills 459/466/467 —
    /// each leaves a Stock puid attached) and exactly ONE `Stock` power record
    /// survives in `state.powers` (only the dying owner's; 465 would leave a
    /// second, the un-removed respawn record).
    #[test]
    fn axebot_respawn_zero_stock_removes_power_record_and_uid() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::{CreatureUid, MonsterKind};

        let mut combat = combat_with_monster(MonsterKind::Axebot, "Axebot");
        let owner = CreatureUid::new(1).expect("nz");
        // Stock(1) → new_stock = 0 → respawn must carry NO StockPower.
        apply_power_to_creature(&mut combat, owner, PowerKind::Stock, 1);
        if let Some(c) = combat.creatures.iter_mut().find(|c| c.id == owner.0) {
            c.current_hp = 0;
        }

        let before: std::collections::HashSet<u32> =
            combat.creatures.iter().map(|c| c.id.get()).collect();
        axebot_stock_after_death(&mut combat, owner);
        let respawn = combat
            .creatures
            .iter()
            .find(|c| {
                !before.contains(&c.id.get())
                    && c.current_hp > 0
                    && c.species == Some(MonsterKind::Axebot)
            })
            .expect("a respawn was created");

        assert!(
            respawn.powers.is_empty(),
            "zero-stock respawn must hold NO power uids, got {:?}",
            respawn.powers
        );
        let stock_records: Vec<i32> = combat
            .powers
            .iter()
            .filter(|p| p.kind == PowerKind::Stock)
            .map(|p| p.amount)
            .collect();
        // Exactly ONE Stock record survives, and it is the DYING OWNER's
        // (amount 1) — NOT the respawn's auto-applied Stock(2). The 465:42
        // `retain(p.id != pid)` → `== pid` mutant inverts the retain so it
        // keeps ONLY the respawn's record (amount 2) and drops the owner's,
        // which still leaves a count of 1 — so the amount, not the count,
        // is the discriminator.
        assert_eq!(
            stock_records,
            vec![1],
            "only the dying owner's Stock(1) record survives; the respawn's \
             auto-applied Stock(2) record must be removed (not retained)"
        );
    }

    /// BOOT_UP is only ever performed by a RESPAWNED Axebot (Axebot.cs:95-99 —
    /// the move machine starts a fresh stock-2 bot at HAMMER_UPPERCUT and never
    /// loops back to BOOT_UP; only a `_stockOverrideAmount` bot opens on it). So
    /// `axebot_boot_up_effect`'s StockAmount ∈ {1, 0}: a 1st respawn (Stock(1))
    /// gains `3 * (2 - 1) = 3` Strength; a 2nd respawn (stock 0, which carries NO
    /// StockPower) gains `3 * (2 - 0) = 6`. An ABSENT StockPower here means stock
    /// 0, NOT the fresh-bot default 2 — defaulting to 2 wrongly gave the stock-0
    /// respawn 0 Strength (Ironclad AXEBOTS_NORMAL seed 224 `MONSTER_STATE`).
    #[test]
    fn axebot_boot_up_strength_scales_with_respawn_stock() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::{CreatureUid, MonsterKind, PowerKind};

        let strength = |combat: &CombatState, owner: CreatureUid| -> i32 {
            combat
                .creatures
                .iter()
                .find(|c| c.id == owner.0)
                .and_then(|c| {
                    c.powers.iter().find_map(|puid| {
                        combat
                            .powers
                            .iter()
                            .find(|p| p.id == puid.0 && p.kind == PowerKind::Strength)
                            .map(|p| p.amount)
                    })
                })
                .unwrap_or(0)
        };

        // 1st respawn: StockPower(1) present → 3 * (2 - 1) = 3 Strength.
        let mut combat = combat_with_monster(MonsterKind::Axebot, "Axebot");
        let owner = CreatureUid::new(1).expect("nz");
        apply_power_to_creature(&mut combat, owner, PowerKind::Stock, 1);
        axebot_boot_up_effect(&mut combat, owner);
        assert_eq!(
            strength(&combat, owner),
            3,
            "Stock(1) respawn BOOT_UP grants 3 * (2 - 1) = 3 Strength"
        );

        // 2nd respawn: NO StockPower (stock 0) → 3 * (2 - 0) = 6 Strength.
        let mut combat = combat_with_monster(MonsterKind::Axebot, "Axebot");
        let owner = CreatureUid::new(1).expect("nz");
        axebot_boot_up_effect(&mut combat, owner);
        assert_eq!(
            strength(&combat, owner),
            6,
            "stock-0 respawn (no StockPower) BOOT_UP grants 3 * (2 - 0) = 6 Strength"
        );
    }

    /// `fabricator_spawn_bot` reads the Fabricator's `_lastSpawned` code from
    /// the OWNER creature (`find(|c| c.id == owner.0)`, line 1196) to exclude
    /// the last-spawned bot kind. With `!=` the read hits the first NON-owner
    /// creature, mis-reading the exclusion code.
    ///
    /// Fixture: Fabricator counter pre-set to 2 (Stabbot's code); a dummy
    /// non-owner enemy (counter 0) sits ahead of the spawn. Correct excludes
    /// code 2 from AGGRO `[Zapbot(1), Stabbot(2)]` → singleton `[Zapbot]` →
    /// spawns Zapbot. Under `!=`, the read sees the dummy's code 0 → no
    /// exclusion → both candidates → niche `monster_ai` roll (index 1) →
    /// Stabbot ≠ Zapbot.
    #[test]
    fn fabricator_reads_lastspawned_from_owner() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};

        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        let mut fab_ai = create_fabricator_ai();
        fab_ai.set_counter(2); // last spawned = Stabbot (code 2)
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 150,
            max_hp: 150,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::Fabricator),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Fabricator"),
            ai: Some(fab_ai),
        });
        state.enemy_uids.push(owner);
        // The Fabricator sits in slot 2 of
        // `[bot1, bot2, fabricator, bot3, bot4]`; combat entry stamps that, and
        // the slot model needs it to place bots either side (GH #552).
        state.assign_encounter_slots(&[2]);
        // A dummy non-owner enemy (no counter → reads as 0) sits ahead of the
        // spawn so a `!=` read latches onto IT instead of the Fabricator.
        let dummy = push_enemy(&mut state, 2, MonsterKind::Zapbot, 30);
        let _ = dummy;

        let before: std::collections::HashSet<u32> =
            state.creatures.iter().map(|c| c.id.get()).collect();
        fabricator_spawn_bot(&mut state, owner, &FABRICATOR_AGGRO);
        let spawned = state
            .creatures
            .iter()
            .find(|c| !before.contains(&c.id.get()))
            .and_then(|c| c.species)
            .expect("a bot spawned");

        assert_eq!(
            spawned,
            MonsterKind::Zapbot,
            "excluding the owner's last-spawned code 2 leaves only Zapbot"
        );
    }

    /// `fabricator_spawn_bot` records the just-spawned bot's code as
    /// `_lastSpawned` on the OWNER's AI (`position(|c| c.id == owner.0)`, line
    /// 1282). With `!=` it writes the counter onto the first NON-owner creature,
    /// leaving the Fabricator's counter stale.
    ///
    /// Fixture: Fabricator counter 0; a dummy non-owner enemy sits ahead of the
    /// spawn. One AGGRO spawn rolls Zapbot (code 1) under v0.107.1 MegaRandom
    /// (was Stabbot=2 under v0.103.2). Correct stamps 1 on the Fabricator's AI;
    /// under `!=` the Fabricator keeps 0 (the dummy is stamped instead).
    /// Asserting the Fabricator's counter == 1 distinguishes them.
    #[test]
    fn fabricator_records_lastspawned_on_owner() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};

        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 150,
            max_hp: 150,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::Fabricator),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Fabricator"),
            ai: Some(create_fabricator_ai()),
        });
        state.enemy_uids.push(owner);
        // The Fabricator sits in slot 2 of
        // `[bot1, bot2, fabricator, bot3, bot4]`; combat entry stamps that, and
        // the slot model needs it to place bots either side (GH #552).
        state.assign_encounter_slots(&[2]);
        // Dummy non-owner enemy WITH an AI carrying a sentinel counter, so a
        // mis-targeted write is observable (and doesn't read back as the
        // Fabricator's value).
        let mut dummy = Creature {
            id: std::num::NonZeroU32::new(2).expect("nz"),
            current_hp: 30,
            max_hp: 30,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::Zapbot),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Zapbot"),
            ai: Some(create_zapbot_ai()),
        };
        if let Some(ai) = dummy.ai.as_mut() {
            ai.set_counter(99);
        }
        state.creatures.push(dummy);
        state
            .enemy_uids
            .push(CreatureUid(std::num::NonZeroU32::new(2).expect("nz")));

        fabricator_spawn_bot(&mut state, owner, &FABRICATOR_AGGRO);

        // The first AGGRO spawn (no prior exclusion) rolls Zapbot (code 1).
        let fab_counter = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .and_then(|c| c.ai.as_ref().map(sts2_core::monster_ai::MonsterAi::counter))
            .expect("fabricator has an AI counter");
        assert_eq!(
            fab_counter, 1,
            "the spawned bot's code (Zapbot = 1) is recorded on the OWNER's AI"
        );
    }

    /// `fabricator_spawn_bot`'s insert-position scan uses `key > slot` (line
    /// 1274) so the new bot sorts AHEAD of living enemies in HIGHER slots and
    /// BEHIND those in equal-or-lower slots. The `>`-vs-`>=` distinction only
    /// bites when a LIVING enemy already occupies the chosen slot — which the
    /// fallback "all four bot slots held, reuse slot 4" overshoot produces.
    ///
    /// Fixture: living bots at slots 0,1,3,4 plus the Fabricator (slot 2). A
    /// fifth spawn finds no free bot slot and falls back to slot 4. With `>`
    /// the scan finds no living enemy with `key > 4` → the bot APPENDS after
    /// the slot-4 bot. With `>=` the scan stops at the existing slot-4 bot →
    /// the new bot inserts BEFORE it. The two orderings differ, so asserting
    /// the new bot lands AFTER the pre-existing slot-4 bot kills `>=`.
    #[test]
    fn fabricator_insert_slot_strict_greater_than() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};

        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 150,
            max_hp: 150,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::Fabricator),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Fabricator"),
            ai: Some(create_fabricator_ai()),
        });
        state.enemy_uids.push(owner);
        // The Fabricator sits in slot 2 of
        // `[bot1, bot2, fabricator, bot3, bot4]`; combat entry stamps that, and
        // the slot model needs it to place bots either side (GH #552).
        state.assign_encounter_slots(&[2]);

        // Living bots occupying ALL four bot slots {0,1,3,4}; record each slot.
        let mut slot4_bot = None;
        for (raw, slot) in [(2u32, 0u8), (3, 1), (4, 3), (5, 4)] {
            let uid = push_enemy(&mut state, raw, MonsterKind::Zapbot, 30);
            state.set_creature_ai(uid, create_zapbot_ai());
            state.set_enemy_slot(uid, slot);
            if slot == 4 {
                slot4_bot = Some(uid);
            }
        }
        let slot4_bot = slot4_bot.expect("slot-4 bot");

        let before: std::collections::HashSet<u32> =
            state.creatures.iter().map(|c| c.id.get()).collect();
        // All bot slots held → fallback slot 4. Spawn an aggro bot.
        fabricator_spawn_bot(&mut state, owner, &FABRICATOR_AGGRO);
        let new_uid = state
            .creatures
            .iter()
            .find(|c| !before.contains(&c.id.get()))
            .map(|c| CreatureUid(c.id))
            .expect("new bot spawned");

        let pos_new = state
            .enemy_uids
            .iter()
            .position(|u| *u == new_uid)
            .expect("new bot in enemy order");
        let pos_slot4 = state
            .enemy_uids
            .iter()
            .position(|u| *u == slot4_bot)
            .expect("slot-4 bot in enemy order");
        assert!(
            pos_new > pos_slot4,
            "with strict `key > slot` the overshoot bot appends AFTER the \
             existing slot-4 bot (got new at {pos_new}, slot-4 at {pos_slot4})"
        );
    }

    #[test]
    fn devoted_sculptor_forbidden_incantation_applies_ritual_nine_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::DevotedSculptor, "DevotedSculptor");
        devoted_sculptor_forbidden_incantation_effect(
            &mut combat,
            CreatureUid::new(1).expect("nz"),
        );
        assert_eq!(self_power(&combat, PowerKind::Ritual), 9);
    }

    /// Bead `sts2-rs-0qo0b`: OwlMagistrate JUDICIAL_FLIGHT applies Soar(1)
    /// to self (halving powered damage received), and VERDICT drops it.
    /// Regression guard: JUDICIAL_FLIGHT was telegraph-only (placeholder
    /// Strength/0) and PowerKind::Soar had no ModifyDamageMultiplicative
    /// hook, so the defensive buff was entirely inert.
    #[test]
    fn owl_magistrate_judicial_flight_soars_then_verdict_drops_it() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};
        use sts2_core::{CreatureUid, MonsterKind};

        let owner = CreatureUid::new(1).expect("nz");
        let mut combat = combat_with_monster(MonsterKind::OwlMagistrate, "OwlMagistrate");

        owl_magistrate_judicial_flight_effect(&mut combat, owner);
        assert_eq!(
            self_power(&combat, PowerKind::Soar),
            1,
            "Soar(1) applied on JUDICIAL_FLIGHT"
        );

        // Soar halves a powered attack received by the owl (10 → 5).
        let halved = apply_damage(
            &mut combat,
            CreatureRef::Player,
            owner,
            10,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(halved, 5, "Soar halves powered damage received");

        // VERDICT drops Soar and applies Vulnerable(4) to the player.
        owl_magistrate_verdict_effect(&mut combat, owner);
        assert_eq!(
            self_power(&combat, PowerKind::Soar),
            0,
            "VERDICT removes Soar"
        );
        assert_eq!(
            player_power(&combat, PowerKind::Vulnerable),
            4,
            "VERDICT applies Vulnerable(4)"
        );

        // With Soar gone the next powered attack lands in full.
        let full = apply_damage(
            &mut combat,
            CreatureRef::Player,
            owner,
            10,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(full, 10, "no Soar → full damage");
    }

    #[test]
    fn axebot_v107_moveset_no_sharpen_no_rand() {
        // `…wb8lk.11`: v0.107.1 removed SHARPEN and the RAND RandomBranch.
        // A normal Axebot opens on HAMMER_UPPERCUT and alternates with
        // ONE_TWO; the stock-respawn opener is BOOT_UP.
        let ai = create_axebot_ai(false);
        assert_eq!(ai.initial, "HAMMER_UPPERCUT");
        assert!(!ai.states.contains_key("SHARPEN"), "SHARPEN removed");
        assert!(!ai.states.contains_key("RAND"), "RAND removed");
        let follow = |id: &str| match ai.states.get(id) {
            Some(MonsterState::Move(m)) => m.follow_up,
            _ => None,
        };
        assert_eq!(follow("HAMMER_UPPERCUT"), Some("ONE_TWO"));
        assert_eq!(follow("ONE_TWO"), Some("HAMMER_UPPERCUT"));
        assert_eq!(follow("BOOT_UP"), Some("HAMMER_UPPERCUT"));
        let Some(MonsterState::Move(one_two)) = ai.states.get("ONE_TWO") else {
            panic!("ONE_TWO missing");
        };
        assert_eq!(
            one_two.intents.as_slice(),
            &[Intent::MultiAttack { damage: 9, hits: 2 }]
        );
        assert_eq!(create_axebot_ai(true).initial, "BOOT_UP");
    }

    #[test]
    fn axebot_hammer_uppercut_applies_weak2_and_frail2_player() {
        // `…wb8lk.11`: v0.107.1 HammerUppercut debuffs bumped 1→2.
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        axebot_hammer_uppercut_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Weak), 2);
        assert_eq!(player_power(&state, PowerKind::Frail), 2);
    }

    #[test]
    fn frog_knight_for_the_queen_applies_strength_five_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::FrogKnight, "FrogKnight");
        frog_knight_for_the_queen_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Strength), 5);
    }

    #[test]
    fn frog_knight_tongue_lash_applies_frail_two_player() {
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        frog_knight_tongue_lash_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Frail), 2);
    }

    #[test]
    fn globe_head_shocking_slap_applies_frail_two_player() {
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        globe_head_shocking_slap_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Frail), 2);
    }

    #[test]
    fn globe_head_galvanic_burst_applies_strength_two_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::GlobeHead, "GlobeHead");
        globe_head_galvanic_burst_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Strength), 2);
    }

    #[test]
    fn slimed_berserker_leeching_hug_applies_weak_player_and_strength_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::SlimedBerserker, "SlimedBerserker");
        slimed_berserker_leeching_hug_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&combat, PowerKind::Weak), 3);
        assert_eq!(self_power(&combat, PowerKind::Strength), 3);
    }

    #[test]
    fn stabbot_stab_applies_frail_one_player() {
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        stabbot_stab_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Frail), 1);
    }

    #[test]
    fn knight_war_chant_applies_strength_three_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::FlailKnight, "FlailKnight");
        knight_war_chant_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Strength), 3);
    }

    #[test]
    fn living_shield_smash_applies_strength_three_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::LivingShield, "LivingShield");
        living_shield_smash_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Strength), 3);
    }

    #[test]
    fn mecha_knight_windup_applies_strength_five_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::MechaKnight, "MechaKnight");
        mecha_knight_windup_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Strength), 5);
    }

    #[test]
    fn owl_magistrate_verdict_applies_vulnerable_four_player() {
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        owl_magistrate_verdict_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Vulnerable), 4);
    }

    #[test]
    fn soul_nexus_drain_life_applies_vulnerable_two_and_weak_two_player() {
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        soul_nexus_drain_life_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Vulnerable), 2);
        assert_eq!(player_power(&state, PowerKind::Weak), 2);
    }

    /// TestSubject deferred-respawn revive pipeline (bead `sts2-rs-jnf4a`,
    /// retimed from `sts2-rs-hjpnj`).
    ///
    /// C# `AdaptablePower` + `TestSubject.cs:252-278`: the revive is performed
    /// on the corpse's NEXT enemy turn (NOT at death), driven from
    /// `combat_loop::enemy_turn_exec`'s revive pass which `lookup`s the
    /// registered `Adaptable` handler. This test drives that handler directly:
    ///   - First revive → SecondFormHp (200), apply PainfulStabs(1).
    ///   - Second revive → ThirdFormHp (300), apply Nemesis(1), remove
    ///     PainfulStabs, remove Adaptable.
    ///   - After Adaptable removed → the handler no-ops (no revive) so the
    ///     third death resolves combat normally.
    #[test]
    fn adaptable_revives_test_subject_twice_then_allows_combat_to_end() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::enemy_death_effects;
        use sts2_core::monsters::test_subject;
        use sts2_core::{
            CombatState, Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, Power,
            PowerUid,
        };

        crate::cards::dispatch::ensure_installed();

        let owner_raw = NonZeroU32::new(1).expect("nz");
        let owner = CreatureUid(owner_raw);

        // Build a state with a TestSubject carrying Adaptable(1) + Enrage(2).
        let mut state = CombatState::default();

        // Mint power ids.
        let adaptable_pid = NonZeroU32::new(1).expect("nz");
        let enrage_pid = NonZeroU32::new(2).expect("nz");
        let mut ts_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        ts_powers.push(PowerUid(adaptable_pid));
        ts_powers.push(PowerUid(enrage_pid));
        state
            .powers
            .push(Power::new(adaptable_pid, PowerKind::Adaptable, 1));
        state
            .powers
            .push(Power::new(enrage_pid, PowerKind::Enrage, 2));

        state.creatures.push(Creature {
            id: owner_raw,
            current_hp: 0, // dead
            max_hp: test_subject::FIRST_FORM_HP,
            block: 0,
            powers: ts_powers,
            species: Some(MonsterKind::TestSubject),
            monster_id: MonsterId("TestSubject"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        state.enemy_uids.push(owner);

        // The revive is driven from the enemy-turn pass via the registered
        // handler lookup (NOT `fire_pending`, which now skips Adaptable).
        let revive = enemy_death_effects::lookup(PowerKind::Adaptable)
            .expect("Adaptable revive handler registered");

        // ── Revive 1 (corpse on its next enemy turn) ────────────────────────
        revive(&mut state, owner);

        {
            let ts = state
                .creatures
                .iter()
                .find(|c| c.id == owner_raw)
                .expect("ts present");
            assert_eq!(
                ts.current_hp,
                test_subject::SECOND_FORM_HP,
                "revived to SecondFormHp on first death"
            );
            assert_eq!(ts.max_hp, test_subject::SECOND_FORM_HP);
        }
        // Adaptable must still have amount > 0 (no-zero-on-revive fix).
        {
            let adaptable_amt = state
                .powers
                .iter()
                .find(|p| p.id == adaptable_pid)
                .map_or(0, |p| p.amount);
            assert!(
                adaptable_amt > 0,
                "Adaptable.amount must remain > 0 after first revive so it fires again"
            );
            let adaptable_ctr = state
                .powers
                .iter()
                .find(|p| p.id == adaptable_pid)
                .map_or(0, |p| p.counter);
            assert_eq!(
                adaptable_ctr, 1,
                "counter incremented to 1 after first death"
            );
        }
        // PainfulStabs(1) applied.
        assert_eq!(
            self_power(&state, PowerKind::PainfulStabs),
            1,
            "PainfulStabs(1) applied on first revive"
        );

        // ── Revive 2 ────────────────────────────────────────────────────────
        // Kill the creature again, then run the next-turn revive pass.
        state
            .creatures
            .iter_mut()
            .find(|c| c.id == owner_raw)
            .expect("ts")
            .current_hp = 0;

        revive(&mut state, owner);

        {
            let ts = state
                .creatures
                .iter()
                .find(|c| c.id == owner_raw)
                .expect("ts present");
            assert_eq!(
                ts.current_hp,
                test_subject::THIRD_FORM_HP,
                "revived to ThirdFormHp on second death"
            );
            assert_eq!(ts.max_hp, test_subject::THIRD_FORM_HP);
        }
        // Adaptable removed after second revive.
        assert!(
            !state.powers.iter().any(|p| p.id == adaptable_pid),
            "Adaptable power removed from arena after second revive"
        );
        assert!(
            !state
                .creatures
                .iter()
                .find(|c| c.id == owner_raw)
                .expect("ts")
                .powers
                .iter()
                .any(|pu| pu.0 == adaptable_pid),
            "Adaptable uid removed from creature's power list"
        );
        // PainfulStabs removed.
        assert_eq!(
            self_power(&state, PowerKind::PainfulStabs),
            0,
            "PainfulStabs removed on second revive"
        );
        // Nemesis applied.
        assert_eq!(
            self_power(&state, PowerKind::Nemesis),
            1,
            "Nemesis(1) applied on second revive"
        );

        // ── Death 3 ────────────────────────────────────────────────────────
        // Kill the creature a third time; Adaptable is gone, so the revive
        // handler no-ops and the corpse stays dead (combat can end).
        state
            .creatures
            .iter_mut()
            .find(|c| c.id == owner_raw)
            .expect("ts")
            .current_hp = 0;

        revive(&mut state, owner);
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == owner_raw)
                .expect("ts")
                .current_hp,
            0,
            "no Adaptable on third death → handler no-ops, corpse stays dead → combat ends"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // Mutant-coverage tests (issue #65)
    // ────────────────────────────────────────────────────────────────────

    /// Push a bare enemy `Creature` of `kind` at `id` with the given hp into
    /// `combat.creatures` + `enemy_uids`. Returns its uid.
    fn push_enemy(
        combat: &mut CombatState,
        id: u32,
        kind: sts2_core::MonsterKind,
        hp: i32,
    ) -> sts2_core::CreatureUid {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, PowerUid};

        let raw = NonZeroU32::new(id).expect("nz");
        combat.creatures.push(Creature {
            id: raw,
            current_hp: hp,
            max_hp: hp.max(1),
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: Some(kind),
            monster_id: MonsterId("X"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        let uid = CreatureUid(raw);
        combat.enemy_uids.push(uid);
        uid
    }

    fn creature_block(state: &CombatState, uid: sts2_core::CreatureUid) -> i32 {
        state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .map_or(0, |c| c.block)
    }

    /// `guardbot_guard_effect` (line 81): blocks ONLY alive Fabricators —
    /// `c.species == Some(Fabricator) && c.current_hp > 0`. A dead Fabricator
    /// (hp 0) and an alive non-Fabricator must NOT receive block; an alive
    /// Fabricator gets exactly 15. Kills the `==`/`!=`, `&&`/`||`, and
    /// `> 0` comparison mutants on the filter.
    #[test]
    fn guardbot_blocks_only_alive_fabricators() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = CombatState::default();
        let alive_fab = push_enemy(&mut combat, 1, MonsterKind::Fabricator, 100);
        let dead_fab = push_enemy(&mut combat, 2, MonsterKind::Fabricator, 0);
        let alive_other = push_enemy(&mut combat, 3, MonsterKind::Zapbot, 50);

        guardbot_guard_effect(&mut combat, CreatureUid::new(1).expect("nz"));

        assert_eq!(
            creature_block(&combat, alive_fab),
            15,
            "alive Fabricator gains 15 block"
        );
        assert_eq!(
            creature_block(&combat, dead_fab),
            0,
            "dead Fabricator (hp 0) gains no block — `> 0` guard"
        );
        assert_eq!(
            creature_block(&combat, alive_other),
            0,
            "non-Fabricator gains no block — species `==` guard"
        );
    }

    /// `apply_power_to_player_via_legacy_surface` (line 197): the existing-power
    /// merge searches for `p.id == puid.0 && p.kind == kind`. A SECOND
    /// application of the same kind must MERGE (amount add) rather than push a
    /// duplicate power. Kills the `==`→`!=` on `p.id == puid.0` (a `!=` would
    /// fail to find the existing instance and push a duplicate).
    #[test]
    fn legacy_surface_merges_same_kind_instead_of_duplicating() {
        let mut state = CombatState::default();
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Weak, 1);
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Weak, 2);
        let weak_instances = state
            .player
            .powers
            .iter()
            .filter(|pu| {
                state
                    .powers
                    .iter()
                    .any(|p| p.id == pu.0 && p.kind == PowerKind::Weak)
            })
            .count();
        assert_eq!(
            weak_instances, 1,
            "same-kind powers merge into one instance"
        );
        assert_eq!(
            player_power(&state, PowerKind::Weak),
            3,
            "amounts add (1+2)"
        );
    }

    /// `apply_signed_power_to_player` line 255 (`p.id == puid.0 && p.kind ==
    /// kind`): the existing-power lookup must match BOTH id and kind. With an
    /// unrelated Dexterity already on the player, applying Strength must create
    /// a SEPARATE Strength power and leave Dexterity untouched. Under `&&`→`||`
    /// the lookup matches the Dexterity power by id alone and folds the
    /// Strength amount INTO Dexterity instead — caught by the midpoint
    /// assertions below.
    #[test]
    fn signed_power_lookup_requires_id_and_kind() {
        let mut state = CombatState::default();
        apply_signed_power_to_player(&mut state, PowerKind::Dexterity, 1);
        apply_signed_power_to_player(&mut state, PowerKind::Strength, 2);
        // Under `&&`→`||` the +2 would have merged into Dexterity (→3) and no
        // distinct Strength power would exist.
        assert_eq!(
            player_power(&state, PowerKind::Dexterity),
            1,
            "Dexterity untouched — Strength must not fold into it"
        );
        assert_eq!(
            player_power(&state, PowerKind::Strength),
            2,
            "a separate Strength(2) power was created"
        );
    }

    /// `apply_signed_power_to_player` lines 269-272: when a signed power nets to
    /// exactly 0 it is removed from the arena (269), the player legacy list
    /// (270), AND every creature's power list (271-273). Build a player whose
    /// MIRROR creature carries the to-be-zeroed Strength uid alongside a decoy
    /// power, then apply -2. Kills the three retain mutants by asserting on raw
    /// uid membership (the deleted-power record can't be observed via an
    /// existence filter, so a dangling uid would otherwise survive unseen):
    /// the zeroed uid must be gone from the creature while the decoy uid stays.
    #[test]
    fn signed_power_net_zero_strips_uid_from_creatures_keeping_others() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, Power, PowerUid};

        let mut state = CombatState::default();
        let player_raw = NonZeroU32::new(1).expect("nz");
        let str_pid = NonZeroU32::new(10).expect("nz");
        let decoy_pid = NonZeroU32::new(11).expect("nz");
        // Strength(2) and a decoy Dexterity(3), both on the player mirror.
        state
            .powers
            .push(Power::new(str_pid, PowerKind::Strength, 2));
        state
            .powers
            .push(Power::new(decoy_pid, PowerKind::Dexterity, 3));
        let mut pw: SmallVec<[PowerUid; 8]> = SmallVec::new();
        pw.push(PowerUid(str_pid));
        pw.push(PowerUid(decoy_pid));
        state.creatures.push(Creature {
            id: player_raw,
            current_hp: 70,
            max_hp: 70,
            block: 0,
            powers: pw,
            species: None,
            monster_id: MonsterId("Player"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        state.player.creature_uid = Some(CreatureUid(player_raw));
        state.player.powers.push(PowerUid(str_pid));
        state.player.powers.push(PowerUid(decoy_pid));

        // Net the Strength to 0 → it must be stripped everywhere.
        apply_signed_power_to_player(&mut state, PowerKind::Strength, -2);

        let player_c = state.creatures.iter().find(|c| c.id == player_raw).unwrap();
        assert!(
            !player_c.powers.iter().any(|u| u.0 == str_pid),
            "zeroed Strength uid removed from the creature's power list (retain `!=`)"
        );
        assert!(
            player_c.powers.iter().any(|u| u.0 == decoy_pid),
            "the decoy Dexterity uid SURVIVES on the creature (retain keeps u.0 != id)"
        );
        assert!(
            !state.powers.iter().any(|p| p.id == str_pid),
            "zeroed Strength record removed from arena"
        );
        assert!(
            !state.player.powers.iter().any(|u| u.0 == str_pid),
            "zeroed Strength uid removed from the player legacy list"
        );
        assert!(
            state.powers.iter().any(|p| p.id == decoy_pid),
            "decoy Dexterity record survives in arena"
        );
    }

    /// `fabricator_bot_code` (lines 1150-1156): each bot kind maps to a
    /// distinct non-zero code; non-bot kinds map to 0. Kills the
    /// constant-return mutants and every per-arm deletion.
    #[test]
    fn fabricator_bot_code_maps_each_kind() {
        use sts2_core::MonsterKind;
        assert_eq!(fabricator_bot_code(MonsterKind::Zapbot), 1);
        assert_eq!(fabricator_bot_code(MonsterKind::Stabbot), 2);
        assert_eq!(fabricator_bot_code(MonsterKind::Guardbot), 3);
        assert_eq!(fabricator_bot_code(MonsterKind::Noisebot), 4);
        assert_eq!(
            fabricator_bot_code(MonsterKind::Fabricator),
            0,
            "non-bot kinds encode 0"
        );
    }

    /// `create_bot_ai` (lines 1295-1298): resolves each bot kind to its
    /// moveset ctor. Kills the per-arm deletions (a deleted arm falls through
    /// to the Zapbot/"ZAP" default).
    #[test]
    fn create_bot_ai_resolves_each_kind_to_its_moveset() {
        use sts2_core::MonsterKind;
        assert_eq!(
            create_bot_ai(MonsterKind::Stabbot).current_state_id(),
            "STAB"
        );
        assert_eq!(
            create_bot_ai(MonsterKind::Guardbot).current_state_id(),
            "GUARD"
        );
        assert_eq!(
            create_bot_ai(MonsterKind::Noisebot).current_state_id(),
            "NOISE"
        );
        assert_eq!(create_bot_ai(MonsterKind::Zapbot).current_state_id(), "ZAP");
    }

    /// `fabricator_can_fabricate` (lines 1335, 1338): the live-enemy count
    /// (`c.id == uid.0 && c.current_hp > 0`) must EXCLUDE dead enemies, and
    /// the gate is `< 4`. With 3 alive + 1 dead enemy the gate is OPEN
    /// (3 < 4); the dead enemy must not be counted. Kills `==`→`!=`,
    /// `&&`→`||`, and `> 0`→`>=` (each would count the corpse → 4 → closed).
    #[test]
    fn fabricator_can_fabricate_excludes_dead_enemies() {
        use sts2_core::MonsterKind;
        let mut combat = CombatState::default();
        push_enemy(&mut combat, 1, MonsterKind::Fabricator, 100);
        push_enemy(&mut combat, 2, MonsterKind::Zapbot, 10);
        push_enemy(&mut combat, 3, MonsterKind::Stabbot, 10);
        push_enemy(&mut combat, 4, MonsterKind::Guardbot, 0); // corpse
        assert!(
            fabricator_can_fabricate(&combat),
            "3 alive + 1 corpse → gate open; corpse not counted"
        );
    }

    /// `fabricator_can_fabricate` line 1338: the threshold is strictly `< 4`.
    /// Exactly 4 alive enemies must CLOSE the gate (kills `<`→`<=`).
    #[test]
    fn fabricator_can_fabricate_closes_at_exactly_four_alive() {
        use sts2_core::MonsterKind;
        let mut combat = CombatState::default();
        push_enemy(&mut combat, 1, MonsterKind::Fabricator, 100);
        push_enemy(&mut combat, 2, MonsterKind::Zapbot, 10);
        push_enemy(&mut combat, 3, MonsterKind::Stabbot, 10);
        push_enemy(&mut combat, 4, MonsterKind::Guardbot, 10);
        assert!(
            !fabricator_can_fabricate(&combat),
            "exactly 4 alive → gate closed (strict `< 4`)"
        );
    }

    /// `frog_knight_should_charge` (lines 797-803): fires only when the AI
    /// counter is 0 AND `current_hp < max_hp / 2`. Kills the `< `→`<=` on the
    /// HP threshold (at exactly half HP it must NOT charge) and exercises the
    /// counter gate.
    #[test]
    fn frog_knight_should_charge_threshold_and_counter() {
        use sts2_core::{CreatureUid, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut combat = CombatState::default();
        push_enemy(&mut combat, 1, MonsterKind::FrogKnight, 0);
        let set_hp = |combat: &mut CombatState, hp: i32, max: i32| {
            let c = combat
                .creatures
                .iter_mut()
                .find(|c| c.id == owner.0)
                .unwrap();
            c.current_hp = hp;
            c.max_hp = max;
        };
        // Below half (hp 9, max 20 → half 10): charges.
        set_hp(&mut combat, 9, 20);
        assert!(frog_knight_should_charge(&combat, owner, 0));
        // Exactly half (hp 10, max 20 → 10 < 10 is false): does NOT charge.
        set_hp(&mut combat, 10, 20);
        assert!(
            !frog_knight_should_charge(&combat, owner, 0),
            "at exactly half HP the strict `<` must not charge"
        );
        // Below half but counter already 1 (already charged): does NOT charge.
        set_hp(&mut combat, 5, 20);
        assert!(
            !frog_knight_should_charge(&combat, owner, 1),
            "counter != 0 gates out a second charge"
        );
    }

    /// `frog_knight_beetle_charge_effect` (line 812): flips the owner's AI
    /// counter to 1 (once-per-combat gate). Kills the body→`()` and the
    /// `c.id == owner.0`→`!=` mutants.
    #[test]
    fn frog_knight_beetle_charge_sets_counter() {
        use sts2_core::{CreatureUid, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut combat = CombatState::default();
        // The owner must carry an AI for the counter to be set.
        push_enemy(&mut combat, 1, MonsterKind::FrogKnight, 30);
        // A second creature ensures `c.id == owner.0` (not `!=`) is required.
        push_enemy(&mut combat, 2, MonsterKind::FrogKnight, 30);
        if let Some(c) = combat.creatures.iter_mut().find(|c| c.id == owner.0) {
            c.ai = Some(create_frog_knight_ai());
        }
        frog_knight_beetle_charge_effect(&mut combat, owner);
        let ctr = combat
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .and_then(|c| c.ai.as_ref().map(MonsterAi::counter))
            .expect("owner ai");
        assert_eq!(ctr, 1, "BEETLE_CHARGE sets the owner's counter to 1");
    }

    /// `remove_power_from_creature` (lines 628, 635-636): removes only the
    /// given creature's instances of `kind`. Kills the `&&`/`==`/`!=` mutants
    /// on the lookup + the per-creature retain — a Soar on a DIFFERENT
    /// creature must be untouched, and the owner's Soar must be gone while its
    /// other powers survive.
    #[test]
    fn remove_power_from_creature_targets_only_owner_and_kind() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = CombatState::default();
        let owner = push_enemy(&mut combat, 1, MonsterKind::OwlMagistrate, 50);
        let other = push_enemy(&mut combat, 2, MonsterKind::OwlMagistrate, 50);
        apply_power_to_creature(&mut combat, owner, PowerKind::Soar, 1);
        apply_power_to_creature(&mut combat, owner, PowerKind::Strength, 3);
        apply_power_to_creature(&mut combat, other, PowerKind::Soar, 1);

        // Capture the owner's Soar power uid so we can assert it is stripped
        // from the OWNER's list specifically (line 635 `c.id == creature.0`).
        // The power record itself is deleted regardless, so an existence
        // filter would miss a dangling uid left on the owner under `!=`.
        let owner_soar_uid = combat
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .and_then(|c| {
                c.powers.iter().copied().find(|pu| {
                    combat
                        .powers
                        .iter()
                        .any(|p| p.id == pu.0 && p.kind == PowerKind::Soar)
                })
            })
            .expect("owner has a Soar uid");

        remove_power_from_creature(&mut combat, owner, PowerKind::Soar);

        // Raw-uid check: the owner's Soar uid is gone from the OWNER (a `!=`
        // find would retain on the wrong creature, leaving it dangling here).
        assert!(
            !combat
                .creatures
                .iter()
                .find(|c| c.id == owner.0)
                .unwrap()
                .powers
                .contains(&owner_soar_uid),
            "owner's Soar uid stripped from the owner's own power list"
        );

        let soar_of = |s: &CombatState, uid: CreatureUid| {
            s.creatures.iter().find(|c| c.id == uid.0).map_or(0, |c| {
                c.powers
                    .iter()
                    .filter(|pu| {
                        s.powers
                            .iter()
                            .any(|p| p.id == pu.0 && p.kind == PowerKind::Soar)
                    })
                    .count()
            })
        };
        assert_eq!(soar_of(&combat, owner), 0, "owner's Soar removed");
        assert_eq!(
            soar_of(&combat, other),
            1,
            "other creature's Soar untouched"
        );
        // Owner keeps its non-Soar power.
        let owner_str = combat
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .map_or(0, |c| {
                c.powers
                    .iter()
                    .filter(|pu| {
                        combat
                            .powers
                            .iter()
                            .any(|p| p.id == pu.0 && p.kind == PowerKind::Strength)
                    })
                    .count()
            });
        assert_eq!(
            owner_str, 1,
            "owner keeps its Strength (kind-scoped removal)"
        );
    }

    /// `record_possess_steal` + `possess_stolen_total` (lines 1704, 1727):
    /// the steal accumulates in the matching possess power's `counter`, keyed
    /// by `c.id == owner.0 && p.kind == possess_kind`. Kills the `==`/`!=`/`&&`
    /// mutants on both lookups — a non-owner creature's same-kind power must
    /// not be read, and a different kind must not match.
    #[test]
    fn possess_steal_records_and_reads_owner_kind_only() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::MonsterKind;
        let mut combat = CombatState::default();
        let owner = push_enemy(&mut combat, 1, MonsterKind::TheLost, 100);
        let other = push_enemy(&mut combat, 2, MonsterKind::TheForgotten, 100);
        // Apply a DIFFERENT possess kind (PossessSpeed) to the owner FIRST, so
        // its uid sorts ahead of PossessStrength in the owner's power list.
        // Under the `&&`→`||` mutant the PossessStrength lookup would match
        // PossessSpeed by id alone (it's the first uid) and write the steal
        // into the WRONG power — caught by the PossessSpeed assertion below.
        apply_power_to_creature(&mut combat, owner, PowerKind::PossessSpeed, 1);
        apply_power_to_creature(&mut combat, owner, PowerKind::PossessStrength, 1);
        // Decoy: another creature with the same possess kind must not be read.
        apply_power_to_creature(&mut combat, other, PowerKind::PossessStrength, 1);

        record_possess_steal(&mut combat, owner, PowerKind::PossessStrength, 2);
        record_possess_steal(&mut combat, owner, PowerKind::PossessStrength, 3);

        assert_eq!(
            possess_stolen_total(&combat, owner, PowerKind::PossessStrength),
            5,
            "owner's PossessStrength counter accumulates 2+3"
        );
        assert_eq!(
            possess_stolen_total(&combat, other, PowerKind::PossessStrength),
            0,
            "decoy creature's same-kind counter is untouched"
        );
        assert_eq!(
            possess_stolen_total(&combat, owner, PowerKind::PossessSpeed),
            0,
            "owner's other possess kind is untouched (kind scoping)"
        );
    }

    /// `possess_speed_after_death` (lines 1752-1753): restores stolen
    /// Dexterity to the player only when the accumulated total is non-zero.
    /// Kills the body→`()` and the `!= 0`→`== 0` guard mutants.
    #[test]
    fn possess_speed_after_death_restores_stolen_dexterity() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::MonsterKind;
        let mut combat = CombatState::default();
        let owner = push_enemy(&mut combat, 1, MonsterKind::TheForgotten, 100);
        apply_power_to_creature(&mut combat, owner, PowerKind::PossessSpeed, 1);
        // Player previously lost 3 Dexterity; the steal total is +3.
        apply_signed_power_to_player(&mut combat, PowerKind::Dexterity, -3);
        record_possess_steal(&mut combat, owner, PowerKind::PossessSpeed, 3);
        // The bot is now dead when the AfterDeath handler runs.
        if let Some(c) = combat.creatures.iter_mut().find(|c| c.id == owner.0) {
            c.current_hp = 0;
        }

        possess_speed_after_death(&mut combat, owner);

        assert_eq!(
            player_power(&combat, PowerKind::Dexterity),
            0,
            "death restores the stolen Dexterity (net 0)"
        );
    }

    /// `living_shield_alive_enemy_count` / `living_shield_alone` (lines 1986,
    /// 1996): the count filters `c.id == uid.0 && c.current_hp > 0`; `alone`
    /// is `count <= 1`. With one alive Living Shield + one corpse the count is
    /// 1 → alone. Kills the `==`/`&&`/`>`→`>=` filter mutants (each would
    /// count the corpse → 2 → not alone) and the `alone`→`true` body mutant.
    #[test]
    fn living_shield_alone_ignores_corpses() {
        use sts2_core::MonsterKind;
        let mut combat = CombatState::default();
        push_enemy(&mut combat, 1, MonsterKind::LivingShield, 30);
        push_enemy(&mut combat, 2, MonsterKind::Zapbot, 0); // corpse
        assert_eq!(
            living_shield_alive_enemy_count(&combat),
            1,
            "corpse not counted"
        );
        assert!(living_shield_alone(&combat), "one alive → alone");

        // Add a second LIVE enemy → no longer alone (kills `alone`→`true`).
        push_enemy(&mut combat, 3, MonsterKind::Zapbot, 30);
        assert_eq!(living_shield_alive_enemy_count(&combat), 2);
        assert!(
            !living_shield_alone(&combat),
            "two alive → not alone (kills constant `true`)"
        );
    }

    /// `magi_knight_dampen_effect` (lines 2129, 2151): applies Dampen(1) to
    /// the player AND stamps the applying MagiKnight as the power's `applier`
    /// (via the `p.id == puid.0 && p.kind == Dampen` lookup). Kills the
    /// body→`()` mutant (no Dampen at all) and the lookup mutants (applier not
    /// stamped).
    #[test]
    fn magi_knight_dampen_applies_and_stamps_applier() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, PowerUid};

        let mut combat = CombatState::default();
        let owner = push_enemy(&mut combat, 1, sts2_core::MonsterKind::MagiKnight, 50);
        // Minimal player creature so the effect's `player.creature_uid.expect`
        // succeeds; no cards present so the card-walk hook is a no-op.
        let player_raw = NonZeroU32::new(2).expect("nz");
        combat.creatures.push(Creature {
            id: player_raw,
            current_hp: 70,
            max_hp: 70,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: None,
            monster_id: MonsterId("Player"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        combat.player.creature_uid = Some(CreatureUid(player_raw));

        magi_knight_dampen_effect(&mut combat, owner);

        assert_eq!(
            player_power(&combat, PowerKind::Dampen),
            1,
            "MagiKnight applies Dampen(1) to the player"
        );
        // Dampen is the player's SOLE power here: this is what kills the
        // `p.id == puid.0`→`!=` mutant (line 2151:28) — under `!=` the lookup
        // can't locate the only Dampen power, leaving the applier unset.
        let applier = combat
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Dampen)
            .and_then(|p| p.applier);
        assert_eq!(
            applier,
            Some(owner),
            "the MagiKnight is stamped as Dampen's applier"
        );
    }

    /// `magi_knight_dampen_effect` line 2151:38 (`&&`): with a DECOY player
    /// power whose uid precedes Dampen, the applier-stamp lookup must still
    /// reject the decoy (kind != Dampen) and stamp the Dampen power. Under
    /// `&&`→`||` the lookup matches the decoy by id alone and stamps it,
    /// leaving Dampen's applier unset. (Kept separate from the no-decoy test
    /// above, which kills the `p.id == puid.0`→`!=` mutant on the same line —
    /// that one requires Dampen to be the player's sole power.)
    #[test]
    fn magi_knight_dampen_stamps_dampen_not_a_decoy_power() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, PowerUid};

        let mut combat = CombatState::default();
        let owner = push_enemy(&mut combat, 1, sts2_core::MonsterKind::MagiKnight, 50);
        let player_raw = NonZeroU32::new(2).expect("nz");
        combat.creatures.push(Creature {
            id: player_raw,
            current_hp: 70,
            max_hp: 70,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: None,
            monster_id: MonsterId("Player"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        combat.player.creature_uid = Some(CreatureUid(player_raw));
        // Decoy player power whose uid sorts AHEAD of Dampen.
        apply_power_to_player_via_legacy_surface(&mut combat, PowerKind::Weak, 1);

        magi_knight_dampen_effect(&mut combat, owner);

        // The Weak decoy must NOT be stamped; only Dampen carries the applier.
        let weak_applier = combat
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Weak)
            .and_then(|p| p.applier);
        assert_eq!(weak_applier, None, "decoy Weak power must not be stamped");
        let dampen_applier = combat
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Dampen)
            .and_then(|p| p.applier);
        assert_eq!(
            dampen_applier,
            Some(owner),
            "Dampen (not the decoy) is stamped — kind guard required"
        );
    }

    /// `queen_burn_bright_effect` (lines 2503, 2508): buffs every alive
    /// teammate Monster (excluding self) with Strength(1) and gains 20 block
    /// on self. Self must be excluded (`uid != owner`), and only Monsters
    /// qualify. Kills the `&&`/`!=`/`==` (2503) and id-compare (2508) mutants.
    /// (The `current_hp > 0` pre-filter on 2501 is equivalent — the downstream
    /// `apply_power_to_creature` re-filters on `current_hp > 0`, so a dead
    /// teammate is never actually buffed whether collected or not.)
    #[test]
    fn queen_burn_bright_buffs_only_alive_other_monsters() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = CombatState::default();
        let queen = push_enemy(&mut combat, 1, MonsterKind::Queen, 200);
        let ally = push_enemy(&mut combat, 2, MonsterKind::FrogKnight, 30);
        let dead_ally = push_enemy(&mut combat, 3, MonsterKind::FrogKnight, 0);

        queen_burn_bright_effect(&mut combat, queen);

        let str_of = |s: &CombatState, uid: CreatureUid| {
            s.creatures.iter().find(|c| c.id == uid.0).map_or(0, |c| {
                c.powers
                    .iter()
                    .filter_map(|pu| {
                        s.powers
                            .iter()
                            .find(|p| p.id == pu.0 && p.kind == PowerKind::Strength)
                            .map(|p| p.amount)
                    })
                    .sum()
            })
        };
        assert_eq!(str_of(&combat, ally), 1, "alive teammate gains Strength(1)");
        assert_eq!(str_of(&combat, dead_ally), 0, "dead teammate not buffed");
        assert_eq!(str_of(&combat, queen), 0, "Queen does not buff herself");
        assert_eq!(creature_block(&combat, queen), 20, "Queen gains 20 block");
    }

    /// `queen_has_amalgam_died` (line 2529): `counter > 0`. Kills the
    /// constant body mutants and the `>`→`==`/`<`/`>=` comparison mutants:
    /// counter 0 → false, counter 1 → true. (`>=` would make 0 → true.)
    #[test]
    fn queen_has_amalgam_died_reads_counter_strictly_positive() {
        use sts2_core::CreatureUid;
        let combat = CombatState::default();
        let owner = CreatureUid::new(1).expect("nz");
        assert!(
            !queen_has_amalgam_died(&combat, owner, 0),
            "counter 0 → false"
        );
        assert!(
            queen_has_amalgam_died(&combat, owner, 1),
            "counter 1 → true"
        );
    }

    /// `test_subject_respawns_lt_two` (line 3094): `counter < 2`. Kills the
    /// constant body mutants and the `<`→`==`/`>`/`<=` mutants: counter 1 →
    /// true, counter 2 → false. (`<=` would make 2 → true; `==` would make
    /// 0 → false.)
    #[test]
    fn test_subject_respawns_lt_two_boundary() {
        use sts2_core::CreatureUid;
        let combat = CombatState::default();
        let owner = CreatureUid::new(1).expect("nz");
        assert!(test_subject_respawns_lt_two(&combat, owner, 0), "0 < 2");
        assert!(test_subject_respawns_lt_two(&combat, owner, 1), "1 < 2");
        assert!(
            !test_subject_respawns_lt_two(&combat, owner, 2),
            "2 is NOT < 2"
        );
    }

    /// `test_subject_skull_bash_effect` (line 2763): applies Vulnerable(1) to
    /// the player. Kills the body→`()` mutant.
    #[test]
    fn test_subject_skull_bash_applies_vulnerable_one() {
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        test_subject_skull_bash_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Vulnerable), 1);
    }

    /// `turret_operator_reload_effect` (line 477): Strength(1) self-buff.
    /// Kills the body→`()` mutant.
    #[test]
    fn turret_operator_reload_applies_strength_one_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::TurretOperator, "TurretOperator");
        turret_operator_reload_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Strength), 1);
    }

    /// `spectral_knight_hex_effect` (lines 488, 496): applies Hex(2) to the
    /// player and stamps the applier on the Hex power. Kills the body→`()`
    /// and the `p.kind == Hex`→`!=` filter mutant (a `!=` would stamp the
    /// wrong/no power, leaving Hex's applier unset).
    #[test]
    fn spectral_knight_hex_applies_two_and_stamps_applier() {
        use sts2_core::CreatureUid;
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        spectral_knight_hex_effect(&mut state, owner);
        assert_eq!(player_power(&state, PowerKind::Hex), 2, "Hex(2) applied");
        let applier = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Hex)
            .and_then(|p| p.applier);
        assert_eq!(applier, Some(owner), "Hex's applier stamped");
    }

    /// `slimed_berserker_vomit_ichor_effect` (line 561): adds 10 Slimed cards
    /// to the player's discard pile. Kills the body→`()` mutant.
    #[test]
    fn slimed_berserker_vomit_ichor_adds_ten_slimed_to_discard() {
        use sts2_core::cards::CardId;
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        let pre = state.player.discard.len();
        slimed_berserker_vomit_ichor_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(
            state.player.discard.len(),
            pre + 10,
            "VOMIT_ICHOR adds 10 Slimed to discard"
        );
        for &uid in state.player.discard.iter().skip(pre) {
            assert_eq!(
                state.cards[uid.0.get() as usize - 1].card_id,
                CardId::Slimed,
                "spawned cards are Slimed"
            );
        }
    }

    /// `scroll_of_biting_more_teeth_effect` (line 851): Strength(2) self-buff.
    /// Kills the body→`()` mutant.
    #[test]
    fn scroll_of_biting_more_teeth_applies_strength_two_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::ScrollOfBiting, "ScrollOfBiting");
        scroll_of_biting_more_teeth_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Strength), 2);
    }

    /// `magi_knight_power_shield_effect` (line 2116): gains 5 block on self.
    /// Kills the body→`()` mutant.
    #[test]
    fn magi_knight_power_shield_gains_five_block_self() {
        use sts2_core::MonsterKind;
        let mut combat = CombatState::default();
        let owner = push_enemy(&mut combat, 1, MonsterKind::MagiKnight, 50);
        magi_knight_power_shield_effect(&mut combat, owner);
        assert_eq!(
            creature_block(&combat, owner),
            5,
            "PowerShield gains 5 block"
        );
    }

    /// `queen_youre_mine_effect` (line 2486): applies Frail/Weak/Vulnerable
    /// (99 each) to the player. Kills the body→`()` mutant.
    #[test]
    fn queen_youre_mine_applies_max_debuffs_to_player() {
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        queen_youre_mine_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Frail), 99);
        assert_eq!(player_power(&state, PowerKind::Weak), 99);
        assert_eq!(player_power(&state, PowerKind::Vulnerable), 99);
    }

    /// `queen_enrage_effect` (line 2516): Strength(2) self-buff. Kills the
    /// body→`()` mutant.
    #[test]
    fn queen_enrage_applies_strength_two_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::Queen, "Queen");
        queen_enrage_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Strength), 2);
    }

    /// `adaptable_after_death` (lines 2840, 2897, 2905, 2916, 2929): on the
    /// FIRST revive the Adaptable lookup (`p.id == pid && p.kind ==
    /// Adaptable`) must find the power, revive to SecondFormHp, apply
    /// PainfulStabs, increment the counter, and seed the AI counter. Kills the
    /// `==`/`!=` mutants on the various power-id/kind lookups by asserting the
    /// observable first-revive outcome with an AI attached.
    #[test]
    fn adaptable_first_revive_reads_power_and_seeds_ai_counter() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::monsters::test_subject;
        use sts2_core::{
            Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, Power, PowerUid,
        };

        let mut state = CombatState::default();
        let owner_raw = NonZeroU32::new(1).expect("nz");
        let owner = CreatureUid(owner_raw);
        let adaptable_pid = NonZeroU32::new(1).expect("nz");
        let mut powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        powers.push(PowerUid(adaptable_pid));
        state
            .powers
            .push(Power::new(adaptable_pid, PowerKind::Adaptable, 1));
        state.creatures.push(Creature {
            id: owner_raw,
            current_hp: 0,
            max_hp: test_subject::FIRST_FORM_HP,
            block: 0,
            powers,
            species: Some(MonsterKind::TestSubject),
            monster_id: MonsterId("TestSubject"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(create_test_subject_ai()),
        });
        state.enemy_uids.push(owner);

        adaptable_after_death(&mut state, owner);

        let ts = state.creatures.iter().find(|c| c.id == owner_raw).unwrap();
        assert_eq!(
            ts.current_hp,
            test_subject::SECOND_FORM_HP,
            "first revive heals to SecondFormHp"
        );
        assert_eq!(
            self_power(&state, PowerKind::PainfulStabs),
            1,
            "first revive applies PainfulStabs(1)"
        );
        let adaptable_ctr = state
            .powers
            .iter()
            .find(|p| p.id == adaptable_pid)
            .map_or(-1, |p| p.counter);
        assert_eq!(adaptable_ctr, 1, "Adaptable counter incremented to 1");
        let ai_ctr = ts.ai.as_ref().map(MonsterAi::counter);
        assert_eq!(ai_ctr, Some(1), "AI counter seeded with respawn count");
    }

    /// `adaptable_after_death` SECOND-revive branch (lines 2905, 2916).
    /// Driving the revive with the Adaptable counter already at 1 routes into
    /// the `respawns_after == 2` arm, which retains `state.player.powers`
    /// dropping the PainfulStabs uid (2905) and the Adaptable uid (2916) — a
    /// `==` retain would instead keep ONLY that uid and drop every other
    /// player-list entry. We seed the PainfulStabs + Adaptable uids onto
    /// `state.player.powers` (alongside a decoy) so the player-list retains are
    /// observable, and assert the decoy survives while both target uids are
    /// stripped. (The `p.id == pid`→`!=` mutant on line 2897 is equivalent: the
    /// `&& p.kind == PainfulStabs` guard uniquely identifies the single
    /// PainfulStabs power, so inverting the id-match still locates it via any
    /// other power-uid.)
    #[test]
    fn adaptable_second_revive_strips_painful_stabs_and_adaptable_from_all_lists() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::monsters::test_subject;
        use sts2_core::{
            Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, Power, PowerUid,
        };

        let mut state = CombatState::default();
        let owner_raw = NonZeroU32::new(1).expect("nz");
        let owner = CreatureUid(owner_raw);
        let adaptable_pid = NonZeroU32::new(10).expect("nz");
        let painful_pid = NonZeroU32::new(11).expect("nz");
        let decoy_pid = NonZeroU32::new(12).expect("nz");

        // Adaptable counter already 1 → this revive is the SECOND.
        let mut adaptable = Power::new(adaptable_pid, PowerKind::Adaptable, 1);
        adaptable.counter = 1;
        state.powers.push(adaptable);
        state
            .powers
            .push(Power::new(painful_pid, PowerKind::PainfulStabs, 1));
        state
            .powers
            .push(Power::new(decoy_pid, PowerKind::Strength, 5));

        let mut owner_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        owner_powers.push(PowerUid(adaptable_pid));
        owner_powers.push(PowerUid(painful_pid));
        state.creatures.push(Creature {
            id: owner_raw,
            current_hp: 0,
            max_hp: test_subject::SECOND_FORM_HP,
            block: 0,
            powers: owner_powers,
            species: Some(MonsterKind::TestSubject),
            monster_id: MonsterId("TestSubject"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(create_test_subject_ai()),
        });
        state.enemy_uids.push(owner);
        // Mirror the target uids + a decoy onto the player legacy list so the
        // `state.player.powers.retain` lines have observable effect.
        state.player.powers.push(PowerUid(painful_pid));
        state.player.powers.push(PowerUid(adaptable_pid));
        state.player.powers.push(PowerUid(decoy_pid));

        adaptable_after_death(&mut state, owner);

        // Second revive heals to ThirdFormHp.
        let ts = state.creatures.iter().find(|c| c.id == owner_raw).unwrap();
        assert_eq!(ts.current_hp, test_subject::THIRD_FORM_HP);
        // PainfulStabs removed from the arena (kills the 2897 `==`→`!=`).
        assert!(
            !state.powers.iter().any(|p| p.id == painful_pid),
            "PainfulStabs record removed (kind-scoped find)"
        );
        // Adaptable removed from the arena.
        assert!(!state.powers.iter().any(|p| p.id == adaptable_pid));
        // Player legacy list: both target uids stripped, decoy survives
        // (kills the 2905 + 2916 `!=`→`==` retains).
        assert!(
            !state.player.powers.iter().any(|u| u.0 == painful_pid),
            "PainfulStabs uid stripped from player list (2905)"
        );
        assert!(
            !state.player.powers.iter().any(|u| u.0 == adaptable_pid),
            "Adaptable uid stripped from player list (2916)"
        );
        assert!(
            state.player.powers.iter().any(|u| u.0 == decoy_pid),
            "decoy player power survives both retains"
        );
    }
}
