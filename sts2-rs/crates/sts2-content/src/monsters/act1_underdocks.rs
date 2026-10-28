// Doc comments use upper-snake-case DSL state names (e.g. `SLEEP_CHECK`,
// `RAMMING_SPEED`) that clippy flags as missing-backticks identifiers.
// They are state-id strings, not Rust paths.
#![allow(clippy::doc_markdown)]

//! Act-4 monster ports (bead sts2-rs-rgm).
//!
//! Ported from sts2-rl-agent@01c094d:sts2_env/monsters/act4.py
//!
//! Ports the 22 monsters from
//! `sts2-rl-agent@01c094d:sts2_env/monsters/act4.py` (630 LOC) to
//! data-driven `MonsterAi` instances. Conventions match [`super::act1_overgrowth`],
//! [`super::act2_hive`], and [`super::act3_glory`]:
//!
//! - Effect bodies are no-ops; intent metadata is preserved.
//! - Constructors return `MonsterAi` only.
//! - `Intent::Summon` landed via bead `sts2-rs-47p5` and is wired here
//!   (bead `sts2-rs-r9pl`) for LivingFog SPAWN.
//!   `Intent::Escape` / `Intent::DeathBlow` do not yet exist in
//!   [`sts2_core::monsters::Intent`]; FatGremlin FLEE and GasBomb
//!   EXPLODE continue to emit
//!   [`sts2_core::monsters::Intent::Unknown`] with a TODO referencing
//!   the follow-up bead that adds the variants.
//! - LagavulinMatriarch's `SLEEP_CHECK` is the parity-critical
//!   `ConditionalBranchState` for the `AsleepPower` mechanic. Python's
//!   predicate reads a per-instance `_state["asleep"]` flag; our
//!   `ConditionFn` can't close over that, so we adopt the
//!   combat-global proxy from [`super::act2_hive`]: `combat.round < 3` is
//!   the surrogate for the 3-turn ASLEEP duration applied at spawn. The
//!   parity test mutates `combat.round` to walk both branches.
//!
//! See [`super::act2_hive`] for the full predicate-evaluation contract.

use std::collections::BTreeMap;

use smallvec::smallvec;
use sts2_core::monster_ai::{
    ConditionFn, ConditionalBranch, ConditionalBranchState, MonsterAi, MonsterState,
    MoveRepeatType, RandomBranchState, WeightedBranch,
};
use sts2_core::monsters::Intent;
use sts2_core::powers::PowerKind;
use sts2_core::CombatState;

use super::dsl::{always_true, br, cb, cb_owner, mv, mv_eff, proto_ai, proto_ais, ProtoAis};
use super::{apply_power_to_player_via_legacy_surface, apply_signed_power_to_player};

// ────────────────────────────────────────────────────────────────────────────
// Telegraph-only Buff/Debuff effect fns (bead sts2-rs-pdsdl)
//
// `perform_intent` runs ONLY a move's `effect_fn` for non-attack intents;
// `Intent::Buff` / `Intent::Debuff` are telegraph metadata and apply no
// power on their own. Each fn below ports the exact `PowerCmd.Apply<…>`
// call from the C# v0.103.2 monster `*Move` method so the buff/debuff
// actually lands. SELF-buffs (Strength/Ritual on the monster) go through
// `apply_power_to_creature`; PLAYER-debuffs (Weak/Frail) go through
// `apply_power_to_player_via_legacy_surface` so they are visible to the
// `state.player.powers` wire-side observers / parity harness.
// ────────────────────────────────────────────────────────────────────────────

/// `ThieveryPower.Steal()` (`ThieveryPower.cs:22-30`), run for every Thievery
/// instance the acting creature holds. GH #368.4.
///
/// [decompiled C# omitted — see the cited path]
///
/// `Amount` is the per-steal CAP (20 for the merc, see
/// `monsters.rs::combat_start_powers`); `DynamicVars.Gold` is the running total
/// actually taken, which the engine keeps on the per-instance
/// [`sts2_core::Power::counter`]. That accumulator is what
/// [`surprise_after_death_spawn`] hands to the FatGremlin as `HeistPower` — so
/// until this was ported the counter was pinned at 0 and the whole
/// Thievery→Heist leg was dead in a pure-sim combat (2 records; the cost is
/// ~60 reward gold, `HeistPower` having no combat effect of its own —
/// `HeistPower.cs:17-29` is a `BeforeDeath` gold refund).
///
/// Called at the END of each of the merc's three move bodies, matching
/// `GremlinMerc.cs:88/104/119` where the `foreach … Steal()` runs after the
/// attack and BEFORE the move's own power application.
fn thievery_steal(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    // C# `Target` is the player; a dead player ends combat before any of this.
    if state.is_over || state.player.current_hp <= 0 || state.gold <= 0 {
        return;
    }
    let uids: smallvec::SmallVec<[sts2_core::PowerUid; 8]> = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .map(|c| c.powers.clone())
        .unwrap_or_default();
    for puid in uids {
        let Some(cap) = state
            .powers
            .get_by_id(puid.0)
            .filter(|p| p.kind == PowerKind::Thievery)
            .map(|p| p.amount)
        else {
            continue;
        };
        let take = cap.min(state.gold).max(0);
        if take == 0 {
            continue;
        }
        state.gold -= take;
        if let Some(p) = state.powers.get_mut_by_id(puid.0) {
            p.counter = p.counter.saturating_add(take);
        }
    }
}

/// `GremlinMerc` GIMME → attack, then `ThieveryPower.Steal()`
/// (`MegaCrit.Sts2.Core.Models.Monsters/GremlinMerc.cs:89-92`). No power
/// application of its own; the steal is the whole tail.
fn gremlin_merc_gimme_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    thievery_steal(state, owner);
}

/// `GremlinMerc` DOUBLE_SMASH → `ThieveryPower.Steal()` then
/// `PowerCmd.Apply<WeakPower>(targets, 2m, …)`
/// (`MegaCrit.Sts2.Core.Models.Monsters/GremlinMerc.cs:105-109`).
fn gremlin_merc_double_smash_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    thievery_steal(state, owner);
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 2);
}

/// `GremlinMerc` HEHE → `ThieveryPower.Steal()` then
/// `PowerCmd.Apply<StrengthPower>(base.Creature, 2m, …)`
/// (`MegaCrit.Sts2.Core.Models.Monsters/GremlinMerc.cs:118-122`).
fn gremlin_merc_hehe_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    thievery_steal(state, owner);
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// `CorpseSlug` GOOP → `PowerCmd.Apply<FrailPower>(targets, GoopFrailAmt=2, …)`
/// (`MegaCrit.Sts2.Core.Models.Monsters/CorpseSlug.cs:128`, `GoopFrailAmt`:46).
fn corpse_slug_goop_effect(state: &mut CombatState, _owner: sts2_core::CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 2);
}

/// `PhantasmalGardener` ENLARGE →
/// `PowerCmd.Apply<StrengthPower>(base.Creature, EnlargeStr=2, …)`
/// (`MegaCrit.Sts2.Core.Models.Monsters/PhantasmalGardener.cs:151`, `EnlargeStr`:46).
fn phantasmal_gardener_enlarge_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    let amount = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::DeadlyEnemies,
        3,
        2,
    );
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// C# `SkittishPower.AfterAttack`
/// (`../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.Models.Powers/SkittishPower.cs:56-69`):
///
/// [decompiled C# omitted — see the cited path]
///
/// PhantasmalGardener spawns with `SkittishPower(6)`
/// (`PhantasmalGardener.cs:92`, wired in `sts2-core::monsters::on_spawn_self_buffs`);
/// the power was recorded but had no listener body at all, so the mirror showed
/// block 0 where the world showed 6 and MCTS over-valued every attack into a
/// gardener by that block.
///
/// **Two-phase, like `CurlUp`.** C# grants the block in `AfterAttack`, i.e.
/// AFTER the whole `AttackCommand` — so none of that command's own hits are
/// absorbed by the fresh block. This hook therefore only LATCHES
/// (`Power::skip_next_tick = true`); the grant runs in the core
/// `hooks::grant_latched_skittish`, from the `dispatch_after_card_played`
/// lane (or before a mid-play selection surfaces), which is the engine's
/// established surrogate for C#'s per-command `AfterAttack` boundary (the same
/// collapse Vigor / Gigantification / CurlUp use). The residual difference is
/// a card that issues TWO separate attack commands — C# would grant between
/// them, we grant after both — but the once-per-turn gate means at most one
/// grant either way.
///
/// Gate notes vs `curl_up_after_damage`:
/// * `ValueProp.Move` (`props.is_card_or_monster_move()`), **not**
///   `IsPoweredAttack()` — an UNPOWERED card attack still triggers Skittish.
/// * `ModelSource is CardModel` is true for `FromCard` AND `FromOsty`
///   (`Commands.Builders/AttackCommand.cs:208-237` both assign
///   `ModelSource = card`), so the player's Osty counts as a card source.
/// * `UnblockedDamage != 0` — a fully-blocked hit does NOT arm the latch
///   (unlike CurlUp, whose C# body ignores the `DamageResult`).
fn skittish_after_damage(
    state: &mut CombatState,
    owner: sts2_core::CreatureUid,
    dealer: sts2_core::hooks::CreatureRef,
    props: sts2_core::hooks::ValueProps,
    unblocked_damage: i32,
) -> sts2_core::card_effects::EffectResult {
    use sts2_core::hooks::{CreatureRef, ValueProps};
    // C# `ValuePropExtensions.IsCardOrMonsterMove` = `props.HasFlag(Move)`.
    if unblocked_damage == 0 || !props.contains(ValueProps::MOVE) {
        return sts2_core::card_effects::EffectResult::NotImplemented;
    }
    // `command.ModelSource is CardModel` — the player's own card, or a card
    // routed through the player's Osty (`FromOsty(osty, card)`).
    let card_sourced = match dealer {
        CreatureRef::Player => true,
        CreatureRef::Enemy(uid) => state.player.pet_uid == Some(uid),
        CreatureRef::Ally(_) | CreatureRef::Nobody => false,
    };
    if !card_sourced {
        return sts2_core::card_effects::EffectResult::NotImplemented;
    }
    // `!HasGainedBlockThisTurn` (`counter == 0`) → arm the grant.
    let pid: Option<std::num::NonZeroU32> = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().find_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| {
                        p.id == puid.0
                            && p.kind == PowerKind::Skittish
                            && p.amount > 0
                            && p.counter == 0
                    })
                    .map(|p| p.id)
            })
        });
    let Some(pid) = pid else {
        return sts2_core::card_effects::EffectResult::NotImplemented;
    };
    if let Some(p) = state.powers.get_mut_by_id(pid) {
        p.skip_next_tick = true;
    }
    sts2_core::card_effects::EffectResult::Resolved
}

/// Idempotent installer for SkittishPower's `AfterDamageReceived` latch.
/// Called via a `Once` guard from [`create_phantasmal_gardener_ai`] — every
/// gardener spawn flows through it, so the handler is present before any
/// combat damage event. Safe to call from multiple boot paths.
pub fn install_skittish_handlers() {
    sts2_core::monster_power_effects::register_after_damage_received_for_power(
        PowerKind::Skittish,
        skittish_after_damage,
    );
}

/// `SkulkingColony` INERTIA →
/// `PowerCmd.Apply<StrengthPower>(base.Creature, InertiaStrengthGain, …)`
/// (`MegaCrit.Sts2.Core.Models.Monsters/SkulkingColony.cs:83`). v0.107.1
/// `InertiaStrengthGain`:49 is `GetValueIfAscension(DeadlyEnemies, 4, 2)` —
/// the A9 value is 4, not the 3 carried over from v0.103.2.
fn skulking_colony_inertia_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    let amount = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::DeadlyEnemies,
        4,
        2,
    );
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// `CalcifiedCultist` INCANTATION →
/// `PowerCmd.Apply<RitualPower>(base.Creature, IncantationAmount=2, …)`
/// (`MegaCrit.Sts2.Core.Models.Monsters/CalcifiedCultist.cs:82`, `IncantationAmount`:33).
fn calcified_cultist_incantation_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Ritual, 2);
}

/// `DampCultist` INCANTATION →
/// `PowerCmd.Apply<RitualPower>(base.Creature, IncantationAmount=5, …)`
/// (`MegaCrit.Sts2.Core.Models.Monsters/DampCultist.cs:82`, `IncantationAmount`:33,
/// non-ascension value 5).
fn damp_cultist_incantation_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    let amount = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::DeadlyEnemies,
        6,
        5,
    );
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Ritual, amount);
}

/// `FossilStalker` TACKLE → `PowerCmd.Apply<FrailPower>(targets, 1m, …)`
/// (`MegaCrit.Sts2.Core.Models.Monsters/FossilStalker.cs:71`).
fn fossil_stalker_tackle_effect(state: &mut CombatState, _owner: sts2_core::CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 1);
}

/// `PunchConstruct` FAST_PUNCH → `PowerCmd.Apply<FrailPower>(targets, 1m, …)`
/// (`v0.107.1/…/PunchConstruct.cs:118`). `…wb8lk.11`: v0.107.1 changed the
/// FastPunch debuff from Weak to Frail.
fn punch_construct_fast_punch_effect(state: &mut CombatState, _owner: sts2_core::CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 1);
}

/// `SludgeSpinner` OIL_SPRAY → `PowerCmd.Apply<WeakPower>(targets, 1m, …)`
/// (`MegaCrit.Sts2.Core.Models.Monsters/SludgeSpinner.cs:58`). Bead
/// `sts2-rs-29kfv`.
fn sludge_spinner_oil_spray_effect(state: &mut CombatState, _owner: sts2_core::CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 1);
}

/// `SludgeSpinner` RAGE → `PowerCmd.Apply<StrengthPower>(base.Creature, 3m, …)`
/// (`MegaCrit.Sts2.Core.Models.Monsters/SludgeSpinner.cs:75`). Bead
/// `sts2-rs-29kfv`.
fn sludge_spinner_rage_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 3);
}

/// `SewerClam` PRESSURIZE → `PowerCmd.Apply<StrengthPower>(base.Creature, 4m, …)`
/// (`MegaCrit.Sts2.Core.Models.Monsters/SewerClam.cs:50`). Bead
/// `sts2-rs-29kfv`.
fn sewer_clam_pressurize_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 4);
}

/// LagavulinMatriarch SOUL_SIPHON effect. Per C# v0.103.2
/// `MegaCrit.Sts2.Core.Models.Monsters/LagavulinMatriarch.cs::SoulSiphonMove`:
/// [decompiled C# omitted — see the cited path]
/// The player stat-reductions route through the signed legacy surface so
/// they are visible to the parity harness; the +2 self-Strength buff routes
/// through `apply_power_to_creature` (the d03 mirror) like every other
/// monster self-buff in this file.
fn lagavulin_soul_siphon_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    apply_signed_power_to_player(state, PowerKind::Strength, -2);
    apply_signed_power_to_player(state, PowerKind::Dexterity, -2);
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

// ────────────────────────────────────────────────────────────────────────────
// Weak / Normal encounters
// ────────────────────────────────────────────────────────────────────────────

/// `CorpseSlug` — WHIP_SLAP → GLOMP → GOOP → WHIP_SLAP. `starter_idx`
/// rotates the initial state (0=WHIP_SLAP, 1=GLOMP, 2=GOOP).
pub fn create_corpse_slug_ai(starter_idx: u8) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "WHIP_SLAP",
        mv(
            "WHIP_SLAP",
            smallvec![Intent::MultiAttack { damage: 3, hits: 2 }],
            "GLOMP",
        ),
    );
    // Ravenous devour stun. C# has no static stunned state — `RavenousPower.
    // AfterDeath` (RavenousPower.cs:24-36) calls `CreatureCmd.Stun(Owner,
    // StunnedMove)` and `Creature.StunInternal` synthesizes a
    // `MoveState("STUNNED", …, StunIntent)` whose FollowUp (null nextMoveId)
    // is `StateLog.Last().Id` — the interrupted telegraph, re-performed after
    // the stun turn. The prebuilt state's static follow-up is a defensive
    // WHIP_SLAP; the Ravenous arm queues the real resume via
    // `Effect::ForceMonsterNextMove`.
    s.insert(
        "STUNNED",
        mv("STUNNED", smallvec![Intent::Stun], "WHIP_SLAP"),
    );
    s.insert(
        "GLOMP",
        mv("GLOMP", smallvec![Intent::Attack { damage: 8 }], "GOOP"),
    );
    s.insert(
        "GOOP",
        mv_eff(
            "GOOP",
            smallvec![Intent::Debuff {
                kind: PowerKind::Frail,
                amount: 2
            }],
            "WHIP_SLAP",
            corpse_slug_goop_effect,
        ),
    );
    let initial = match starter_idx {
        1 => "GLOMP",
        2 => "GOOP",
        _ => "WHIP_SLAP",
    };
    MonsterAi::new(s, initial)
}

/// `Seapunk` BUBBLE_BURP → `GainBlock(7)` + `PowerCmd.Apply<StrengthPower>(1m)`
/// (Seapunk.cs `BubbleBurpMove`, Asc-0 `BubbleStr` = 1). BUBBLE_BURP's
/// intents are `[Defend { block: 7 }, Buff { Strength }]`; the **primary**
/// `Intent::Defend` is applied by `perform_intent` (bead `sts2-rs-qk2du`
/// gains block for a move's `intents.first()`), so this effect_fn applies
/// ONLY the secondary `Strength(1)`. Do NOT add a block-gain here — that
/// would double the 7 block.
fn seapunk_bubble_burp_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    // The block lives HERE, not on the telegraph. C# `BubbleBurpMove`
    // (`Seapunk.cs:84-90`) gains `BubbleBlock` and then applies `BubbleStr`,
    // while the move's intents are declared `(BuffIntent, DefendIntent)` —
    // Buff FIRST (`Seapunk.cs:56`). Only a move's primary intent reaches
    // `perform_intent`, so with the C#-correct order the head is `Buff` and
    // the engine's head-`Defend` block arm never fires. Leaving the block on
    // the telegraph instead meant a mirror rebuilt from a real wire (whose
    // head is Buff) silently dropped 7 block — the `Seapunk block +7` class
    // in all four 2026-08-03 live A1 corpora, always one round after the
    // intent divergence that caused it.
    let block = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::ToughEnemies,
        8,
        7,
    );
    sts2_core::hooks::apply_effects(
        state,
        &[sts2_core::hooks::Effect::GainBlockCreature {
            target: sts2_core::hooks::CreatureRef::Enemy(owner),
            amount: block,
        }],
    );
    let amount = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::DeadlyEnemies,
        2,
        1,
    );
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// `Seapunk` — cyclic SEA_KICK → SPINNING_KICK → BUBBLE_BURP. Mirrors C#
/// `../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.Models.Monsters/Seapunk.cs`
/// `GenerateMoveStateMachine`: SEA_KICK = `SingleAttackIntent(11)`;
/// SPINNING_KICK = `MultiAttackIntent(2, 4)`; BUBBLE_BURP =
/// `(BuffIntent, DefendIntent)` in that order (`Seapunk.cs:56`) with the
/// body granting `BubbleBlock` then `BubbleStr` (`:84-90`). Bead
/// `sts2-rs-29kfv`: replaces the fabricated JAB/POISON_SPIT moveset, which
/// has no basis in the C# decomp (Seapunk has no Poison move).
pub fn create_seapunk_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SEA_KICK",
        mv(
            "SEA_KICK",
            smallvec![Intent::Attack { damage: 11 }],
            "SPINNING_KICK",
        ),
    );
    s.insert(
        "SPINNING_KICK",
        mv(
            "SPINNING_KICK",
            smallvec![Intent::MultiAttack { damage: 2, hits: 4 }],
            "BUBBLE_BURP",
        ),
    );
    s.insert(
        "BUBBLE_BURP",
        mv_eff(
            "BUBBLE_BURP",
            // C# order, and it is observable: `Seapunk.cs:56` declares
            // `MoveState("BUBBLE_BURP_MOVE", BubbleBurpMove, new BuffIntent(),
            // new DefendIntent())` — Buff is the HEAD. Both wire producers
            // ship only the head intent, so the live game telegraphs `Buff`
            // where this shipped `Defend`; 20 records across all four
            // 2026-08-03 live A1 corpora.
            smallvec![
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 1
                },
                Intent::Defend { block: 7 }
            ],
            "SEA_KICK",
            seapunk_bubble_burp_effect,
        ),
    );
    MonsterAi::new(s, "SEA_KICK")
}

/// `SludgeSpinner` — OIL_SPRAY (init) → RAND(OIL_SPRAY | SLAM | RAGE,
/// CannotRepeat). Rebased to C# v0.103.2
/// (`MegaCrit.Sts2.Core.Models.Monsters/SludgeSpinner.cs`): the prior
/// TOXIC_SPRAY/SLUDGE_BALL self-loop was fabricated. C# state machine starts
/// on `OIL_SPRAY_MOVE`; every move's follow-up is the shared `RAND` branch
/// (three `CannotRepeat` branches, equal weight). Bead `sts2-rs-29kfv`.
///
/// - OIL_SPRAY: `SingleAttackIntent(8)` + `DebuffIntent` → 8 dmg + Weak 1.
/// - SLAM:      `SingleAttackIntent(11)`            → 11 dmg.
/// - RAGE:      `SingleAttackIntent(6)` + `BuffIntent` → 6 dmg + Strength 3
///   (self).
///
/// HP (37–39 non-asc) lives in `sts2-core` `monsters.rs`, not this moveset
/// file; tracked separately under `sts2-rs-29kfv`.
pub fn create_sludge_spinner_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            branches: smallvec![
                br("OIL_SPRAY", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("SLAM", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("RAGE", MoveRepeatType::CannotRepeat, 1, 1.0),
            ],
        }),
    );
    s.insert(
        "OIL_SPRAY",
        mv_eff(
            "OIL_SPRAY",
            smallvec![
                Intent::Attack { damage: 8 },
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 1
                }
            ],
            "RAND",
            sludge_spinner_oil_spray_effect,
        ),
    );
    s.insert(
        "SLAM",
        mv("SLAM", smallvec![Intent::Attack { damage: 11 }], "RAND"),
    );
    s.insert(
        "RAGE",
        mv_eff(
            "RAGE",
            smallvec![
                Intent::Attack { damage: 6 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 3
                }
            ],
            "RAND",
            sludge_spinner_rage_effect,
        ),
    );
    MonsterAi::new(s, "OIL_SPRAY")
}

/// `Toadpole` — conditional init on `IsFront`, then a 3-move cycle:
/// SPIKEN (Thorns +2 to self) → SPIKE_SPIT (Thorns −2 to self, then 3×3
/// attack) → WHIRL (single 7) → SPIKEN. Mirrors C#
/// `Toadpole.GenerateMoveStateMachine`
/// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/Toadpole.cs`):
/// the `ConditionalBranchState` initial picks `WHIRL_MOVE` when `!IsFront`
/// and `SPIKEN_MOVE` when `IsFront`. `ToadpolesWeak` spawns two — index 0
/// `IsFront = true`, index 1 `IsFront = false`. Non-ascension values:
/// `SpikeSpit MultiAttack(3, 3)`, `Whirl 7`, `Spiken Thorns 2`. The prior
/// port was a fabricated TONGUE(5)/CHOMP(9) RandomBranch with no Thorns
/// (bead `sts2-rs-pn8mr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToadpoleSlot {
    First,
    Second,
    Other,
}

const TOADPOLE_THORNS: i32 = 2;

/// `SpikenMove` body — `PowerCmd.Apply<ThornsPower>(self, +2)`.
fn toadpole_spiken_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(
        state,
        owner,
        PowerKind::Thorns,
        TOADPOLE_THORNS,
    );
}

/// `SpikeSpitMove` body — `PowerCmd.Apply<ThornsPower>(self, -2)` (consumes
/// the Thorns the preceding SPIKEN granted) before the 3×3 attack. The
/// attack damage lands in `perform_intent` ahead of this effect_fn; the
/// Thorns decrement is on SELF and never interacts with the player-bound
/// hits, so the order is observationally identical to C#.
fn toadpole_spike_spit_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    decrement_creature_power(state, owner, PowerKind::Thorns, TOADPOLE_THORNS);
}

/// Subtract `amount` (>0) from a creature's `kind` power, clamping at 0 and
/// removing the instance at 0 — the decrement analogue of
/// `card_effects::apply_power_to_creature` (which early-returns on
/// non-positive amounts). Mirrors C# `PowerCmd.Apply<Power>(creature,
/// -amount)` self-decrements.
fn decrement_creature_power(
    state: &mut CombatState,
    owner: sts2_core::CreatureUid,
    kind: PowerKind,
    amount: i32,
) {
    let power_uids = match state.creatures.iter().find(|c| c.id == owner.0) {
        Some(c) => c.powers.clone(),
        None => return,
    };
    let pid = power_uids.iter().copied().find_map(|uid| {
        state
            .powers
            .get_by_id(uid.0)
            .filter(|p| p.kind == kind && p.amount > 0)
            .map(|_| uid.0)
    });
    let Some(pid) = pid else {
        return;
    };
    let mut remove = false;
    if let Some(p) = state.powers.get_mut_by_id(pid) {
        p.amount = (p.amount - amount).max(0);
        remove = p.amount == 0;
    }
    if remove {
        state.powers.retain(|p| p.id != pid);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != pid);
        }
    }
}

pub fn create_toadpole_ai(slot: ToadpoleSlot) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SPIKE_SPIT_MOVE",
        mv_eff(
            "SPIKE_SPIT_MOVE",
            smallvec![Intent::MultiAttack { damage: 3, hits: 3 }],
            "WHIRL_MOVE",
            toadpole_spike_spit_effect,
        ),
    );
    s.insert(
        "WHIRL_MOVE",
        mv(
            "WHIRL_MOVE",
            smallvec![Intent::Attack { damage: 7 }],
            "SPIKEN_MOVE",
        ),
    );
    s.insert(
        "SPIKEN_MOVE",
        mv_eff(
            "SPIKEN_MOVE",
            smallvec![Intent::Buff {
                kind: PowerKind::Thorns,
                amount: TOADPOLE_THORNS
            }],
            "SPIKE_SPIT_MOVE",
            toadpole_spiken_effect,
        ),
    );
    // C# `ConditionalBranchState`: `!IsFront → WHIRL_MOVE`,
    // `IsFront → SPIKEN_MOVE`. IsFront is static (set at spawn), so the
    // branch resolves once at construction from the spawn slot.
    let initial = match slot {
        ToadpoleSlot::First => "SPIKEN_MOVE",
        ToadpoleSlot::Second | ToadpoleSlot::Other => "WHIRL_MOVE",
    };
    MonsterAi::new(s, initial)
}

/// `CalcifiedCultist` — INCANTATION → DARK_STRIKE (self-loop).
pub fn create_calcified_cultist_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "INCANTATION",
        mv_eff(
            "INCANTATION",
            smallvec![Intent::Buff {
                kind: PowerKind::Ritual,
                amount: 2
            }],
            "DARK_STRIKE",
            calcified_cultist_incantation_effect,
        ),
    );
    s.insert(
        "DARK_STRIKE",
        mv(
            "DARK_STRIKE",
            smallvec![Intent::Attack { damage: 9 }],
            "DARK_STRIKE",
        ),
    );
    MonsterAi::new(s, "INCANTATION")
}

/// `DampCultist` — INCANTATION (Ritual+5) → DARK_STRIKE (1 dmg
/// self-loop).
pub fn create_damp_cultist_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "INCANTATION",
        mv_eff(
            "INCANTATION",
            smallvec![Intent::Buff {
                kind: PowerKind::Ritual,
                amount: 5
            }],
            "DARK_STRIKE",
            damp_cultist_incantation_effect,
        ),
    );
    s.insert(
        "DARK_STRIKE",
        mv(
            "DARK_STRIKE",
            smallvec![Intent::Attack { damage: 1 }],
            "DARK_STRIKE",
        ),
    );
    MonsterAi::new(s, "INCANTATION")
}

/// `FossilStalker` — RAND(LATCH | TACKLE | LASH, equal weight, each
/// repeatable at most twice consecutively). C# `FossilStalker.cs`:
/// `AddBranch(LATCH, 2)` / `AddBranch(TACKLE, 2)` / `AddBranch(LASH, 2)`
/// — the `2` is the `maxRepeats` overload (`CanRepeatXTimes(2)`, weight
/// defaults to 1f), NOT a weight. Equal weights make the magnitude
/// irrelevant to selection, but `CanRepeatForever` would drop the
/// consecutive-repeat cap. Bead `sts2-rs-pp5ld`. Initial state matches
/// Python `LATCH`.
pub fn create_fossil_stalker_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            branches: smallvec![
                br("LATCH", MoveRepeatType::CanRepeatXTimes, 2, 1.0),
                br("TACKLE", MoveRepeatType::CanRepeatXTimes, 2, 1.0),
                br("LASH", MoveRepeatType::CanRepeatXTimes, 2, 1.0),
            ],
        }),
    );
    s.insert(
        "TACKLE",
        mv_eff(
            "TACKLE",
            smallvec![
                Intent::Attack { damage: 9 },
                Intent::Debuff {
                    kind: PowerKind::Frail,
                    amount: 1
                }
            ],
            "RAND",
            fossil_stalker_tackle_effect,
        ),
    );
    s.insert(
        "LATCH",
        mv("LATCH", smallvec![Intent::Attack { damage: 12 }], "RAND"),
    );
    s.insert(
        "LASH",
        mv(
            "LASH",
            smallvec![Intent::MultiAttack { damage: 3, hits: 2 }],
            "RAND",
        ),
    );
    MonsterAi::new(s, "LATCH")
}

/// `SneakyGremlin` — SPAWNED (Stun, wake-up) → TACKLE (SingleAttack 9,
/// self-loop). GremlinMerc-spawned ally. Mirrors C#
/// `SneakyGremlin.GenerateMoveStateMachine`
/// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/SneakyGremlin.cs`):
/// the spawn turn telegraphs `StunIntent` and `SpawnedMove` only sets
/// `IsAwake` (no damage — cosmetic in headless), then `TackleMove`
/// (`SingleAttackIntent(TackleDamage = 9)`) loops on itself
/// (`moveState2.FollowUpState = moveState2`). The prior port was a
/// fabricated `STAB 8` self-loop with no spawn-stun turn (bead
/// `sts2-rs-kr7r7`). Non-ascension `TackleDamage = 9` (Deadly = 10).
///
/// Summoned by the GremlinMerc's `SurprisePower`, so a clone of a per-thread
/// prototype (`proto_ai`).
pub fn create_sneaky_gremlin_ai() -> MonsterAi {
    thread_local! {
        static PROTO: ProtoAis<1> = const { proto_ais() };
    }
    proto_ai(&PROTO, 0, build_sneaky_gremlin_ai)
}

pub(crate) fn build_sneaky_gremlin_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert("SPAWNED", mv("SPAWNED", smallvec![Intent::Stun], "TACKLE"));
    s.insert(
        "TACKLE",
        mv("TACKLE", smallvec![Intent::Attack { damage: 9 }], "TACKLE"),
    );
    MonsterAi::new(s, "SPAWNED")
}

/// `FatGremlin` — SPAWNED (Stun) → FLEE (Escape, self-loop). FLEE
/// flees combat (C# `FatGremlin.FleeMove`: `CreatureCmd.Escape(base.Creature)`,
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/FatGremlin.cs`).
/// `perform_intent` handles removal by zeroing the fleeing creature's
/// HP — Rust keeps corpses (see `feedback_dead_enemy_array_removal_divergence`).
/// Bead `sts2-rs-8f336`.
///
/// Summoned by the GremlinMerc's `SurprisePower`, so a clone of a per-thread
/// prototype (`proto_ai`).
pub fn create_fat_gremlin_ai() -> MonsterAi {
    thread_local! {
        static PROTO: ProtoAis<1> = const { proto_ais() };
    }
    proto_ai(&PROTO, 0, build_fat_gremlin_ai)
}

pub(crate) fn build_fat_gremlin_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert("SPAWNED", mv("SPAWNED", smallvec![Intent::Stun], "FLEE"));
    s.insert("FLEE", mv("FLEE", smallvec![Intent::Escape], "FLEE"));
    MonsterAi::new(s, "SPAWNED")
}

/// `SurprisePower.AfterDeath` — the GremlinMerc's second phase.
///
/// C# `MegaCrit.Sts2.Core.Models.Powers/SurprisePower.cs:18-43`:
/// [decompiled C# omitted — see the cited path]
///
/// `GremlinMercNormal.GenerateMonsters` (`GremlinMercNormal.cs:46-49`) spawns
/// ONLY the merc — the SneakyGremlin and FatGremlin exist solely through this
/// hook. Without it the engine reported a VICTORY the instant the merc dropped
/// while the real game was still in `PlayerPlayPhase` with both gremlins alive
/// (mirror corpus records 71 / 218), so every rollout that killed the merc was
/// scored as a guaranteed win.
///
/// The `ShouldStopCombatFromEnding() => true` contract is modelled by the
/// existing [`sts2_core::enemy_death_effects`] mechanism (the same one
/// `InfestedPower` / `StockPower` use): the handler is fired from the
/// `just_died` chokepoint via `fire_for_owner` (C# runs `AfterDeath` inside the
/// death pipeline, not at the combat-end check), and `check_combat_end`'s
/// all-enemies-dead branch re-checks for live enemies after
/// `fire_pending` — the two freshly-spawned gremlins keep the combat open. C#'s
/// vote expires the same way: `CombatState.IterateHookListeners` walks
/// `_enemies`, and the merc corpse is removed right after `AfterDeath`, so once
/// the gremlins die/flee combat resolves normally.
///
/// **Spawn order / HP roll.** C# adds `sneaky` then `fat`; both slots follow
/// `merc` in `GremlinMercNormal`, so plain `spawn_enemy` (append) reproduces the
/// observed order (`[merc-corpse, sneaky, fat]`). `spawn_enemy` recomputes the
/// unique-HP exclusion set from the LIVING enemies before each roll, so the
/// sneaky's rolled MaxHp is correctly excluded from the fat gremlin's
/// overlapping 13..=17 range; the dying merc is excluded, which is identity here
/// because its 47..=53 band cannot collide with either gremlin's.
///
/// **Thievery → Heist.** The transferred amount is C#'s
/// `ThieveryPower.DynamicVars.Gold` — the gold ACTUALLY stolen so far, not the
/// power's `Amount` (the per-steal cap, 20). The Rust analogue of that
/// accumulator is the per-instance [`sts2_core::Power::counter`] scratch slot.
/// `PowerCmd.Apply` early-returns on `amount == 0m` (`PowerCmd.cs:103`), so a
/// merc that stole nothing grants NO HeistPower — the `> 0` guard below is
/// exact. `ThieveryPower.Steal()` is now ported ([`thievery_steal`], called
/// from each of the merc's three move bodies per
/// `GremlinMerc.cs:88/104/119`), so the counter is a real running total and
/// this leg fires in a pure-sim combat — it previously only ever ran when a
/// reconstructed mirror state happened to carry a non-zero counter (GH #368.4).
///
/// `GremlinMercNormal.MarkGoldStolen()` (which feeds
/// `CalculateGoldProportion`, mirrored by
/// `crate::run_bridge::gremlin_merc_gold_proportion`) is likewise not wired —
/// the engine has no per-encounter mutable model to set the flag on.
///
/// Registered into [`sts2_core::enemy_death_effects`] (keyed by
/// `PowerKind::Surprise`) via [`crate::cards::dispatch::install_default_registry`].
pub fn surprise_after_death_spawn(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    use sts2_core::MonsterKind;

    // C# `CombatManager.AfterCreatureAdded` (`CombatManager.cs:689-696`) rolls
    // a mid-combat-added enemy's move ONLY when `CurrentSide == Player`; on an
    // enemy-turn death `MonsterModel.SpawnedThisTurn` also makes the fresh
    // creature skip `PerformMove` for the turn it spawned. Mirrors the Axebot
    // respawn telegraph rule in `act3_glory::axebot_stock_after_death`.
    let telegraph = state.side == sts2_core::Side::Player;

    for (kind, ai) in [
        (MonsterKind::SneakyGremlin, create_sneaky_gremlin_ai()),
        (MonsterKind::FatGremlin, create_fat_gremlin_ai()),
    ] {
        let ai = crate::ascension_scaling::summoned_ai(state, kind, ai);
        let uid = state.spawn_enemy(kind);
        let initial_intent = if telegraph {
            ai.current_move().intents.first().copied()
        } else {
            None
        };
        state.set_creature_ai(uid, ai);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.intent = initial_intent;
        }
        // The FatGremlin is C#'s `fatGremlin` local — the Heist target.
        if kind == MonsterKind::FatGremlin {
            transfer_thievery_gold_as_heist(state, owner, uid);
        }
    }
}

/// Move each `ThieveryPower` instance's accumulated stolen gold off the dying
/// GremlinMerc and onto the freshly-spawned FatGremlin as `HeistPower`
/// (`SurprisePower.cs:24-36`). See [`surprise_after_death_spawn`] for why the
/// amount is the per-instance counter (C# `DynamicVars.Gold`) rather than the
/// power's `Amount`.
fn transfer_thievery_gold_as_heist(
    state: &mut CombatState,
    owner: sts2_core::CreatureUid,
    fat_gremlin: sts2_core::CreatureUid,
) {
    let stolen: Vec<i32> = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .map(|c| {
            c.powers
                .iter()
                .filter_map(|puid| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == puid.0 && p.kind == PowerKind::Thievery)
                        .map(|p| p.counter)
                })
                .collect()
        })
        .unwrap_or_default();
    for amount in stolen {
        // C# `PowerCmd.Apply` early-returns on `amount == 0m` (PowerCmd.cs:103).
        if amount > 0 {
            sts2_core::card_effects::apply_power_to_creature(
                state,
                fat_gremlin,
                PowerKind::Heist,
                amount,
            );
        }
    }
}

/// `GremlinMerc` — GIMME → DOUBLE_SMASH → HEHE → GIMME.
pub fn create_gremlin_merc_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "GIMME",
        mv_eff(
            "GIMME",
            smallvec![Intent::MultiAttack { damage: 7, hits: 2 }],
            "DOUBLE_SMASH",
            gremlin_merc_gimme_effect,
        ),
    );
    s.insert(
        "DOUBLE_SMASH",
        mv_eff(
            "DOUBLE_SMASH",
            smallvec![
                Intent::MultiAttack { damage: 6, hits: 2 },
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 2
                }
            ],
            "HEHE",
            gremlin_merc_double_smash_effect,
        ),
    );
    s.insert(
        "HEHE",
        mv_eff(
            "HEHE",
            smallvec![
                Intent::Attack { damage: 8 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2
                }
            ],
            "GIMME",
            gremlin_merc_hehe_effect,
        ),
    );
    MonsterAi::new(s, "GIMME")
}

/// `HauntedShip` HAUNT (v0.107.1) applies Weak 3 to the player and adds 5
/// Dazed cards to the player's discard pile. Decomp
/// `v0.107.1/…/HauntedShip.cs:72-81` `HauntMove`:
/// `PowerCmd.Apply<WeakPower>(targets, 3m, …)` then
/// `CardPileCmd.AddToCombatAndPreview<Dazed>(targets, PileType.Discard, 5, …)`.
/// `…wb8lk.11`: v0.107.1 moved the Weak debuff onto HAUNT (3 stacks, turn 1)
/// and bumped it from 1 (the removed RAMMING_SPEED's Weak) to 3. The
/// `Intent::Weak`/`Intent::StatusCard` telegraphs are informational; this
/// effect lands both (matching the EyeWithTeeth DISTRACT idiom).
fn haunted_ship_haunt_effect(state: &mut CombatState, _owner: sts2_core::CreatureUid) {
    use sts2_core::card_effects::PileKind;
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 3);
    for _ in 0..5 {
        let _ = super::super::cards::status::spawn_dazed(state, PileKind::Discard);
    }
}

/// `HauntedShip` (v0.107.1) — HAUNT (initial) → SWIPE → STOMP → SWIPE → …
///
/// `…wb8lk.11`: v0.107.1 rework (decomp `v0.107.1/…/HauntedShip.cs:39-51`).
/// RAMMING_SPEED was removed and the round-parity RandomBranch gate replaced
/// by a plain linear chain:
///   - HAUNT_MOVE (initial, fires once on turn 1): apply Weak 3 + add 5 Dazed
///     to the player's discard. → SWIPE_MOVE.
///   - SWIPE_MOVE: Attack 13 (A9 DeadlyEnemies 14). → STOMP_MOVE.
///   - STOMP_MOVE: MultiAttack 4 (A9 5) × 3. → SWIPE_MOVE.
/// After the turn-1 HAUNT, the ship alternates SWIPE ↔ STOMP forever; HAUNT
/// does not recur. This draws no per-turn RNG (no RandomBranch), matching the
/// deterministic C# follow-up graph. Asc-0 base damages live here; A9
/// `DeadlyEnemies` scaling is applied engine-side.
pub fn create_haunted_ship_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "HAUNT_MOVE",
        mv_eff(
            "HAUNT_MOVE",
            smallvec![
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 3
                },
                Intent::StatusCard
            ],
            "SWIPE_MOVE",
            haunted_ship_haunt_effect,
        ),
    );
    s.insert(
        "SWIPE_MOVE",
        mv(
            "SWIPE_MOVE",
            smallvec![Intent::Attack { damage: 13 }],
            "STOMP_MOVE",
        ),
    );
    s.insert(
        "STOMP_MOVE",
        mv(
            "STOMP_MOVE",
            smallvec![Intent::MultiAttack { damage: 4, hits: 3 }],
            "SWIPE_MOVE",
        ),
    );
    MonsterAi::new(s, "HAUNT_MOVE")
}

/// `GasBomb` — EXPLODE self-loop. LivingFog-spawned ally; EXPLODE
/// telegraphs `Intent::DeathBlow { damage: 8 }`, which deals the hit
/// and then kills the bomb (C# `GasBomb.ExplodeMove`:
/// `DamageCmd.Attack(ExplodeDamage)` → `CreatureCmd.Kill(base.Creature)`,
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/GasBomb.cs`).
/// Bead `sts2-rs-f99e7`.
///
/// LivingFog summons up to five per combat, so each is a clone of a
/// per-thread prototype (`proto_ai`).
pub fn create_gas_bomb_ai() -> MonsterAi {
    thread_local! {
        static PROTO: ProtoAis<1> = const { proto_ais() };
    }
    proto_ai(&PROTO, 0, build_gas_bomb_ai)
}

pub(crate) fn build_gas_bomb_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "EXPLODE",
        mv(
            "EXPLODE",
            smallvec![Intent::DeathBlow { damage: 8 }],
            "EXPLODE",
        ),
    );
    MonsterAi::new(s, "EXPLODE")
}

/// `LivingFog` — SPAWN (`Intent::Summon`) ↔ ENGULF. Bead
/// `sts2-rs-r9pl` swapped SPAWN's telegraph from `Intent::Unknown`
/// to `Intent::Summon` (variant landed by `sts2-rs-47p5`).
/// ADVANCED_GAS: `SmoggyPower(1)` on the player (C# `CardDebuffIntent` →
/// `PowerCmd.Apply<SmoggyPower>(targets, 1m)`), alongside the 8-damage
/// attack. The Smoggy debuff lands here; the attack lands via the
/// `Intent::Attack` telegraph.
fn living_fog_advanced_gas_effect(state: &mut CombatState, _owner: sts2_core::CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Smoggy, 1);
}

/// Encounter slots available to LivingFog's spawns.
///
/// `LivingFogNormal.Slots = ["bomb1".."bomb5", "livingFog"]` — five bomb
/// slots, and they are never recycled, so it is five bombs per COMBAT rather
/// than five at a time.
///
/// This is the documented exception to the shared slot model (GH #552), which
/// treats a corpse's slot as free. `GetNextSlot` tests
/// `combatState.Enemies.All(c => c.SlotName != s)` (`EncounterModel.cs:247`),
/// and a dead GasBomb is still IN `Enemies`: `CreatureCmd`'s removal is
/// guarded by `!monster.IsPerformingMove` (`CreatureCmd.cs:525-529`) and a
/// bomb dies inside its own EXPLODE. Monsters the PLAYER kills (Two-Tailed
/// Rats, Ovicopter eggs) are not performing a move, so they ARE removed and
/// their slots do recycle — which is why those two use
/// `CombatState::last_free_enemy_slot` and this does not.
const LIVING_FOG_BOMB_SLOTS: usize = 5;

/// BLOAT: spawn one `GasBomb` (C# `BloatAmount = 1`) running its own EXPLODE
/// moveset, alongside the 5-damage attack.
///
/// The spawn takes the summoner's slot position, it does NOT append (GH #366).
/// The bomb slots all precede `"livingFog"` in the encounter's `Slots` list,
/// and `CombatState.AddCreature` re-runs `SortEnemiesBySlotName`
/// (`CombatState.cs:678-684`) on every add — so a spawned bomb lands *ahead of
/// its summoner*. That list is the turn order (`CombatManager.cs:1075-1084`)
/// and it is also the bridge's `target_index` space, so appending was not
/// cosmetic: it inverted who acts first. Measured on all four 2026-08-03 live
/// A1 corpora as 8 pure-permutation records, world `["GasBomb#0","LivingFog#0"]`
/// vs mirror reversed with `$.encounter.roster` the only diverging path.
///
/// Inserting at the OWNER's index rather than a hard-coded 0 is what makes
/// this right for the second and later bombs too: each new bomb takes the
/// next free bomb slot, which is still left of the fog but right of every
/// bomb already placed — exactly where the owner sits once the earlier bombs
/// are in front of it.
///
/// C# spawns just before the attack; the effect_fn fires post-damage, which is
/// state-equivalent here (the spawn does not alter the 5-damage hit).
fn living_fog_bloat_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    // `GetNextSlot` returning "" makes `BloatMove` skip the spawn entirely
    // (`LivingFog.cs:91-97`). Counts the dead, per the slot-retention note on
    // `LIVING_FOG_BOMB_SLOTS`.
    let bombs_placed = state
        .enemy_uids
        .iter()
        .filter(|u| {
            state
                .creatures
                .iter()
                .find(|c| c.id == u.0)
                .and_then(|c| c.species)
                == Some(sts2_core::MonsterKind::GasBomb)
        })
        .count();
    if bombs_placed >= LIVING_FOG_BOMB_SLOTS {
        return;
    }
    let at = state
        .enemy_uids
        .iter()
        .position(|&u| u == owner)
        .unwrap_or(state.enemy_uids.len());
    let uid = state.spawn_enemy_at_index(sts2_core::MonsterKind::GasBomb, at);
    let ai = crate::ascension_scaling::summoned_ai(
        state,
        sts2_core::MonsterKind::GasBomb,
        create_gas_bomb_ai(),
    );
    state.set_creature_ai(uid, ai);
}

/// `LivingFog` — ADVANCED_GAS (init) → BLOAT → SUPER_GAS_BLAST → BLOAT …
///
/// Rebased to C# v0.103.2 `LivingFog.cs` (source of truth). The prior
/// SPAWN/ENGULF Rust moveset was fabricated: its `Intent::Summon` was a
/// plain `mv()` that spawned nothing, and ENGULF's `Attack(10)+Weak(1)`
/// does not exist in the decomp. Non-ascension constants: AdvancedGasDamage
/// 8, BloatDamage 5, SuperGasBlastDamage 8, BloatAmount 1.
/// - ADVANCED_GAS: Attack 8 + SmoggyPower(1) on player (`CardDebuffIntent`).
/// - BLOAT:        Attack 5 + spawn 1 GasBomb (`SummonIntent`).
/// - SUPER_GAS_BLAST: Attack 8 (no rider).
/// Initial is ADVANCED_GAS, performed once; thereafter BLOAT ↔
/// SUPER_GAS_BLAST loop forever. Bead `sts2-rs-1sspw` (29kfv follow-up).
pub fn create_living_fog_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "ADVANCED_GAS",
        mv_eff(
            "ADVANCED_GAS",
            smallvec![
                Intent::Attack { damage: 8 },
                Intent::Debuff {
                    kind: PowerKind::Smoggy,
                    amount: 1
                }
            ],
            "BLOAT",
            living_fog_advanced_gas_effect,
        ),
    );
    s.insert(
        "BLOAT",
        mv_eff(
            "BLOAT",
            smallvec![Intent::Attack { damage: 5 }, Intent::Summon],
            "SUPER_GAS_BLAST",
            living_fog_bloat_effect,
        ),
    );
    s.insert(
        "SUPER_GAS_BLAST",
        mv(
            "SUPER_GAS_BLAST",
            smallvec![Intent::Attack { damage: 8 }],
            "BLOAT",
        ),
    );
    MonsterAi::new(s, "ADVANCED_GAS")
}

/// `PunchConstruct` (v0.107.1) — READY → FAST_PUNCH → STRONG_PUNCH → READY.
///
/// `…wb8lk.11`: v0.107.1 rework (decomp `v0.107.1/…/PunchConstruct.cs:81-93`):
/// READY now follows up into FAST_PUNCH (was STRONG_PUNCH), FAST_PUNCH into
/// STRONG_PUNCH (was READY), and FAST_PUNCH applies **Frail** instead of Weak.
/// The starter flag became `StartsWithFastPunch` (was `StartsWithStrongPunch`)
/// — `starts_with_fast_punch=true` opens on FAST_PUNCH. Damages
/// (StrongPunch 14/16, FastPunch 5/6 ×2) are unchanged.
pub fn create_punch_construct_ai(starts_with_fast_punch: bool) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "READY",
        mv(
            "READY",
            smallvec![Intent::Defend { block: 10 }],
            "FAST_PUNCH",
        ),
    );
    s.insert(
        "FAST_PUNCH",
        mv_eff(
            "FAST_PUNCH",
            smallvec![
                Intent::MultiAttack { damage: 5, hits: 2 },
                Intent::Debuff {
                    kind: PowerKind::Frail,
                    amount: 1
                }
            ],
            "STRONG_PUNCH",
            punch_construct_fast_punch_effect,
        ),
    );
    s.insert(
        "STRONG_PUNCH",
        mv(
            "STRONG_PUNCH",
            smallvec![Intent::Attack { damage: 14 }],
            "READY",
        ),
    );
    let initial = if starts_with_fast_punch {
        "FAST_PUNCH"
    } else {
        "READY"
    };
    MonsterAi::new(s, initial)
}

/// `SewerClam` — JET (init) ↔ PRESSURIZE. Rebased to C# v0.103.2
/// (`MegaCrit.Sts2.Core.Models.Monsters/SewerClam.cs`): the prior
/// SHELL/SNAP/PEARL_SPIT loop was fabricated. C# state machine is a two-state
/// alternation whose initial state is `JET_MOVE` (`new
/// MonsterMoveStateMachine(list, moveState2)`), with `PRESSURIZE ↔ JET`
/// follow-ups. Bead `sts2-rs-29kfv`.
///
/// - JET:        `SingleAttackIntent(10)`     → 10 dmg.
/// - PRESSURIZE: `BuffIntent`                 → Strength 4 (self).
///
/// C# `AfterAddedToRoom` also applies `PlatingPower 8` to itself on spawn, and
/// HP is 56 (non-asc); both live in `sts2-core` `monsters.rs`
/// (`on_spawn_self_buffs` + the HP table), not this moveset file — tracked
/// separately under `sts2-rs-29kfv`.
pub fn create_sewer_clam_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "JET",
        mv(
            "JET",
            smallvec![Intent::Attack { damage: 10 }],
            "PRESSURIZE",
        ),
    );
    s.insert(
        "PRESSURIZE",
        mv_eff(
            "PRESSURIZE",
            smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 4
            }],
            "JET",
            sewer_clam_pressurize_effect,
        ),
    );
    MonsterAi::new(s, "JET")
}

/// TwoTailedRatsNormal has 5 encounter slots
/// (C# `TwoTailedRatsNormal.Slots = [first..fifth]`); `CanSummon`'s
/// `GetNextSlot` non-empty check is a free-slot test against this count.
const TWO_TAILED_RAT_SLOTS: u8 = 5;
/// C# `TwoTailedRat._callForBackupLimit` — at most 3 backups across the
/// whole fight (shared `CallForBackupCount` cap).
const TWO_TAILED_RAT_CALL_LIMIT: i32 = 3;
/// C# `TwoTailedRat._turnsUntilSummonable` initial value: a rat must
/// perform 2 moves before it becomes summon-eligible.
const TWO_TAILED_RAT_SUMMON_DELAY: i32 = 2;
/// Number of TwoTailedRats the `two_tailed_rats_normal` encounter seeds.
/// `CallForBackupCount` is derived from `(#rats) - this` (see
/// [`two_tailed_rat_can_summon`]).
const TWO_TAILED_RAT_SEED_COUNT: i32 = 3;

/// Alive enemy creatures — the free-slot denominator for `CanSummon` and
/// the re-check guard in [`two_tailed_rat_call_for_backup_effect`].
fn two_tailed_rat_alive_enemies(state: &CombatState) -> usize {
    state
        .enemy_uids
        .iter()
        .filter(|uid| {
            state
                .creatures
                .iter()
                .any(|c| c.id == uid.0 && c.current_hp > 0)
        })
        .count()
}

/// C# `TwoTailedRat.CanSummon()` (source of truth `TwoTailedRat.cs`),
/// threaded as a [`ConditionFn::CounterAware`] predicate where
/// `turns_until_summonable` is the rat's per-instance `MonsterAi::counter`
/// (the C# `TurnsUntilSummonable` field). All four C# guards:
///
/// 1. `TurnsUntilSummonable > 0` → not yet (counter starts at 2,
///    decremented by each SCRATCH / DISEASE_BITE / SCREECH move).
/// 2. `CallForBackupCount >= 3`. C# syncs that counter across every rat;
///    we instead derive it from the arena — the encounter seeds 3 rats,
///    each CALL_FOR_BACKUP adds one, and Rust retains corpses, so the
///    lifetime backup count equals `(#TwoTailedRat creatures) - 3`.
/// 3. `GetNextSlot` non-empty ⟺ a free slot ⟺ alive enemies < slot count.
/// 4. No teammate already telegraphing `CALL_FOR_BACKUP` this round — C#
///    rejects a second concurrent summon so the slot cap is never
///    overshot. Teammates roll sequentially in `enemy_turn_exec` step 4,
///    so an earlier rat's just-rolled CALL is visible here via its AI's
///    `current_state_id()`.
fn two_tailed_rat_can_summon(
    state: &CombatState,
    owner: sts2_core::CreatureUid,
    turns_until_summonable: i32,
) -> bool {
    if turns_until_summonable > 0 {
        return false;
    }
    let total_rats = state
        .creatures
        .iter()
        .filter(|c| c.species == Some(sts2_core::MonsterKind::TwoTailedRat))
        .count();
    if i32::try_from(total_rats).unwrap_or(i32::MAX) - TWO_TAILED_RAT_SEED_COUNT
        >= TWO_TAILED_RAT_CALL_LIMIT
    {
        return false;
    }
    if two_tailed_rat_alive_enemies(state) >= usize::from(TWO_TAILED_RAT_SLOTS) {
        return false;
    }
    let teammate_summoning = state.creatures.iter().any(|c| {
        c.id != owner.0
            && c.current_hp > 0
            && c.species == Some(sts2_core::MonsterKind::TwoTailedRat)
            && c.ai
                .as_ref()
                .is_some_and(|a| a.current_state_id() == "CALL_FOR_BACKUP")
    });
    !teammate_summoning
}

/// Decrement the owning rat's `TurnsUntilSummonable` (saturating at 0).
/// C# does `TurnsUntilSummonable--` at the head of every attack/screech
/// move; CALL_FOR_BACKUP does NOT tick it. The AI is present on the
/// creature during the perform walk (only `enemy_turn_exec` step 4
/// `take()`s it), so this mutation is safe from a move effect.
fn two_tailed_rat_tick_summon_timer(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    if let Some(ai) = state
        .creatures
        .iter_mut()
        .find(|c| c.id == owner.0)
        .and_then(|c| c.ai.as_mut())
    {
        let n = ai.counter();
        ai.set_counter((n - 1).max(0));
    }
}

/// C# `ScratchMove`: `TurnsUntilSummonable--` then Attack(8) (the damage
/// lands via the move's `Intent::Attack`).
fn two_tailed_rat_scratch_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    two_tailed_rat_tick_summon_timer(state, owner);
}

/// C# `DiseaseBiteMove`: `TurnsUntilSummonable--` then Attack(6).
fn two_tailed_rat_disease_bite_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    two_tailed_rat_tick_summon_timer(state, owner);
}

/// C# `ScreechMove`: `TurnsUntilSummonable--` then `Apply<FrailPower>(1)`
/// to the player. The `DebuffIntent` telegraph is no-op (per the
/// monster-buff-intent-is-telegraph-only rule), so the Frail is applied
/// here via the legacy player surface the parity harness reads.
fn two_tailed_rat_screech_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    two_tailed_rat_tick_summon_timer(state, owner);
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 1);
}

/// C# `CallForBackup`: summon another TwoTailedRat into the last free
/// slot (`CreatureCmd.Add<TwoTailedRat>`). CanSummon already gated this
/// move at roll time, but a teammate could have summoned in the same turn
/// before this perform, so we re-check the slot here. The new rat gets
/// its own full AI (initial `RAND`, fresh `TurnsUntilSummonable = 2`).
/// We do NOT tick the summon timer (C# CallForBackup omits the
/// `TurnsUntilSummonable--`). C#'s `CallForBackupCount` sync across rats
/// is implicit in our arena-derived count (see [`two_tailed_rat_can_summon`]).
fn two_tailed_rat_call_for_backup_effect(state: &mut CombatState, _owner: sts2_core::CreatureUid) {
    // C# `CallForBackup` takes `Slots.LastOrDefault(s => Enemies.All(c =>
    // c.SlotName != s))` — the HIGHEST-index FREE slot — and `AddCreature`
    // re-sorts the enemy list by slot index. `TwoTailedRatsNormal.Slots` is
    // `[first .. fifth]` with `GenerateMonsters` seeding the last three, so the
    // first summon takes "second" and lands AHEAD of its summoner; and because
    // C# drops corpses from `Enemies`, a dead rat's slot frees up and a later
    // summon can land BETWEEN survivors. Appending put the newcomer last, which
    // swapped which rat consumed which `monster_ai` draw — same read count,
    // different moves (GH #544/#552).
    //
    // `None` here is exactly C#'s empty-string `LastOrDefault` default, which
    // it guards with `if (!string.IsNullOrEmpty(nextSlot))`. It also subsumes
    // the old alive-count check: no free slot IS the full board.
    let Some(slot) = state.last_free_enemy_slot(TWO_TAILED_RAT_SLOTS) else {
        return;
    };
    // The summoned rat carries `StarterMoveIndex == -1`, so C# starts its
    // machine on the RandomBranch and its opening telegraph costs one real
    // `Rng.MonsterAi` read. That read is taken by the end-of-turn
    // `roll_next_enemy_intents` pass, in enemy order — NOT here at spawn time:
    // C# rolls the summoner's next move before the newcomer's, and resolving
    // early swaps which draw each rat gets (GH #544).
    let ai = crate::ascension_scaling::summoned_ai(
        state,
        sts2_core::MonsterKind::TwoTailedRat,
        create_two_tailed_rat_ai(TwoTailedRatSlot::Other),
    );
    state.spawn_enemy_in_slot(sts2_core::MonsterKind::TwoTailedRat, slot, ai);
}

/// Starter-move selector for [`create_two_tailed_rat_ai`], mirroring C#'s
/// `StarterMoveIndex` (`TwoTailedRat.GenerateMoveStateMachine`):
/// `0 → SCRATCH`, `1 → DISEASE_BITE`, `2 → SCREECH`, `-1 → RAND` (a fresh
/// random roll). The `two_tailed_rats_normal` encounter assigns one of each
/// concrete starter to its three rats via `build_ai`'s `position.index`
/// (the same per-slot dispatch `KinFollower` / `Inklet` use), so all three
/// telegraph distinct turn-1 moves exactly as C#'s
/// `GenerateMonsters` does. (C# rolls a random *offset* into the
/// SCRATCH/DISEASE/SCREECH rotation, but since the three rats are otherwise
/// identical — same pinned HP, same AI — that offset is a permutation of
/// indistinguishable actors with no effect on the fight; pinning it to 0 is
/// behaviorally exact, not an approximation.) Mid-combat summoned rats use
/// `Rand`, matching C# `CreatureCmd.Add<TwoTailedRat>` (default
/// `StarterMoveIndex == -1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TwoTailedRatSlot {
    /// StarterMoveIndex 0 → SCRATCH.
    First,
    /// StarterMoveIndex 1 → DISEASE_BITE.
    Second,
    /// StarterMoveIndex 2 → SCREECH.
    Third,
    /// StarterMoveIndex -1 → RAND (independent first roll; summoned rats).
    Other,
}

/// `TwoTailedRat` — C# summoner ported from `TwoTailedRat.cs` (v0.103.2,
/// source of truth), replacing the prior fabricated GNAW/TAIL_WHIP stub
/// (bead `sts2-rs-dvude`).
///
/// Moveset: SCRATCH (Attack 8) / DISEASE_BITE (Attack 6) / SCREECH
/// (Frail 1 on the player, cooldown 3) / CALL_FOR_BACKUP (summon another
/// rat, `UseOnlyOnce` per rat).
///
/// C# uses one `RandomBranchState` with `Func<float>` weight lambdas that
/// swing on `CanSummon()`: the three attack/screech branches weigh 1 each
/// normally and 1/12 each when summoning is available, and CALL_FOR_BACKUP
/// weighs 0 normally and 0.75 when available. Rust `WeightedBranch.weight`
/// is static, so we split the single dynamic branch into a
/// `ConditionalBranch` (`RAND`) that routes to one of two static
/// `RandomBranch`es. This is **RNG-identical** to C#: a `ConditionalBranch`
/// consumes zero RNG, `RAND_NORMAL` is exactly C#'s `!CanSummon` 3-way draw
/// (total 3), and `RAND_SUMMON` is exactly C#'s `CanSummon` 4-way draw
/// (total 1.0 = 1/12·3 + 0.75) — both draw a single `NextFloat(total)`
/// against the same effective weights and `state_log`.
///
/// CALL_FOR_BACKUP summons another one mid-combat, so each is a clone of a
/// per-thread prototype (`proto_ai`). The build also constructs a default
/// `CombatState` to resolve the opening state, which makes it the dearest of
/// the summon builds.
pub fn create_two_tailed_rat_ai(slot: TwoTailedRatSlot) -> MonsterAi {
    thread_local! {
        static PROTO: ProtoAis<4> = const { proto_ais() };
    }
    proto_ai(&PROTO, slot as usize, || build_two_tailed_rat_ai(slot))
}

pub(crate) fn build_two_tailed_rat_ai(slot: TwoTailedRatSlot) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "RAND",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "RAND",
            branches: smallvec![
                ConditionalBranch {
                    condition: ConditionFn::CounterAware(two_tailed_rat_can_summon),
                    state_id: "RAND_SUMMON",
                },
                cb(always_true, "RAND_NORMAL"),
            ],
        }),
    );
    // !CanSummon: SCRATCH / DISEASE_BITE / SCREECH at weight 1 each (C#
    // CALL weight 0 → omitted). SCREECH carries cooldown 3.
    s.insert(
        "RAND_NORMAL",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND_NORMAL",
            branches: smallvec![
                br("SCRATCH", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("DISEASE_BITE", MoveRepeatType::CannotRepeat, 1, 1.0),
                WeightedBranch {
                    state_id: "SCREECH",
                    repeat_type: MoveRepeatType::CannotRepeat,
                    max_times: 1,
                    weight: 1.0,
                    cooldown: 3,
                },
            ],
        }),
    );
    // CanSummon: the three attack/screech branches at 1/12 each, plus
    // CALL_FOR_BACKUP at 0.75 (UseOnlyOnce per rat) — total 1.0, matching
    // C#'s `() => 1f/12f` / `() => 0.75f` lambdas exactly.
    s.insert(
        "RAND_SUMMON",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND_SUMMON",
            branches: smallvec![
                br("SCRATCH", MoveRepeatType::CannotRepeat, 1, 1.0 / 12.0),
                br("DISEASE_BITE", MoveRepeatType::CannotRepeat, 1, 1.0 / 12.0),
                WeightedBranch {
                    state_id: "SCREECH",
                    repeat_type: MoveRepeatType::CannotRepeat,
                    max_times: 1,
                    weight: 1.0 / 12.0,
                    cooldown: 3,
                },
                br("CALL_FOR_BACKUP", MoveRepeatType::UseOnlyOnce, 1, 0.75),
            ],
        }),
    );
    s.insert(
        "SCRATCH",
        mv_eff(
            "SCRATCH",
            smallvec![Intent::Attack { damage: 8 }],
            "RAND",
            two_tailed_rat_scratch_effect,
        ),
    );
    s.insert(
        "DISEASE_BITE",
        mv_eff(
            "DISEASE_BITE",
            smallvec![Intent::Attack { damage: 6 }],
            "RAND",
            two_tailed_rat_disease_bite_effect,
        ),
    );
    s.insert(
        "SCREECH",
        mv_eff(
            "SCREECH",
            smallvec![Intent::Debuff {
                kind: PowerKind::Frail,
                amount: 1,
            }],
            "RAND",
            two_tailed_rat_screech_effect,
        ),
    );
    s.insert(
        "CALL_FOR_BACKUP",
        mv_eff(
            "CALL_FOR_BACKUP",
            smallvec![Intent::Summon],
            "RAND",
            two_tailed_rat_call_for_backup_effect,
        ),
    );
    let initial = match slot {
        TwoTailedRatSlot::First => "SCRATCH",
        TwoTailedRatSlot::Second => "DISEASE_BITE",
        TwoTailedRatSlot::Third => "SCREECH",
        TwoTailedRatSlot::Other => "RAND",
    };
    // `RAND` is a ConditionalBranch entry; `new_with_combat` resolves it
    // through the always-true RAND_NORMAL fallback (the CounterAware
    // summon branch resolves `false` with no owner set at ctor time, so a
    // fresh rat never opens on CALL_FOR_BACKUP). `set_owner` +
    // `resolve_initial_move_with_combat` re-resolve against the run RNG at
    // combat start (combat_loop.rs).
    let mut ai = MonsterAi::new_with_combat(s, initial, &CombatState::default());
    // C# `TwoTailedRat._turnsUntilSummonable = 2`.
    ai.set_counter(TWO_TAILED_RAT_SUMMON_DELAY);
    ai
}

// ────────────────────────────────────────────────────────────────────────────
// Elite encounters
// ────────────────────────────────────────────────────────────────────────────

/// Per-slot entry-move selector for [`create_phantasmal_gardener_ai`],
/// mirroring C# `PhantasmalGardener`'s `INIT_MOVE` ConditionalBranch on
/// `Creature.SlotName`: first→FLAIL, second→BITE, third→LASH,
/// fourth→ENLARGE. All four feed the same deterministic ring, so the slot
/// only sets the starting phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhantasmalGardenerSlot {
    /// SlotName "first" → FLAIL entry.
    First,
    /// SlotName "second" → BITE entry.
    Second,
    /// SlotName "third" → LASH entry.
    Third,
    /// SlotName "fourth" → ENLARGE entry.
    Fourth,
}

/// `PhantasmalGardener` — deterministic BITE → LASH → FLAIL → ENLARGE
/// cycle per the C# decomp
/// `MegaCrit.Sts2.Core.Models.Monsters/PhantasmalGardener.cs`
/// (`GenerateMoveStateMachine`). In the real game each gardener has a
/// `SlotName` ∈ {first, second, third, fourth} that picks the entry
/// move (first→FLAIL, second→BITE, third→LASH, fourth→ENLARGE); the
/// follow-up edges form a single ring so all slots converge on the
/// same BITE→LASH→FLAIL→ENLARGE→BITE cycle.
///
/// C#'s `INIT_MOVE` ConditionalBranch picks that entry from `SlotName`.
/// We take the same slot via `slot` (resolved from `build_ai`'s
/// `position.index`) and set it as the initial Move directly — a Move
/// initial consumes zero RNG, matching C#'s ConditionalBranch init. The
/// prior init-only `RAND` branch was both an approximation (random entry
/// instead of slot-keyed) and consumed an extra `MonsterAi` RNG tick.
/// Bead `sts2-rs-dvude`.
///
/// Move payloads track the C# constants verbatim:
/// - BITE → `SingleAttackIntent(BiteDamage=5)`
/// - LASH → `SingleAttackIntent(LashDamage=7)`
/// - FLAIL → `MultiAttackIntent(FlailDamage=1, FlailRepeat=3)`
/// - ENLARGE → `BuffIntent` → `StrengthPower(EnlargeStr=2)` self-buff
///
/// The C# also applies `SkittishPower(SkittishAmount=6)` on
/// `AfterAddedToRoom` (`PhantasmalGardener.cs:92`); the spawn grant lives in
/// `sts2-core::monsters::on_spawn_self_buffs`, and its reactive body is the
/// [`skittish_after_damage`] latch + the core `dispatch_after_card_played`
/// grant lane, installed once here.
pub fn create_phantasmal_gardener_ai(slot: PhantasmalGardenerSlot) -> MonsterAi {
    // Register the Skittish latch once per process — every PhantasmalGardener
    // spawn flows through this factory, so the handler is in place before any
    // combat damage event (same shape as `install_curl_up_handlers`).
    {
        use std::sync::OnceLock;
        static INSTALL_SKITTISH: OnceLock<()> = OnceLock::new();
        INSTALL_SKITTISH.get_or_init(install_skittish_handlers);
    }
    let mut s = BTreeMap::new();
    s.insert(
        "BITE",
        mv("BITE", smallvec![Intent::Attack { damage: 5 }], "LASH"),
    );
    s.insert(
        "LASH",
        mv("LASH", smallvec![Intent::Attack { damage: 7 }], "FLAIL"),
    );
    s.insert(
        "FLAIL",
        mv(
            "FLAIL",
            smallvec![Intent::MultiAttack { damage: 1, hits: 3 }],
            "ENLARGE",
        ),
    );
    s.insert(
        "ENLARGE",
        mv_eff(
            "ENLARGE",
            smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2,
            }],
            "BITE",
            phantasmal_gardener_enlarge_effect,
        ),
    );
    // C# `INIT_MOVE` slot→entry mapping; all four feed the same ring.
    let initial = match slot {
        PhantasmalGardenerSlot::First => "FLAIL",
        PhantasmalGardenerSlot::Second => "BITE",
        PhantasmalGardenerSlot::Third => "LASH",
        PhantasmalGardenerSlot::Fourth => "ENLARGE",
    };
    MonsterAi::new(s, initial)
}

/// `SkulkingColony` (v0.107.1) — ZOOM_MOVE → ZOOM_MOVE_2 → INERTIA_MOVE →
/// PIERCING_STABS_MOVE → ZOOM_MOVE.
///
/// `…wb8lk.11`: v0.107.1 rework (decomp
/// `v0.107.1/…/SkulkingColony.cs:59-75`): SMASH_MOVE was removed and a second
/// opening Zoom (ZOOM_MOVE_2) added, so the colony Zooms on turns 1–2. Zoom no
/// longer grants Block (the v0.103.2 `ZoomBlock` Defend rider is gone), so the
/// move is a plain attack and the secondary-block effect was dropped. Initial
/// state is ZOOM_MOVE. Spawn HardenedShell is 20 (was 15) and HP 75 (was 70);
/// both live in `sts2-core/monsters.rs`. `InertiaStrengthGain` stays 2 at A0
/// (decomp `GetValueIfAscension(DeadlyEnemies, 4, 2)`; the A9 bump to 4 is not
/// modelled — Strength buffs are outside the attack-scaling table). The
/// Armor→Stone `TakeDamageSfxType` change is cosmetic.
pub fn create_skulking_colony_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "ZOOM_MOVE",
        mv(
            "ZOOM_MOVE",
            smallvec![Intent::Attack { damage: 14 }],
            "ZOOM_MOVE_2",
        ),
    );
    s.insert(
        "ZOOM_MOVE_2",
        mv(
            "ZOOM_MOVE_2",
            smallvec![Intent::Attack { damage: 14 }],
            "INERTIA_MOVE",
        ),
    );
    s.insert(
        "INERTIA_MOVE",
        mv_eff(
            "INERTIA_MOVE",
            smallvec![
                Intent::Attack { damage: 9 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2,
                },
            ],
            "PIERCING_STABS_MOVE",
            skulking_colony_inertia_effect,
        ),
    );
    s.insert(
        "PIERCING_STABS_MOVE",
        mv(
            "PIERCING_STABS_MOVE",
            smallvec![Intent::MultiAttack { damage: 7, hits: 2 }],
            "ZOOM_MOVE",
        ),
    );
    MonsterAi::new(s, "ZOOM_MOVE")
}

/// `TerrorEel` Thrash self-buff: applies Vigor(6) to itself after the triple
/// attack. C# `TerrorEel.ThrashMove` →
/// `PowerCmd.Apply<VigorPower>(base.Creature, 6m, ...)` (TerrorEel.cs:108).
/// Self-buff routes through `apply_power_to_creature` (the d03 mirror) like
/// every other monster self-buff in this file. Bead `sts2-rs-7iuyk`.
fn terror_eel_thrash_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Vigor, 6);
}

/// `TerrorEel` Terror debuff: applies Vulnerable(99) to the player. C#
/// `TerrorEel.TerrorMove` → `PowerCmd.Apply<VulnerablePower>(targets, 99m, ...)`
/// (TerrorEel.cs:122). Player-debuff routes through the legacy surface so it is
/// visible to `state_to_dict` / the parity harness. Bead `sts2-rs-7iuyk`.
fn terror_eel_terror_effect(state: &mut CombatState, _owner: sts2_core::CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Vulnerable, 99);
}

/// `TerrorEel` — rebased to C# v0.103.2 (bead `sts2-rs-7iuyk`, under 29kfv;
/// was a fabricated SHOCK/COIL/SURGE chain). Per
/// `MegaCrit.Sts2.Core.Models.Monsters/TerrorEel.cs::GenerateMoveStateMachine`:
/// the normal cycle is CRASH_MOVE ↔ ThrashMove; STUN_MOVE → TERROR_MOVE →
/// CRASH_MOVE is reached only via the Shriek-driven stun trigger — the core
/// `shriek_stun_below_threshold` reaction (hooks.rs, C# ShriekPower.cs:25-33)
/// forces STUN_MOVE when unblocked damage leaves the eel at
/// `current_hp <= Shriek` and removes the power. HP (140), the opening intent, and the on-spawn ShriekPower(70)
/// live in `sts2-core/monsters.rs` (HP table + `on_spawn_self_buffs`).
/// Damage/amounts are non-ascension (identity at pin 01c094d): CrashDamage 16,
/// ThrashDamage 3 × ThrashRepeat 3, Vigor 6, Vulnerable 99.
pub fn create_terror_eel_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "CRASH_MOVE",
        mv(
            "CRASH_MOVE",
            smallvec![Intent::Attack { damage: 16 }],
            "ThrashMove",
        ),
    );
    s.insert(
        "ThrashMove",
        mv_eff(
            "ThrashMove",
            smallvec![
                Intent::MultiAttack { damage: 3, hits: 3 },
                Intent::Buff {
                    kind: PowerKind::Vigor,
                    amount: 6
                }
            ],
            "CRASH_MOVE",
            terror_eel_thrash_effect,
        ),
    );
    s.insert(
        "STUN_MOVE",
        mv("STUN_MOVE", smallvec![Intent::Stun], "TERROR_MOVE"),
    );
    s.insert(
        "TERROR_MOVE",
        mv_eff(
            "TERROR_MOVE",
            smallvec![Intent::Debuff {
                kind: PowerKind::Vulnerable,
                amount: 99
            }],
            "CRASH_MOVE",
            terror_eel_terror_effect,
        ),
    );
    MonsterAi::new(s, "CRASH_MOVE")
}

// ────────────────────────────────────────────────────────────────────────────
// Boss encounters
// ────────────────────────────────────────────────────────────────────────────

// ── WaterfallGiant (Act-? pressure boss) effect fns ──────────────────────────
//
// Bead `sts2-rs-k3pxa` (sub-scope of 29kfv) slice 1: rebase the FABRICATED
// ROAR/SLAM/CRUSH/TORRENT moveset to the C# v0.103.2 move sequence + damages
// (`MegaCrit.Sts2.Core.Models.Monsters/WaterfallGiant.cs`, the source of
// truth). Non-ascension base values:
//   PressurizeAmount=15, StompDamage=15, RamDamage=10, PressureUpDamage=13,
//   BasePressureGunDamage=20 (+5/use), SiphonHeal=15, per-move SteamEruption(+3).
// Main loop: PRESSURIZE → STOMP → RAM → SIPHON → PRESSURE_GUN → PRESSURE_UP
//            → STOMP (loops back to STOMP, not PRESSURIZE).
//
// Each move stacks `SteamEruptionPower` on self (a self-buff `PowerCmd.Apply`),
// applied here via `apply_power_to_creature` (SELF target, like GremlinMerc's
// HEHE Strength buff). SteamEruption has no runtime consumer at this pin, so
// the stacks are a faithful no-op for now.
//
// PRESSURE_GUN +5/use scaling (bead `sts2-rs-f4fbo`) is implemented in
// `waterfall_giant_pressure_gun_effect` below: C#'s
// `CurrentPressureGunDamage` starts at `BasePressureGunDamage=20`, the move
// deals it, then `CurrentPressureGunDamage += PressureGunIncrease(5)` AFTER
// the hit (`PressureGunMove`). We mirror this by mutating the stored
// `PRESSURE_GUN` `Intent::Attack { damage }` in-place by +5 in the post-damage
// effect_fn; the next turn's telegraph re-reads `current_move().intents`
// (combat_loop intent-priming), so the ramp shows up on the following
// PRESSURE_GUN exactly as in the decomp. No core infra needed — `MonsterAi`
// is per-instance and `states`/`MoveState.intents` are public.
//
// DEFERRED to follow-up sub-beads (need new infra; tracked off k3pxa):
//   - ShouldDisappearFromDoom gate (`WaterfallGiant.cs:84`).
//   - Multiplayer SiphonHeal (15 × player-count); single-player heals 15.
//
// IMPLEMENTED since: the AboutToBlow → EXPLODE finish (bead `sts2-rs-wz8qc`)
// and the death-interrupt that TRIGGERS it (`steam_eruption_death_interrupt`
// below — C# `SteamEruptionPower.AfterDeath` → `TriggerAboutToBlowState`).

/// PRESSURIZE: `SteamEruptionPower` (+15) self. C# `PressurizeAmount=15`.
fn waterfall_giant_pressurize_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    let amount = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::DeadlyEnemies,
        20,
        15,
    );
    sts2_core::card_effects::apply_power_to_creature(
        state,
        owner,
        PowerKind::SteamEruption,
        amount,
    );
}

/// STOMP: Weak(1) player + `SteamEruptionPower`(+3) self (alongside the
/// 15-damage attack). C# `StompMove`.
fn waterfall_giant_stomp_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 1);
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::SteamEruption, 3);
}

/// SteamEruption(+3) self only — shared by RAM / PRESSURE_GUN / PRESSURE_UP
/// (their attacks land via the `Intent::Attack` telegraph).
fn waterfall_giant_steam_only_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::SteamEruption, 3);
}

/// SIPHON: Heal `SiphonHeal` (×player-count in C#; single-player here) +
/// SteamEruption(+3) self. `…wb8lk.11`: v0.107.1 made SiphonHeal
/// ascension-scaled — decomp `v0.107.1/…/WaterfallGiant.cs:72`
/// `GetValueIfAscension(ToughEnemies, 15, 10)` ⇒ A0 10 / A8 15 (was a flat 15).
fn waterfall_giant_siphon_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    let heal = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::ToughEnemies,
        15,
        10,
    );
    state.creature_heal(owner, heal);
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::SteamEruption, 3);
}

/// PRESSURE_GUN: SteamEruption(+3) self, then ramp this creature's stored
/// PRESSURE_GUN attack damage by +5 (bead `sts2-rs-f4fbo`). Mirrors C#
/// `PressureGunMove`: the attack deals `CurrentPressureGunDamage` (base 20),
/// then `CurrentPressureGunDamage += PressureGunIncrease(5)`. We run AFTER
/// damage (effect_fn fires post-hit for `Intent::Attack`), so the hit just
/// landed used the pre-increment value and the next PRESSURE_GUN telegraph
/// (re-read from `current_move().intents`) shows +5. The ramp is stored on
/// this monster's own `MonsterAi`, so multiple WaterfallGiants scale
/// independently.
fn waterfall_giant_pressure_gun_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::SteamEruption, 3);
    if let Some(idx) = state.creatures.iter().position(|c| c.id == owner.0) {
        if let Some(ai) = state.creatures[idx].ai.as_mut() {
            if let Some(MonsterState::Move(m)) =
                std::sync::Arc::make_mut(&mut ai.states).get_mut("PRESSURE_GUN")
            {
                for intent in &mut m.intents {
                    if let Intent::Attack { damage } = intent {
                        *damage = damage.saturating_add(5);
                    }
                }
            }
        }
    }
}

/// WaterfallGiant EXPLODE damage = its accumulated `SteamEruptionPower`
/// amount. Mirrors C# `WaterfallGiant.cs:285`
/// (`SteamEruptionDamage = base.Creature.GetPowerAmount<SteamEruptionPower>()`),
/// captured just before the power is removed and the EXPLODE `DeathBlow`
/// is dealt. The per-move `SteamEruption` self-buffs (PRESSURIZE +15,
/// every other move +3) accumulate this total, so the explode hit scales
/// with how long the giant has pressurized — the "SteamEruption
/// scaling-damage consumer" of bead `sts2-rs-dp49t`.
///
/// Consumed by the EXPLODE move's `DeathBlowIntent(SteamEruptionDamage)`
/// (bead `sts2-rs-wz8qc`, the coupled AboutToBlow→EXPLODE state machine);
/// the C# `ShouldDisappearFromDoom => !HasPower<SteamEruptionPower>` gate
/// fires once EXPLODE removes the power (see the `DoomPower` disappear
/// path, `DoomPower.cs:75`). Returns 0 when the creature is absent or
/// carries no `SteamEruption` stacks.
#[must_use]
pub fn waterfall_giant_explode_damage(state: &CombatState, owner: sts2_core::CreatureUid) -> i32 {
    state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().find_map(|pu| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == pu.0 && p.kind == PowerKind::SteamEruption)
                    .map(|p| p.amount)
            })
        })
        .unwrap_or(0)
}

/// ABOUT_TO_BLOW: capture the giant's accumulated `SteamEruptionPower`
/// stacks as the EXPLODE-damage payload, then strip the power from the
/// creature. Mirrors C# `WaterfallGiant.AboutToBlowMove` (lines 283-289):
/// `SteamEruptionDamage = base.Creature.GetPowerAmount<SteamEruptionPower>();`
/// then `await PowerCmd.Remove<SteamEruptionPower>(base.Creature);`.
///
/// The captured value is written into THIS creature's own EXPLODE state's
/// `Intent::DeathBlow { damage }` (same in-AI rewrite shape as
/// [`waterfall_giant_pressure_gun_effect`]'s +5 ramp), so each WaterfallGiant
/// detonates with its own pressurization total — independent of any
/// other WaterfallGiant on the field. Bead `sts2-rs-wz8qc` (k3pxa.3).
fn waterfall_giant_about_to_blow_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    // Step 1 — capture the SteamEruptionPower amount on the owner. We
    // reuse the existing `waterfall_giant_explode_damage` reader, which
    // returns 0 if no stacks are present (matching C#
    // `GetPowerAmount<SteamEruptionPower>` on a creature without it).
    let captured = waterfall_giant_explode_damage(state, owner);

    // Step 2 — remove the SteamEruptionPower instance(s) from the owner.
    // The C# call is `PowerCmd.Remove<SteamEruptionPower>(base.Creature)`,
    // which detaches the single-stack power model entirely (powers of
    // `PowerStackType.Single`-equivalent semantics in Rust live as one
    // arena entry per owner+kind). Drop it from both the arena and the
    // owner's powers list.
    if let Some(idx) = state.creatures.iter().position(|c| c.id == owner.0) {
        let drop_uids: smallvec::SmallVec<[sts2_core::PowerUid; 4]> = state.creatures[idx]
            .powers
            .iter()
            .copied()
            .filter(|pu| {
                state
                    .powers
                    .iter()
                    .any(|p| p.id == pu.0 && p.kind == PowerKind::SteamEruption)
            })
            .collect();
        state.creatures[idx]
            .powers
            .retain(|pu| !drop_uids.contains(pu));
        for pu in drop_uids {
            state.powers.retain(|p| p.id != pu.0);
        }

        // Step 3 — rewrite THIS creature's EXPLODE intent damage to the
        // captured value. The state is constructed with a placeholder
        // `Intent::DeathBlow { damage: 0 }`; ABOUT_TO_BLOW must run before
        // EXPLODE per the FollowUpState wiring, so the placeholder is
        // always overwritten before EXPLODE's `perform_intent` reads it.
        if let Some(ai) = state.creatures[idx].ai.as_mut() {
            if let Some(MonsterState::Move(m)) =
                std::sync::Arc::make_mut(&mut ai.states).get_mut("EXPLODE")
            {
                for intent in &mut m.intents {
                    if let Intent::DeathBlow { damage } = intent {
                        *damage = captured;
                    }
                }
            }
        }
    }
}

/// C# `SteamEruptionPower.AfterDeath` (`SteamEruptionPower.cs:15-21`) →
/// `WaterfallGiant.TriggerAboutToBlowState` (`WaterfallGiant.cs:301-313`):
/// killing the pressurized giant does NOT win the fight — its death is
/// interrupted. `CreatureCmd.SetMaxAndCurrentHp(base.Creature, 999999999m)`
/// revives it at effectively infinite HP and `SetMoveImmediate(
/// AboutToBlowState, forceTransition: true)` forces the ABOUT_TO_BLOW
/// telegraph (Stun, `MustPerformOnceBeforeTransitioning = true`), whose
/// follow-up EXPLODE deals a DeathBlow equal to the accumulated
/// `SteamEruption` stacks and then kills the giant for real.
/// `SteamEruptionPower.ShouldStopCombatFromEnding() => true` +
/// `ShouldPowerBeRemovedAfterOwnerDeath() => false` keep the combat open
/// and the power observable through the interim.
///
/// Registered under [`PowerKind::SteamEruption`] on the
/// [`sts2_core::enemy_death_effects`] just-died chokepoint
/// (`damage.rs::apply_damage_with_final` → `fire_for_owner`), the same
/// per-death dispatch StockPower's Axebot respawn uses — so every kill
/// path (player card, thorns, poison) routes here the instant the giant
/// dies, before any `check_combat_end` can resolve a victory. Because the
/// handler REVIVES its owner, `fire_for_owner` deliberately does NOT zero
/// the triggering power (multi-revive pattern).
///
/// Pre-fix the engine resolved the giant's first death as an immediate
/// VICTORY: mirror_live2 shows the mirror declaring Victory while the
/// live game continued at hp 999999999 with the EXPLODE DeathBlow
/// telegraphed — a wrong victory gate that also poisoned every MCTS
/// rollout of this fight with a phantom no-explosion win.
fn steam_eruption_death_interrupt(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    let Some(c) = state.creatures.iter_mut().find(|c| c.id == owner.0) else {
        return;
    };
    // C# `SetMaxAndCurrentHp(999999999m)` — both max and current.
    c.max_hp = 999_999_999;
    c.current_hp = 999_999_999;
    // C# `SetMoveImmediate(AboutToBlowState, forceTransition: true)`. Route
    // through `Effect::ForceMonsterMove` (NOT a bare `force_current_state`)
    // so the snapshotted `creature.intent` telegraph is re-pointed too —
    // the same gpdng-fix shape as the CeremonialBeast Plow interrupt.
    sts2_core::hooks::apply_effects(
        state,
        &[sts2_core::hooks::Effect::ForceMonsterMove {
            target: owner,
            state_id: "ABOUT_TO_BLOW",
        }],
    );
}

/// Idempotent installer for the SteamEruption death-interrupt. Called via
/// a `OnceLock` from [`create_waterfall_giant_ai`] (every WaterfallGiant
/// spawn flows through it, so the handler is registered before any combat
/// damage event). Safe to call from multiple boot paths.
fn install_steam_eruption_handlers() {
    sts2_core::enemy_death_effects::register(
        PowerKind::SteamEruption,
        steam_eruption_death_interrupt,
    );
}

/// `WaterfallGiant` — PRESSURIZE → STOMP → RAM → SIPHON → PRESSURE_GUN →
/// PRESSURE_UP → STOMP, with the death-interrupt ABOUT_TO_BLOW → EXPLODE
/// finish (`steam_eruption_death_interrupt` above forces ABOUT_TO_BLOW the
/// instant the pressurized giant is killed). Ports C# v0.103.2
/// `WaterfallGiant.cs` (source of truth). Bead `sts2-rs-k3pxa` (cycle),
/// `sts2-rs-wz8qc` (AboutToBlow + EXPLODE states + SteamEruption capture).
pub fn create_waterfall_giant_ai() -> MonsterAi {
    // Register the SteamEruption death-interrupt once per process — every
    // WaterfallGiant spawn flows through this factory, so the handler is in
    // place before any combat damage event (same shape as the CeremonialBeast
    // `install_plow_handlers` guard).
    {
        use std::sync::OnceLock;
        static INSTALL_STEAM: OnceLock<()> = OnceLock::new();
        INSTALL_STEAM.get_or_init(install_steam_eruption_handlers);
    }
    let mut s = BTreeMap::new();
    s.insert(
        "PRESSURIZE",
        mv_eff(
            "PRESSURIZE",
            smallvec![Intent::Buff {
                kind: PowerKind::SteamEruption,
                amount: 15
            }],
            "STOMP",
            waterfall_giant_pressurize_effect,
        ),
    );
    s.insert(
        "STOMP",
        mv_eff(
            "STOMP",
            smallvec![
                Intent::Attack { damage: 15 },
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 1
                },
                Intent::Buff {
                    kind: PowerKind::SteamEruption,
                    amount: 3
                }
            ],
            "RAM",
            waterfall_giant_stomp_effect,
        ),
    );
    s.insert(
        "RAM",
        mv_eff(
            "RAM",
            smallvec![
                Intent::Attack { damage: 10 },
                Intent::Buff {
                    kind: PowerKind::SteamEruption,
                    amount: 3
                }
            ],
            "SIPHON",
            waterfall_giant_steam_only_effect,
        ),
    );
    s.insert(
        "SIPHON",
        mv_eff(
            "SIPHON",
            smallvec![
                Intent::Heal,
                Intent::Buff {
                    kind: PowerKind::SteamEruption,
                    amount: 3
                }
            ],
            "PRESSURE_GUN",
            waterfall_giant_siphon_effect,
        ),
    );
    s.insert(
        "PRESSURE_GUN",
        mv_eff(
            // Base damage 20 (C# BasePressureGunDamage); the effect_fn ramps
            // this stored intent +5/use (PressureGunIncrease) — bead f4fbo.
            "PRESSURE_GUN",
            smallvec![
                Intent::Attack { damage: 20 },
                Intent::Buff {
                    kind: PowerKind::SteamEruption,
                    amount: 3
                }
            ],
            "PRESSURE_UP",
            waterfall_giant_pressure_gun_effect,
        ),
    );
    s.insert(
        "PRESSURE_UP",
        mv_eff(
            "PRESSURE_UP",
            smallvec![
                Intent::Attack { damage: 13 },
                Intent::Buff {
                    kind: PowerKind::SteamEruption,
                    amount: 3
                }
            ],
            "STOMP",
            waterfall_giant_steam_only_effect,
        ),
    );
    // Bead `sts2-rs-wz8qc`: doom-triggered finish. ABOUT_TO_BLOW
    // telegraphs `Intent::Stun` (C# `StunIntent`, MustPerformOnce) and
    // captures the giant's SteamEruption stacks into EXPLODE's
    // `Intent::DeathBlow` damage payload via
    // `waterfall_giant_about_to_blow_effect`. EXPLODE then deals that
    // captured damage and kills the dealer via the standard `DeathBlow`
    // dispatch arm (bead `sts2-rs-f99e7`); the C# follow-up loop
    // (`moveState7.FollowUpState = moveState7`) is preserved here as
    // EXPLODE→EXPLODE so the explode self-references after the bomb is
    // already dead (matches C#'s no-op self-loop on a 0-HP creature).
    s.insert(
        "ABOUT_TO_BLOW",
        MonsterState::Move(sts2_core::monster_ai::MoveState {
            id: "ABOUT_TO_BLOW",
            effect: waterfall_giant_about_to_blow_effect,
            intents: smallvec![Intent::Stun],
            follow_up: Some("EXPLODE"),
            // C# `WaterfallGiant.cs:200-203`: `MustPerformOnceBeforeTransitioning
            // = true` — the forced ABOUT_TO_BLOW must perform (capture + strip
            // the SteamEruption stacks) before the machine may advance.
            must_perform_once: true,
        }),
    );
    s.insert(
        "EXPLODE",
        mv(
            "EXPLODE",
            // Damage starts at 0 (placeholder); ABOUT_TO_BLOW's effect_fn
            // overwrites it with the captured SteamEruption amount before
            // EXPLODE telegraphs. C# `DeathBlowIntent(() => SteamEruptionDamage)`.
            smallvec![Intent::DeathBlow { damage: 0 }],
            "EXPLODE",
        ),
    );
    MonsterAi::new(s, "PRESSURIZE")
}

/// `SoulFysh` — CHOMP → SOUL_DRAIN → DEEP_DIVE → CHOMP.
/// SoulFysh BECKON_MOVE: add 2 Beckon status cards — one to Draw, one to
/// Discard. Mirrors C# v0.103.2 `SoulFysh.cs::BeckonMove` (BeckonMoveAmount
/// = 2: `AddGeneratedCardToCombat(Beckon, Draw)` + `(Beckon, Discard)`).
fn soul_fysh_beckon_effect(state: &mut CombatState, _owner: sts2_core::CreatureUid) {
    // C# `SoulFysh.BeckonMove` (v0.107.1, SoulFysh.cs:140-147) passes
    // `CardPilePosition.Random` for the DRAW copy:
    //
    // ```text
    // AddGeneratedCardToCombat(card,  PileType.Draw,    null, CardPilePosition.Random)
    // AddGeneratedCardToCombat(card2, PileType.Discard, null)
    // ```
    //
    // `CardPilePosition.Random` is `Rng.Shuffle.NextInt(Count + 1)` — a real
    // read on the run-level `shuffle` stream, ALWAYS, even into an empty pile.
    // The prior port used the plain top-of-draw spawn, which both put the
    // Beckon in the wrong slot and left the `shuffle` cursor one read behind
    // C# for the rest of the combat, so the next reshuffle dealt a different
    // hand. Found by the Underdocks encounter sweep (GH #544).
    crate::cards::status::spawn_beckon_into_draw_at_random(state);
    crate::cards::status::spawn_beckon(state, sts2_core::card_effects::PileKind::Discard);
}

/// SoulFysh GAZE_MOVE status leg: add 1 Beckon status card to Discard
/// (alongside its 7-damage attack). Mirrors C# `SoulFysh.cs::GazeMove`
/// `StatusIntent(GazeMoveAmount=1)` → one Beckon to the Discard pile.
fn soul_fysh_gaze_effect(state: &mut CombatState, _owner: sts2_core::CreatureUid) {
    crate::cards::status::spawn_beckon(state, sts2_core::card_effects::PileKind::Discard);
}

/// SoulFysh FADE_MOVE: become intangible — Intangible(2) to self. Mirrors
/// C# `SoulFysh.cs::FadeMove` `PowerCmd.Apply<IntangiblePower>(self, 2m)`.
/// Bead `sts2-rs-fcz8k`.
fn soul_fysh_fade_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Intangible, 2);
}

/// SoulFysh SCREAM_MOVE debuff leg: Vulnerable(3) to the player (alongside
/// its 11-damage attack). Mirrors C# `SoulFysh.cs::ScreamMove`
/// `PowerCmd.Apply<VulnerablePower>(targets, ScreamMoveAmount=3)`.
fn soul_fysh_scream_effect(state: &mut CombatState, _owner: sts2_core::CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Vulnerable, 3);
}

/// `SoulFysh` — BECKON → DE_GAS → GAZE → FADE → SCREAM → BECKON …
///
/// Rebased to C# v0.103.2 `SoulFysh.cs` (bead `sts2-rs-fcz8k`). The prior
/// Rust port (CHOMP / SOUL_DRAIN / DEEP_DIVE) was a fabricated Python-pin
/// moveset that does not exist in the decomp; in particular it lacked
/// FADE_MOVE, the only move that applies the SoulFysh's defensive
/// Intangible(2). Damage values are the non-ascension C# constants
/// (DeGas 16, Gaze 7, Scream 11).
pub fn create_soul_fysh_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "BECKON",
        mv_eff(
            "BECKON",
            smallvec![Intent::StatusCard],
            "DE_GAS",
            soul_fysh_beckon_effect,
        ),
    );
    s.insert(
        "DE_GAS",
        mv("DE_GAS", smallvec![Intent::Attack { damage: 16 }], "GAZE"),
    );
    s.insert(
        "GAZE",
        mv_eff(
            "GAZE",
            smallvec![Intent::Attack { damage: 7 }, Intent::StatusCard],
            "FADE",
            soul_fysh_gaze_effect,
        ),
    );
    s.insert(
        "FADE",
        mv_eff(
            "FADE",
            smallvec![Intent::Buff {
                kind: PowerKind::Intangible,
                amount: 2
            }],
            "SCREAM",
            soul_fysh_fade_effect,
        ),
    );
    s.insert(
        "SCREAM",
        mv_eff(
            "SCREAM",
            // `…wb8lk.11`: v0.107.1 ScreamDamage 11→13 at A0 (decomp
            // `SoulFysh.cs:56 GetValueIfAscension(DeadlyEnemies, 15, 13)`).
            smallvec![
                Intent::Attack { damage: 13 },
                Intent::Debuff {
                    kind: PowerKind::Vulnerable,
                    amount: 3
                }
            ],
            "BECKON",
            soul_fysh_scream_effect,
        ),
    );
    MonsterAi::new(s, "BECKON")
}

/// `LagavulinMatriarch` — SLEEP → SLEEP_CHECK → {SLEEP | SLASH} →
/// DISEMBOWEL → SLASH2 → SOUL_SIPHON → SLASH …
///
/// SLEEP_BRANCH predicate — C# `base.Creature.HasPower<AsleepPower>()`
/// (`LagavulinMatriarch.cs:173-174`).
///
/// GH #362: this used to be the combat-global proxy `combat.round < 3`,
/// adopted from Python's `_state["asleep"]` flag. Two things it cannot see.
/// A **damage wake** strips Asleep early (`AsleepPower.AfterDamageReceived`),
/// and the proxy kept the boss asleep anyway until round 3. And the actual
/// duration — the boss spawns with `AsleepPower(3)` which ticks at each
/// enemy-side turn end, so the window is three of ITS turns, not three
/// combat rounds. Reading the power directly makes both exact and is what
/// lets `tick_asleep_and_maybe_wake` wake the boss simply by removing it.
fn lagavulin_asleep(combat: &CombatState, owner: sts2_core::CreatureUid) -> bool {
    combat
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .is_some_and(|c| {
            c.powers.iter().any(|u| {
                combat
                    .powers
                    .iter()
                    .any(|p| p.id == u.0 && p.kind == PowerKind::Asleep)
            })
        })
}

/// SLASH2_MOVE's secondary `Intent::Defend { block: 12 }` is dropped by
/// `perform_intent` (primary-intent-only block-gain, bead `sts2-rs-qk2du`),
/// so grant the 12 block here. Mirrors C# `LagavulinMatriarch.Slash2Move`
/// (`Slash2Block`=12 non-asc) `CreatureCmd.GainBlock(.., 12)`. Bead
/// `sts2-rs-viwkh` (same class as SkulkingColony ZOOM, `sts2-rs-71kog`).
fn lagavulin_matriarch_slash2_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    let amount = crate::ascension_scaling::tough_block(
        state.ascension,
        sts2_core::MonsterKind::LagavulinMatriarch,
        "SLASH2",
        12,
    );
    sts2_core::hooks::apply_effects(
        state,
        &[sts2_core::hooks::Effect::GainBlockCreature {
            target: sts2_core::hooks::CreatureRef::Enemy(owner),
            amount,
        }],
    );
}

pub fn create_lagavulin_matriarch_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SLEEP",
        mv("SLEEP", smallvec![Intent::Sleep], "SLEEP_CHECK"),
    );
    // Damage wake stun. C# `AsleepPower.AfterDamageReceived`
    // (AsleepPower.cs:21-36) fires on ANY unblocked damage: Plating is
    // removed, then `CreatureCmd.Stun(Owner, WakeUpMove, "SLASH_MOVE")` —
    // the boss wastes one stunned turn (whose move body is `WakeUpMove`,
    // LagavulinMatriarch.cs:189, an anim/IsAwake flip with no combat-state
    // effect), then SLASHes. The core `asleep_wake_on_unblocked_damage`
    // reaction (hooks.rs) forces this state; it never enters via a normal
    // transition.
    s.insert(
        "WAKE_UP_MOVE",
        mv("WAKE_UP_MOVE", smallvec![Intent::Stun], "SLASH"),
    );
    s.insert(
        "SLEEP_CHECK",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "SLEEP_CHECK",
            branches: smallvec![
                cb_owner(lagavulin_asleep, "SLEEP"),
                cb(always_true, "SLASH"),
            ],
        }),
    );
    s.insert(
        "SLASH",
        mv(
            "SLASH",
            smallvec![Intent::Attack { damage: 19 }],
            "DISEMBOWEL",
        ),
    );
    s.insert(
        "DISEMBOWEL",
        mv(
            "DISEMBOWEL",
            smallvec![Intent::MultiAttack { damage: 9, hits: 2 }],
            "SLASH2",
        ),
    );
    s.insert(
        "SLASH2",
        mv_eff(
            "SLASH2",
            smallvec![Intent::Attack { damage: 12 }, Intent::Defend { block: 12 }],
            "SOUL_SIPHON",
            lagavulin_matriarch_slash2_effect,
        ),
    );
    s.insert(
        // Bead `sts2-rs-esvmk` (split from `sts2-rs-xz2dy`): SOUL_SIPHON per
        // C# v0.103.2 `LagavulinMatriarch.cs::SoulSiphonMove` applies
        // Strength -2 AND Dexterity -2 to the player and Strength +2 to
        // self. The earlier `Strength/1` placeholder was a stale Python-pin
        // port (`act4.py`, magnitude 1, no Dexterity leg) AND telegraph-only
        // (plain `mv` runs no effect_fn — see the module header note), so
        // the debuff never actually landed. C# telegraphs this as one
        // generic DebuffIntent + one BuffIntent; we keep one Debuff + one
        // Buff intent (magnitude 2) for display and apply the full
        // Str-2 / Dex-2 / +2 set in `lagavulin_soul_siphon_effect`.
        "SOUL_SIPHON",
        mv_eff(
            "SOUL_SIPHON",
            smallvec![
                Intent::Debuff {
                    kind: PowerKind::Strength,
                    amount: 2
                },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2
                }
            ],
            "SLASH",
            lagavulin_soul_siphon_effect,
        ),
    );
    MonsterAi::new(s, "SLEEP")
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};

    /// Build a 1-enemy combat state and return `(state, owner_uid)`.
    fn combat_with_one_enemy() -> (CombatState, CreatureUid) {
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let owner = CreatureUid::new(1).expect("nonzero");
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 100,
            max_hp: 100,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::SludgeSpinner),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("SludgeSpinner"),
            ai: None,
        });
        state.enemy_uids.push(owner);
        (state, owner)
    }

    /// GH #368.4 — `ThieveryPower.Steal()` is ported, so the merc's steal
    /// accumulator is a real running total and the Thievery -> Heist transfer
    /// finally fires in a pure-sim combat.
    ///
    /// C# `ThieveryPower.Steal` (ThieveryPower.cs:22-30) takes
    /// `min(Amount, player.Gold)` per call and banks it in
    /// `DynamicVars.Gold`; the engine keeps that total on the instance's
    /// `counter`. `Amount` (20 for the merc) is the per-steal CAP, not the
    /// total — pinning both is what distinguishes a real port from reusing
    /// `amount`.
    #[test]
    fn thievery_steal_banks_capped_gold_onto_the_instance_counter() {
        let (mut state, owner) = combat_with_one_enemy();
        state.gold = 55;
        sts2_core::card_effects::apply_power_to_creature(
            &mut state,
            owner,
            PowerKind::Thievery,
            20,
        );

        let counter = |st: &CombatState| {
            st.creatures
                .iter()
                .find(|c| c.id == owner.0)
                .and_then(|c| {
                    c.powers.iter().find_map(|pu| {
                        st.powers
                            .get_by_id(pu.0)
                            .filter(|p| p.kind == PowerKind::Thievery)
                            .map(|p| p.counter)
                    })
                })
                .expect("thievery instance")
        };

        // First steal: capped at Amount (20), not the whole 55.
        thievery_steal(&mut state, owner);
        assert_eq!(state.gold, 35, "capped at the power's Amount");
        assert_eq!(counter(&state), 20, "banked into the running total");

        // Second: another 20, accumulating rather than replacing.
        thievery_steal(&mut state, owner);
        assert_eq!(state.gold, 15);
        assert_eq!(counter(&state), 40, "accumulates across steals");

        // Third: only 15 left, so the take is the REMAINING gold.
        thievery_steal(&mut state, owner);
        assert_eq!(state.gold, 0);
        assert_eq!(counter(&state), 55);

        // Fourth: nothing left to take, and no phantom bank.
        thievery_steal(&mut state, owner);
        assert_eq!(state.gold, 0);
        assert_eq!(counter(&state), 55, "a broke player grants nothing");
    }

    /// The merc's three move bodies all end in the steal
    /// (`GremlinMerc.cs:88/104/119`) — including GIMME, which has no power
    /// application of its own and so used to carry the shared no-op effect.
    #[test]
    fn every_gremlin_merc_move_steals() {
        let ai = create_gremlin_merc_ai();
        for id in ["GIMME", "DOUBLE_SMASH", "HEHE"] {
            let sts2_core::monster_ai::MonsterState::Move(mv) = ai.states.get(id).expect("state")
            else {
                panic!("{id} is not a Move state");
            };
            let (mut state, owner) = combat_with_one_enemy();
            state.gold = 99;
            sts2_core::card_effects::apply_power_to_creature(
                &mut state,
                owner,
                PowerKind::Thievery,
                20,
            );
            (mv.effect)(&mut state, owner);
            assert_eq!(state.gold, 79, "{id} runs ThieveryPower.Steal");
        }
    }

    /// Seapunk BUBBLE_BURP telegraphs `Buff` and grants its block from the
    /// move body, matching C# `Seapunk.cs:56` / `:84-90`. Shipping
    /// `[Defend, Buff]` made the live game's `Buff` telegraph read as a
    /// divergence, and — because `perform_intent` only honours a HEAD
    /// `Defend` — putting the block back on the telegraph would silently
    /// drop it on any state rebuilt from a real wire.
    #[test]
    fn seapunk_bubble_burp_buffs_first_and_blocks_from_its_body() {
        let ai = create_seapunk_ai();
        let sts2_core::monster_ai::MonsterState::Move(burp) =
            ai.states.get("BUBBLE_BURP").expect("BUBBLE_BURP state")
        else {
            panic!("BUBBLE_BURP is a plain move state");
        };
        assert_eq!(
            burp.intents.first(),
            Some(&Intent::Buff {
                kind: PowerKind::Strength,
                amount: 1
            }),
            "C# declares BuffIntent before DefendIntent (Seapunk.cs:56)"
        );
        assert_eq!(burp.intents.get(1), Some(&Intent::Defend { block: 7 }));

        let (mut state, owner) = combat_with_one_enemy();
        seapunk_bubble_burp_effect(&mut state, owner);
        let creature = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .expect("owner");
        assert_eq!(creature.block, 7, "BubbleBlock is granted by the move body");
        assert_eq!(creature_power_amount(&state, owner, PowerKind::Strength), 1);
    }

    /// Player's total `kind` stacks (0 if absent).
    fn player_power_amount(state: &CombatState, kind: PowerKind) -> i32 {
        state
            .player
            .powers
            .iter()
            .find_map(|pu| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == pu.0 && p.kind == kind)
                    .map(|p| p.amount)
            })
            .unwrap_or(0)
    }

    /// Count of distinct player-attached powers of `kind`.
    fn player_power_count(state: &CombatState, kind: PowerKind) -> usize {
        state
            .player
            .powers
            .iter()
            .filter(|pu| state.powers.iter().any(|p| p.id == pu.0 && p.kind == kind))
            .count()
    }

    /// Owner creature's `kind` stacks (0 if absent).
    fn creature_power_amount(state: &CombatState, owner: CreatureUid, kind: PowerKind) -> i32 {
        state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .and_then(|c| {
                c.powers.iter().find_map(|pu| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == pu.0 && p.kind == kind)
                        .map(|p| p.amount)
                })
            })
            .unwrap_or(0)
    }

    /// Build a 1-enemy combat holding a GremlinMerc (with its real AI) at `hp`
    /// plus its two `AfterAddedToRoom` self-buffs (`GremlinMerc.cs:47-56`:
    /// `SurprisePower(1)` then `ThieveryPower(20)`).
    fn gremlin_merc_combat(hp: i32) -> (CombatState, CreatureUid) {
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        state.side = sts2_core::Side::Player;
        let owner = CreatureUid::new(1).expect("nonzero");
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::GremlinMerc),
            intent: Some(Intent::MultiAttack { damage: 7, hits: 2 }),
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("GremlinMerc"),
            ai: Some(create_gremlin_merc_ai()),
        });
        state.enemy_uids.push(owner);
        for (kind, amount) in MonsterKind::GremlinMerc.on_spawn_self_buffs() {
            sts2_core::card_effects::apply_power_to_creature(&mut state, owner, *kind, *amount);
        }
        // The SurprisePower death-spawn handler is registered by the content
        // boot pass; calling it here pins that registration too.
        crate::cards::dispatch::install_default_registry();
        (state, owner)
    }

    /// C# `SurprisePower.AfterDeath` (`SurprisePower.cs:18-38`) +
    /// `ShouldStopCombatFromEnding() => true` (`SurprisePower.cs:40-43`):
    /// killing the GremlinMerc spawns a SneakyGremlin and a FatGremlin and must
    /// NOT win the fight. `GremlinMercNormal.GenerateMonsters`
    /// (`GremlinMercNormal.cs:46-49`) spawns ONLY the merc, so those two
    /// gremlins exist solely through this hook.
    ///
    /// Pre-fix `PowerKind::Surprise` was an inert marker, so the engine scored a
    /// PHANTOM VICTORY the instant the merc dropped — mirror corpus records 71 /
    /// 218 show `phase == "Victory"` with a dead merc while the live game sat in
    /// PlayerPlayPhase with both gremlins alive. Every rollout that killed the
    /// merc was therefore scored as a guaranteed win.
    #[test]
    fn surprise_power_death_spawns_both_gremlins_and_keeps_combat_open() {
        use sts2_core::combat_loop::check_combat_end;
        use sts2_core::hooks::{CreatureRef, ValueProps};
        use sts2_core::StepResult;

        let (mut state, owner) = gremlin_merc_combat(30);
        assert_eq!(creature_power_amount(&state, owner, PowerKind::Surprise), 1);

        // Lethal player hit through the real damage pipeline — the just-died
        // chokepoint fires `enemy_death_effects::fire_for_owner`, which is where
        // C# runs `AfterDeath` (inside the death pipeline, not at combat end).
        let _ = sts2_core::damage::apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            owner,
            999,
            ValueProps::MOVE,
        );

        assert!(
            state
                .creatures
                .iter()
                .any(|c| c.id == owner.0 && c.current_hp <= 0),
            "the merc stays dead (the power spawns allies, it does not revive)"
        );
        assert_eq!(
            state.enemy_uids.len(),
            3,
            "merc corpse + SneakyGremlin + FatGremlin"
        );

        // C# adds `sneaky` then `fat`; both slots follow `merc`.
        let species: Vec<_> = state
            .enemy_uids
            .iter()
            .filter_map(|u| state.creatures.iter().find(|c| c.id == u.0))
            .map(|c| c.species)
            .collect();
        assert_eq!(
            species,
            vec![
                Some(MonsterKind::GremlinMerc),
                Some(MonsterKind::SneakyGremlin),
                Some(MonsterKind::FatGremlin),
            ]
        );

        for uid in state.enemy_uids.iter().skip(1) {
            let c = state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .expect("spawned gremlin present");
            assert!(c.current_hp > 0, "spawned gremlin is alive");
            assert_eq!(
                c.ai.as_ref().map(sts2_core::MonsterAi::current_state_id),
                Some("SPAWNED"),
                "both gremlins carry their real moveset, opening on SPAWNED (stun)"
            );
            assert_eq!(
                c.intent,
                Some(Intent::Stun),
                "spawned mid-player-turn, so C# `AfterCreatureAdded` rolls the move now"
            );
        }

        // THE FIX: `ShouldStopCombatFromEnding` — no phantom victory.
        assert_eq!(
            check_combat_end(&mut state),
            StepResult::Advanced,
            "combat must NOT resolve as a win while the spawned gremlins live"
        );
        assert!(!state.is_over, "combat stays open");
        assert!(!state.player_won, "no victory was stamped");

        // Killing the two gremlins resolves the fight normally (C#'s vote
        // expires with the merc corpse — `IterateHookListeners` walks
        // `_enemies`, which no longer holds it).
        let gremlins: Vec<_> = state.enemy_uids.iter().skip(1).copied().collect();
        for uid in gremlins {
            let _ = sts2_core::damage::apply_damage_with_final(
                &mut state,
                CreatureRef::Player,
                uid,
                999,
                ValueProps::MOVE,
            );
        }
        assert_eq!(
            check_combat_end(&mut state),
            StepResult::CombatWon,
            "with every gremlin dead the fight resolves as a normal victory"
        );
    }

    /// `SurprisePower.AfterDeath` moves each `ThieveryPower` instance's
    /// accumulated stolen gold (C# `DynamicVars.Gold.IntValue`, modelled by the
    /// per-instance [`sts2_core::Power::counter`]) onto the FatGremlin as
    /// `HeistPower`.
    #[test]
    fn surprise_power_death_transfers_thievery_gold_as_heist() {
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let (mut state, owner) = gremlin_merc_combat(30);
        // Stamp the accumulated-stolen-gold slot the way `ThieveryPower.Steal`
        // would (that move-side accumulation is not ported at this pin).
        let thievery_id = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .and_then(|c| {
                c.powers.iter().find_map(|pu| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == pu.0 && p.kind == PowerKind::Thievery)
                        .map(|p| p.id)
                })
            })
            .expect("merc carries ThieveryPower");
        state
            .powers
            .get_mut_by_id(thievery_id)
            .expect("thievery instance")
            .counter = 37;

        let _ = sts2_core::damage::apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            owner,
            999,
            ValueProps::MOVE,
        );

        let fat = *state.enemy_uids.last().expect("fat gremlin spawned");
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == fat.0)
                .and_then(|c| c.species),
            Some(MonsterKind::FatGremlin)
        );
        assert_eq!(
            creature_power_amount(&state, fat, PowerKind::Heist),
            37,
            "the merc's stolen gold rides out on the FatGremlin as Heist"
        );
        assert_eq!(
            creature_power_amount(&state, state.enemy_uids[1], PowerKind::Heist),
            0,
            "only the FatGremlin receives the Heist (C#'s `fatGremlin` local)"
        );
    }

    /// A merc that stole nothing grants NO HeistPower — C# `PowerCmd.Apply`
    /// bails on `amount == 0m` (`PowerCmd.cs:103`), so the fat gremlin must not
    /// carry a 0-stack Heist.
    #[test]
    fn surprise_power_death_grants_no_heist_when_nothing_was_stolen() {
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let (mut state, owner) = gremlin_merc_combat(30);
        let _ = sts2_core::damage::apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            owner,
            999,
            ValueProps::MOVE,
        );
        let fat = *state.enemy_uids.last().expect("fat gremlin spawned");
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == fat.0)
                .map_or(1, |c| c.powers.len()),
            0,
            "no Heist(0) on the FatGremlin when the merc stole nothing"
        );
    }

    /// Build a combat holding one WaterfallGiant (with its real AI) at
    /// `hp`, telegraphing STOMP.
    fn waterfall_giant_combat(hp: i32) -> (CombatState, CreatureUid) {
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let owner = CreatureUid::new(1).expect("nonzero");
        let mut ai = create_waterfall_giant_ai();
        ai.set_owner(owner);
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: hp,
            max_hp: 240,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::WaterfallGiant),
            intent: Some(Intent::Attack { damage: 15 }),
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("WaterfallGiant"),
            ai: Some(ai),
        });
        state.enemy_uids.push(owner);
        (state, owner)
    }

    /// C# `SteamEruptionPower.AfterDeath` (`SteamEruptionPower.cs:15-21`) →
    /// `WaterfallGiant.TriggerAboutToBlowState` (`WaterfallGiant.cs:301-313`):
    /// killing the pressurized giant must NOT win the fight. The death is
    /// interrupted — hp/max reset to 999999999, move forced to ABOUT_TO_BLOW
    /// (Stun telegraph) — and `check_combat_end` keeps the combat open.
    /// Pre-fix the engine declared Victory here while the live game continued
    /// (mirror_live2: world giant at hp 999999999 with a DeathBlow intent).
    #[test]
    fn waterfall_giant_death_is_interrupted_while_pressurized() {
        use sts2_core::combat_loop::check_combat_end;
        use sts2_core::hooks::{CreatureRef, ValueProps};
        use sts2_core::StepResult;

        let (mut state, owner) = waterfall_giant_combat(30);
        sts2_core::card_effects::apply_power_to_creature(
            &mut state,
            owner,
            PowerKind::SteamEruption,
            24,
        );

        // Lethal player hit through the real damage pipeline (the just-died
        // chokepoint fires `enemy_death_effects::fire_for_owner`).
        let _ = sts2_core::damage::apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            owner,
            999,
            ValueProps::MOVE,
        );

        let giant = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .expect("giant present");
        assert_eq!(
            giant.current_hp, 999_999_999,
            "death interrupted: hp reset to C# SetMaxAndCurrentHp(999999999)"
        );
        assert_eq!(giant.max_hp, 999_999_999, "max hp reset too");
        assert_eq!(
            giant
                .ai
                .as_ref()
                .map(sts2_core::MonsterAi::current_state_id),
            Some("ABOUT_TO_BLOW"),
            "move forced to ABOUT_TO_BLOW"
        );
        assert_eq!(
            giant.intent,
            Some(Intent::Stun),
            "the telegraph is re-pointed to ABOUT_TO_BLOW's StunIntent"
        );
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::SteamEruption),
            24,
            "SteamEruption stays armed (fire_for_owner's revive exception) — \
             ABOUT_TO_BLOW's move body captures + strips it"
        );
        assert_eq!(
            check_combat_end(&mut state),
            StepResult::Advanced,
            "no victory while the interrupted giant lives at 999999999"
        );
        assert!(!state.is_over, "combat stays open");
    }

    /// Without SteamEruption stacks (killed before its first move ever
    /// pressurizes), the giant dies for real and the fight resolves as a
    /// normal victory — the interrupt is gated on the power, as in C#.
    #[test]
    fn waterfall_giant_dies_normally_without_steam_eruption() {
        use sts2_core::combat_loop::check_combat_end;
        use sts2_core::hooks::{CreatureRef, ValueProps};
        use sts2_core::StepResult;

        let (mut state, owner) = waterfall_giant_combat(30);
        let _ = sts2_core::damage::apply_damage_with_final(
            &mut state,
            CreatureRef::Player,
            owner,
            999,
            ValueProps::MOVE,
        );
        assert!(
            state
                .creatures
                .iter()
                .any(|c| c.id == owner.0 && c.current_hp <= 0),
            "unpressurized giant stays dead"
        );
        assert_eq!(
            check_combat_end(&mut state),
            StepResult::CombatWon,
            "normal victory when no SteamEruption interrupt applies"
        );
    }

    /// Kills `sludge_spinner_oil_spray_effect -> ()`: the move applies Weak 1
    /// to the player (C# `PowerCmd.Apply<WeakPower>(targets, 1m)`).
    #[test]
    fn sludge_spinner_oil_spray_applies_weak_to_player() {
        let (mut state, owner) = combat_with_one_enemy();
        sludge_spinner_oil_spray_effect(&mut state, owner);
        assert_eq!(
            player_power_amount(&state, PowerKind::Weak),
            1,
            "OIL_SPRAY must apply 1 Weak to the player"
        );
    }

    /// Kills `sludge_spinner_rage_effect -> ()`: RAGE buffs the casting
    /// monster with Strength 3 (C# `PowerCmd.Apply<StrengthPower>(self, 3m)`).
    #[test]
    fn sludge_spinner_rage_buffs_self_strength() {
        let (mut state, owner) = combat_with_one_enemy();
        sludge_spinner_rage_effect(&mut state, owner);
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Strength),
            3,
            "RAGE must grant the monster 3 Strength"
        );
    }

    /// Kills `sewer_clam_pressurize_effect -> ()`: PRESSURIZE buffs the
    /// casting monster with Strength 4 (C# `Apply<StrengthPower>(self, 4m)`).
    #[test]
    fn sewer_clam_pressurize_buffs_self_strength() {
        let (mut state, owner) = combat_with_one_enemy();
        sewer_clam_pressurize_effect(&mut state, owner);
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Strength),
            4,
            "PRESSURIZE must grant the monster 4 Strength"
        );
    }

    /// Kills the `p.id == puid.0` (col 32) and `p.kind == kind` (col 52)
    /// existing-power lookup mutations in
    /// `apply_power_to_player_via_legacy_surface`: applying the SAME debuff
    /// twice must STACK into a single power (amount 2), not create a duplicate.
    /// Both `==→!=` mutants fail to find the existing Weak and would push a
    /// second Weak power.
    #[test]
    fn unsigned_surface_stacks_same_kind_into_one_power() {
        let (mut state, _owner) = combat_with_one_enemy();
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Weak, 1);
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Weak, 1);
        assert_eq!(
            player_power_amount(&state, PowerKind::Weak),
            2,
            "two Weak applications stack to amount 2"
        );
        assert_eq!(
            player_power_count(&state, PowerKind::Weak),
            1,
            "stacking must not create a duplicate Weak power"
        );
    }

    /// Kills the `&& → ||` mutation (col 42) in
    /// `apply_power_to_player_via_legacy_surface`'s existing-power lookup.
    /// With a pre-existing Strength power on the player, applying Weak must
    /// create a SEPARATE Weak power and leave Strength untouched. Under `||`
    /// the id-match alone would route the Weak amount onto the Strength power
    /// (Strength → 6, no Weak created).
    #[test]
    fn unsigned_surface_does_not_merge_across_kinds() {
        let (mut state, _owner) = combat_with_one_enemy();
        // Seed an unrelated player power of a different kind.
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Strength, 5);
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Weak, 1);
        assert_eq!(
            player_power_amount(&state, PowerKind::Strength),
            5,
            "applying Weak must not bleed into the Strength stack"
        );
        assert_eq!(
            player_power_amount(&state, PowerKind::Weak),
            1,
            "Weak must land as its own power"
        );
        assert_eq!(player_power_count(&state, PowerKind::Weak), 1);
    }

    /// Bead `sts2-rs-60kxh`, second half. This file carried its own
    /// `apply_signed_power_to_player_via_legacy_surface` — a SIXTH copy of the
    /// legacy surface, missed by the de-duplication because of the differing
    /// name. It scanned `state.player.powers` alone and never removed a stack
    /// that netted to 0, and its doc-comment asserted the removal did not
    /// apply. C# `PowerModel.ShouldRemoveDueToAmount` (`PowerModel.cs:480-490`)
    /// says the opposite for `AllowNegative` powers: *"Powers that allow
    /// negative amounts (Strength, DexterityPower, etc.) should be removed when
    /// they reach exactly 0."*
    ///
    /// `LagavulinMatriarch` SOUL_SIPHON is the only caller and hits both bugs
    /// at once: against a player carrying a card-granted `Strength(+2)` — which
    /// lives on the mirror Creature — the old copy pushed a DUPLICATE
    /// `Strength(-2)` next to it, where C# folds the two and removes the
    /// instance at 0.
    #[test]
    fn soul_siphon_folds_into_a_card_granted_strength_and_removes_it_at_zero() {
        let (mut state, owner) = combat_with_one_enemy();
        state.init_player_creature();
        let player = state.player.creature_uid.expect("mirror creature");
        // The card-granted buff: `apply_power_to_creature` writes to the mirror
        // Creature, NOT to `state.player.powers` — the shape a legacy-only scan
        // cannot see.
        sts2_core::card_effects::apply_power_to_creature(
            &mut state,
            player,
            PowerKind::Strength,
            2,
        );

        lagavulin_soul_siphon_effect(&mut state, owner);

        let player_instances = |state: &CombatState, kind: PowerKind| {
            state
                .player_power_uids()
                .into_iter()
                .filter(|pu| state.powers.iter().any(|p| p.id == pu.0 && p.kind == kind))
                .count()
        };
        assert_eq!(
            player_instances(&state, PowerKind::Strength),
            0,
            "+2 (card) + -2 (siphon) nets to 0, so C# removes the instance",
        );
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Strength
                && !state
                    .creatures
                    .iter()
                    .any(|c| c.id == owner.0 && c.powers.iter().any(|pu| pu.0 == p.id))),
            "the dead player Strength must be gone from the arena too — only \
             the matriarch's own +2 self-buff may remain",
        );
        // Dexterity has no card-granted counterpart, so it lands as a single
        // fresh instance rather than merging.
        assert_eq!(player_instances(&state, PowerKind::Dexterity), 1);
        assert_eq!(player_power_amount(&state, PowerKind::Dexterity), -2);
    }

    /// Kills the `p.id == puid.0 → !=` (col 32) lookup mutation in
    /// `apply_signed_power_to_player`: applying the same signed power
    /// (Strength) twice stacks into one power. Under `!=` the existing
    /// Strength is never found and a duplicate is pushed.
    #[test]
    fn signed_surface_stacks_same_kind_into_one_power() {
        let (mut state, _owner) = combat_with_one_enemy();
        apply_signed_power_to_player(&mut state, PowerKind::Strength, -2);
        apply_signed_power_to_player(&mut state, PowerKind::Strength, -2);
        assert_eq!(
            player_power_amount(&state, PowerKind::Strength),
            -4,
            "two signed Strength debuffs stack to -4"
        );
        assert_eq!(
            player_power_count(&state, PowerKind::Strength),
            1,
            "stacking must not create a duplicate Strength power"
        );
    }

    /// Seed `amount` stacks of `kind` directly onto the owner creature.
    fn seed_creature_power(
        state: &mut CombatState,
        owner: CreatureUid,
        kind: PowerKind,
        amount: i32,
    ) {
        sts2_core::card_effects::apply_power_to_creature(state, owner, kind, amount);
    }

    /// Kills `decrement_creature_power` `&& → ||` (col 40) and `> → >=`
    /// (col 52) in the existing-power filter `p.kind == kind && p.amount > 0`.
    ///   - `&&→||`: an owner Thorns(5) plus an unrelated Strength(0) — the
    ///     `||` form would match the Strength (amount>0 OR kind==Thorns), and
    ///     could decrement the wrong power. We assert ONLY Thorns drops.
    ///   - `>→>=`: with a Thorns(0) present, the correct `>0` skips it (early
    ///     return, no removal), while `>=0` would select and decrement it.
    /// Here we decrement Thorns(5) by 2 and require Thorns→3, Strength→4 intact.
    #[test]
    fn decrement_creature_power_targets_correct_positive_power() {
        let (mut state, owner) = combat_with_one_enemy();
        seed_creature_power(&mut state, owner, PowerKind::Strength, 4);
        seed_creature_power(&mut state, owner, PowerKind::Thorns, 5);
        decrement_creature_power(&mut state, owner, PowerKind::Thorns, 2);
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Thorns),
            3,
            "Thorns 5 - 2 = 3"
        );
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Strength),
            4,
            "the unrelated Strength power must be untouched"
        );
    }

    /// Kills `decrement_creature_power` `p.amount == 0 → !=` (col 38), the
    /// remove-at-zero guard. Decrementing Thorns(2) by 2 must drop the stack to
    /// 0 AND remove the power instance (and unlink it from the creature). Under
    /// `!=` the non-zero result would be (wrongly) removed and the zero result
    /// kept; we assert the power is gone exactly when it hits 0, and present
    /// (amount 1) when it does not.
    #[test]
    fn decrement_creature_power_removes_only_at_zero() {
        // Non-lethal decrement: power survives at amount 1.
        let (mut state, owner) = combat_with_one_enemy();
        seed_creature_power(&mut state, owner, PowerKind::Thorns, 3);
        decrement_creature_power(&mut state, owner, PowerKind::Thorns, 2);
        assert_eq!(creature_power_amount(&state, owner, PowerKind::Thorns), 1);
        let still_linked = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .is_some_and(|c| {
                c.powers.iter().any(|pu| {
                    state
                        .powers
                        .iter()
                        .any(|p| p.id == pu.0 && p.kind == PowerKind::Thorns)
                })
            });
        assert!(
            still_linked,
            "non-zero power must remain linked to the creature"
        );

        // Exact-zero decrement: power instance must be removed + unlinked.
        let (mut state2, owner2) = combat_with_one_enemy();
        seed_creature_power(&mut state2, owner2, PowerKind::Thorns, 2);
        decrement_creature_power(&mut state2, owner2, PowerKind::Thorns, 2);
        assert_eq!(creature_power_amount(&state2, owner2, PowerKind::Thorns), 0);
        let any_thorns = state2.powers.iter().any(|p| p.kind == PowerKind::Thorns);
        assert!(!any_thorns, "a power reduced to 0 must be removed entirely");
        let linked = state2
            .creatures
            .iter()
            .find(|c| c.id == owner2.0)
            .is_some_and(|c| !c.powers.is_empty());
        assert!(!linked, "removed power must be unlinked from the creature");
    }

    /// Smoke test: every ctor produces an AI whose initial move resolves.
    #[test]
    fn every_ctor_has_resolvable_initial_move() {
        let smokes: Vec<MonsterAi> = vec![
            create_corpse_slug_ai(0),
            create_corpse_slug_ai(1),
            create_corpse_slug_ai(2),
            create_seapunk_ai(),
            create_sludge_spinner_ai(),
            create_toadpole_ai(ToadpoleSlot::First),
            create_toadpole_ai(ToadpoleSlot::Second),
            create_toadpole_ai(ToadpoleSlot::Other),
            create_calcified_cultist_ai(),
            create_damp_cultist_ai(),
            create_fossil_stalker_ai(),
            create_sneaky_gremlin_ai(),
            create_fat_gremlin_ai(),
            create_gremlin_merc_ai(),
            create_haunted_ship_ai(),
            create_gas_bomb_ai(),
            create_living_fog_ai(),
            create_punch_construct_ai(false),
            create_punch_construct_ai(true),
            create_sewer_clam_ai(),
            create_two_tailed_rat_ai(TwoTailedRatSlot::First),
            create_two_tailed_rat_ai(TwoTailedRatSlot::Second),
            create_two_tailed_rat_ai(TwoTailedRatSlot::Other),
            create_phantasmal_gardener_ai(PhantasmalGardenerSlot::First),
            create_skulking_colony_ai(),
            create_terror_eel_ai(),
            create_waterfall_giant_ai(),
            create_soul_fysh_ai(),
            create_lagavulin_matriarch_ai(),
        ];
        for ai in smokes {
            let _ = ai.current_move();
        }
    }

    /// Bead `sts2-rs-esvmk`: SOUL_SIPHON applies the full C# v0.103.2
    /// effect — player Strength -2 AND Dexterity -2 (signed debuff via the
    /// legacy player surface), plus +2 Strength to the casting monster.
    /// Regression guard: the prior `mv` was telegraph-only (no effect_fn)
    /// and carried the stale Python `Strength/-1` (no Dexterity leg), so
    /// the debuff never landed.
    #[test]
    fn soul_siphon_debuffs_player_str_and_dex_and_buffs_self() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let owner = CreatureUid::new(1).expect("nonzero");
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 100,
            max_hp: 100,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::LagavulinMatriarch),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("LagavulinMatriarch"),
            ai: None,
        });
        state.enemy_uids.push(owner);

        lagavulin_soul_siphon_effect(&mut state, owner);

        let player_amt = |kind: PowerKind| -> i32 {
            state
                .player
                .powers
                .iter()
                .find_map(|pu| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == pu.0 && p.kind == kind)
                        .map(|p| p.amount)
                })
                .unwrap_or(0)
        };
        assert_eq!(
            player_amt(PowerKind::Strength),
            -2,
            "player loses 2 Strength"
        );
        assert_eq!(
            player_amt(PowerKind::Dexterity),
            -2,
            "player loses 2 Dexterity"
        );

        let owner_str = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .and_then(|c| {
                c.powers.iter().find_map(|pu| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == pu.0 && p.kind == PowerKind::Strength)
                        .map(|p| p.amount)
                })
            })
            .unwrap_or(0);
        assert_eq!(owner_str, 2, "LagavulinMatriarch gains 2 Strength");
    }

    /// Bead `sts2-rs-fcz8k`: SoulFysh rebased to C# v0.103.2 — the move
    /// cycle is BECKON → DE_GAS → GAZE → FADE → SCREAM, FADE applies
    /// Intangible(2) to self, SCREAM applies Vulnerable(3) to the player,
    /// and BECKON adds two Beckon status cards (one Draw, one Discard).
    /// Replaces the fabricated Python-pin CHOMP/SOUL_DRAIN/DEEP_DIVE port.
    #[test]
    fn soul_fysh_rebased_moveset_and_effects_match_csharp() {
        use sts2_core::cards::CardId;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};

        // Linear move cycle.
        let mut ai = create_soul_fysh_ai();
        let mut seq = vec![ai.current_move().id];
        for seed in 0..5 {
            ai.on_move_performed();
            seq.push(ai.roll_move_seeded(seed).id);
        }
        assert_eq!(
            seq,
            vec!["BECKON", "DE_GAS", "GAZE", "FADE", "SCREAM", "BECKON"],
            "SoulFysh cycles BECKON→DE_GAS→GAZE→FADE→SCREAM"
        );

        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 211,
            max_hp: 211,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::SoulFysh),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("SoulFysh"),
            ai: None,
        });
        state.enemy_uids.push(owner);

        soul_fysh_fade_effect(&mut state, owner);
        let intangible = state.creatures[0]
            .powers
            .iter()
            .find_map(|pu| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == pu.0 && p.kind == PowerKind::Intangible)
                    .map(|p| p.amount)
            })
            .unwrap_or(0);
        assert_eq!(intangible, 2, "FADE applies Intangible(2) to self");

        soul_fysh_scream_effect(&mut state, owner);
        let vuln = state
            .player
            .powers
            .iter()
            .find_map(|pu| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == pu.0 && p.kind == PowerKind::Vulnerable)
                    .map(|p| p.amount)
            })
            .unwrap_or(0);
        assert_eq!(vuln, 3, "SCREAM applies Vulnerable(3) to player");

        // BECKON adds two Beckon status cards: one Draw, one Discard.
        soul_fysh_beckon_effect(&mut state, owner);
        let beckons = state
            .cards
            .iter()
            .filter(|c| c.card_id == CardId::Beckon)
            .count();
        assert_eq!(beckons, 2, "BECKON spawns two Beckon status cards");
        assert_eq!(state.player.draw.len(), 1, "one Beckon to Draw");
        assert_eq!(state.player.discard.len(), 1, "one Beckon to Discard");
    }

    #[test]
    fn waterfall_giant_move_effects_apply_steam_weak_and_heal() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 200,
            max_hp: 240,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::WaterfallGiant),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("WaterfallGiant"),
            ai: None,
        });
        state.enemy_uids.push(owner);

        let steam = |state: &CombatState| -> i32 {
            state.creatures[0]
                .powers
                .iter()
                .find_map(|pu| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == pu.0 && p.kind == PowerKind::SteamEruption)
                        .map(|p| p.amount)
                })
                .unwrap_or(0)
        };

        // PRESSURIZE: SteamEruption +15 self.
        waterfall_giant_pressurize_effect(&mut state, owner);
        assert_eq!(steam(&state), 15, "PRESSURIZE stacks SteamEruption(15)");

        // STOMP: Weak(1) player + SteamEruption +3 self (-> 18).
        waterfall_giant_stomp_effect(&mut state, owner);
        assert_eq!(steam(&state), 18, "STOMP adds SteamEruption(+3) -> 18");
        let weak = state
            .player
            .powers
            .iter()
            .find_map(|pu| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == pu.0 && p.kind == PowerKind::Weak)
                    .map(|p| p.amount)
            })
            .unwrap_or(0);
        assert_eq!(weak, 1, "STOMP applies Weak(1) to player");

        // SIPHON: `…wb8lk.11` v0.107.1 SiphonHeal is 10 at A0 (200 -> 210,
        // clamped <= 240) + SteamEruption +3 (-> 21).
        waterfall_giant_siphon_effect(&mut state, owner);
        assert_eq!(state.creatures[0].current_hp, 210, "SIPHON heals 10 at A0");
        assert_eq!(steam(&state), 21, "SIPHON adds SteamEruption(+3) -> 21");
    }

    /// Bead `sts2-rs-dp49t`: the EXPLODE damage tracks the accumulated
    /// `SteamEruptionPower` (C# `SteamEruptionDamage = GetPowerAmount`),
    /// so the explode hit scales with how long the giant pressurized.
    #[test]
    fn waterfall_giant_explode_damage_equals_steam_eruption_amount() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 240,
            max_hp: 240,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::WaterfallGiant),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("WaterfallGiant"),
            ai: None,
        });
        state.enemy_uids.push(owner);

        // No SteamEruption yet → explode damage is 0.
        assert_eq!(waterfall_giant_explode_damage(&state, owner), 0);

        // PRESSURIZE (+15) then RAM's SteamEruption(+3) → 18 accumulated.
        waterfall_giant_pressurize_effect(&mut state, owner);
        waterfall_giant_steam_only_effect(&mut state, owner);
        assert_eq!(
            waterfall_giant_explode_damage(&state, owner),
            18,
            "explode damage equals the accumulated SteamEruption amount",
        );
    }

    /// Bead `sts2-rs-wz8qc`: ABOUT_TO_BLOW captures the giant's
    /// accumulated `SteamEruptionPower` stacks into EXPLODE's
    /// `Intent::DeathBlow { damage }` payload, then removes the power
    /// from the creature. Mirrors C# `WaterfallGiant.AboutToBlowMove`
    /// (lines 283-289: `SteamEruptionDamage = GetPowerAmount`, then
    /// `PowerCmd.Remove<SteamEruptionPower>`).
    #[test]
    fn waterfall_giant_about_to_blow_captures_steam_into_explode_damage() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 240,
            max_hp: 240,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::WaterfallGiant),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("WaterfallGiant"),
            // Attach the real AI so ABOUT_TO_BLOW can rewrite EXPLODE.
            ai: Some(create_waterfall_giant_ai()),
        });
        state.enemy_uids.push(owner);

        // Pressurize twice (15 + 3 = 18 SteamEruption).
        waterfall_giant_pressurize_effect(&mut state, owner);
        waterfall_giant_steam_only_effect(&mut state, owner);
        assert_eq!(waterfall_giant_explode_damage(&state, owner), 18);

        // Fire ABOUT_TO_BLOW: captures 18 into EXPLODE damage, strips power.
        waterfall_giant_about_to_blow_effect(&mut state, owner);
        assert_eq!(
            waterfall_giant_explode_damage(&state, owner),
            0,
            "SteamEruptionPower is removed after ABOUT_TO_BLOW"
        );

        // EXPLODE intent now telegraphs DeathBlow { damage: 18 }.
        let explode_damage = {
            let c = &state.creatures[0];
            let ai = c.ai.as_ref().expect("ai attached");
            let explode_state = ai.states.get("EXPLODE").expect("EXPLODE state");
            match explode_state {
                MonsterState::Move(m) => m
                    .intents
                    .iter()
                    .find_map(|i| match i {
                        Intent::DeathBlow { damage } => Some(*damage),
                        _ => None,
                    })
                    .expect("DeathBlow intent"),
                _ => panic!("EXPLODE must be a Move state"),
            }
        };
        assert_eq!(
            explode_damage, 18,
            "ABOUT_TO_BLOW rewrites EXPLODE DeathBlow damage to captured value"
        );

        // Zero-steam baseline: with no SteamEruption stacks, the capture
        // is 0 and EXPLODE telegraphs `DeathBlow { damage: 0 }` (still a
        // valid no-damage explosion that kills the dealer).
        let owner2 = CreatureUid::new(2).expect("nz");
        state.creatures.push(Creature {
            id: owner2.0,
            current_hp: 240,
            max_hp: 240,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::WaterfallGiant),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("WaterfallGiant"),
            ai: Some(create_waterfall_giant_ai()),
        });
        state.enemy_uids.push(owner2);
        waterfall_giant_about_to_blow_effect(&mut state, owner2);
        let baseline_explode = match state.creatures[1]
            .ai
            .as_ref()
            .unwrap()
            .states
            .get("EXPLODE")
            .unwrap()
        {
            MonsterState::Move(m) => m
                .intents
                .iter()
                .find_map(|i| match i {
                    Intent::DeathBlow { damage } => Some(*damage),
                    _ => None,
                })
                .unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(
            baseline_explode, 0,
            "no SteamEruption -> DeathBlow damage = 0"
        );
    }

    #[test]
    fn waterfall_giant_pressure_gun_ramps_stored_attack_by_five() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 240,
            max_hp: 240,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::WaterfallGiant),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("WaterfallGiant"),
            // Attach the real AI so the effect can mutate its stored intent.
            ai: Some(create_waterfall_giant_ai()),
        });
        state.enemy_uids.push(owner);

        // Read the stored PRESSURE_GUN attack damage off the creature's AI.
        let gun_dmg = |state: &CombatState| -> i32 {
            state.creatures[0]
                .ai
                .as_ref()
                .and_then(|ai| match ai.states.get("PRESSURE_GUN") {
                    Some(MonsterState::Move(m)) => m.intents.iter().find_map(|i| match i {
                        Intent::Attack { damage } => Some(*damage),
                        _ => None,
                    }),
                    _ => None,
                })
                .expect("PRESSURE_GUN attack intent")
        };

        // C# BasePressureGunDamage = 20 before any use.
        assert_eq!(gun_dmg(&state), 20, "base pressure-gun damage is 20");
        // Each PRESSURE_GUN resolution ramps the stored attack +5 (post-hit).
        waterfall_giant_pressure_gun_effect(&mut state, owner);
        assert_eq!(gun_dmg(&state), 25, "first use -> next telegraph 25");
        waterfall_giant_pressure_gun_effect(&mut state, owner);
        assert_eq!(gun_dmg(&state), 30, "second use -> next telegraph 30");
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
    fn corpse_slug_starter_dispatch() {
        assert_eq!(create_corpse_slug_ai(0).current_move().id, "WHIP_SLAP");
        assert_eq!(create_corpse_slug_ai(1).current_move().id, "GLOMP");
        assert_eq!(create_corpse_slug_ai(2).current_move().id, "GOOP");
    }

    #[test]
    fn seapunk_alternates() {
        let mut ai = create_seapunk_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["SEA_KICK", "SPINNING_KICK", "BUBBLE_BURP"]
        );
    }

    #[test]
    fn sludge_spinner_oil_spray_init_then_cannot_repeat() {
        // Rebased to C# SludgeSpinner.cs: init OIL_SPRAY, then a
        // `RAND` branch (CannotRepeat) over OIL_SPRAY / SLAM / RAGE.
        let mut ai = create_sludge_spinner_ai();
        let seq = run_three_moves(&mut ai);
        assert_eq!(seq[0], "OIL_SPRAY");
        let valid = ["OIL_SPRAY", "SLAM", "RAGE"];
        for id in &seq {
            assert!(valid.contains(id), "unexpected SludgeSpinner move {id}");
        }
        // CannotRepeat: the branch never re-selects the just-performed move.
        assert_ne!(seq[0], seq[1]);
        assert_ne!(seq[1], seq[2]);
    }

    #[test]
    fn calcified_cultist_then_self_loop() {
        let mut ai = create_calcified_cultist_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["INCANTATION", "DARK_STRIKE", "DARK_STRIKE"]
        );
    }

    #[test]
    fn damp_cultist_then_self_loop() {
        let mut ai = create_damp_cultist_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["INCANTATION", "DARK_STRIKE", "DARK_STRIKE"]
        );
    }

    #[test]
    fn fossil_stalker_initial_is_latch() {
        assert_eq!(create_fossil_stalker_ai().current_move().id, "LATCH");
    }

    #[test]
    fn fat_gremlin_spawned_then_flee_self_loop() {
        let mut ai = create_fat_gremlin_ai();
        assert_eq!(ai.current_move().id, "SPAWNED");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "FLEE");
        for _ in 0..3 {
            ai.on_move_performed();
            assert_eq!(ai.roll_move_seeded(0).id, "FLEE");
        }
    }

    #[test]
    fn gremlin_merc_three_chain() {
        let mut ai = create_gremlin_merc_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["GIMME", "DOUBLE_SMASH", "HEHE"]
        );
    }

    // `haunted_ship_alternates` (the fabricated BROADSIDE/RAMMING_SPEED loop)
    // was removed when HauntedShip was ported to the real C# moveset; see
    // `haunted_ship_moveset_matches_csharp` below. Bead `sts2-rs-e1q9q`.

    #[test]
    fn gas_bomb_self_loop() {
        let mut ai = create_gas_bomb_ai();
        for _ in 0..5 {
            assert_eq!(ai.current_move().id, "EXPLODE");
            ai.on_move_performed();
            ai.roll_move_seeded(0);
        }
    }

    #[test]
    fn living_fog_alternates() {
        // C# LivingFog.cs: ADVANCED_GAS (once) → BLOAT → SUPER_GAS_BLAST →
        // BLOAT … The prior SPAWN/ENGULF loop was fabricated. Bead 1sspw.
        let mut ai = create_living_fog_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["ADVANCED_GAS", "BLOAT", "SUPER_GAS_BLAST"]
        );
        // BLOAT ↔ SUPER_GAS_BLAST loops thereafter.
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "BLOAT");
    }

    #[test]
    fn living_fog_move_effects_apply_smoggy_and_spawn_gasbomb() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::LivingFog),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("LivingFog"),
            ai: None,
        });
        state.enemy_uids.push(owner);

        // ADVANCED_GAS: SmoggyPower(1) on the player.
        living_fog_advanced_gas_effect(&mut state, owner);
        let smoggy = state
            .player
            .powers
            .iter()
            .find_map(|pu| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == pu.0 && p.kind == PowerKind::Smoggy)
                    .map(|p| p.amount)
            })
            .unwrap_or(0);
        assert_eq!(smoggy, 1, "ADVANCED_GAS applies Smoggy(1) to player");

        // BLOAT: spawns one GasBomb running its own AI.
        living_fog_bloat_effect(&mut state, owner);
        let bombs = state
            .creatures
            .iter()
            .filter(|c| c.species == Some(MonsterKind::GasBomb))
            .count();
        assert_eq!(bombs, 1, "BLOAT spawns one GasBomb (BloatAmount=1)");
        let bomb = state
            .creatures
            .iter()
            .find(|c| c.species == Some(MonsterKind::GasBomb))
            .expect("gasbomb");
        assert!(bomb.ai.is_some(), "spawned GasBomb runs its own moveset");
        // GH #366: the bomb takes a slot AHEAD of its summoner, it does not
        // append. `LivingFogNormal.Slots` puts bomb1..bomb5 before
        // "livingFog", and `AddCreature` re-sorts by slot on every add — and
        // that list IS the turn order.
        assert_eq!(
            state.enemy_uids.first().map(|u| u.0),
            Some(bomb.id),
            "the spawned bomb acts BEFORE the LivingFog that summoned it"
        );
        assert_eq!(
            state.enemy_uids.last(),
            Some(&owner),
            "and the summoner stays last"
        );
    }

    /// GH #366: bombs 2..5 keep landing between the earlier bombs and the fog,
    /// and the 6th never spawns.
    #[test]
    fn living_fog_bloat_fills_five_slots_in_order_then_stops() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::LivingFog),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("LivingFog"),
            ai: None,
        });
        state.enemy_uids.push(owner);

        for expected in 1..=LIVING_FOG_BOMB_SLOTS {
            living_fog_bloat_effect(&mut state, owner);
            assert_eq!(
                state.enemy_uids.len(),
                expected + 1,
                "bloat {expected} spawned"
            );
            assert_eq!(
                state.enemy_uids.last(),
                Some(&owner),
                "every bomb lands LEFT of the fog, so the fog stays last"
            );
        }

        // Sixth bloat: `GetNextSlot` returns "" and `BloatMove` skips the
        // spawn (LivingFog.cs:91-97). Slots are never recycled — C# only drops
        // creatures from `_enemies` at combat teardown — so killing a bomb
        // would not buy another one either.
        living_fog_bloat_effect(&mut state, owner);
        assert_eq!(
            state.enemy_uids.len(),
            LIVING_FOG_BOMB_SLOTS + 1,
            "the 6th bloat finds no free slot and spawns nothing"
        );
    }

    #[test]
    fn punch_construct_starter_dispatch() {
        // `…wb8lk.11`: v0.107.1 opener flag is StartsWithFastPunch (FAST_PUNCH),
        // and READY now follows up into FAST_PUNCH → STRONG_PUNCH → READY.
        assert_eq!(create_punch_construct_ai(false).current_move().id, "READY");
        assert_eq!(
            create_punch_construct_ai(true).current_move().id,
            "FAST_PUNCH"
        );
        let ai = create_punch_construct_ai(false);
        let follow = |id: &str| match ai.states.get(id) {
            Some(MonsterState::Move(m)) => m.follow_up,
            _ => None,
        };
        assert_eq!(follow("READY"), Some("FAST_PUNCH"));
        assert_eq!(follow("FAST_PUNCH"), Some("STRONG_PUNCH"));
        assert_eq!(follow("STRONG_PUNCH"), Some("READY"));
    }

    #[test]
    fn sewer_clam_jet_pressurize_loop() {
        // Rebased to C# SewerClam.cs: two-state loop, init JET →
        // PRESSURIZE (Strength 4 self) → JET … The prior SHELL/SNAP/
        // PEARL_SPIT moveset was fabricated.
        let mut ai = create_sewer_clam_ai();
        assert_eq!(run_three_moves(&mut ai), vec!["JET", "PRESSURIZE", "JET"]);
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "PRESSURIZE");
    }

    #[test]
    fn two_tailed_rat_slot_dispatch() {
        assert_eq!(
            create_two_tailed_rat_ai(TwoTailedRatSlot::First)
                .current_move()
                .id,
            "SCRATCH"
        );
        assert_eq!(
            create_two_tailed_rat_ai(TwoTailedRatSlot::Second)
                .current_move()
                .id,
            "DISEASE_BITE"
        );
        assert_eq!(
            create_two_tailed_rat_ai(TwoTailedRatSlot::Third)
                .current_move()
                .id,
            "SCREECH"
        );
        // `Other` opens on RAND, which resolves to a normal move (never
        // CALL_FOR_BACKUP: a fresh rat's TurnsUntilSummonable is 2).
        assert!(matches!(
            create_two_tailed_rat_ai(TwoTailedRatSlot::Other)
                .current_move()
                .id,
            "SCRATCH" | "DISEASE_BITE" | "SCREECH"
        ));
    }

    /// C# `INIT_MOVE` slot→entry mapping: first→FLAIL, second→BITE,
    /// third→LASH, fourth→ENLARGE (bead `sts2-rs-dvude`).
    #[test]
    fn phantasmal_gardener_slot_selects_entry_move() {
        assert_eq!(
            create_phantasmal_gardener_ai(PhantasmalGardenerSlot::First)
                .current_move()
                .id,
            "FLAIL"
        );
        assert_eq!(
            create_phantasmal_gardener_ai(PhantasmalGardenerSlot::Second)
                .current_move()
                .id,
            "BITE"
        );
        assert_eq!(
            create_phantasmal_gardener_ai(PhantasmalGardenerSlot::Third)
                .current_move()
                .id,
            "LASH"
        );
        assert_eq!(
            create_phantasmal_gardener_ai(PhantasmalGardenerSlot::Fourth)
                .current_move()
                .id,
            "ENLARGE"
        );
    }

    #[test]
    fn phantasmal_gardener_cycle_follows_bite_lash_flail_enlarge() {
        // Enter at BITE (second slot); from there the ring is
        // deterministic. Walk 3 steps and check BITE→LASH→FLAIL→ENLARGE.
        let mut ai = create_phantasmal_gardener_ai(PhantasmalGardenerSlot::Second);
        let order = ["BITE", "LASH", "FLAIL", "ENLARGE"];
        let start = ai.current_move().id;
        let start_idx = order
            .iter()
            .position(|m| *m == start)
            .expect("init leaf must be one of the 4 moves");
        for step in 1..=3 {
            ai.on_move_performed();
            let want = order[(start_idx + step) % 4];
            assert_eq!(ai.roll_move_seeded(0).id, want, "step {step}");
        }
    }

    #[test]
    fn skulking_colony_four_move_cycle() {
        // `…wb8lk.11`: v0.107.1 chain — ZOOM -> ZOOM_2 -> INERTIA ->
        // PIERCING_STABS -> ZOOM (SMASH removed, second Zoom added).
        let mut ai = create_skulking_colony_ai();
        let mut got = Vec::new();
        for _ in 0..5 {
            got.push(ai.roll_move_seeded(0).id);
            ai.on_move_performed();
        }
        assert_eq!(
            got,
            vec![
                "ZOOM_MOVE",
                "ZOOM_MOVE_2",
                "INERTIA_MOVE",
                "PIERCING_STABS_MOVE",
                "ZOOM_MOVE",
            ]
        );
    }

    #[test]
    fn toadpole_front_and_nonfront_cycles_match_csharp() {
        // C# v0.103.2: IsFront → SPIKEN_MOVE, !IsFront → WHIRL_MOVE; the cycle
        // is SPIKEN(Thorns+2) → SPIKE_SPIT(3×3) → WHIRL(7) → SPIKEN. Replaces
        // the fabricated TONGUE(5)/CHOMP(9) RandomBranch. Bead `sts2-rs-pn8mr`.
        let mut front = create_toadpole_ai(ToadpoleSlot::First);
        let mut got = Vec::new();
        for _ in 0..4 {
            got.push(front.roll_move_seeded(0).id);
            front.on_move_performed();
        }
        assert_eq!(
            got,
            vec![
                "SPIKEN_MOVE",
                "SPIKE_SPIT_MOVE",
                "WHIRL_MOVE",
                "SPIKEN_MOVE"
            ]
        );

        // Telegraph intents on the front cycle.
        let mut f = create_toadpole_ai(ToadpoleSlot::First);
        assert_eq!(
            f.roll_move_seeded(0).intents.as_slice(),
            &[Intent::Buff {
                kind: PowerKind::Thorns,
                amount: 2
            }]
        );
        f.on_move_performed();
        assert_eq!(
            f.roll_move_seeded(0).intents.as_slice(),
            &[Intent::MultiAttack { damage: 3, hits: 3 }]
        );
        f.on_move_performed();
        assert_eq!(
            f.roll_move_seeded(0).intents.as_slice(),
            &[Intent::Attack { damage: 7 }]
        );

        // Non-front (and the Other fallback) start at WHIRL_MOVE.
        assert_eq!(
            create_toadpole_ai(ToadpoleSlot::Second)
                .roll_move_seeded(0)
                .id,
            "WHIRL_MOVE"
        );
        assert_eq!(
            create_toadpole_ai(ToadpoleSlot::Other)
                .roll_move_seeded(0)
                .id,
            "WHIRL_MOVE"
        );
    }

    #[test]
    fn toadpole_spiken_grants_thorns_and_spike_spit_consumes_it() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 25,
            max_hp: 25,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::Toadpole),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Toadpole"),
            ai: None,
        });
        state.enemy_uids.push(owner);

        let thorns = |state: &CombatState| -> i32 {
            state.creatures[0]
                .powers
                .iter()
                .find_map(|pu| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == pu.0 && p.kind == PowerKind::Thorns)
                        .map(|p| p.amount)
                })
                .unwrap_or(0)
        };

        // SPIKEN applies Thorns +2 to self.
        toadpole_spiken_effect(&mut state, owner);
        assert_eq!(thorns(&state), 2, "SPIKEN grants Thorns(2)");

        // SPIKE_SPIT consumes it back to 0 (and the instance is removed).
        toadpole_spike_spit_effect(&mut state, owner);
        assert_eq!(thorns(&state), 0, "SPIKE_SPIT consumes the Thorns");
        assert!(
            state.creatures[0].powers.is_empty(),
            "the depleted Thorns instance is unlinked at amount 0"
        );
    }

    #[test]
    fn sneaky_gremlin_spawned_stun_then_tackle_loop() {
        // C# v0.103.2 `SneakyGremlin.GenerateMoveStateMachine`: SPAWNED
        // (StunIntent, wake-up no-damage) → TACKLE (SingleAttack 9), then
        // TACKLE self-loops (`moveState2.FollowUpState = moveState2`). Was a
        // fabricated STAB-8 self-loop with no spawn-stun. Bead `sts2-rs-kr7r7`.
        let mut ai = create_sneaky_gremlin_ai();
        let m0 = ai.roll_move_seeded(0);
        assert_eq!(m0.id, "SPAWNED");
        assert_eq!(m0.intents.as_slice(), &[Intent::Stun]);

        ai.on_move_performed();
        let m1 = ai.roll_move_seeded(0);
        assert_eq!(m1.id, "TACKLE");
        assert_eq!(m1.intents.as_slice(), &[Intent::Attack { damage: 9 }]);

        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_seeded(0).id,
            "TACKLE",
            "TACKLE follows up on itself"
        );
    }

    #[test]
    fn haunted_ship_moveset_matches_csharp() {
        // v0.107.1 `HauntedShip.GenerateMoveStateMachine`: initial HAUNT_MOVE
        // (Weak 3 + 5 Dazed) → SWIPE (Attack 13) → STOMP (4x3) → SWIPE → …
        // RAMMING_SPEED and the round-parity RandomBranch gate were removed;
        // the chain is deterministic (no RNG). Bead `…wb8lk.11`.
        let mut ai = create_haunted_ship_ai();
        // HAUNT is the Move initial (0 RNG): Weak 3 telegraph + status card.
        assert_eq!(ai.current_move().id, "HAUNT_MOVE");
        assert_eq!(
            ai.current_move().intents.as_slice(),
            &[
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 3
                },
                Intent::StatusCard
            ]
        );
        ai.on_move_performed();

        // Turn 1 HAUNT → then SWIPE ↔ STOMP alternating, HAUNT never recurs.
        let expected = ["SWIPE_MOVE", "STOMP_MOVE", "SWIPE_MOVE", "STOMP_MOVE"];
        for (i, want) in expected.iter().enumerate() {
            let m = ai.roll_move_seeded(0);
            assert_eq!(m.id, *want, "step {i}");
            match m.id {
                "SWIPE_MOVE" => {
                    assert_eq!(m.intents.as_slice(), &[Intent::Attack { damage: 13 }]);
                }
                "STOMP_MOVE" => assert_eq!(
                    m.intents.as_slice(),
                    &[Intent::MultiAttack { damage: 4, hits: 3 }]
                ),
                other => panic!("step {i}: HAUNT must not recur; got {other}"),
            }
            ai.on_move_performed();
        }
    }

    #[test]
    fn terror_eel_crash_thrash_cycle() {
        // Rebased to C# v0.103.2 (bead `sts2-rs-7iuyk`): the normal cycle is
        // CRASH_MOVE ↔ ThrashMove (was the fabricated SHOCK/COIL/SURGE).
        // STUN_MOVE → TERROR_MOVE → CRASH_MOVE is only reachable via the
        // Shriek stun trigger (core `shriek_stun_below_threshold`, a
        // damage-path force), so the free-running roll stays on the
        // crash/thrash loop.
        let mut ai = create_terror_eel_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["CRASH_MOVE", "ThrashMove", "CRASH_MOVE"]
        );
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "ThrashMove");
    }

    #[test]
    fn waterfall_giant_pressure_loop_chain() {
        // Rebased to C# v0.103.2 (bead `sts2-rs-k3pxa`): PRESSURIZE → STOMP →
        // RAM → SIPHON → PRESSURE_GUN → PRESSURE_UP → STOMP (loops back to
        // STOMP, not PRESSURIZE). Was the fabricated ROAR/SLAM/CRUSH/TORRENT.
        let mut ai = create_waterfall_giant_ai();
        assert_eq!(run_three_moves(&mut ai), vec!["PRESSURIZE", "STOMP", "RAM"]);
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "SIPHON");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "PRESSURE_GUN");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "PRESSURE_UP");
        // PRESSURE_UP loops back to STOMP (not PRESSURIZE).
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "STOMP");
    }

    #[test]
    fn soul_fysh_three_chain() {
        // Rebased to C# v0.103.2 (bead `sts2-rs-fcz8k`): the cycle now opens
        // BECKON → DE_GAS → GAZE (was the fabricated CHOMP/SOUL_DRAIN/DEEP_DIVE).
        let mut ai = create_soul_fysh_ai();
        assert_eq!(run_three_moves(&mut ai), vec!["BECKON", "DE_GAS", "GAZE"]);
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "FADE");
    }

    /// Bead acceptance: LagavulinMatriarch sleep→wake transition locked
    /// by deterministic test walking ≥5 turns.
    #[test]
    fn lagavulin_sleep_then_wake_parity() {
        // GH #362: the branch reads the boss's own AsleepPower, so the sleep
        // window is driven by the real per-turn tick rather than the old
        // `combat.round < 3` proxy. Three enemy-side turn-ends strip the
        // power, and the machine falls through to the attack chain by itself.
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut combat = CombatState::default();
        combat.player.current_hp = 70;
        combat.player.max_hp = 70;
        combat.creatures.push(Creature {
            id: owner.0,
            current_hp: 90,
            max_hp: 90,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::LagavulinMatriarch),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("LagavulinMatriarch"),
            ai: None,
        });
        combat.enemy_uids.push(owner);
        // Spawn kit: Plating(12) + Asleep(3) (`monsters.rs:1939`).
        sts2_core::card_effects::apply_power_to_creature(
            &mut combat,
            owner,
            PowerKind::Plating,
            12,
        );
        sts2_core::card_effects::apply_power_to_creature(&mut combat, owner, PowerKind::Asleep, 3);

        let mut ai = create_lagavulin_matriarch_ai();
        ai.set_owner(owner);
        let mut trace = Vec::new();
        for turn in 0u64..10 {
            trace.push(ai.current_move().id);
            ai.on_move_performed();
            // The enemy-side turn end is what ticks Asleep.
            sts2_core::hooks::dispatch_after_turn_end(
                &mut combat,
                &sts2_core::hooks::TurnSideCtx {
                    side: sts2_core::Side::Enemy,
                },
            );
            ai.roll_move_with_combat_seeded(turn, &combat);
        }
        assert_eq!(
            trace,
            vec![
                "SLEEP",
                "SLEEP",
                "SLEEP",
                "SLASH",
                "DISEMBOWEL",
                "SLASH2",
                "SOUL_SIPHON",
                "SLASH",
                "DISEMBOWEL",
                "SLASH2",
            ],
            "Asleep(3) holds the SLEEP loop for three of the boss's own turns, \
             then the chain plays out"
        );
    }

    /// GH #362 — the tick itself: ASLEEP 3 → 2 → 1 → gone, with Plating shed
    /// on the last sleeping turn.
    ///
    /// The world ticks exactly this; pre-fix nothing decremented Asleep at all
    /// (the only `PowerKind::Asleep` sites in the workspace were the spawn
    /// table and the damage-wake), so the mirror sat +1 at every snapshot and
    /// still held ASLEEP 1 + PLATING 12 on the wake turn.
    #[test]
    fn lagavulin_asleep_ticks_down_and_sheds_plating_on_the_last_turn() {
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 90,
            max_hp: 90,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::LagavulinMatriarch),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("LagavulinMatriarch"),
            ai: None,
        });
        state.enemy_uids.push(owner);
        sts2_core::card_effects::apply_power_to_creature(&mut state, owner, PowerKind::Plating, 12);
        sts2_core::card_effects::apply_power_to_creature(&mut state, owner, PowerKind::Asleep, 3);

        let end_enemy_turn = |s: &mut CombatState| {
            sts2_core::hooks::dispatch_after_turn_end(
                s,
                &sts2_core::hooks::TurnSideCtx {
                    side: sts2_core::Side::Enemy,
                },
            );
        };

        end_enemy_turn(&mut state);
        assert_eq!(creature_power_amount(&state, owner, PowerKind::Asleep), 2);
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Plating),
            11,
            "Plating survives while Asleep > 1 — it is only shedding its own \
             per-turn point of decay"
        );

        end_enemy_turn(&mut state);
        assert_eq!(creature_power_amount(&state, owner, PowerKind::Asleep), 1);
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Plating),
            10,
            "this is the PLATING 10 the mirror was still holding on the wake \
             turn in the live Necrobinder A1 corpus"
        );

        // Last sleeping turn: `BeforeSideTurnEndVeryEarly` sheds Plating at
        // Amount <= 1, then the decrement takes Asleep to 0 and removes it.
        end_enemy_turn(&mut state);
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Asleep),
            0,
            "Asleep is gone after three of the boss's turn-ends"
        );
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Plating),
            0,
            "and the Plating went with it"
        );
        // And crucially the BLOCK goes with it too. VeryEarly runs before
        // Early, so on the wake turn the Plating is already gone when
        // `PlatingPower.BeforeTurnEndEarly` would have granted — C# ends the
        // turn on 0 block, not 10.
        //
        // Asserting only the POWER amount above was not enough: Rust removed
        // the power but had already granted its block, so the boss carried 10
        // block C# had shed and every later attack under-damaged by up to 10
        // for the rest of the fight. Caught by the v2 fuzz axis (seed 445:
        // cs 204 vs rs 214, and the gap never closed).
        let block = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .map_or(-1, |c| c.block);
        assert_eq!(block, 0, "the wake turn grants no Plating block");
    }

    // ────────────────────────────────────────────────────────────────────
    // Telegraph-only Buff/Debuff power-application tests (bead sts2-rs-pdsdl)
    // ────────────────────────────────────────────────────────────────────

    /// Build a CombatState with a single monster Creature at uid 1.
    fn combat_with_monster(species: sts2_core::MonsterKind, name: &'static str) -> CombatState {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, PowerUid};
        let mut combat = CombatState::default();
        let uid = NonZeroU32::new(1).expect("nz");
        combat.creatures.push(Creature {
            id: uid,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: Some(species),
            monster_id: MonsterId(name),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        combat.enemy_uids.push(CreatureUid(uid));
        combat
    }

    fn self_power(combat: &CombatState, kind: PowerKind) -> i32 {
        let owner = std::num::NonZeroU32::new(1).expect("nz");
        combat
            .creatures
            .iter()
            .find(|c| c.id == owner)
            .expect("monster present")
            .powers
            .iter()
            .filter_map(|puid| {
                combat
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == kind)
                    .map(|p| p.amount)
            })
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

    #[test]
    fn gremlin_merc_double_smash_applies_weak_two_to_player() {
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        gremlin_merc_double_smash_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Weak), 2);
    }

    #[test]
    fn gremlin_merc_hehe_applies_strength_two_to_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::GremlinMerc, "GremlinMerc");
        gremlin_merc_hehe_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Strength), 2);
    }

    #[test]
    fn corpse_slug_goop_applies_frail_two_to_player() {
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        corpse_slug_goop_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Frail), 2);
    }

    #[test]
    fn phantasmal_gardener_enlarge_applies_strength_two_to_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::PhantasmalGardener, "PhantasmalGardener");
        phantasmal_gardener_enlarge_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Strength), 2);
    }

    #[test]
    fn skulking_colony_inertia_applies_strength_two_to_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::SkulkingColony, "SkulkingColony");
        skulking_colony_inertia_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Strength), 2);
    }

    /// `…wb8lk.11`: v0.107.1 SkulkingColony Zoom no longer grants Block
    /// (the `ZoomBlock` Defend rider was removed) and the colony now opens
    /// with two Zooms (ZOOM_MOVE → ZOOM_MOVE_2) before INERTIA. SMASH_MOVE
    /// was deleted.
    #[test]
    fn skulking_colony_v107_moveset_no_smash_double_zoom() {
        let ai = create_skulking_colony_ai();
        assert_eq!(ai.initial, "ZOOM_MOVE");
        assert!(
            !ai.states.contains_key("SMASH_MOVE"),
            "SMASH_MOVE removed in v0.107.1"
        );
        assert!(ai.states.contains_key("ZOOM_MOVE_2"), "second Zoom added");
        let follow = |id: &str| match ai.states.get(id) {
            Some(MonsterState::Move(m)) => m.follow_up,
            _ => None,
        };
        assert_eq!(follow("ZOOM_MOVE"), Some("ZOOM_MOVE_2"));
        assert_eq!(follow("ZOOM_MOVE_2"), Some("INERTIA_MOVE"));
        assert_eq!(follow("PIERCING_STABS_MOVE"), Some("ZOOM_MOVE"));
        // Zoom is a plain attack now — no secondary Defend intent.
        let Some(MonsterState::Move(zoom)) = ai.states.get("ZOOM_MOVE") else {
            panic!("ZOOM_MOVE missing");
        };
        assert!(
            !zoom
                .intents
                .iter()
                .any(|i| matches!(i, Intent::Defend { .. })),
            "Zoom no longer grants Block"
        );
    }

    /// Bead `sts2-rs-viwkh`: LagavulinMatriarch SLASH2_MOVE's secondary
    /// `Intent::Defend{12}` is dropped by `perform_intent`, so the
    /// effect_fn grants the 12 block (C# `Slash2Block`=12).
    #[test]
    fn lagavulin_matriarch_slash2_gains_twelve_block() {
        use sts2_core::{CreatureUid, MonsterKind};
        let owner = CreatureUid::new(1).expect("nz");
        let mut combat = combat_with_monster(MonsterKind::LagavulinMatriarch, "LagavulinMatriarch");
        let block_of = |c: &CombatState| {
            c.creatures
                .iter()
                .find(|cr| cr.id == owner.0)
                .map_or(0, |cr| cr.block)
        };
        assert_eq!(block_of(&combat), 0, "no block before SLASH2");
        lagavulin_matriarch_slash2_effect(&mut combat, owner);
        assert_eq!(block_of(&combat), 12, "SLASH2 grants 12 block to self");
    }

    #[test]
    fn calcified_cultist_incantation_applies_ritual_two_to_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::CalcifiedCultist, "CalcifiedCultist");
        calcified_cultist_incantation_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Ritual), 2);
    }

    #[test]
    fn damp_cultist_incantation_applies_ritual_five_to_self() {
        use sts2_core::{CreatureUid, MonsterKind};
        let mut combat = combat_with_monster(MonsterKind::DampCultist, "DampCultist");
        damp_cultist_incantation_effect(&mut combat, CreatureUid::new(1).expect("nz"));
        assert_eq!(self_power(&combat, PowerKind::Ritual), 5);
    }

    #[test]
    fn fossil_stalker_tackle_applies_frail_one_to_player() {
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        fossil_stalker_tackle_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Frail), 1);
    }

    #[test]
    fn punch_construct_fast_punch_applies_frail_one_to_player() {
        // `…wb8lk.11`: v0.107.1 FastPunch applies Frail (was Weak).
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        punch_construct_fast_punch_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power(&state, PowerKind::Frail), 1);
        assert_eq!(player_power(&state, PowerKind::Weak), 0);
    }

    // ── TwoTailedRat summoner (sts2-rs-dvude) ──────────────────────────

    /// Bead `sts2-rs-dvude`: the moveset matches C# `TwoTailedRat.cs` —
    /// SCRATCH (Attack 8), DISEASE_BITE (Attack 6), SCREECH (Frail
    /// telegraph), CALL_FOR_BACKUP (Summon). Replaces the fabricated
    /// GNAW/TAIL_WHIP stub.
    #[test]
    fn two_tailed_rat_moveset_matches_csharp() {
        use sts2_core::monster_ai::MonsterState;
        let ai = create_two_tailed_rat_ai(TwoTailedRatSlot::First);
        let intent_of = |id: &str| match &ai.states[id] {
            MonsterState::Move(m) => m.intents.first().copied(),
            _ => None,
        };
        assert_eq!(intent_of("SCRATCH"), Some(Intent::Attack { damage: 8 }));
        assert_eq!(
            intent_of("DISEASE_BITE"),
            Some(Intent::Attack { damage: 6 })
        );
        assert_eq!(
            intent_of("SCREECH"),
            Some(Intent::Debuff {
                kind: PowerKind::Frail,
                amount: 1,
            })
        );
        assert_eq!(intent_of("CALL_FOR_BACKUP"), Some(Intent::Summon));
        // The fabricated stub's moves must be gone.
        assert!(!ai.states.contains_key("GNAW"));
        assert!(!ai.states.contains_key("TAIL_WHIP"));
    }

    /// C# `_turnsUntilSummonable = 2`: a fresh rat (incl. a summoned one)
    /// needs two moves before becoming summon-eligible.
    #[test]
    fn two_tailed_rat_starts_with_summon_delay_two() {
        use sts2_core::monster_ai::MonsterAi;
        assert_eq!(
            MonsterAi::counter(&create_two_tailed_rat_ai(TwoTailedRatSlot::Other)),
            2
        );
    }

    /// Helper: a combat seeded with `n` alive TwoTailedRats, each with a
    /// full AI attached (so the teammate-telegraph guard can read them).
    ///
    /// Slots are stamped the way the encounter does it (GH #552). The real
    /// `TwoTailedRatsNormal` seeds its three rats into slots 2/3/4 of five,
    /// leaving 1 and 0 for summons; an `n`-rat helper fills the TOP `n` slots
    /// so the free ones are always the low indices, which is what
    /// `CallForBackup`'s `LastOrDefault` rule consumes. Without the stamp the
    /// summon sees an all-free board and the slot cap never engages.
    fn combat_with_n_rats(n: usize) -> (CombatState, Vec<sts2_core::CreatureUid>) {
        let mut state = CombatState::default();
        let mut uids = Vec::new();
        for _ in 0..n {
            let uid = state.spawn_enemy(sts2_core::MonsterKind::TwoTailedRat);
            state.set_creature_ai(uid, create_two_tailed_rat_ai(TwoTailedRatSlot::First));
            uids.push(uid);
        }
        // Over-full boards (used to exercise the backup-COUNT gate, which
        // binds before the slot gate) have no valid slot assignment — the
        // encounter only has five. Leave those unstamped; the count gate is
        // what those tests read.
        if n <= usize::from(TWO_TAILED_RAT_SLOTS) {
            let slots: Vec<u8> = (0..n)
                .map(|i| TWO_TAILED_RAT_SLOTS - u8::try_from(n - i).expect("n <= 5"))
                .collect();
            state.assign_encounter_slots(&slots);
        }
        (state, uids)
    }

    /// C# `CanSummon`: `TurnsUntilSummonable > 0` blocks summoning.
    #[test]
    fn two_tailed_rat_cannot_summon_while_timer_positive() {
        let (state, uids) = combat_with_n_rats(3);
        assert!(!two_tailed_rat_can_summon(&state, uids[0], 2));
        assert!(!two_tailed_rat_can_summon(&state, uids[0], 1));
        // Timer elapsed, 3 alive of 5 slots, 0 prior backups → eligible.
        assert!(two_tailed_rat_can_summon(&state, uids[0], 0));
    }

    /// C# `GetNextSlot` empty (all 5 slots filled) blocks summoning.
    #[test]
    fn two_tailed_rat_cannot_summon_when_slots_full() {
        let (state, uids) = combat_with_n_rats(5);
        assert!(
            !two_tailed_rat_can_summon(&state, uids[0], 0),
            "5 alive == 5 slots, no free slot"
        );
    }

    /// C# `CallForBackupCount >= 3` blocks summoning. Derived as
    /// `(#rats) - 3`; 6 rats (3 of them corpses, freeing slots) means 3
    /// backups already spent.
    #[test]
    fn two_tailed_rat_cannot_summon_at_backup_limit() {
        let (mut state, uids) = combat_with_n_rats(6);
        // Kill three so the slot gate is open but the count gate binds.
        for &uid in &uids[3..6] {
            if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
                c.current_hp = 0;
            }
        }
        assert_eq!(two_tailed_rat_alive_enemies(&state), 3, "3 alive of 6");
        assert!(
            !two_tailed_rat_can_summon(&state, uids[0], 0),
            "6 rats == 3 backups spent == limit"
        );
    }

    /// CALL_FOR_BACKUP spawns another TwoTailedRat (with its own AI) when
    /// a slot is free, and is a no-op when all 5 slots are filled.
    #[test]
    fn two_tailed_rat_call_for_backup_spawns_rat() {
        let (mut state, uids) = combat_with_n_rats(3);
        two_tailed_rat_call_for_backup_effect(&mut state, uids[0]);
        let rat_count = state
            .creatures
            .iter()
            .filter(|c| c.species == Some(sts2_core::MonsterKind::TwoTailedRat))
            .count();
        assert_eq!(rat_count, 4, "summon adds a fourth rat");
        // GH #552: the newcomer takes the LAST FREE slot (1, with the seeds in
        // 2/3/4) and therefore lands at the FRONT of the turn order, not the
        // back. Asserting the position is the point — appending gave the same
        // rat count while handing the wrong monster each `monster_ai` draw.
        let newest = state.enemy_uids[0];
        assert!(
            !uids.contains(&newest),
            "the summon sorts ahead of all three seeded rats"
        );
        assert_eq!(state.enemy_slot(newest), Some(1), "last free slot of 0..=4");
        let ai_counter = state
            .creatures
            .iter()
            .find(|c| c.id == newest.0)
            .and_then(|c| c.ai.as_ref().map(sts2_core::monster_ai::MonsterAi::counter));
        assert_eq!(ai_counter, Some(2), "summoned rat starts with delay 2");

        // Fill to 5, then a further call is a no-op (slot cap).
        let (mut full, uids_full) = combat_with_n_rats(5);
        two_tailed_rat_call_for_backup_effect(&mut full, uids_full[0]);
        assert_eq!(
            full.creatures
                .iter()
                .filter(|c| c.species == Some(sts2_core::MonsterKind::TwoTailedRat))
                .count(),
            5,
            "no summon when slots are full"
        );
    }

    /// SCREECH applies Frail 1 to the player and decrements the summon
    /// timer; SCRATCH decrements the timer without touching the player.
    #[test]
    fn two_tailed_rat_screech_applies_frail_and_ticks_timer() {
        let (mut state, uids) = combat_with_n_rats(1);
        let counter_before = state
            .creatures
            .iter()
            .find(|c| c.id == uids[0].0)
            .and_then(|c| c.ai.as_ref().map(sts2_core::monster_ai::MonsterAi::counter));
        assert_eq!(counter_before, Some(2));

        two_tailed_rat_screech_effect(&mut state, uids[0]);
        assert_eq!(player_power(&state, PowerKind::Frail), 1);
        let counter_after = state
            .creatures
            .iter()
            .find(|c| c.id == uids[0].0)
            .and_then(|c| c.ai.as_ref().map(sts2_core::monster_ai::MonsterAi::counter));
        assert_eq!(counter_after, Some(1), "screech ticks the summon timer");

        two_tailed_rat_scratch_effect(&mut state, uids[0]);
        let counter_scratch = state
            .creatures
            .iter()
            .find(|c| c.id == uids[0].0)
            .and_then(|c| c.ai.as_ref().map(sts2_core::monster_ai::MonsterAi::counter));
        assert_eq!(counter_scratch, Some(0), "scratch ticks too");
        // No further Frail from scratch.
        assert_eq!(player_power(&state, PowerKind::Frail), 1);
    }

    /// Kills `two_tailed_rat_disease_bite_effect -> ()`: DISEASE_BITE ticks
    /// the summon timer (C# `TurnsUntilSummonable--`) exactly like SCRATCH.
    #[test]
    fn two_tailed_rat_disease_bite_ticks_summon_timer() {
        let (mut state, uids) = combat_with_n_rats(1);
        // Fresh rat starts at summon delay 2; one DISEASE_BITE drops it to 1.
        two_tailed_rat_disease_bite_effect(&mut state, uids[0]);
        let counter = state
            .creatures
            .iter()
            .find(|c| c.id == uids[0].0)
            .and_then(|c| c.ai.as_ref().map(sts2_core::monster_ai::MonsterAi::counter));
        assert_eq!(counter, Some(1), "DISEASE_BITE ticks the summon timer");
    }

    /// Kills the teammate-summoning guard mutations in
    /// `two_tailed_rat_can_summon` (lines 1052-1054): a LIVE teammate rat
    /// already in `CALL_FOR_BACKUP` blocks this rat from also summoning (C#
    /// `CallForBackupCount` cross-rat sync). Covers `c.id != owner`,
    /// `c.current_hp > 0` (`==`/`<` arms), and `c.species == TwoTailedRat`.
    #[test]
    fn two_tailed_rat_live_teammate_in_call_for_backup_blocks_summon() {
        let (mut state, uids) = combat_with_n_rats(2);
        // Owner is eligible on its own (timer 0, 2/5 slots, 0 prior backups)
        // — confirming the teammate is not yet counted as summoning.
        assert!(two_tailed_rat_can_summon(&state, uids[0], 0));
        // Put the *teammate* into CALL_FOR_BACKUP with positive HP.
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uids[1].0) {
            c.current_hp = 5;
            assert!(c
                .ai
                .as_mut()
                .expect("teammate ai")
                .force_current_state_by_name("CALL_FOR_BACKUP"));
        }
        assert!(
            !two_tailed_rat_can_summon(&state, uids[0], 0),
            "a live teammate already summoning blocks this rat",
        );
    }

    /// Kills the `c.current_hp > 0` → `>=` arm of the teammate guard: a
    /// DEAD teammate (hp 0) sitting in `CALL_FOR_BACKUP` must NOT count as
    /// summoning, so this rat stays eligible.
    #[test]
    fn two_tailed_rat_dead_teammate_in_call_for_backup_does_not_block_summon() {
        let (mut state, uids) = combat_with_n_rats(2);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uids[1].0) {
            c.current_hp = 0;
            assert!(c
                .ai
                .as_mut()
                .expect("teammate ai")
                .force_current_state_by_name("CALL_FOR_BACKUP"));
        }
        assert!(
            two_tailed_rat_can_summon(&state, uids[0], 0),
            "a dead (hp 0) teammate does not block summoning",
        );
    }

    /// Kills `terror_eel_thrash_effect -> ()`: THRASH self-buffs Vigor(6)
    /// (C# `PowerCmd.Apply<VigorPower>(self, 6m)`, TerrorEel.cs:108).
    #[test]
    fn terror_eel_thrash_buffs_self_vigor() {
        let (mut state, owner) = combat_with_one_enemy();
        terror_eel_thrash_effect(&mut state, owner);
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Vigor),
            6,
            "THRASH grants the eel 6 Vigor",
        );
    }

    /// Kills `terror_eel_terror_effect -> ()`: TERROR applies Vulnerable(99)
    /// to the player (C# `Apply<VulnerablePower>(player, 99m)`, TerrorEel.cs:122).
    #[test]
    fn terror_eel_terror_applies_vulnerable_to_player() {
        let (mut state, owner) = combat_with_one_enemy();
        terror_eel_terror_effect(&mut state, owner);
        assert_eq!(
            player_power_amount(&state, PowerKind::Vulnerable),
            99,
            "TERROR applies 99 Vulnerable to the player",
        );
    }

    /// Kills `soul_fysh_gaze_effect -> ()`: GAZE's status leg adds one
    /// Beckon card to the Discard pile (C# `StatusIntent(GazeMoveAmount=1)`).
    #[test]
    fn soul_fysh_gaze_adds_one_beckon_to_discard() {
        use sts2_core::cards::CardId;
        let (mut state, owner) = combat_with_one_enemy();
        soul_fysh_gaze_effect(&mut state, owner);
        let beckons = state
            .cards
            .iter()
            .filter(|c| c.card_id == CardId::Beckon)
            .count();
        assert_eq!(beckons, 1, "GAZE spawns one Beckon status card");
        assert_eq!(state.player.discard.len(), 1, "the Beckon goes to Discard");
        assert!(state.player.draw.is_empty(), "GAZE adds nothing to Draw");
    }

    /// Kills `waterfall_giant_explode_damage` `&&`→`||` (line 1636): a
    /// non-SteamEruption power on the giant must NOT be read as explode
    /// damage. With `||` the reader would wrongly return the Strength
    /// amount; correct `&&` requires both id AND kind to match.
    #[test]
    fn waterfall_giant_explode_damage_ignores_non_steam_powers() {
        let (mut state, owner) = combat_with_one_enemy();
        // Strength on the giant, but NO SteamEruption.
        sts2_core::card_effects::apply_power_to_creature(&mut state, owner, PowerKind::Strength, 5);
        assert_eq!(
            waterfall_giant_explode_damage(&state, owner),
            0,
            "a non-Steam power must not be read as explode damage",
        );
    }

    /// Kills the power-stripping mutations in
    /// `waterfall_giant_about_to_blow_effect` (lines 1676/1681/1683): the
    /// move must remove ONLY SteamEruption from the owner and the arena,
    /// leaving co-resident non-Steam powers (here Strength) intact.
    #[test]
    fn waterfall_giant_about_to_blow_strips_only_steam_eruption() {
        let owner = CreatureUid::new(1).expect("nz");
        let mut state = CombatState::default();
        state.creatures.push(Creature {
            id: owner.0,
            current_hp: 240,
            max_hp: 240,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::WaterfallGiant),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("WaterfallGiant"),
            ai: Some(create_waterfall_giant_ai()),
        });
        state.enemy_uids.push(owner);

        // Accumulate SteamEruption(15) plus a co-resident Strength(5).
        waterfall_giant_pressurize_effect(&mut state, owner);
        sts2_core::card_effects::apply_power_to_creature(&mut state, owner, PowerKind::Strength, 5);
        assert_eq!(waterfall_giant_explode_damage(&state, owner), 15);
        assert_eq!(creature_power_amount(&state, owner, PowerKind::Strength), 5);

        waterfall_giant_about_to_blow_effect(&mut state, owner);

        // SteamEruption gone from both owner.powers and the arena …
        assert_eq!(
            waterfall_giant_explode_damage(&state, owner),
            0,
            "SteamEruption removed from owner + arena",
        );
        // … but Strength is untouched (kills the `||`, inverted-retain, and
        // `==`-arena-retain mutants, all of which would drop Strength too).
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Strength),
            5,
            "ABOUT_TO_BLOW must not strip non-Steam powers",
        );
    }

    /// Spawn a bare enemy creature with `ai` installed and its opening
    /// telegraph snapshotted, at the given uid. Shared scaffolding for the
    /// Shriek / Asleep / Ravenous reaction tests below (mirrors the
    /// ThievingHopper Flutter test's discipline: no player creature is
    /// initialized, so enemy uid 1 can't collide with the player mirror).
    fn spawn_enemy_with_ai(
        state: &mut CombatState,
        raw_uid: u32,
        kind: MonsterKind,
        monster_id: &'static str,
        hp: i32,
        ai: Option<MonsterAi>,
    ) -> CreatureUid {
        let uid = CreatureUid::new(raw_uid).expect("nonzero");
        state.creatures.push(Creature {
            id: uid.0,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(kind),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId(monster_id),
            ai: None,
        });
        state.enemy_uids.push(uid);
        if let Some(mut ai) = ai {
            ai.set_owner(uid);
            let intent = ai.current_move().intents.first().copied();
            if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
                c.ai = Some(ai);
                c.intent = intent;
            }
        }
        uid
    }

    fn current_state(state: &CombatState, uid: CreatureUid) -> Option<&'static str> {
        state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .and_then(|c| c.ai.as_ref())
            .map(sts2_core::MonsterAi::current_state_id)
    }

    fn creature_intent(state: &CombatState, uid: CreatureUid) -> Option<Intent> {
        state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .and_then(|c| c.intent)
    }

    /// **ShriekPower threshold stun** (C# `ShriekPower.cs:25-33`, mirror_live2
    /// TerrorEel rd2/rd4): unblocked damage that leaves the eel at
    /// `current_hp <= Shriek(70)` removes Shriek and stuns it into
    /// STUN_MOVE (→ TERROR_MOVE, TerrorEel.cs:73/77). Damage that leaves it
    /// ABOVE the threshold does nothing.
    #[test]
    fn terror_eel_shriek_stuns_at_hp_threshold() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let uid = spawn_enemy_with_ai(
            &mut state,
            1,
            MonsterKind::TerrorEel,
            "TerrorEel",
            140,
            Some(create_terror_eel_ai()),
        );
        sts2_core::card_effects::apply_power_to_creature(&mut state, uid, PowerKind::Shriek, 70);

        // 140 → 100: above the threshold — no stun, Shriek intact.
        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            40,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(creature_power_amount(&state, uid, PowerKind::Shriek), 70);
        assert_eq!(current_state(&state, uid), Some("CRASH_MOVE"));

        // 100 → 60 ≤ 70: Shriek removed, eel stunned into STUN_MOVE.
        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            40,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            creature_power_amount(&state, uid, PowerKind::Shriek),
            0,
            "Shriek is removed when the threshold trips",
        );
        assert_eq!(current_state(&state, uid), Some("STUN_MOVE"));
        assert_eq!(
            creature_intent(&state, uid),
            Some(Intent::Stun),
            "the stale attack telegraph is re-pointed at the stun",
        );
        // Post-stun resume: STUN_MOVE's machine follow-up is TERROR_MOVE.
        {
            let c = state.creatures.iter_mut().find(|c| c.id == uid.0).unwrap();
            let ai = c.ai.as_mut().unwrap();
            ai.on_move_performed();
            assert_eq!(ai.roll_move_seeded(0).id, "TERROR_MOVE");
        }
    }

    /// **AsleepPower damage wake** (C# `AsleepPower.cs:21-36`, mirror_live2
    /// LagavulinMatriarch rd1/rd2): a fully blocked hit does NOT wake the
    /// boss; the first unblocked point of damage strips Plating + Asleep and
    /// stuns her into WAKE_UP_MOVE, whose follow-up is SLASH (C#
    /// `Stun(Owner, WakeUpMove, "SLASH_MOVE")`).
    #[test]
    fn lagavulin_matriarch_wakes_stunned_on_unblocked_damage() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let uid = spawn_enemy_with_ai(
            &mut state,
            1,
            MonsterKind::LagavulinMatriarch,
            "LagavulinMatriarch",
            220,
            Some(create_lagavulin_matriarch_ai()),
        );
        sts2_core::card_effects::apply_power_to_creature(&mut state, uid, PowerKind::Plating, 12);
        sts2_core::card_effects::apply_power_to_creature(&mut state, uid, PowerKind::Asleep, 3);

        // Fully blocked hit: UnblockedDamage == 0 — still asleep.
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.block = 20;
        }
        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            5,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            creature_power_amount(&state, uid, PowerKind::Asleep),
            3,
            "a Plating-blocked hit must not wake the boss",
        );
        assert_eq!(current_state(&state, uid), Some("SLEEP"));

        // Unblocked hit: wake — Asleep AND Plating stripped, stun telegraphed.
        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            20,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(creature_power_amount(&state, uid, PowerKind::Asleep), 0);
        assert_eq!(
            creature_power_amount(&state, uid, PowerKind::Plating),
            0,
            "Plating is removed immediately on the damage wake (AsleepPower.cs:25-28)",
        );
        assert_eq!(current_state(&state, uid), Some("WAKE_UP_MOVE"));
        assert_eq!(creature_intent(&state, uid), Some(Intent::Stun));
        // Post-stun resume: WAKE_UP_MOVE's follow-up is SLASH.
        {
            let c = state.creatures.iter_mut().find(|c| c.id == uid.0).unwrap();
            let ai = c.ai.as_mut().unwrap();
            ai.on_move_performed();
            assert_eq!(ai.roll_move_seeded(0).id, "SLASH");
        }
    }

    /// **RavenousPower ally-death devour** (C# `RavenousPower.cs:24-36`,
    /// mirror_live2 CorpseSlug rd2 in both live runs): when an ALLY dies, the
    /// slug gains Strength = the Ravenous amount and is stunned for one turn,
    /// after which it RE-PERFORMS the move it had telegraphed (null
    /// `nextMoveId` → `StateLog.Last().Id`, Creature.cs `StunInternal`).
    /// Ravenous itself is not consumed.
    #[test]
    fn corpse_slug_ravenous_ally_death_stuns_and_strengthens() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        // starter_idx 1 → the slug telegraphs GLOMP (the interrupted move).
        let slug = spawn_enemy_with_ai(
            &mut state,
            1,
            MonsterKind::CorpseSlug,
            "CorpseSlug",
            40,
            Some(create_corpse_slug_ai(1)),
        );
        let ally = spawn_enemy_with_ai(
            &mut state,
            2,
            MonsterKind::SludgeSpinner,
            "SludgeSpinner",
            5,
            None,
        );
        sts2_core::card_effects::apply_power_to_creature(&mut state, slug, PowerKind::Ravenous, 4);
        assert_eq!(current_state(&state, slug), Some("GLOMP"));

        // Kill the ally: the slug devours it.
        apply_damage(
            &mut state,
            CreatureRef::Player,
            ally,
            10,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            creature_power_amount(&state, slug, PowerKind::Strength),
            4,
            "the devour grants Strength = the Ravenous amount",
        );
        assert_eq!(
            creature_power_amount(&state, slug, PowerKind::Ravenous),
            4,
            "Ravenous is NOT consumed by the devour",
        );
        assert_eq!(current_state(&state, slug), Some("STUNNED"));
        assert_eq!(creature_intent(&state, slug), Some(Intent::Stun));
        // Post-stun resume: the INTERRUPTED telegraph (GLOMP), not the
        // static follow-up.
        {
            let c = state.creatures.iter_mut().find(|c| c.id == slug.0).unwrap();
            let ai = c.ai.as_mut().unwrap();
            assert_eq!(ai.forced_next_state(), Some("GLOMP"));
            ai.on_move_performed();
            assert_eq!(ai.roll_move_seeded(0).id, "GLOMP");
        }
    }

    /// **HardenedShellPower per-turn cap** (C# `HardenedShellPower.cs:32-43`
    /// cap, `:50-68` accumulator, `:70-76` reset; mirror_live2 SkulkingColony
    /// rd1/rd2 — the mirror overdealt by 12/5 where the world's shell had
    /// exhausted its per-turn budget): unblocked HP loss is capped at
    /// `Amount - damageReceivedThisTurn`, saturates to 0 within the turn, and
    /// the budget refreshes at every side turn start.
    #[test]
    fn hardened_shell_caps_per_turn_hp_loss_and_resets_each_side_turn() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, TurnSideCtx, ValueProps};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let uid = spawn_enemy_with_ai(
            &mut state,
            1,
            MonsterKind::SkulkingColony,
            "SkulkingColony",
            75,
            None,
        );
        sts2_core::card_effects::apply_power_to_creature(
            &mut state,
            uid,
            PowerKind::HardenedShell,
            20,
        );

        // Hit 1 (12): under the 20 budget — full damage.
        let loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            12,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(loss, 12, "first hit passes untouched (12 < 20 budget)");
        // Hit 2 (15): only 20-12=8 of the budget remains.
        let loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            15,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(loss, 8, "second hit is capped at the remaining budget");
        // Hit 3: budget exhausted — 0.
        let loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            9,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(loss, 0, "the shell absorbs everything past its budget");
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .map(|c| c.current_hp),
            Some(75 - 20),
            "exactly Amount HP lost this turn in total",
        );

        // Side turn start refreshes the budget (C# resets on EVERY side).
        sts2_core::hooks::dispatch_before_side_turn_start(&mut state, &TurnSideCtx::default());
        let loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            10,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(loss, 10, "the absorption budget refreshes each side turn");
    }
}
