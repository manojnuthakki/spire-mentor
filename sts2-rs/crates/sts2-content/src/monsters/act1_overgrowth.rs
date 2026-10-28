// Doc comments use upper-snake-case DSL state names (e.g. `CHARGE_UP`,
// `RIP_AND_TEAR`) that clippy flags as missing-backticks identifiers. They
// are state-id strings, not Rust paths.
#![allow(clippy::doc_markdown)]

//! Act-1 monster ports (bead sts2-rs-05f).
//!
//! Ported from sts2-rl-agent@01c094d:sts2_env/monsters/act1.py
//!
//! Ports the 22 monsters from
//! `sts2-rl-agent@01c094d:sts2_env/monsters/act1.py` (685 LOC) to
//! data-driven `MonsterAi` instances using the DSL skeleton from P5-1
//! (`sts2-rs-gqy`). Conventions match [`super::act1_weak`]:
//!
//! - Effect bodies are no-ops; intent metadata is preserved.
//! - Constructors return `MonsterAi` only (not `(Creature, MonsterAi)`)
//!   until the dispatcher integration bead lands.
//! - `ConditionalBranchState`s that depend on construction-time captures
//!   (e.g. `CeremonialBeast`'s "is Plow broken?" check) collapse to the
//!   default branch — the DSL's `fn(&CombatState) -> bool` is a function
//!   pointer with no closure capture. The behavior is documented inline
//!   with a TODO referencing the bead that should wire it up.
//! - `Intent::Summon` (Python `IntentType.SUMMON`) landed via bead
//!   `sts2-rs-47p5`; Fogmog ILLUSION + Vantom CONSUME both route to
//!   `Intent::Summon` now (previously stubbed as `Intent::Unknown`).

use std::collections::BTreeMap;

use smallvec::smallvec;
use sts2_core::monster_ai::{
    ConditionFn, ConditionalBranch, ConditionalBranchState, MonsterAi, MonsterState,
    MoveRepeatType, MoveState, RandomBranchState, WeightedBranch,
};
use sts2_core::monsters::Intent;
use sts2_core::powers::PowerKind;
use sts2_core::{CombatState, CreatureUid, MonsterKind};

use super::apply_power_to_player_via_legacy_surface;
use super::dsl::{br, mv, mv_eff, proto_ai, proto_ais, ProtoAis};

/// Bead `sts2-rs-vn0m`: Fogmog ILLUSION spawns 1 [`MonsterKind::EyeWithTeeth`].
/// Mirrors Python pin `sts2-rl-agent@01c094d:sts2_env/monsters/act1.py:159-161`
/// (`illusion()` calls `combat.add_enemy(eye, eye_ai)`).
///
/// Bead `sts2-rs-lc58p`: attach `create_eye_with_teeth_ai()` to the
/// freshly-spawned Eye (same pattern as `infested_after_death_spawn`
/// above). Without the AI, the spawned Eye fell back to the legacy
/// [`MonsterKind::roll_intent`] placeholder (Debuff stub) instead of
/// its real DISTRACT/bite state machine, leaving the Dazed cards from
/// `eye_with_teeth_distract_effect` un-fired.
fn fogmog_illusion_effect(state: &mut CombatState, _owner: CreatureUid) {
    // C# `Fogmog.IllusionMove` adds into the LITERAL slot "illusion", index 0
    // of `FogmogNormal.Slots = ["illusion", "fogmog"]` — no free-slot search,
    // so a second illusion simply shares the slot. `spawn_enemy_in_slot` then
    // reproduces `SortEnemiesBySlotName`, putting the Eye ahead of the Fogmog
    // (slot 1).
    //
    // Was a bare `spawn_enemy_at_front`, which happened to be right only
    // because "illusion" is index 0. Going through the slot model retires the
    // `TODO(o1mwv)` per-encounter workaround (GH #552).
    state.spawn_enemy_in_slot(
        MonsterKind::EyeWithTeeth,
        FOGMOG_ILLUSION_SLOT,
        create_eye_with_teeth_ai(),
    );
}

/// `FogmogNormal.Slots = ["illusion", "fogmog"]` — the summoned Eye's slot.
const FOGMOG_ILLUSION_SLOT: u8 = 0;

/// Bead `sts2-rs-r8x8h`: Vantom Dismember adds 3 Wounds to the
/// player's discard pile. Mirrors C# at
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/Vantom.cs:148-167`:
/// `await CardPileCmd.AddToCombatAndPreview<Wound>(targets, PileType.Discard, 3, addedByPlayer: false)`.
fn vantom_dismember_effect(state: &mut CombatState, _owner: CreatureUid) {
    use sts2_core::card_effects::PileKind;
    for _ in 0..3 {
        let _ = super::super::cards::status::spawn_wound(state, PileKind::Discard);
    }
}

/// Bead `sts2-rs-8ukqd` (bug C): PhrogParasite INFECT adds 3 Infection
/// cards to the player's discard pile. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/PhrogParasite.cs:84-100`
/// `InfectMove`: `await CardPileCmd.AddToCombatAndPreview<Infection>(targets, PileType.Discard, 3, addedByPlayer: false)`.
/// The leading `Intent::StatusCard` is telegraph-only (C# `StatusIntent(3)`);
/// the card add lives here, same shape as [`vantom_dismember_effect`].
fn phrog_parasite_infect_effect(state: &mut CombatState, _owner: CreatureUid) {
    use sts2_core::card_effects::PileKind;
    for _ in 0..3 {
        let _ = super::super::cards::status::spawn_infection(state, PileKind::Discard);
    }
}

/// `InfestedPower.AfterDeath` — the PhrogParasite elite's second phase.
/// When the phrog (which carries `InfestedPower(4)` from
/// `on_spawn_self_buffs`) dies, spawn 4 stunned `Wriggler`s. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Powers/InfestedPower.cs`:
/// `for (int i = 0; i < 4; i++) { wriggler.StartStunned = true;
/// CreatureCmd.Add(wriggler, …, wriggler{i+1}) }`.
///
/// `create_infested_wriggler_ai(slot)` starts at `SPAWNED_MOVE` (Stun), so
/// the freshly-spawned wrigglers spend their first turn stunned — modelling
/// `StartStunned = true`. Its `FollowUpState` is the slot-conditional branch
/// (C# `Wriggler.cs::GenerateMoveStateMachine` `ConditionalBranchState`):
/// `wriggler1`/`wriggler3` → NASTY_BITE, `wriggler2`/`wriggler4` → WRIGGLE.
/// The 4 wrigglers spawn into slots `wriggler1..wriggler4` in order
/// (`InfestedPower.cs:28-32` `PhrogParasiteElite.GetWrigglerSlotName(i)`),
/// so they map to [`DenseVegetationWrigglerSlot`] First..Fourth.
///
/// Bead `sts2-rs-2u9w1`: this previously used the slot-LESS
/// `act2_hive::create_wriggler_ai()`, which always followed the stun with
/// NASTY_BITE regardless of slot — the MONSTER_STATE / PLAYER_HP divergence.
///
/// Registered into [`sts2_core::enemy_death_effects`] (keyed by
/// `PowerKind::Infested`) via
/// [`crate::cards::dispatch::install_default_registry`]; fired by the
/// all-enemies-dead branch of `combat_loop::check_combat_end` (bead
/// `sts2-rs-h9u` Phase-7 sliver).
pub fn infested_after_death_spawn(state: &mut CombatState, _owner: CreatureUid) {
    use super::shared::{create_infested_wriggler_ai, DenseVegetationWrigglerSlot};
    let slots = [
        DenseVegetationWrigglerSlot::First,
        DenseVegetationWrigglerSlot::Second,
        DenseVegetationWrigglerSlot::Third,
        DenseVegetationWrigglerSlot::Fourth,
    ];
    for slot in slots {
        let uid = state.spawn_enemy(MonsterKind::Wriggler);
        let ai = create_infested_wriggler_ai(slot);
        // Telegraph the spawned wriggler's initial move (SPAWNED_MOVE = Stun)
        // on the fresh creature so the NEXT enemy turn's `acting_enemies`
        // snapshot — which filters on `creature.intent.is_some()` — includes
        // it. C# `CombatManager.AfterCreatureAdded` calls `Monster.RollMove`
        // immediately when a creature is added while `CurrentSide == Player`
        // (`CombatManager.cs:689-695`); InfestedPower's wrigglers are spawned
        // mid-player-turn (the killing blow on the PhrogParasite), so C# gives
        // them their SPAWNED_MOVE telegraph at once and they perform the stun
        // on the very next enemy turn — advancing to NASTY_BITE/WRIGGLE the
        // turn after. Without this the wriggler's `intent` stayed `None` until
        // the post-exec re-roll, so it sat out one enemy turn and trailed C#
        // by a full move-state step (the bite/wriggle landed one turn late —
        // an off-by-one that under-damaged the player and desynced the
        // downstream discard/reshuffle pile size, surfacing as a `shuffle`
        // RNG-counter divergence). Mirrors the Axebot respawn telegraph in
        // `act3_glory::respawn_axebot`.
        let initial_intent = ai.current_move().intents.first().copied();
        state.set_creature_ai(uid, ai);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.intent = initial_intent;
        }
    }
}

/// Bead `sts2-rs-fqcx0`: EyeWithTeeth DISTRACT adds 3 `Dazed` cards to the
/// player's discard pile. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/EyeWithTeeth.cs:38,49`
/// `DistractMove`: `StatusIntent(3)` /
/// `await CardPileCmd.AddToCombatAndPreview<Dazed>(targets, PileType.Discard, 3, addedByPlayer: false)`.
/// The `Intent::StatusCard` on DISTRACT is telegraph-only (`perform_intent`
/// runs only the effect_fn for non-attack intents); previously `mv()` /
/// `noop_effect`, so the Fogmog-summoned Eye never dazed the player —
/// surfaced by parity-fuzz `discard_count` drift on FOGMOG_NORMAL.
fn eye_with_teeth_distract_effect(state: &mut CombatState, _owner: CreatureUid) {
    use sts2_core::card_effects::PileKind;
    for _ in 0..3 {
        let _ = super::super::cards::status::spawn_dazed(state, PileKind::Discard);
    }
}

/// Bead `sts2-rs-r8x8h`: Vantom Prepare applies +2 Strength to self.
/// Mirrors C# `Vantom.cs:169-184` `await PowerCmd.Apply<StrengthPower>(base.Creature, 2m, base.Creature, null)`.
fn vantom_prepare_effect(state: &mut CombatState, owner: CreatureUid) {
    use sts2_core::card_effects::apply_power_to_creature;
    apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// Bead `sts2-rs-o1mwv` (C.3): SnappingJaxfruit ENERGY_ORB applies +2
/// Strength to self after the attack. Mirrors C#
/// `SnappingJaxfruit.cs:88-94` `EnergyOrb`'s
/// `await PowerCmd.Apply<StrengthPower>(base.Creature, 2m, base.Creature, null)`.
/// The trailing `Intent::Buff{Strength,2}` in the move is telegraph-only
/// (`perform_intent` performs `intents.first()` and fires the move's
/// `effect_fn` after the attack) — the actual self-buff lives here.
fn snapping_jaxfruit_energy_orb_effect(state: &mut CombatState, owner: CreatureUid) {
    use sts2_core::card_effects::apply_power_to_creature;
    apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// Bead `sts2-rs-wk1sa`: Fogmog SWIPE applies +1 Strength to self after
/// the attack. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/Fogmog.cs:61-69`
/// `SwipeMove`: `Attack(SwipeDamage)` then
/// `await PowerCmd.Apply<StrengthPower>(base.Creature, 1m, base.Creature, null)`.
/// Both SWIPE_MOVE and SWIPE_RANDOM_MOVE share `SwipeMove`. The trailing
/// `Intent::Buff{Strength,1}` in each move is telegraph-only
/// (`perform_intent` performs `intents.first()` — the attack — and fires
/// the move's `effect_fn` after); the actual self-buff lives here.
fn fogmog_swipe_effect(state: &mut CombatState, owner: CreatureUid) {
    use sts2_core::card_effects::apply_power_to_creature;
    apply_power_to_creature(state, owner, PowerKind::Strength, 1);
}

/// Bead `sts2-rs-p8n60` (o1mwv C.5): SlitheringStrangler CONSTRICT_MOVE
/// applies `ConstrictPower(3)` to the player. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/SlitheringStrangler.cs:59-64`
/// `ConstrictMove`: `await PowerCmd.Apply<ConstrictPower>(targets, 3m, base.Creature, null)`.
/// ConstrictPower is a `Counter`-stack debuff that deals `Amount`
/// unpowered damage to the player at the player's turn end (the
/// `PowerKind::Constrict` arm in `hooks::after_turn_end`). The leading
/// `Intent::Debuff` is telegraph-only; the application lives here.
fn slithering_strangler_constrict_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Constrict, 3);
    // Record this strangler as the Constrict applier so its death removes
    // the player's Constrict — C# `ConstrictPower.AfterDeath`
    // (../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Powers/ConstrictPower.cs:24-31).
    // "Latest applier wins" (`Power.applier` doc) matches C#
    // `PowerCmd.Apply` rebinding the applier on each re-application.
    let constrict_ids: smallvec::SmallVec<[std::num::NonZeroU32; 2]> = state
        .player
        .powers
        .iter()
        .filter_map(|puid| {
            state
                .powers
                .iter()
                .find(|p| p.id == puid.0 && p.kind == PowerKind::Constrict)
                .map(|p| p.id)
        })
        .collect();
    for id in constrict_ids {
        if let Some(p) = state.powers.get_mut_by_id(id) {
            p.applier = Some(owner);
        }
    }
}

/// Bead `sts2-rs-p8n60` (o1mwv C.5): SlitheringStrangler TWACK_MOVE gains
/// 5 block after its attack. Mirrors C# `ThwackMove`
/// (SlitheringStrangler.cs:66-73): `DamageCmd.Attack(7) ... ;
/// CreatureCmd.GainBlock(base.Creature, 5m, ...)`. The trailing
/// `Intent::Defend` is telegraph-only; the block gain lives here.
///
/// Routed through `Effect::GainBlockCreature` — the engine's
/// `CreatureCmd.GainBlock` — rather than writing `c.block` directly. The two
/// agree on the number today (no block modifier is in play for this enemy),
/// but the funnel is where the modify chain runs, where the
/// `after_block_gained` buses fire, and where the combat log records its
/// `BlockGained` row. A direct write silently opts out of all three, and this
/// move was invisible in the log because of it.
fn slithering_strangler_twack_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::hooks::apply_effects(
        state,
        &[sts2_core::hooks::Effect::GainBlockCreature {
            target: sts2_core::hooks::CreatureRef::Enemy(owner),
            amount: 5,
        }],
    );
}

/// Bead `sts2-rs-fad0f`: Flyconid FRAIL_SPORES applies Frail(2) to the
/// player. Mirrors C# `Flyconid.cs:65-78`
/// `await PowerCmd.Apply<FrailPower>(targets, 2m, base.Creature, null)`.
/// The intent metadata alone (`Intent::Debuff{Frail,2}`) is just
/// telegraph — the actual power application lives in the move's
/// `effect_fn` per the DSL convention. Routes through the player's
/// creature_uid mirror established by sts2-rs-d03; if unset (e.g.
/// tests that bypass start_combat) the apply is silently dropped.
fn flyconid_frail_spores_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 2);
}

/// Bead `sts2-rs-fad0f`: Flyconid VULNERABLE_SPORES applies
/// Vulnerable(2) to the player. Mirrors C# `Flyconid.cs:53-63`
/// `await PowerCmd.Apply<VulnerablePower>(targets, 2m, base.Creature, null)`.
fn flyconid_vulnerable_spores_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Vulnerable, 2);
}

/// Mawler ROAR applies `Vulnerable(3)` to the player. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/Mawler.cs:53-57`
/// `RoarMove`: `await PowerCmd.Apply<VulnerablePower>(targets, 3m, base.Creature, null)`.
/// The `Intent::Debuff{Vulnerable,3}` on the ROAR state is telegraph-only
/// (`perform_intent` runs the effect_fn for non-attack intents); the real
/// apply lives here. Previously ROAR used the noop-effect `mv()` helper, so
/// Mawler's signature Vulnerable was never applied — the dominant
/// `MAWLER player.hp` parity-fuzz drift (C# player took more damage under
/// the missing Vulnerable).
fn mawler_roar_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Vulnerable, 3);
}

/// BygoneEffigy WAKE applies `Strength(10)` to self. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/BygoneEffigy.cs:69-75`
/// `WakeMove`: `await PowerCmd.Apply<StrengthPower>(base.Creature, 10m, base.Creature, null)`.
/// The `Intent::Buff{Strength,10}` on the WAKE state is telegraph-only
/// (`perform_intent` runs the effect_fn for non-attack intents); the real
/// apply lives here. Previously WAKE used the noop-effect `mv()` helper, so
/// the effigy never gained its wake Strength — `BYGONE_EFFIGY_ELITE`
/// parity-fuzz `monsters[0].powers.len` (C#=2 [Slow+Strength] vs Rust=1).
fn bygone_effigy_wake_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 10);
}

/// Bead `sts2-rs-pdsdl`: CubexConstruct CHARGE_UP applies +2 Strength to
/// self. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/CubexConstruct.cs:104-110`
/// `ChargeUpMove`: `await PowerCmd.Apply<StrengthPower>(base.Creature, 2m, base.Creature, null)`.
/// The `Intent::Buff{Strength,2}` (`BuffIntent()` in C#) on CHARGE_UP is
/// telegraph-only; previously the `mv()` noop effect dropped the self-buff.
fn cubex_construct_charge_up_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// Bead `sts2-rs-pdsdl`: CubexConstruct REPEATER (and REPEATER_2) apply
/// +2 Strength to self after the attack. Mirrors C#
/// `CubexConstruct.cs:112-124` `RepeaterBlastMove`: `DamageCmd.Attack(...)`
/// then `await PowerCmd.Apply<StrengthPower>(base.Creature, 2m, base.Creature, null)`.
/// Both REPEATER_MOVE and REPEATER_MOVE_2 share `RepeaterBlastMove`
/// (`CubexConstruct.cs:87-88`). The attack is listed first so
/// `perform_intent` performs it; the trailing `Intent::Buff{Strength,2}`
/// (`BuffIntent()`) is telegraph-only with the real apply here.
fn cubex_construct_repeater_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// Bead `sts2-rs-pdsdl`: BruteRubyRaider ROAR applies +3 Strength to self.
/// Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/BruteRubyRaider.cs:45-50`
/// `RoarMove`: `await PowerCmd.Apply<StrengthPower>(base.Creature, 3m, base.Creature, null)`
/// (`_roarStrength = 3`). The `Intent::Buff{Strength,3}` (`BuffIntent()`) is
/// telegraph-only; previously the `mv()` noop effect dropped the self-buff.
fn brute_ruby_raider_roar_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 3);
}

/// Bead `sts2-rs-pdsdl`: TrackerRubyRaider TRACK applies Frail(2) to the
/// player. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/TrackerRubyRaider.cs:37-42`
/// `TrackMove`: `await PowerCmd.Apply<FrailPower>(targets, 2m, base.Creature, null)`.
/// The `Intent::Debuff{Frail,2}` (`DebuffIntent()`) is telegraph-only;
/// previously the `mv()` noop effect dropped the player debuff.
fn tracker_ruby_raider_track_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 2);
}

/// Bead `sts2-rs-pdsdl`: CeremonialBeast STAMP applies `PlowPower(150)` to
/// self. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/CeremonialBeast.cs:154-160`
/// `StampMove`: `await PowerCmd.Apply<PlowPower>(base.Creature, PlowAmount, base.Creature, null)`
/// where `PlowAmount = 150` (non-ascension; 160 at DeadlyEnemies, identity
/// at pin `01c094d`). NOTE: the C# move uses a generic `BuffIntent()` and the
/// applied power is **PlowPower**, not Strength — the prior Rust intent
/// metadata (`Strength,2`) was a fabrication. The PLOW power gates phase 2
/// of the boss state machine via `PLOW_CHECK` (`ceremonial_beast_plow_depleted`).
fn ceremonial_beast_stamp_effect(state: &mut CombatState, owner: CreatureUid) {
    let amount = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::DeadlyEnemies,
        160,
        150,
    );
    // C# `PowerCmd.Apply<PlowPower>(base.Creature, PlowAmount, base.Creature, null)`
    // records the BEAST itself as the applier. Route through the applier-aware
    // surface with `applier = owner` so the self-applied PlowPower (a Debuff) is
    // NOT misattributed to the player: the bare `apply_power_to_creature` path
    // hardcodes `applier = Player` in its `after_power_applied` dispatch, which
    // would wrongly fire the player's SleightOfFlesh ("player applied a debuff to
    // an enemy → deal Amount") off the Beast's own PLOW (Necrobinder fuzz seed
    // 135: SleightOfFlesh(13) dealt a phantom 13 to the Beast at STAMP — C# hp
    // 206, Rust 193).
    sts2_core::card_effects::apply_power_to_creature_with_applier(
        state,
        owner,
        PowerKind::Plow,
        amount,
        Some(owner),
    );
}

/// Bead `sts2-rs-pdsdl`: CeremonialBeast PLOW applies +2 Strength to self
/// after its attack. Mirrors C# `CeremonialBeast.cs:162-190` `PlowMove`:
/// `DamageCmd.Attack(PlowDamage)` then
/// `await PowerCmd.Apply<StrengthPower>(base.Creature, PlowStrength, base.Creature, null)`
/// where `PlowStrength = 2`. The attack is listed first so `perform_intent`
/// performs it; the trailing `Intent::Buff{Strength,2}` (`BuffIntent()`) is
/// telegraph-only with the real apply here.
fn ceremonial_beast_plow_effect(state: &mut CombatState, owner: CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// Bead `sts2-rs-pdsdl`: CeremonialBeast BEAST_CRY applies `RingingPower(1)`
/// to the player. Mirrors C# `CeremonialBeast.cs:205-213` `BeastCryMove`:
/// `await PowerCmd.Apply<RingingPower>(targets, 1m, base.Creature, null)`.
/// NOTE: the C# move uses a generic `DebuffIntent()` and the applied power
/// is **RingingPower**, not Weak — the prior Rust intent metadata
/// (`Weak,1`) was a fabrication.
fn ceremonial_beast_beast_cry_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Ringing, 1);
}

/// Bead `sts2-rs-pdsdl`: CeremonialBeast CRUSH applies +3 Strength to self
/// after its attack. Mirrors C# `CeremonialBeast.cs:229-243` `CrushMove`:
/// `DamageCmd.Attack(CrushDamage)` then
/// `await PowerCmd.Apply<StrengthPower>(base.Creature, CrushStrength, base.Creature, null)`
/// where `CrushStrength = 3` (non-ascension; 4 at DeadlyEnemies). The attack
/// is listed first so `perform_intent` performs it; the trailing
/// `Intent::Buff{Strength,3}` (`BuffIntent()`) is telegraph-only.
fn ceremonial_beast_crush_effect(state: &mut CombatState, owner: CreatureUid) {
    let amount = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::DeadlyEnemies,
        4,
        3,
    );
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// Bead `sts2-rs-pdsdl`: KinPriest ORB_OF_FRAILTY applies Frail(1) to the
/// player after its attack. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/KinPriest.cs:132-140`
/// `OrbOfFrailtyMove`: `DamageCmd.Attack(8)` then
/// `await PowerCmd.Apply<FrailPower>(targets, 1m, base.Creature, null)`.
/// The attack is listed first so `perform_intent` performs it; the trailing
/// `Intent::Debuff{Frail,1}` (`DebuffIntent()`) is telegraph-only.
fn kin_priest_orb_of_frailty_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 1);
}

/// Bead `sts2-rs-pdsdl`: KinPriest ORB_OF_WEAKNESS applies Weak(1) to the
/// player after its attack. Mirrors C# `KinPriest.cs:142-150`
/// `OrbOfWeaknessMove`: `DamageCmd.Attack(8)` then
/// `await PowerCmd.Apply<WeakPower>(targets, 1m, base.Creature, null)`.
/// The attack is listed first so `perform_intent` performs it; the trailing
/// `Intent::Debuff{Weak,1}` (`DebuffIntent()`) is telegraph-only.
fn kin_priest_orb_of_weakness_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 1);
}

/// Bead `sts2-rs-pdsdl`: KinPriest RITUAL applies +2 Strength to self.
/// Mirrors C# `KinPriest.cs:167-177` `RitualMove`:
/// `await PowerCmd.Apply<StrengthPower>(base.Creature, RitualStrength, base.Creature, null)`
/// where `RitualStrength = 2` (non-ascension; 3 at DeadlyEnemies). The
/// `Intent::Buff{Strength,2}` (`BuffIntent()`) is telegraph-only.
fn kin_priest_ritual_effect(state: &mut CombatState, owner: CreatureUid) {
    let amount = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::DeadlyEnemies,
        3,
        2,
    );
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// Bead `sts2-rs-8ukqd` (bug C): VineShambler GRASPING_VINES applies
/// `TangledPower(1)` to the player. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/VineShambler.cs:59-69`
/// `await PowerCmd.Apply<TangledPower>(targets, 1m, base.Creature, null)`.
/// The `Intent::Debuff{Tangled,1}` telegraph metadata only mirrors the
/// `CardDebuffIntent` icon; the real apply lives here per the DSL
/// convention. `TangledPower`'s combat behaviour is wired: a fresh instance
/// Entangles the player's unafflicted Attacks (`AfterApplied`, C# fires it
/// only for a new instance, `PowerCmd.Apply`), an Entangled card costs
/// +amount (the `PowerKind::Tangled` arm in `hooks::modify_card_cost`), and
/// its removal at the player's turn end clears them (`hooks::after_turn_end`,
/// `monster_power_effects::tangled_after_removed`).
fn vine_shambler_grasping_vines_effect(state: &mut CombatState, _owner: CreatureUid) {
    let fresh = state.player_power_amount(PowerKind::Tangled) <= 0;
    apply_power_to_player_via_legacy_surface(state, PowerKind::Tangled, 1);
    if fresh && state.player_power_amount(PowerKind::Tangled) > 0 {
        sts2_core::monster_power_effects::tangled_after_applied(state);
    }
}

/// Cooldown-aware branch builder. Mirrors the C# `RandomBranchState`
/// overload `AddBranch(state, int cooldown, MoveRepeatType)` — the
/// integer arg is a *cooldown* (the branch is weight 0 if its move
/// appears in the last `cooldown` logged moves), NOT a weight; the
/// weight defaults to `1f`. See bead `sts2-rs-8ukqd` (R1): the original
/// Flyconid port misread these cooldown args as weights, skewing the
/// `RandomBranch` total and picking the wrong later-turn move.
fn br_cd(state_id: &'static str, repeat: MoveRepeatType, cooldown: u8) -> WeightedBranch {
    WeightedBranch {
        state_id,
        repeat_type: repeat,
        max_times: 1,
        weight: 1.0,
        cooldown,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Normal encounters
// ────────────────────────────────────────────────────────────────────────────

/// `CubexConstruct` — CHARGE_UP → REPEATER → REPEATER_2 → EXPEL_BLAST → REPEATER …
///
/// `…wb8lk.11`: v0.107.1 removed the `SUBMERGE` move (decomp
/// `v0.107.1/…/CubexConstruct.cs` has no submerge state) — it was already
/// unreachable here (no inbound edge), so this is behaviour-preserving. The
/// per-move damage (Blast 7/8, Expel 5/6) is unchanged from v0.103.2. The
/// v0.107.1 decomp also renamed the moves (REPEATER→REPEATER_BLAST_MOVE,
/// EXPEL_BLAST→EXPEL_MOVE) and added an `_isBurrowed`/`_isCharging`
/// animation-only state machine; the move-id strings and the burrow/charge
/// flags are cosmetic (they drive Spine animations, not combat), so they are
/// a declared deferral.
pub fn create_cubex_construct_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "CHARGE_UP",
        mv_eff(
            "CHARGE_UP",
            smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2
            }],
            "REPEATER",
            cubex_construct_charge_up_effect,
        ),
    );
    s.insert(
        "REPEATER",
        mv_eff(
            "REPEATER",
            smallvec![
                Intent::Attack { damage: 7 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2
                }
            ],
            "REPEATER_2",
            cubex_construct_repeater_effect,
        ),
    );
    s.insert(
        "REPEATER_2",
        mv_eff(
            "REPEATER_2",
            smallvec![
                Intent::Attack { damage: 7 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2
                }
            ],
            "EXPEL_BLAST",
            cubex_construct_repeater_effect,
        ),
    );
    s.insert(
        "EXPEL_BLAST",
        mv(
            "EXPEL_BLAST",
            smallvec![Intent::MultiAttack { damage: 5, hits: 2 }],
            "REPEATER",
        ),
    );
    MonsterAi::new(s, "CHARGE_UP")
}

/// `Flyconid` — INITIAL_RAND → main RAND. Both branches are equal-weight
/// (all `1f` in C#); the integers in `Flyconid.cs` `AddBranch(state, N,
/// CannotRepeat)` are **cooldowns**, not weights (bead `sts2-rs-8ukqd`,
/// R1 root). C# `Flyconid.cs:35-44`:
/// - INITIAL: FRAIL_SPORES (cd 2, CannotRepeat) : SMASH (cd 0, CannotRepeat).
/// - RAND: VULNERABLE_SPORES (cd 3) : FRAIL_SPORES (cd 2) : SMASH (cd 0),
///   all `CannotRepeat`, all weight 1.
pub fn create_flyconid_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "INITIAL_RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "INITIAL_RAND",
            branches: smallvec![
                br_cd("FRAIL_SPORES", MoveRepeatType::CannotRepeat, 2),
                br_cd("SMASH", MoveRepeatType::CannotRepeat, 0),
            ],
        }),
    );
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            branches: smallvec![
                br_cd("VULNERABLE_SPORES", MoveRepeatType::CannotRepeat, 3),
                br_cd("FRAIL_SPORES", MoveRepeatType::CannotRepeat, 2),
                br_cd("SMASH", MoveRepeatType::CannotRepeat, 0),
            ],
        }),
    );
    s.insert(
        "VULNERABLE_SPORES",
        mv_eff(
            "VULNERABLE_SPORES",
            smallvec![Intent::Debuff {
                kind: PowerKind::Vulnerable,
                amount: 2
            }],
            "RAND",
            flyconid_vulnerable_spores_effect,
        ),
    );
    s.insert(
        "FRAIL_SPORES",
        mv_eff(
            "FRAIL_SPORES",
            smallvec![
                Intent::Attack { damage: 8 },
                Intent::Debuff {
                    kind: PowerKind::Frail,
                    amount: 2
                }
            ],
            "RAND",
            flyconid_frail_spores_effect,
        ),
    );
    s.insert(
        "SMASH",
        mv("SMASH", smallvec![Intent::Attack { damage: 11 }], "RAND"),
    );
    MonsterAi::new(s, "INITIAL_RAND")
}

/// `EyeWithTeeth` — DISTRACT self-loop. Spawned by Fogmog, so a clone of a
/// per-thread prototype (`proto_ai`).
pub fn create_eye_with_teeth_ai() -> MonsterAi {
    thread_local! {
        static PROTO: ProtoAis<1> = const { proto_ais() };
    }
    proto_ai(&PROTO, 0, build_eye_with_teeth_ai)
}

pub(crate) fn build_eye_with_teeth_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "DISTRACT",
        mv_eff(
            "DISTRACT",
            smallvec![Intent::StatusCard],
            "DISTRACT",
            eye_with_teeth_distract_effect,
        ),
    );
    MonsterAi::new(s, "DISTRACT")
}

/// `Fogmog` — ILLUSION → SWIPE → RAND(SWIPE_RANDOM 0.4 | HEADBUTT 0.6, CannotRepeat).
/// ILLUSION emits `Intent::Summon` (Python `IntentType.SUMMON` for the
/// EyeWithTeeth spawn); the variant landed via bead `sts2-rs-47p5`.
pub fn create_fogmog_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "ILLUSION",
        mv_eff(
            "ILLUSION",
            smallvec![Intent::Summon],
            "SWIPE",
            fogmog_illusion_effect,
        ),
    );
    s.insert(
        "SWIPE",
        mv_eff(
            "SWIPE",
            smallvec![
                Intent::Attack { damage: 8 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 1
                }
            ],
            "RAND",
            fogmog_swipe_effect,
        ),
    );
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            branches: smallvec![
                br("SWIPE_RANDOM", MoveRepeatType::CannotRepeat, 1, 0.4),
                br("HEADBUTT", MoveRepeatType::CannotRepeat, 1, 0.6),
            ],
        }),
    );
    s.insert(
        "SWIPE_RANDOM",
        mv_eff(
            "SWIPE_RANDOM",
            smallvec![
                Intent::Attack { damage: 8 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 1
                }
            ],
            "HEADBUTT",
            fogmog_swipe_effect,
        ),
    );
    s.insert(
        "HEADBUTT",
        mv(
            "HEADBUTT",
            smallvec![Intent::Attack { damage: 14 }],
            "SWIPE",
        ),
    );
    MonsterAi::new(s, "ILLUSION")
}

/// `Inklet` — mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/Inklet.cs`
/// `GenerateMoveStateMachine` (sts2-rs-ztx5u rebase of the prior fabricated
/// SPLATTER/SUBMERGE Python-pin moveset, which had no basis in the decomp):
///   - JAB_MOVE           = `SingleAttackIntent(JabDamage=3)`;     follow-up RAND.
///   - WHIRLWIND_MOVE     = `MultiAttackIntent(WhirlwindDamage=2, 3)`; follow-up JAB.
///   - PIERCING_GAZE_MOVE = `SingleAttackIntent(PiercingGazeDamage=10)`; follow-up JAB.
///   - RAND               = `RandomBranchState(PIERCING_GAZE CannotRepeat |
///                           WHIRLWIND CannotRepeat)`.
/// (Non-ascension identity amounts; Deadly bumps Jab→4 / Whirlwind→3 /
/// Gaze→11 are not modeled at the `01c094d` ascension-identity pin.)
///
/// The C# `INIT_RAND` `RandomBranchState` is dead code — it is constructed
/// and given branches but never referenced as the initial state or any
/// follow-up. The real initial state is FORCED: `_middleInklet ? WHIRLWIND
/// : JAB`. So no Inklet rolls `monster_ai` on turn 0 (this fixes the prior
/// `monster_ai 0/1` desync where the fabricated `Other` slot started at a
/// `RandomBranch` and consumed a turn-0 read C# never spends).
///
/// `InkletsNormal.cs` spawns exactly three Inklets and sets
/// `MiddleInklet = true` on index 1, so the slot→initial mapping is:
/// First (idx 0) → JAB, Second (idx 1, the middle) → WHIRLWIND, Other → JAB.
/// HP 11-17 + `SlipperyPower(1)` on spawn live in
/// `sts2-core/monsters.rs` (HP table + `on_spawn_self_buffs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InkletSlot {
    First,
    Second,
    Other,
}

pub fn create_inklet_ai(slot: InkletSlot) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "JAB",
        mv("JAB", smallvec![Intent::Attack { damage: 3 }], "RAND"),
    );
    s.insert(
        "WHIRLWIND",
        mv(
            "WHIRLWIND",
            smallvec![Intent::MultiAttack { damage: 2, hits: 3 }],
            "JAB",
        ),
    );
    s.insert(
        "PIERCING_GAZE",
        mv(
            "PIERCING_GAZE",
            smallvec![Intent::Attack { damage: 10 }],
            "JAB",
        ),
    );
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            branches: smallvec![
                br("PIERCING_GAZE", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("WHIRLWIND", MoveRepeatType::CannotRepeat, 1, 1.0),
            ],
        }),
    );
    // C# `_middleInklet ? WHIRLWIND : JAB`. InkletsNormal sets the middle
    // (index 1 → InkletSlot::Second) Inklet only.
    let initial = match slot {
        InkletSlot::Second => "WHIRLWIND",
        InkletSlot::First | InkletSlot::Other => "JAB",
    };
    MonsterAi::new(s, initial)
}

/// `Mawler` — RAND(RIP_AND_TEAR CannotRepeat | ROAR UseOnlyOnce | CLAW CannotRepeat).
pub fn create_mawler_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            branches: smallvec![
                br("RIP_AND_TEAR", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("ROAR", MoveRepeatType::UseOnlyOnce, 1, 1.0),
                br("CLAW", MoveRepeatType::CannotRepeat, 1, 1.0),
            ],
        }),
    );
    s.insert(
        "RIP_AND_TEAR",
        mv(
            "RIP_AND_TEAR",
            smallvec![Intent::Attack { damage: 14 }],
            "RAND",
        ),
    );
    s.insert(
        "ROAR",
        MonsterState::Move(MoveState {
            id: "ROAR",
            effect: mawler_roar_effect,
            intents: smallvec![Intent::Debuff {
                kind: PowerKind::Vulnerable,
                amount: 3
            }],
            follow_up: Some("RAND"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "CLAW",
        mv(
            "CLAW",
            smallvec![Intent::MultiAttack { damage: 4, hits: 2 }],
            "RAND",
        ),
    );
    MonsterAi::new(s, "CLAW")
}

/// `VineShambler` — SWIPE → GRASPING_VINES → CHOMP → SWIPE …
///
/// Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/VineShambler.cs:43-87`:
///   - SWIPE_MOVE           = `MultiAttackIntent(SwipeDamage=6, 2)`; follow-up GRASPING_VINES.
///   - GRASPING_VINES_MOVE  = `SingleAttackIntent(GraspingVinesDamage=8)` + `CardDebuffIntent`,
///     body applies `TangledPower(1)` to the player; follow-up CHOMP.
///   - CHOMP_MOVE           = `SingleAttackIntent(ChompDamage=16)`; follow-up SWIPE.
/// The state machine's initial state is SWIPE_MOVE
/// (`return new MonsterMoveStateMachine(list, moveState2)`).
///
/// Bead `sts2-rs-8ukqd` (bug C) — replaces the prior fabricated
/// `RAND(VINE_WHIP{7+Weak} | TANGLE{10})` moveset, which had no basis in
/// the v0.103.2 decomp (wrong move names, wrong damage, wrong cycle) and
/// drove the VINE_SHAMBLER `player.hp` parity drift (SWIPE 6×2=12 vs the
/// fabricated VINE_WHIP 7). Attack is listed first in GRASPING_VINES so
/// `perform_intent` performs the attack and fires the `effect_fn`; the
/// trailing `Debuff` is the `CardDebuffIntent` telegraph icon.
pub fn create_vine_shambler_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SWIPE",
        mv(
            "SWIPE",
            smallvec![Intent::MultiAttack { damage: 6, hits: 2 }],
            "GRASPING_VINES",
        ),
    );
    s.insert(
        "GRASPING_VINES",
        mv_eff(
            "GRASPING_VINES",
            smallvec![
                Intent::Attack { damage: 8 },
                Intent::Debuff {
                    kind: PowerKind::Tangled,
                    amount: 1
                }
            ],
            "CHOMP",
            vine_shambler_grasping_vines_effect,
        ),
    );
    s.insert(
        "CHOMP",
        mv("CHOMP", smallvec![Intent::Attack { damage: 16 }], "SWIPE"),
    );
    MonsterAi::new(s, "SWIPE")
}

/// `SlitheringStrangler` — CONSTRICT → rand(TWACK | LASH) → CONSTRICT …
///
/// Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/SlitheringStrangler.cs:38-55`:
///   - CONSTRICT (DebuffIntent → `ConstrictPower(3)` on the player),
///     follow-up = RandomBranch(TWACK | LASH), each `CanRepeatForever`.
///   - TWACK   = SingleAttack(7) + DefendIntent → gain 5 block; follow-up CONSTRICT.
///   - LASH    = SingleAttack(12); follow-up CONSTRICT.
/// The state machine's initial state is CONSTRICT. Bead `sts2-rs-p8n60`
/// (C.5) — replaces the prior fabricated SQUEEZE/STRANGLE moveset (which
/// did not match the v0.103.2 decomp).
pub fn create_slithering_strangler_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "CONSTRICT",
        mv_eff(
            "CONSTRICT",
            smallvec![Intent::Debuff {
                kind: PowerKind::Constrict,
                amount: 3
            }],
            "RAND",
            slithering_strangler_constrict_effect,
        ),
    );
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            branches: smallvec![
                br("TWACK", MoveRepeatType::CanRepeatForever, 0, 1.0),
                br("LASH", MoveRepeatType::CanRepeatForever, 0, 1.0),
            ],
        }),
    );
    s.insert(
        "TWACK",
        mv_eff(
            "TWACK",
            smallvec![Intent::Attack { damage: 7 }, Intent::Defend { block: 5 }],
            "CONSTRICT",
            slithering_strangler_twack_effect,
        ),
    );
    s.insert(
        "LASH",
        mv(
            "LASH",
            smallvec![Intent::Attack { damage: 12 }],
            "CONSTRICT",
        ),
    );
    MonsterAi::new(s, "CONSTRICT")
}

/// `SnappingJaxfruit` — single self-looping `ENERGY_ORB_MOVE`.
///
/// Mirrors C# `SnappingJaxfruit.cs:62-95`: `GenerateMoveStateMachine`
/// builds exactly one `MoveState("ENERGY_ORB_MOVE", EnergyOrb,
/// new SingleAttackIntent(EnergyDamage), new BuffIntent())` whose
/// `FollowUpState` is itself — so every turn is Attack(3) plus the
/// `EnergyOrb` body's `PowerCmd.Apply<StrengthPower>(self, 2)`.
/// `EnergyDamage` = 3 (4 on DeadlyEnemies; ascension is identity at
/// pin `01c094d`).
///
/// The prior 3-move RAND(SNAP | SEED_SPIT | BURROW) branch was a port
/// invention with no C# basis (sub-bug C.3 of `sts2-rs-o1mwv`); it
/// produced the wrong intents and never applied the self-Strength.
/// Attack is listed first so `perform_intent` performs it; the
/// trailing `Buff` is the telegraph icon C# shows via `BuffIntent`,
/// with the real apply in [`snapping_jaxfruit_energy_orb_effect`].
pub fn create_snapping_jaxfruit_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "ENERGY_ORB_MOVE",
        mv_eff(
            "ENERGY_ORB_MOVE",
            smallvec![
                Intent::Attack { damage: 3 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2,
                },
            ],
            "ENERGY_ORB_MOVE",
            snapping_jaxfruit_energy_orb_effect,
        ),
    );
    MonsterAi::new(s, "ENERGY_ORB_MOVE")
}

// ────────────────────────────────────────────────────────────────────────────
// Ruby Raiders
// ────────────────────────────────────────────────────────────────────────────

/// `AssassinRubyRaider` — KILLSHOT self-loop.
pub fn create_assassin_ruby_raider_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "KILLSHOT",
        mv(
            "KILLSHOT",
            // `…wb8lk.11`: v0.107.1 KillshotDamage 11→10 at A0 (decomp
            // `AssassinRubyRaider.cs:19 GetValueIfAscension(DeadlyEnemies, 11, 10)`).
            smallvec![Intent::Attack { damage: 10 }],
            "KILLSHOT",
        ),
    );
    MonsterAi::new(s, "KILLSHOT")
}

/// SWING_1 / SWING_2 secondary block — C# `AxeRubyRaider.SwingMove`
/// `GainBlock(SwingBlock=5)`. Both swings share `SwingMove`; the
/// `Intent::Defend { block: 5 }` is the move's SECONDARY intent, which
/// `perform_intent` drops (bead `sts2-rs-qk2du` only applies block for a
/// move's primary intent), so grant it here. Bead `sts2-rs-owm9q` (same
/// class as `sts2-rs-71kog` / `sts2-rs-viwkh` / `sts2-rs-97w3y`).
fn axe_ruby_raider_swing_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    // `SwingBlock` (`AxeRubyRaider.cs:26`) is `(.., 6, 5)` — SWING_1 and
    // SWING_2 share the same row.
    let amount = crate::ascension_scaling::deadly_block(
        state.ascension,
        sts2_core::MonsterKind::AxeRubyRaider,
        "SWING_1",
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

/// `AxeRubyRaider` — SWING_1 → SWING_2 → BIG_SWING → SWING_1 …
pub fn create_axe_ruby_raider_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SWING_1",
        mv_eff(
            "SWING_1",
            smallvec![Intent::Attack { damage: 5 }, Intent::Defend { block: 5 }],
            "SWING_2",
            axe_ruby_raider_swing_effect,
        ),
    );
    s.insert(
        "SWING_2",
        mv_eff(
            "SWING_2",
            smallvec![Intent::Attack { damage: 5 }, Intent::Defend { block: 5 }],
            "BIG_SWING",
            axe_ruby_raider_swing_effect,
        ),
    );
    s.insert(
        "BIG_SWING",
        mv(
            "BIG_SWING",
            smallvec![Intent::Attack { damage: 12 }],
            "SWING_1",
        ),
    );
    MonsterAi::new(s, "SWING_1")
}

/// `BruteRubyRaider` — BEAT ↔ ROAR.
pub fn create_brute_ruby_raider_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "BEAT",
        mv("BEAT", smallvec![Intent::Attack { damage: 7 }], "ROAR"),
    );
    s.insert(
        "ROAR",
        mv_eff(
            "ROAR",
            smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 3
            }],
            "BEAT",
            brute_ruby_raider_roar_effect,
        ),
    );
    MonsterAi::new(s, "BEAT")
}

/// `CrossbowRubyRaider` — RELOAD ↔ FIRE.
pub fn create_crossbow_ruby_raider_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "RELOAD",
        mv("RELOAD", smallvec![Intent::Defend { block: 3 }], "FIRE"),
    );
    s.insert(
        "FIRE",
        mv("FIRE", smallvec![Intent::Attack { damage: 14 }], "RELOAD"),
    );
    MonsterAi::new(s, "RELOAD")
}

/// `TrackerRubyRaider` — TRACK → HOUNDS (self-loop on HOUNDS).
pub fn create_tracker_ruby_raider_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "TRACK",
        mv_eff(
            "TRACK",
            smallvec![Intent::Debuff {
                kind: PowerKind::Frail,
                amount: 2
            }],
            "HOUNDS",
            tracker_ruby_raider_track_effect,
        ),
    );
    s.insert(
        "HOUNDS",
        mv(
            "HOUNDS",
            smallvec![Intent::MultiAttack { damage: 1, hits: 8 }],
            "HOUNDS",
        ),
    );
    MonsterAi::new(s, "TRACK")
}

// ────────────────────────────────────────────────────────────────────────────
// Elite encounters
// ────────────────────────────────────────────────────────────────────────────

/// `BygoneEffigy` — INITIAL_SLEEP → WAKE → SLASHES (loop).
///
/// Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/BygoneEffigy.cs:43-58`:
/// INITIAL_SLEEP_MOVE (SleepIntent) → WAKE_MOVE (`StrengthPower(10)`) →
/// SLASHES_MOVE (`SingleAttackIntent(SlashDamage)`, self-loop). On-spawn
/// `SlowPower(1)` is applied via [`MonsterKind::on_spawn_self_buffs`].
/// `SlashDamage` is `GetValueIfAscension(DeadlyEnemies, 15, 13)` — base 13
/// (15 only at the DeadlyEnemies ascension; ascension is identity at pin
/// `01c094d`). Bead `sts2-rs-8ukqd` (bug C) corrected SLASHES from the
/// fabricated 15 to the non-ascension base 13.
pub fn create_bygone_effigy_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "INITIAL_SLEEP",
        mv("INITIAL_SLEEP", smallvec![Intent::Sleep], "WAKE"),
    );
    s.insert(
        "WAKE",
        MonsterState::Move(MoveState {
            id: "WAKE",
            effect: bygone_effigy_wake_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 10
            }],
            follow_up: Some("SLASHES"),
            must_perform_once: false,
        }),
    );
    s.insert("SLEEP", mv("SLEEP", smallvec![Intent::Sleep], "SLASHES"));
    s.insert(
        "SLASHES",
        mv(
            "SLASHES",
            smallvec![Intent::Attack { damage: 13 }],
            "SLASHES",
        ),
    );
    MonsterAi::new(s, "INITIAL_SLEEP")
}

/// `Byrdonis` — SWOOP ↔ PECK.
///
/// Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/Byrdonis.cs:23-27`:
///   - SWOOP = `SingleAttackIntent(SwoopDamage)`, `SwoopDamage =
///     GetValueIfAscension(DeadlyEnemies, 19, 17)` → 17 at the
///     non-ascension baseline (ascension is identity at pin `01c094d`).
///   - PECK  = `MultiAttackIntent(PeckDamage=3, PeckRepeat=3)`.
/// SWOOP was previously mis-ported as 16 (no C# basis); corrected to the
/// non-ascension `SwoopDamage` of 17.
pub fn create_byrdonis_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SWOOP",
        mv("SWOOP", smallvec![Intent::Attack { damage: 17 }], "PECK"),
    );
    s.insert(
        "PECK",
        mv(
            "PECK",
            smallvec![Intent::MultiAttack { damage: 3, hits: 3 }],
            "SWOOP",
        ),
    );
    MonsterAi::new(s, "SWOOP")
}

/// `PhrogParasite` — INFECT ↔ LASH (start INFECT).
///
/// Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/PhrogParasite.cs:55-100`:
///   - INFECT_MOVE = `StatusIntent(3)`; body adds 3 Infection cards to the
///     player's discard pile (via [`phrog_parasite_infect_effect`]);
///     follow-up LASH.
///   - LASH_MOVE   = `MultiAttackIntent(LashDamage=4, 4)` — 4 damage × 4
///     hits; follow-up INFECT.
/// The state machine's initial state is INFECT_MOVE
/// (`return new MonsterMoveStateMachine(list, moveState)`). The C# also
/// builds a `RandomBranchState`, but never wires it as any move's
/// `FollowUpState` — it is dead code; the live cycle is the linear
/// INFECT ↔ LASH alternation. On-spawn `InfestedPower(4)` is applied via
/// [`MonsterKind::on_spawn_self_buffs`].
///
/// Bead `sts2-rs-8ukqd` (bug C) — replaces the prior fabricated
/// `LUNGE{12} → INFEST{StatusCard} → BITE{4×3}` moveset, which had no
/// basis in the v0.103.2 decomp (wrong move names, wrong damage, wrong
/// cycle, and INFEST added no cards).
pub fn create_phrog_parasite_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "INFECT",
        mv_eff(
            "INFECT",
            smallvec![Intent::StatusCard],
            "LASH",
            phrog_parasite_infect_effect,
        ),
    );
    s.insert(
        "LASH",
        mv(
            "LASH",
            smallvec![Intent::MultiAttack { damage: 4, hits: 4 }],
            "INFECT",
        ),
    );
    MonsterAi::new(s, "INFECT")
}

// ────────────────────────────────────────────────────────────────────────────
// Boss encounters
// ────────────────────────────────────────────────────────────────────────────

/// `Parafright` — SLAM self-loop per C# decomp `Parafright.cs`.
/// Bead `sts2-rs-ksa5` bumped from the pre-rebaseline Python pin
/// 01c094d minion stub (HAUNT = Attack 5 + Frail 1, HP 1). The C#
/// spec is a single SLAM Attack 16 with HP 21. It is spawned mid-combat
/// by TheObscura's ILLUSION (the old Vantom CONSUME path was fabricated and
/// removed, bead sts2-rs-r8x8h), so it is a clone of a per-thread
/// prototype (`proto_ai`).
pub fn create_parafright_ai() -> MonsterAi {
    thread_local! {
        static PROTO: ProtoAis<1> = const { proto_ais() };
    }
    proto_ai(&PROTO, 0, build_parafright_ai)
}

pub(crate) fn build_parafright_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SLAM",
        mv("SLAM", smallvec![Intent::Attack { damage: 16 }], "SLAM"),
    );
    MonsterAi::new(s, "SLAM")
}

/// `Vantom` — 4-state linear cycle per C# decomp
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/Vantom.cs:93-109`:
///
/// ```text
///   INK_BLOT → INKY_LANCE → DISMEMBER → PREPARE → INK_BLOT (loops)
/// ```
///
/// - `INK_BLOT`: single Attack 7 (8 at DeadlyEnemies ascension)
/// - `INKY_LANCE`: MultiAttack 6 × 2 (7 × 2 at DeadlyEnemies)
/// - `DISMEMBER`: Attack 26 (30 at DeadlyEnemies) + StatusIntent(3) →
///   3 Wounds added to player's discard pile via `vantom_dismember_effect`
///   (v0.107.1 nerf: DismemberDamage 27→26 at A0, decomp
///   `Vantom.cs:72 GetValueIfAscension(DeadlyEnemies, 30, 26)`).
/// - `PREPARE`: Buff intent → effect body applies +2 Strength to self
///
/// Vantom also gets `SlipperyPower(8)` applied on spawn via
/// [`crate::sts2_core::MonsterKind::on_spawn_self_buffs`] (the first 9
/// hits to Vantom deal 1 damage each, see C# `Vantom.AfterAddedToRoom`).
///
/// Damage values are the non-ascension baseline; ascension scaling
/// (DeadlyEnemies → 8/7×2/30) is a follow-up if needed.
///
/// **Bead history:** the prior implementation (beads `sts2-rs-vn0m`,
/// `sts2-rs-47p5`, `sts2-rs-5est`) had Vantom spawning 2 Parafrights via
/// a fake CONSUME move and then random-branching into
/// CHOMP/GHASTLY_SMASH/WAIL. None of that exists in v0.103.2 C# —
/// VantomBoss.GenerateMonsters returns a single Vantom and Vantom's
/// move-state machine is purely the InkBlot/InkyLance/Dismember/Prepare
/// loop. Rewritten 2026-05-24 (bead `sts2-rs-r8x8h`) to match the C#
/// source-of-truth.
pub fn create_vantom_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "INK_BLOT",
        mv(
            "INK_BLOT",
            smallvec![Intent::Attack { damage: 7 }],
            "INKY_LANCE",
        ),
    );
    s.insert(
        "INKY_LANCE",
        mv(
            "INKY_LANCE",
            smallvec![Intent::MultiAttack { damage: 6, hits: 2 }],
            "DISMEMBER",
        ),
    );
    s.insert(
        "DISMEMBER",
        mv_eff(
            "DISMEMBER",
            smallvec![Intent::Attack { damage: 26 }, Intent::StatusCard],
            "PREPARE",
            vantom_dismember_effect,
        ),
    );
    s.insert(
        "PREPARE",
        mv_eff(
            "PREPARE",
            smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2,
            }],
            "INK_BLOT",
            vantom_prepare_effect,
        ),
    );
    MonsterAi::new(s, "INK_BLOT")
}

/// Owner-aware predicate: the CeremonialBeast's current HP has dropped to
/// `<= its PLOW power amount` — the C# trigger for the stun phase.
///
/// C# `PlowPower.AfterDamageReceived`
/// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Powers/PlowPower.cs`)
/// stuns the beast when `target.CurrentHp <= base.Amount` (PlowAmount =
/// 150). PLOW does NOT tick down — the prior Rust model gated on
/// `PLOW <= 0`, which never happened, so the beast never entered phase 2.
/// Returns `false` once PLOW has been removed (the STUN move's effect
/// strips it), keeping the beast in its phase-2 BEAST_CRY/STOMP/CRUSH
/// cycle. Beads `sts2-rs-msi1`, `sts2-rs-plow-stun`.
fn ceremonial_beast_hp_at_plow_threshold(
    combat: &sts2_core::CombatState,
    owner: CreatureUid,
) -> bool {
    let Some(creature) = combat.creatures.iter().find(|c| c.id == owner.0) else {
        return false;
    };
    // No PLOW power → already stunned (or never stamped); do not re-enter.
    let Some(plow_amount) = creature.powers.iter().find_map(|puid| {
        combat
            .powers
            .iter()
            .find(|p| p.id == puid.0 && p.kind == PowerKind::Plow)
            .map(|p| p.amount)
    }) else {
        return false;
    };
    creature.current_hp <= plow_amount
}

/// C# `PlowPower.AfterDamageReceived` on-stun cleanup: when the beast
/// enters its stun phase it loses all Strength and the PLOW power is
/// removed (`PowerCmd.Remove<StrengthPower>` + `PowerCmd.Remove(this)`).
/// C# fires this from the damage hook the instant HP crosses the
/// threshold; the Rust port performs the wipe as the STUN move's effect
/// (one beast-turn later — after the already-telegraphed PLOW resolves).
/// Bead `sts2-rs-plow-stun`.
/// C# `PlowPower.AfterDamageReceived` on-damage immediate cleanup: when the
/// beast takes damage and its HP drops to `<= PlowAmount` (150), immediately
/// strip Strength and Plow from the creature. Mirrors C#
/// `PlowPower.cs:AfterDamageReceived` which fires synchronously within the
/// damage-received event handler. Bead `sts2-rs-plow-stun` (follow-up).
fn plow_after_damage(
    state: &mut CombatState,
    owner: CreatureUid,
    _dealer: sts2_core::hooks::CreatureRef,
    _props: sts2_core::hooks::ValueProps,
    unblocked_damage: i32,
) -> sts2_core::EffectResult {
    use sts2_core::EffectResult;
    // C# gate: target == owner (PlowPower fires only on the beast that carries it).
    // The dispatch already scopes to the target creature, so this is implicit.
    //
    // C# gate 2 (`PlowPower.cs:30`): `result.UnblockedDamage <= 0` → return.
    // A fully-blocked hit must NOT trigger the stun, even if the beast's HP is
    // already at/below the threshold (only reachable when HP crossed 150 via a
    // path that skipped the damage-received bus).
    if unblocked_damage <= 0 {
        return EffectResult::NotImplemented;
    }
    let Some(creature) = state.creatures.iter().find(|c| c.id == owner.0) else {
        return EffectResult::NotImplemented;
    };
    // Find the Plow power amount on this creature.
    let Some(plow_amount) = creature.powers.iter().find_map(|puid| {
        state
            .powers
            .iter()
            .find(|p| p.id == puid.0 && p.kind == PowerKind::Plow)
            .map(|p| p.amount)
    }) else {
        return EffectResult::NotImplemented;
    };
    // C# gate: `target.CurrentHp <= base.Amount` (PlowAmount = 150).
    if creature.current_hp > plow_amount {
        return EffectResult::NotImplemented;
    }
    // C# `PlowPower.cs:34` flashes right here — both guards above passed —
    // before the Strength/Plow strip.
    sts2_core::event_log::flash_power(state, owner, PowerKind::Plow);
    // Strip the WHOLE TemporaryStrengthPower family + Strength + Plow.
    // C# `PlowPower.cs:35-40,51`: `GetPowerInstances<TemporaryStrengthPower>()`
    // (an `is`-typed walk over all 13 subclasses) is removed FIRST, then
    // `PowerCmd.Remove<StrengthPower>`, then (post-stun) `PowerCmd.Remove(this)`.
    // Removing the temporaries matters beyond their own additive contribution:
    // a survivor's `TemporaryStrengthPower.AfterSideTurnEnd`
    // (`TemporaryStrengthPower.cs:167-176`) refunds `-Sign * Amount` Strength
    // at the beast's turn end — Strength C# deleted with the stun. Pre-fix
    // Rust stripped only `Strength | Plow`, so a player-parked PiercingWail /
    // DarkShackles / Mangle stack resurrected beast Strength one turn later
    // and every phase-2 STOMP/CRUSH hit diverged upward.
    let pids: smallvec::SmallVec<[std::num::NonZeroU32; 4]> = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .map(|c| {
            c.powers
                .iter()
                .filter(|puid| {
                    state.powers.iter().any(|p| {
                        p.id == puid.0
                            && (p.kind == PowerKind::Strength
                                || p.kind == PowerKind::Plow
                                || sts2_core::afflictions::is_temporary_strength_family(p.kind))
                    })
                })
                .map(|puid| puid.0)
                .collect()
        })
        .unwrap_or_default();
    if pids.is_empty() {
        return EffectResult::NotImplemented;
    }
    state.powers.retain(|p| !pids.contains(&p.id));
    for c in &mut state.creatures {
        c.powers.retain(|u| !pids.contains(&u.0));
    }
    // C# `PlowPower.AfterDamageReceived` also `CreatureCmd.Stun(StunnedMove)` —
    // an IMMEDIATE move override that INTERRUPTS the already-telegraphed PLOW so
    // the beast performs STUN (0 dmg, no Strength) next turn instead of one more
    // PLOW (18 dmg + 2 Str). Route through `Effect::ForceMonsterMove`, NOT a bare
    // `ai.force_current_state`: the Effect ALSO re-points the snapshotted
    // `creature.intent` telegraph (gpdng fix), without which `enemy_turn_exec`
    // performs the STALE PLOW attack (coverage seed 1536: player.hp cs=44 rs=26
    // — Strength matched but the 18-damage PLOW still landed). Bead
    // `sts2-rs-plow-interrupt`.
    sts2_core::hooks::apply_effects(
        state,
        &[sts2_core::hooks::Effect::ForceMonsterMove {
            target: owner,
            state_id: "STUN",
        }],
    );
    EffectResult::Resolved
}

/// Idempotent installer for Plow's AfterDamageReceived hook. Called via a
/// `Once` guard from [`create_ceremonial_beast_ai`]. Safe to call from
/// multiple boot paths.
fn install_plow_handlers() {
    sts2_core::monster_power_effects::register_after_damage_received_for_power(
        PowerKind::Plow,
        plow_after_damage,
    );
}

fn ceremonial_beast_stun_effect(state: &mut CombatState, owner: CreatureUid) {
    // C# `StunnedMove` (CeremonialBeast.cs:191-196) only clears the
    // `IsStunnedByPlowRemoval` flag + plays the Unstun anim — it does **not**
    // remove any power. The Strength + Plow strip is owned solely by C#
    // `PlowPower.AfterDamageReceived` (mirrored by `plow_after_damage`), which
    // fires the instant HP crosses the threshold. This STUN-move effect must
    // therefore NOT touch Strength: a Strength stack applied to the beast
    // AFTER the threshold-crossing strip (which both engines agree on) is
    // retained by C# but was wrongly re-stripped here (parity seed 5374:
    // `monsters[0].powers.len` cs=1 [Strength] rs=0 — Rust dropped the post-
    // stun Strength one enemy turn later).
    //
    // The Plow strip is kept as a defensive phase-gate fallback: in the normal
    // path `plow_after_damage` already removed it (so this is a no-op), but if
    // the beast ever reached the threshold without a damage-received event the
    // gate (`ceremonial_beast_hp_at_plow_threshold`) still needs Plow gone to
    // stay in phase 2. Beads `sts2-rs-plow-stun`, `sts2-rs-1opt9`
    // (Strength-retention fix).
    let pids: smallvec::SmallVec<[std::num::NonZeroU32; 4]> = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .map(|c| {
            c.powers
                .iter()
                .filter(|puid| {
                    state
                        .powers
                        .iter()
                        .any(|p| p.id == puid.0 && p.kind == PowerKind::Plow)
                })
                .map(|puid| puid.0)
                .collect()
        })
        .unwrap_or_default();
    if pids.is_empty() {
        return;
    }
    state.powers.retain(|p| !pids.contains(&p.id));
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
        c.powers.retain(|u| !pids.contains(&u.0));
    }
}

/// `CeremonialBeast` — STAMP → PLOW → PLOW_CHECK (STUN if PLOW power
/// depleted, else PLOW) → STUN once → BEAST_CRY → STOMP → CRUSH →
/// BEAST_CRY …
///
/// `PLOW_CHECK` is a `ConditionalBranchState` whose first branch fires
/// when the beast's HP has dropped to `<= its PLOW amount` (C#
/// `PlowPower.AfterDamageReceived`'s `CurrentHp <= base.Amount`); the
/// fallthrough always-true branch loops back to `PLOW`. The PLOW power
/// stays at 150 the whole time — it is a *threshold*, not a counter. So
/// the beast self-loops PLOW (18 dmg + Strength) until the player drops
/// it to ≤150 HP, then phase 2 (STUN → BEAST_CRY → STOMP → CRUSH) begins;
/// the STUN move strips the beast's Strength + PLOW on entry.
///
/// Bead `sts2-rs-msi1`: PLOW_CHECK is wired via the owner-aware
/// `ConditionFn::OwnerAware` variant, threaded through
/// [`MonsterAi::owner_uid`]. The combat loop sets the owner before
/// each `roll_move_with_combat` (`combat_loop.rs:end_enemy_turn`) so
/// the predicate reads the boss's actual HP + PLOW stack. AI-only unit
/// tests that don't go through the combat loop see `owner_uid = None`
/// and the predicate degrades to `false`, keeping the PLOW self-loop.
///
/// Bead `sts2-rs-plow-stun`: switched the gate from the never-true
/// `PLOW <= 0` (PLOW never depleted, so the beast never stunned) to the
/// C#-faithful HP threshold. The one-beast-turn lag (the already-
/// telegraphed PLOW resolves before STUN) is a minor divergence from
/// C#'s immediate `CreatureCmd.Stun` interrupt.
pub fn create_ceremonial_beast_ai() -> MonsterAi {
    // Install the plow_after_damage handler once.
    use std::sync::OnceLock;
    static INSTALL_PLOW: OnceLock<()> = OnceLock::new();
    INSTALL_PLOW.get_or_init(install_plow_handlers);

    let mut s = BTreeMap::new();
    s.insert(
        "STAMP",
        mv_eff(
            "STAMP",
            // C# `StampMove` applies PlowPower(150) to self via a generic
            // `BuffIntent()`; the telegraph icon carries no power amount in
            // C#. Rust models the telegraph as Buff{Plow,150}.
            smallvec![Intent::Buff {
                kind: PowerKind::Plow,
                amount: 150
            }],
            "PLOW",
            ceremonial_beast_stamp_effect,
        ),
    );
    s.insert(
        "PLOW",
        mv_eff(
            "PLOW",
            smallvec![
                Intent::Attack { damage: 18 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2
                }
            ],
            "PLOW_CHECK",
            ceremonial_beast_plow_effect,
        ),
    );
    s.insert(
        "PLOW_CHECK",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "PLOW_CHECK",
            branches: smallvec![
                ConditionalBranch {
                    condition: ConditionFn::OwnerAware(ceremonial_beast_hp_at_plow_threshold),
                    state_id: "STUN",
                },
                ConditionalBranch {
                    condition: ConditionFn::Stateless(|_| true),
                    state_id: "PLOW",
                },
            ],
        }),
    );
    s.insert(
        "STUN",
        // STUN performs once (Intent::Stun, a no-op turn) then advances to
        // BEAST_CRY. Its effect strips the beast's Strength + PLOW power
        // (C# `PlowPower.AfterDamageReceived`). `must_perform_once: true`
        // mirrors C# `StunMove.MustPerformOnceBeforeTransitioning`.
        MonsterState::Move(MoveState {
            id: "STUN",
            effect: ceremonial_beast_stun_effect,
            intents: smallvec![Intent::Stun],
            follow_up: Some("BEAST_CRY"),
            must_perform_once: true,
        }),
    );
    s.insert(
        "BEAST_CRY",
        mv_eff(
            "BEAST_CRY",
            // C# `BeastCryMove` applies RingingPower(1) to the player via a
            // generic `DebuffIntent()`; the prior Rust metadata Weak,1 was a
            // fabrication.
            smallvec![Intent::Debuff {
                kind: PowerKind::Ringing,
                amount: 1
            }],
            "STOMP",
            ceremonial_beast_beast_cry_effect,
        ),
    );
    s.insert(
        "STOMP",
        mv("STOMP", smallvec![Intent::Attack { damage: 15 }], "CRUSH"),
    );
    s.insert(
        "CRUSH",
        mv_eff(
            "CRUSH",
            smallvec![
                Intent::Attack { damage: 17 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 3
                }
            ],
            "BEAST_CRY",
            ceremonial_beast_crush_effect,
        ),
    );
    MonsterAi::new(s, "STAMP")
}

/// `KinPriest` — ORB_OF_FRAILTY → ORB_OF_WEAKNESS → BEAM → RITUAL →
/// loop. Mirrors C# decomp
/// `MegaCrit.Sts2.Core.Models.Monsters/KinPriest.cs` v0.103.2 (bead
/// `sts2-rs-1jlo`, parent `sts2-rs-kgwj`). Python pin `01c094d` ships
/// a stale 119 HP / 2-move CONVERSION-SMITE variant we deliberately
/// diverge from per the monster-stat audit
/// (`docs/perf/2026-05-18-monster-stat-audit.md`, Pattern A -37% HP).
///
/// Move payloads track C# constants:
/// - ORB_OF_FRAILTY_MOVE → `SingleAttackIntent(8)` + `DebuffIntent`
///   applying `FrailPower(1)`.
/// - ORB_OF_WEAKNESS_MOVE → `SingleAttackIntent(8)` + `DebuffIntent`
///   applying `WeakPower(1)`.
/// - BEAM_MOVE → `MultiAttackIntent(3, 3)`.
/// - RITUAL_MOVE → `BuffIntent` applying `StrengthPower(2)` to self.
pub fn create_kin_priest_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "ORB_OF_FRAILTY_MOVE",
        mv_eff(
            "ORB_OF_FRAILTY_MOVE",
            smallvec![
                Intent::Attack { damage: 8 },
                Intent::Debuff {
                    kind: PowerKind::Frail,
                    amount: 1,
                },
            ],
            "ORB_OF_WEAKNESS_MOVE",
            kin_priest_orb_of_frailty_effect,
        ),
    );
    s.insert(
        "ORB_OF_WEAKNESS_MOVE",
        mv_eff(
            "ORB_OF_WEAKNESS_MOVE",
            smallvec![
                Intent::Attack { damage: 8 },
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 1,
                },
            ],
            "BEAM_MOVE",
            kin_priest_orb_of_weakness_effect,
        ),
    );
    s.insert(
        "BEAM_MOVE",
        mv(
            "BEAM_MOVE",
            smallvec![Intent::MultiAttack { damage: 3, hits: 3 }],
            "RITUAL_MOVE",
        ),
    );
    s.insert(
        "RITUAL_MOVE",
        mv_eff(
            "RITUAL_MOVE",
            smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2,
            }],
            "ORB_OF_FRAILTY_MOVE",
            kin_priest_ritual_effect,
        ),
    );
    MonsterAi::new(s, "ORB_OF_FRAILTY_MOVE")
}

/// `KinFollower` start-state selector. C# `KinFollower.StartsWithDance` is an
/// encounter-set flag (default false → start on QUICK_SLASH); the Rust port
/// has no encounter-flag plumbing yet, so `slot=Third` is the stand-in for
/// `StartsWithDance=true` (start on POWER_DANCE) and everyone else starts on
/// QUICK_SLASH. Bead `sts2-rs-29kfv`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KinFollowerSlot {
    First,
    Second,
    Third,
}

/// `KinFollower` POWER_DANCE → `PowerCmd.Apply<StrengthPower>(base.Creature,
/// DanceStrength=2)` (`KinFollower.cs:132`, Asc-0). `Intent::Buff` is
/// telegraph-only; the Strength self-buff is applied here.
fn kin_follower_power_dance_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    let amount = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::DeadlyEnemies,
        3,
        2,
    );
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// `KinFollower` — cyclic QUICK_SLASH → BOOMERANG → POWER_DANCE. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/KinFollower.cs`
/// `GenerateMoveStateMachine` (Asc-0): QUICK_SLASH `SingleAttackIntent(5)`;
/// BOOMERANG `MultiAttackIntent(2, 2)`; POWER_DANCE `BuffIntent` →
/// `Strength(2)` self. Bead `sts2-rs-29kfv`: replaces the fabricated
/// BASH/BITE moveset (no C# basis — KinFollower has no BASH/BITE/Weak move).
/// The on-spawn `MinionPower(1)` from `AfterAddedToRoom` is a separate
/// `on_spawn_self_buffs` concern, not part of this moveset rebase.
pub fn create_kin_follower_ai(slot: KinFollowerSlot) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "QUICK_SLASH",
        mv(
            "QUICK_SLASH",
            smallvec![Intent::Attack { damage: 5 }],
            "BOOMERANG",
        ),
    );
    s.insert(
        "BOOMERANG",
        mv(
            "BOOMERANG",
            smallvec![Intent::MultiAttack { damage: 2, hits: 2 }],
            "POWER_DANCE",
        ),
    );
    s.insert(
        "POWER_DANCE",
        mv_eff(
            "POWER_DANCE",
            smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2
            }],
            "QUICK_SLASH",
            kin_follower_power_dance_effect,
        ),
    );
    let initial = if matches!(slot, KinFollowerSlot::Third) {
        "POWER_DANCE"
    } else {
        "QUICK_SLASH"
    };
    MonsterAi::new(s, initial)
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: every ctor produces an AI whose initial move can be
    /// fetched without panicking.
    #[test]
    fn every_ctor_has_resolvable_initial_move() {
        let smokes: Vec<MonsterAi> = vec![
            create_cubex_construct_ai(),
            create_flyconid_ai(),
            create_eye_with_teeth_ai(),
            create_fogmog_ai(),
            create_inklet_ai(InkletSlot::First),
            create_inklet_ai(InkletSlot::Second),
            create_inklet_ai(InkletSlot::Other),
            create_mawler_ai(),
            create_vine_shambler_ai(),
            create_slithering_strangler_ai(),
            create_snapping_jaxfruit_ai(),
            create_assassin_ruby_raider_ai(),
            create_axe_ruby_raider_ai(),
            create_brute_ruby_raider_ai(),
            create_crossbow_ruby_raider_ai(),
            create_tracker_ruby_raider_ai(),
            create_bygone_effigy_ai(),
            create_byrdonis_ai(),
            create_phrog_parasite_ai(),
            create_parafright_ai(),
            create_vantom_ai(),
            create_ceremonial_beast_ai(),
            create_kin_priest_ai(),
            create_kin_follower_ai(KinFollowerSlot::First),
            create_kin_follower_ai(KinFollowerSlot::Third),
        ];
        for ai in smokes {
            let _ = ai.current_move();
        }
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
    fn cubex_construct_repeater_chain() {
        let mut ai = create_cubex_construct_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["CHARGE_UP", "REPEATER", "REPEATER_2"]
        );
    }

    #[test]
    fn axe_raider_swing_chain() {
        let mut ai = create_axe_ruby_raider_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["SWING_1", "SWING_2", "BIG_SWING"]
        );
    }

    #[test]
    fn brute_raider_alternates() {
        let mut ai = create_brute_ruby_raider_ai();
        assert_eq!(run_three_moves(&mut ai), vec!["BEAT", "ROAR", "BEAT"]);
    }

    #[test]
    fn crossbow_raider_alternates() {
        let mut ai = create_crossbow_ruby_raider_ai();
        assert_eq!(run_three_moves(&mut ai), vec!["RELOAD", "FIRE", "RELOAD"]);
    }

    #[test]
    fn assassin_raider_self_loop() {
        let mut ai = create_assassin_ruby_raider_ai();
        for _ in 0..5 {
            assert_eq!(ai.current_move().id, "KILLSHOT");
            ai.on_move_performed();
            ai.roll_move_seeded(0);
        }
    }

    #[test]
    fn tracker_raider_lock_into_hounds() {
        let mut ai = create_tracker_ruby_raider_ai();
        assert_eq!(ai.current_move().id, "TRACK");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "HOUNDS");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(1).id, "HOUNDS");
    }

    /// Bead `sts2-rs-p8n60` (o1mwv C.5): CONSTRICT → rand(TWACK | LASH)
    /// → CONSTRICT, per C# `SlitheringStrangler.GenerateMoveStateMachine`.
    #[test]
    fn slithering_strangler_constrict_then_random_attack_then_constrict() {
        let mut ai = create_slithering_strangler_ai();
        let seq = run_three_moves(&mut ai);
        assert_eq!(seq[0], "CONSTRICT", "initial move is CONSTRICT");
        assert!(
            seq[1] == "TWACK" || seq[1] == "LASH",
            "CONSTRICT branches to TWACK or LASH, got {}",
            seq[1]
        );
        assert_eq!(seq[2], "CONSTRICT", "TWACK/LASH follow up to CONSTRICT");
    }

    /// CONSTRICT telegraphs a DEBUFF intent and applies ConstrictPower(3)
    /// to the player; TWACK telegraphs Attack(7) + Defend.
    #[test]
    fn slithering_strangler_constrict_move_shape() {
        let ai = create_slithering_strangler_ai();
        assert_eq!(
            ai.current_move().intents.as_slice(),
            &[Intent::Debuff {
                kind: PowerKind::Constrict,
                amount: 3
            }]
        );
    }

    #[test]
    fn byrdonis_alternates() {
        let mut ai = create_byrdonis_ai();
        assert_eq!(run_three_moves(&mut ai), vec!["SWOOP", "PECK", "SWOOP"]);
    }

    /// Bead `sts2-rs-8ukqd` (R1): the Flyconid `RAND` branch weights must
    /// all be `1.0` with C# cooldowns — the integers in `Flyconid.cs`
    /// `AddBranch(state, N, CannotRepeat)` are cooldowns, NOT weights. The
    /// original port encoded them as weights `[3, 2, 1]` (total 4), which
    /// skewed later-turn move selection: a roll-fraction in `[0.5, 0.75)`
    /// over the correct total-2 weights picks SMASH, but over the buggy
    /// total-4 weights picks VULNERABLE_SPORES.
    ///
    /// This drives the `RAND` branch on a *shared* `monster_ai` stream
    /// (mirroring `combat_loop`, not the fresh-per-call `roll_move_seeded`)
    /// seeded at `1918040094` — the `parity_fuzz` `monster_ai` stream seed,
    /// whose `NextDouble` sequence `[0.4615, 0.6811, 0.7285, …]` is verified
    /// byte-identical to C# `System.Random`. After the initial FRAIL_SPORES
    /// (`state_log = [FRAIL_SPORES]`, FRAIL excluded by CannotRepeat) the
    /// `RAND` weights are `[VULN 1, FRAIL 0, SMASH 1]` (total 2): the
    /// 0.7285 sample → SMASH. Under the old `[3, 0, 1]` (total 4) it was
    /// VULNERABLE_SPORES — exactly the seed-0 PLAYER_HP divergence.
    #[test]
    fn flyconid_rand_unit_weights_pick_smash_8ukqd() {
        use sts2_core::DotNetRng;
        let mut ai = create_flyconid_ai();
        // Drive the machine so the most-recent logged move is FRAIL_SPORES
        // (CannotRepeat then excludes FRAIL from the RAND branch). The
        // constructor's throwaway initial is SMASH; force a FRAIL_SPORES
        // performance on top so `state_log` ends in FRAIL_SPORES.
        ai.on_move_performed(); // performs throwaway initial → log = [SMASH]
        ai.force_next_state("FRAIL_SPORES");
        let mut forced = DotNetRng::new(0); // forced transition draws nothing
        assert_eq!(ai.roll_move(&mut forced).id, "FRAIL_SPORES");
        ai.on_move_performed(); // log = [SMASH, FRAIL_SPORES]

        // RAND weights with FRAIL excluded: fix = [VULN 1, FRAIL 0, SMASH 1]
        // (total 2) vs the old cooldown-as-weight bug = [3, 0, 1] (total 4).
        // The verified 0.7285 sample (seed 1918040094, draw #3) → SMASH
        // under total 2, but VULNERABLE_SPORES under the buggy total 4.
        let mut rng = DotNetRng::new(1_918_040_094_u32 as i32);
        let _ = rng.next_float(1.0); // draw #1
        let _ = rng.next_float(1.0); // draw #2 → next sample is 0.7285
        assert_eq!(ai.roll_move(&mut rng).id, "SMASH");
    }

    #[test]
    fn phrog_parasite_three_chain() {
        // C# PhrogParasite.cs: INFECT_MOVE ↔ LASH_MOVE, start INFECT.
        let mut ai = create_phrog_parasite_ai();
        assert_eq!(run_three_moves(&mut ai), vec!["INFECT", "LASH", "INFECT"]);
    }

    /// Bead `sts2-rs-8ukqd` (bug C): PhrogParasite INFECT telegraphs a
    /// `StatusCard` and its `effect_fn` adds 3 Infection cards to the
    /// player's discard pile; LASH is `MultiAttack(4, 4)`.
    #[test]
    fn phrog_parasite_move_shapes_match_v103_decomp() {
        let ai = create_phrog_parasite_ai();
        assert_eq!(ai.current_move().id, "INFECT");
        assert_eq!(ai.current_move().intents.as_slice(), &[Intent::StatusCard]);
        let MonsterState::Move(lash) = ai.states.get("LASH").expect("LASH exists") else {
            panic!("LASH should be a Move state");
        };
        assert_eq!(
            lash.intents.as_slice(),
            &[Intent::MultiAttack { damage: 4, hits: 4 }]
        );
    }

    /// Bead `sts2-rs-8ukqd` (bug C): INFECT adds exactly 3 Infection
    /// cards to the player's discard pile.
    #[test]
    fn phrog_parasite_infect_effect_adds_three_infection_cards() {
        use sts2_core::cards::CardId;
        use sts2_core::CombatState;

        let mut state = CombatState::default();
        let pre = state.player.discard.len();
        phrog_parasite_infect_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(state.player.discard.len(), pre + 3);
        for uid in &state.player.discard {
            let idx = uid.0.get() as usize - 1;
            assert_eq!(state.cards[idx].card_id, CardId::Infection);
        }
    }

    /// Bead `sts2-rs-fqcx0`: EyeWithTeeth DISTRACT adds exactly 3 `Dazed`
    /// cards to the player's discard (C# `StatusIntent(3)` /
    /// `AddToCombatAndPreview<Dazed>(…, Discard, 3, …)`).
    #[test]
    fn eye_with_teeth_distract_effect_adds_three_dazed() {
        use sts2_core::cards::CardId;
        use sts2_core::CombatState;

        let mut state = CombatState::default();
        let pre = state.player.discard.len();
        eye_with_teeth_distract_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(state.player.discard.len(), pre + 3);
        for uid in &state.player.discard {
            let idx = uid.0.get() as usize - 1;
            assert_eq!(state.cards[idx].card_id, CardId::Dazed);
        }
    }

    /// Bead `sts2-rs-ksa5`: post-rebaseline Parafright self-loops
    /// on SLAM (Attack 16) per C# decomp. Replaces the
    /// pre-rebaseline HAUNT (Attack 5 + Frail 1) self-loop.
    #[test]
    fn parafright_slam_self_loop() {
        let mut ai = create_parafright_ai();
        for _ in 0..5 {
            assert_eq!(ai.current_move().id, "SLAM");
            assert_eq!(
                ai.current_move().intents.as_slice(),
                &[Intent::Attack { damage: 16 }]
            );
            ai.on_move_performed();
            ai.roll_move_seeded(0);
        }
    }

    /// Bead `sts2-rs-o1mwv` (C.3): SnappingJaxfruit self-loops on a
    /// single `ENERGY_ORB_MOVE` per C# `SnappingJaxfruit.cs:62-95`
    /// (`GenerateMoveStateMachine` builds one self-`FollowUpState` move
    /// with `SingleAttackIntent(3)` + `BuffIntent()`). Replaces the
    /// port-invented 3-move RAND(SNAP|SEED_SPIT|BURROW) branch.
    #[test]
    fn snapping_jaxfruit_energy_orb_self_loop() {
        let mut ai = create_snapping_jaxfruit_ai();
        for _ in 0..5 {
            assert_eq!(ai.current_move().id, "ENERGY_ORB_MOVE");
            assert_eq!(
                ai.current_move().intents.as_slice(),
                &[
                    Intent::Attack { damage: 3 },
                    Intent::Buff {
                        kind: PowerKind::Strength,
                        amount: 2,
                    },
                ]
            );
            ai.on_move_performed();
            ai.roll_move_seeded(0);
        }
    }

    #[test]
    fn kin_priest_orb_weak_beam_ritual_cycle() {
        // Per C# decomp: ORB_OF_FRAILTY → ORB_OF_WEAKNESS → BEAM →
        // RITUAL → ORB_OF_FRAILTY.
        let mut ai = create_kin_priest_ai();
        assert_eq!(ai.current_move().id, "ORB_OF_FRAILTY_MOVE");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "ORB_OF_WEAKNESS_MOVE");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "BEAM_MOVE");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "RITUAL_MOVE");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "ORB_OF_FRAILTY_MOVE");
    }

    #[test]
    fn kin_follower_slot_dependent_initial() {
        assert_eq!(
            create_kin_follower_ai(KinFollowerSlot::First)
                .current_move()
                .id,
            "QUICK_SLASH"
        );
        assert_eq!(
            create_kin_follower_ai(KinFollowerSlot::Second)
                .current_move()
                .id,
            "QUICK_SLASH"
        );
        assert_eq!(
            create_kin_follower_ai(KinFollowerSlot::Third)
                .current_move()
                .id,
            "POWER_DANCE"
        );
    }

    #[test]
    fn inklet_slot_dependent_initial() {
        // C# `_middleInklet ? WHIRLWIND : JAB`. Only the middle Inklet
        // (encounter index 1 → InkletSlot::Second) opens with WHIRLWIND;
        // every other slot opens with JAB. All three are FORCED moves, so
        // none consumes a turn-0 `monster_ai` roll.
        assert_eq!(create_inklet_ai(InkletSlot::First).current_move().id, "JAB");
        assert_eq!(
            create_inklet_ai(InkletSlot::Second).current_move().id,
            "WHIRLWIND"
        );
        assert_eq!(create_inklet_ai(InkletSlot::Other).current_move().id, "JAB");
    }

    #[test]
    fn mawler_random_never_roars_twice() {
        let mut ai = create_mawler_ai();
        let mut roars = 0u32;
        for seed in 0..200 {
            ai.on_move_performed();
            let id = ai.roll_move_seeded(seed).id;
            if id == "ROAR" {
                roars += 1;
                assert!(
                    roars <= 1,
                    "ROAR fired twice (UseOnlyOnce broken) at seed {seed}"
                );
            }
        }
    }

    /// Bead `sts2-rs-r8x8h`: Vantom's move-state machine is the
    /// 4-state linear cycle InkBlot → InkyLance → Dismember → Prepare,
    /// per C# `Vantom.GenerateMoveStateMachine` at
    /// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/Vantom.cs:93-109`.
    /// Each state carries the intents the player sees telegraphed.
    #[test]
    fn vantom_intents_match_csharp_pin() {
        use sts2_core::monster_ai::MonsterState;
        use sts2_core::monsters::Intent;
        let ai = create_vantom_ai();

        let MonsterState::Move(ink_blot) = ai.states.get("INK_BLOT").expect("INK_BLOT exists")
        else {
            panic!("INK_BLOT is a MoveState")
        };
        assert_eq!(ink_blot.intents.len(), 1);
        assert!(matches!(ink_blot.intents[0], Intent::Attack { damage: 7 }));
        assert_eq!(ink_blot.follow_up, Some("INKY_LANCE"));

        let MonsterState::Move(inky_lance) =
            ai.states.get("INKY_LANCE").expect("INKY_LANCE exists")
        else {
            panic!("INKY_LANCE is a MoveState")
        };
        assert_eq!(inky_lance.intents.len(), 1);
        assert!(matches!(
            inky_lance.intents[0],
            Intent::MultiAttack { damage: 6, hits: 2 }
        ));
        assert_eq!(inky_lance.follow_up, Some("DISMEMBER"));

        let MonsterState::Move(dismember) = ai.states.get("DISMEMBER").expect("DISMEMBER exists")
        else {
            panic!("DISMEMBER is a MoveState")
        };
        assert_eq!(dismember.intents.len(), 2);
        assert!(matches!(
            dismember.intents[0],
            Intent::Attack { damage: 26 }
        ));
        assert!(matches!(dismember.intents[1], Intent::StatusCard));
        assert_eq!(dismember.follow_up, Some("PREPARE"));

        let MonsterState::Move(prepare) = ai.states.get("PREPARE").expect("PREPARE exists") else {
            panic!("PREPARE is a MoveState")
        };
        assert_eq!(prepare.intents.len(), 1);
        assert!(matches!(
            prepare.intents[0],
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2
            }
        ));
        assert_eq!(
            prepare.follow_up,
            Some("INK_BLOT"),
            "PREPARE loops back to INK_BLOT"
        );

        // Old states from the fabricated CONSUME AI must be gone.
        assert!(!ai.states.contains_key("CONSUME"));
        assert!(!ai.states.contains_key("CHOMP"));
        assert!(!ai.states.contains_key("WAIL"));
    }

    /// Bead `sts2-rs-8ukqd` (bug C): VineShambler state machine matches
    /// C# `VineShambler.cs:43-87` — SWIPE (6×2) → GRASPING_VINES (8 +
    /// Tangled) → CHOMP (16) → SWIPE, initial state SWIPE. Guards against
    /// regressing to the fabricated VINE_WHIP/TANGLE moveset that drove
    /// the VINE_SHAMBLER `player.hp` parity drift.
    #[test]
    fn vine_shambler_ai_matches_v103_decomp() {
        let ai = create_vine_shambler_ai();
        assert_eq!(ai.current_move().id, "SWIPE", "initial state is SWIPE");

        let MonsterState::Move(swipe) = ai.states.get("SWIPE").expect("SWIPE exists") else {
            panic!("SWIPE is a MoveState")
        };
        assert!(matches!(
            swipe.intents[0],
            Intent::MultiAttack { damage: 6, hits: 2 }
        ));
        assert_eq!(swipe.follow_up, Some("GRASPING_VINES"));

        let MonsterState::Move(grasp) = ai
            .states
            .get("GRASPING_VINES")
            .expect("GRASPING_VINES exists")
        else {
            panic!("GRASPING_VINES is a MoveState")
        };
        assert!(matches!(grasp.intents[0], Intent::Attack { damage: 8 }));
        assert!(matches!(
            grasp.intents[1],
            Intent::Debuff {
                kind: PowerKind::Tangled,
                amount: 1
            }
        ));
        assert_eq!(grasp.follow_up, Some("CHOMP"));

        let MonsterState::Move(chomp) = ai.states.get("CHOMP").expect("CHOMP exists") else {
            panic!("CHOMP is a MoveState")
        };
        assert!(matches!(chomp.intents[0], Intent::Attack { damage: 16 }));
        assert_eq!(chomp.follow_up, Some("SWIPE"));

        // Fabricated moveset gone.
        assert!(!ai.states.contains_key("VINE_WHIP"));
        assert!(!ai.states.contains_key("TANGLE"));
        assert!(!ai.states.contains_key("RAND"));
    }

    /// Bead `sts2-rs-r8x8h`: VantomBoss spawns only Vantom (no
    /// Parafrights). Verified by walking the AI cycle and confirming
    /// no effect body spawns enemies.
    #[test]
    fn vantom_dismember_effect_adds_three_wounds_no_spawn() {
        use sts2_core::cards::CardId;
        use sts2_core::CombatState;
        let mut state = CombatState::default();
        let creatures_pre = state.creatures.len();
        let discard_pre = state.player.discard.len();
        vantom_dismember_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(
            state.creatures.len(),
            creatures_pre,
            "Vantom Dismember does NOT spawn enemies"
        );
        assert_eq!(
            state.player.discard.len() - discard_pre,
            3,
            "3 Wounds added to discard"
        );
        for &uid in state.player.discard.iter().rev().take(3) {
            let card = state
                .cards
                .iter()
                .find(|c| c.uid == uid)
                .expect("card exists");
            assert_eq!(card.card_id, CardId::Wound);
        }
    }

    #[test]
    fn bygone_effigy_initial_sleep_then_wake_then_slash() {
        let mut ai = create_bygone_effigy_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["INITIAL_SLEEP", "WAKE", "SLASHES"]
        );
    }

    #[test]
    fn ceremonial_beast_stamp_then_plow_self_loops() {
        // Bead sts2-rs-msi1: in the no-combat AI path (no owner uid set)
        // the OwnerAware PLOW_CHECK branch resolves to `false` and the
        // always-true fallthrough fires — PLOW self-loops, matching the
        // dominant Python case until external power-decrement kicks in.
        let mut ai = create_ceremonial_beast_ai();
        assert_eq!(run_three_moves(&mut ai), vec!["STAMP", "PLOW", "PLOW"]);
        for seed in 0..10 {
            ai.on_move_performed();
            assert_eq!(
                ai.roll_move_seeded(seed).id,
                "PLOW",
                "PLOW should self-loop in the no-combat path (seed {seed})"
            );
        }
    }

    /// Bead sts2-rs-vn0m: Fogmog ILLUSION's `effect_fn` spawns 1
    /// EyeWithTeeth. Per `sts2-rs-lc58p` (commit 7d7e6268) the freshly-
    /// spawned creature now carries a fresh `create_eye_with_teeth_ai()`
    /// so subsequent end-of-turn rolls produce the canonical
    /// EyeWithTeeth move state machine rather than falling back to
    /// `MonsterKind::roll_intent`. Bead `sts2-rs-4zxc0`: corrected
    /// the stale `is_none()` assertion (this test predated lc58p).
    #[test]
    fn fogmog_illusion_effect_spawns_eye_with_teeth() {
        use sts2_core::CombatState;

        let mut state = CombatState::default();
        let pre = state.creatures.len();
        fogmog_illusion_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(state.creatures.len(), pre + 1);
        let new_eye = state.creatures.last().expect("spawned");
        assert_eq!(new_eye.species, Some(MonsterKind::EyeWithTeeth));
        assert_eq!(new_eye.current_hp, 6);
        assert!(
            new_eye.ai.is_some(),
            "lc58p attached EyeWithTeeth AI to Fogmog ILLUSION spawn"
        );
    }

    /// Bead `sts2-rs-o1mwv` (C.1): the spawned EyeWithTeeth must sort
    /// AHEAD of Fogmog in enemy order. C# `FogmogNormal.Slots =
    /// ["illusion", "fogmog"]` and `SortEnemiesBySlotName` orders
    /// enemies by slot index; the illusion (slot "illusion", index 0)
    /// precedes Fogmog (slot "fogmog", index 1). Rust models this by
    /// inserting the summon at the front of `enemy_uids`. Without it,
    /// every FOGMOG parity trial diverges on monsters[0]/[1]
    /// kind/hp/max_hp/powers.len from the index swap.
    #[test]
    fn fogmog_illusion_inserts_eye_before_fogmog() {
        use sts2_core::CombatState;

        let mut state = CombatState::default();
        let fogmog = state.spawn_enemy(MonsterKind::Fogmog);
        // The slot rides on the creature's `MonsterAi`, so it needs one.
        state.set_creature_ai(fogmog, create_fogmog_ai());
        // Stamp the encounter's slots the way combat entry does:
        // `FogmogNormal.Slots = ["illusion", "fogmog"]` and `GenerateMonsters`
        // puts the Fogmog in "fogmog" (index 1), leaving "illusion" (0) for
        // the summon. Without the stamp the slot model has nothing to sort
        // against and degrades to append (GH #552).
        state.assign_encounter_slots(&[1]);
        fogmog_illusion_effect(&mut state, fogmog);

        assert_eq!(state.enemy_uids.len(), 2, "Fogmog + spawned Eye");
        let kind_of = |uid: sts2_core::CreatureUid| {
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .and_then(|c| c.species)
        };
        assert_eq!(
            kind_of(state.enemy_uids[0]),
            Some(MonsterKind::EyeWithTeeth),
            "illusion slot precedes fogmog slot — Eye must be enemy_uids[0]",
        );
        assert_eq!(kind_of(state.enemy_uids[1]), Some(MonsterKind::Fogmog));
    }

    /// Bead `sts2-rs-h9u`: `InfestedPower.AfterDeath` spawns 4 Wrigglers
    /// (the PhrogParasite elite's second phase). Each carries its real
    /// `create_wriggler_ai()` machine — which starts at `SPAWNED_MOVE`
    /// (Stun), modelling C#'s `StartStunned = true` — rather than the
    /// static `MonsterKind::roll_intent` fallback.
    #[test]
    fn infested_after_death_spawn_adds_four_stunned_wrigglers() {
        use sts2_core::CombatState;

        let mut state = CombatState::default();
        let phrog = state.spawn_enemy(MonsterKind::PhrogParasite);
        let pre = state.creatures.len();

        infested_after_death_spawn(&mut state, phrog);

        let wrigglers: Vec<_> = state
            .creatures
            .iter()
            .filter(|c| c.species == Some(MonsterKind::Wriggler))
            .collect();
        assert_eq!(wrigglers.len(), 4, "four Wrigglers spawn on phrog death");
        assert_eq!(state.creatures.len(), pre + 4);
        for w in &wrigglers {
            assert!(
                w.ai.is_some(),
                "each Wriggler gets its real AI, not the static fallback"
            );
            assert!(
                (17..=21).contains(&w.max_hp),
                "Wriggler HP in C# 17-21 range, got {}",
                w.max_hp
            );
        }
    }

    /// Regression: the InfestedPower-death wrigglers are spawned mid-player-
    /// turn (the killing blow on the PhrogParasite). C#
    /// `CombatManager.AfterCreatureAdded` rolls their initial move (telegraphing
    /// `SPAWNED_MOVE`'s Stun) immediately because `CurrentSide == Player`, so
    /// they perform the stun on the very next enemy turn and advance to
    /// NASTY_BITE/WRIGGLE the turn after. Without the on-spawn telegraph the
    /// wriggler's `intent` stayed `None`, so `enemy_turn_exec`'s `acting_enemies`
    /// snapshot (which filters on `intent.is_some()`) excluded it for one enemy
    /// turn — trailing C# by a full move-state step (under-damaged the player,
    /// desynced the discard/reshuffle pile size → a `shuffle` RNG-counter
    /// divergence on parity-fuzz seeds 1199/1374/...).
    #[test]
    fn infested_death_wrigglers_telegraph_initial_intent_on_spawn() {
        use sts2_core::monsters::Intent;
        use sts2_core::CombatState;

        let mut state = CombatState::default();
        let phrog = state.spawn_enemy(MonsterKind::PhrogParasite);

        infested_after_death_spawn(&mut state, phrog);

        let wrigglers: Vec<_> = state
            .creatures
            .iter()
            .filter(|c| c.species == Some(MonsterKind::Wriggler))
            .collect();
        assert_eq!(wrigglers.len(), 4, "four Wrigglers spawn");
        for (i, w) in wrigglers.iter().enumerate() {
            // SPAWNED_MOVE telegraphs a Stun intent; it must be set on the
            // freshly-spawned creature so the next enemy turn includes it.
            assert_eq!(
                w.intent,
                Some(Intent::Stun),
                "slot {} must telegraph its SPAWNED_MOVE (Stun) intent on spawn \
                 so it acts on the next enemy turn (C# AfterCreatureAdded)",
                i + 1,
            );
        }
    }

    /// Bead `sts2-rs-2u9w1`: the 4 InfestedPower-death Wrigglers must use the
    /// SLOT-AWARE AI (slots wriggler1..wriggler4), not the slot-less
    /// `act2_hive::create_wriggler_ai()`. Per C#
    /// `Wriggler.cs::GenerateMoveStateMachine` + `InfestedPower.cs` (which sets
    /// `StartStunned = true`), each spawns stunned on turn 1 (SPAWNED_MOVE),
    /// then its FollowUpState is the slot-conditional branch:
    /// wriggler1/wriggler3 → NASTY_BITE, wriggler2/wriggler4 → WRIGGLE.
    /// The slot-less ctor always followed up with NASTY_BITE regardless of
    /// slot — the MONSTER_STATE / PLAYER_HP parity divergence.
    #[test]
    fn infested_death_wrigglers_use_slot_aware_followups() {
        use sts2_core::CombatState;

        let mut state = CombatState::default();
        let phrog = state.spawn_enemy(MonsterKind::PhrogParasite);

        infested_after_death_spawn(&mut state, phrog);

        // Spawn order maps to slots wriggler1..wriggler4. After the
        // StartStunned SPAWNED_MOVE (Stun) turn, the per-slot follow-up is
        // NASTY_BITE for slots 1/3 and WRIGGLE for slots 2/4.
        let expected = ["NASTY_BITE", "WRIGGLE", "NASTY_BITE", "WRIGGLE"];
        let wriggler_uids: Vec<_> = state
            .creatures
            .iter()
            .filter(|c| c.species == Some(MonsterKind::Wriggler))
            .map(|c| c.id)
            .collect();
        assert_eq!(wriggler_uids.len(), 4, "four Wrigglers spawn");

        for (i, uid) in wriggler_uids.iter().enumerate() {
            let ai = state
                .creatures
                .iter_mut()
                .find(|c| c.id == *uid)
                .and_then(|c| c.ai.as_mut())
                .expect("spawned Wriggler has an AI");
            // Turn 1: stunned (StartStunned = true).
            assert_eq!(
                ai.current_move().id,
                "SPAWNED_MOVE",
                "slot {} starts stunned (StartStunned=true)",
                i + 1
            );
            // Advance past the stun turn and roll the slot-conditional follow-up.
            ai.on_move_performed();
            assert_eq!(
                ai.roll_move_seeded(0).id,
                expected[i],
                "slot {} post-stun follow-up should be {}",
                i + 1,
                expected[i]
            );
        }
    }

    // Bead `sts2-rs-r8x8h`: removed `vantom_consume_effect_spawns_two_parafrights`
    // — the underlying behavior was fabricated. VantomBoss in C# decomp
    // (v0.103.2) spawns ONLY Vantom; no Parafright summon path exists.
    // Replaced with `vantom_dismember_effect_adds_three_wounds_no_spawn`
    // above. Parafright the monster is still spawned by TheObscura /
    // IllusionPower elsewhere; only the Vantom-summon path was bogus.

    /// Helper: a CeremonialBeast (uid 1) carrying PLOW(`plow`) and
    /// optional Strength(`strength`) at `hp`/252 HP, plus a combat to
    /// drive its AI. Bead `sts2-rs-plow-stun`.
    #[cfg(test)]
    fn beast_combat(
        hp: i32,
        plow: i32,
        strength: i32,
    ) -> (sts2_core::CombatState, sts2_core::CreatureUid) {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{
            CombatState, Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, Power,
            PowerUid,
        };

        let mut combat = CombatState::default();
        let owner = NonZeroU32::new(1).expect("nz");
        let mut beast_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        let mut next_pid = 2u32;
        let mut push_power = |combat: &mut CombatState,
                              beast_powers: &mut SmallVec<[PowerUid; 8]>,
                              kind: PowerKind,
                              amount: i32| {
            let pid = NonZeroU32::new(next_pid).expect("nz");
            next_pid += 1;
            combat.powers.push(Power {
                id: pid,
                kind,
                amount,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            beast_powers.push(PowerUid(pid));
        };
        push_power(&mut combat, &mut beast_powers, PowerKind::Plow, plow);
        if strength != 0 {
            push_power(
                &mut combat,
                &mut beast_powers,
                PowerKind::Strength,
                strength,
            );
        }
        combat.creatures.push(Creature {
            id: owner,
            current_hp: hp,
            max_hp: 252,
            block: 0,
            powers: beast_powers,
            species: Some(MonsterKind::CeremonialBeast),
            monster_id: MonsterId("CeremonialBeast"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        (combat, CreatureUid(owner))
    }

    /// Bead `sts2-rs-plow-stun`: PLOW_CHECK transitions to STUN once the
    /// beast's HP has dropped to `<= its PLOW amount` (the C# trigger),
    /// and stays on PLOW while HP is above the threshold.
    #[test]
    fn ceremonial_beast_stuns_when_hp_reaches_plow_threshold() {
        // HP above threshold (200 > 150) → keeps plowing.
        let (above, owner) = beast_combat(200, 150, 0);
        let mut ai = create_ceremonial_beast_ai();
        ai.set_owner(owner);
        assert_eq!(ai.current_move().id, "STAMP");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_with_combat_seeded(0, &above).id, "PLOW");
        ai.on_move_performed();
        assert_eq!(
            ai.roll_move_with_combat_seeded(0, &above).id,
            "PLOW",
            "HP above PLOW threshold should keep self-looping PLOW"
        );

        // HP at/under threshold (100 <= 150) → enters STUN phase.
        let (below, owner2) = beast_combat(100, 150, 0);
        let mut ai2 = create_ceremonial_beast_ai();
        ai2.set_owner(owner2);
        ai2.on_move_performed(); // STAMP
        assert_eq!(ai2.roll_move_with_combat_seeded(0, &below).id, "PLOW");
        ai2.on_move_performed();
        assert_eq!(
            ai2.roll_move_with_combat_seeded(0, &below).id,
            "STUN",
            "HP at/under the PLOW threshold should route PLOW_CHECK to STUN"
        );
    }

    /// Bead `sts2-rs-plow-stun` (Strength-retention fix): the STUN move's
    /// effect removes only PLOW — it does NOT strip Strength. C#
    /// `StunnedMove` (CeremonialBeast.cs:191-196) is a pure no-op for powers;
    /// the Strength + Plow strip is owned by `PlowPower.AfterDamageReceived`
    /// (`plow_after_damage`) at the threshold-crossing damage event. A
    /// Strength stack present when the STUN move performs (re-applied after
    /// the AfterDamageReceived strip) must SURVIVE — C# retains it, and the
    /// old "strip Strength here too" double-strip dropped it (parity seed
    /// 5374: `monsters[0].powers.len` cs=1 vs rs=0).
    #[test]
    fn ceremonial_beast_stun_effect_removes_plow_but_keeps_strength() {
        let (mut combat, owner) = beast_combat(100, 150, 1);
        // Pre-condition: beast carries both PLOW and a (post-stun-reapplied)
        // Strength stack.
        let has = |c: &sts2_core::CombatState, k: PowerKind| {
            c.creatures[0]
                .powers
                .iter()
                .any(|pu| c.powers.iter().any(|p| p.id == pu.0 && p.kind == k))
        };
        assert!(has(&combat, PowerKind::Plow) && has(&combat, PowerKind::Strength));

        ceremonial_beast_stun_effect(&mut combat, owner);

        assert!(
            has(&combat, PowerKind::Strength),
            "Strength must be RETAINED by the STUN move (C# StunnedMove is a power no-op)"
        );
        assert!(!has(&combat, PowerKind::Plow), "PLOW removed on stun");
        // The threshold predicate now returns false (no PLOW) so the beast
        // does not re-enter STUN from its phase-2 cycle.
        assert!(!ceremonial_beast_hp_at_plow_threshold(&combat, owner));
    }

    /// Bead `sts2-rs-plow-stun` (Strength-retention fix): the on-damage
    /// `plow_after_damage` hook is the sole owner of the Strength + Plow
    /// strip, mirroring C# `PlowPower.AfterDamageReceived`. It must wipe BOTH
    /// the instant HP crosses the PlowAmount threshold.
    #[test]
    fn plow_after_damage_strips_strength_and_plow_at_threshold() {
        let (mut combat, owner) = beast_combat(149, 150, 6);
        let has = |c: &sts2_core::CombatState, k: PowerKind| {
            c.creatures[0]
                .powers
                .iter()
                .any(|pu| c.powers.iter().any(|p| p.id == pu.0 && p.kind == k))
        };
        assert!(has(&combat, PowerKind::Plow) && has(&combat, PowerKind::Strength));

        let res = plow_after_damage(
            &mut combat,
            owner,
            sts2_core::hooks::CreatureRef::Player,
            sts2_core::hooks::ValueProps::MOVE,
            1,
        );
        assert!(matches!(res, sts2_core::EffectResult::Resolved));
        assert!(
            !has(&combat, PowerKind::Strength),
            "AfterDamageReceived strips Strength at the threshold"
        );
        assert!(
            !has(&combat, PowerKind::Plow),
            "AfterDamageReceived strips Plow at the threshold"
        );
    }

    /// C# `PlowPower.AfterDamageReceived` (`PlowPower.cs:35-39`) removes
    /// EVERY `TemporaryStrengthPower` instance (`GetPowerInstances
    /// <TemporaryStrengthPower>()` — an `is`-typed walk over all 13
    /// subclasses) before the plain Strength removal. A surviving instance
    /// (e.g. a player-parked PiercingWail −Strength stack) would refund
    /// `-Sign * Amount` Strength at the beast's turn end
    /// (`TemporaryStrengthPower.cs:167-176`) — Strength C# deleted with the
    /// stun. Pre-fix Rust stripped only `Strength | Plow`.
    #[test]
    fn plow_after_damage_strips_the_temporary_strength_family_too() {
        use std::num::NonZeroU32;
        let (mut combat, owner) = beast_combat(149, 150, 6);
        // Park a PiercingWail(2) (a TemporaryStrengthPower subclass) and a
        // Weak(1) (NOT in the family — must survive) on the beast.
        for (pid_raw, kind, amount) in [
            (900_u32, PowerKind::PiercingWail, 2),
            (901_u32, PowerKind::Weak, 1),
        ] {
            let pid = NonZeroU32::new(pid_raw).expect("nz");
            combat.powers.push(sts2_core::Power {
                id: pid,
                kind,
                amount,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            combat.creatures[0].powers.push(sts2_core::PowerUid(pid));
        }
        let has = |c: &sts2_core::CombatState, k: PowerKind| {
            c.creatures[0]
                .powers
                .iter()
                .any(|pu| c.powers.iter().any(|p| p.id == pu.0 && p.kind == k))
        };
        let res = plow_after_damage(
            &mut combat,
            owner,
            sts2_core::hooks::CreatureRef::Player,
            sts2_core::hooks::ValueProps::MOVE,
            1,
        );
        assert!(matches!(res, sts2_core::EffectResult::Resolved));
        assert!(
            !has(&combat, PowerKind::PiercingWail),
            "the TemporaryStrengthPower-family stack is stripped with the stun \
             (no turn-end Strength refund may survive)"
        );
        assert!(!has(&combat, PowerKind::Strength) && !has(&combat, PowerKind::Plow));
        assert!(
            has(&combat, PowerKind::Weak),
            "non-family debuffs (Weak) survive the stun strip, as in C#"
        );
    }

    /// C# `PlowPower.AfterDamageReceived` gate (`PlowPower.cs:30`):
    /// `result.UnblockedDamage <= 0` → return. A fully-blocked hit must not
    /// stun the beast even when its HP already sits at/below the threshold.
    #[test]
    fn plow_after_damage_ignores_fully_blocked_hits() {
        let (mut combat, owner) = beast_combat(149, 150, 6);
        let has = |c: &sts2_core::CombatState, k: PowerKind| {
            c.creatures[0]
                .powers
                .iter()
                .any(|pu| c.powers.iter().any(|p| p.id == pu.0 && p.kind == k))
        };
        let res = plow_after_damage(
            &mut combat,
            owner,
            sts2_core::hooks::CreatureRef::Player,
            sts2_core::hooks::ValueProps::MOVE,
            0,
        );
        assert!(
            matches!(res, sts2_core::EffectResult::NotImplemented),
            "a hit with UnblockedDamage <= 0 must not trigger the Plow stun"
        );
        assert!(
            has(&combat, PowerKind::Plow) && has(&combat, PowerKind::Strength),
            "Plow and Strength survive a fully-blocked hit"
        );
    }

    /// Bead sts2-rs-wk1sa: Fogmog's SWIPE move effect_fn applies +1
    /// Strength to itself (C# `Fogmog.SwipeMove` → `PowerCmd.Apply<
    /// StrengthPower>(base.Creature, 1m, ...)`). The trailing
    /// `Intent::Buff{Strength,1}` in the move is telegraph-only; the
    /// real application lives in `fogmog_swipe_effect`, so a missing
    /// effect_fn (the pre-fix `mv` ctor) silently dropped the buff.
    #[test]
    fn fogmog_swipe_effect_applies_strength_one_to_self() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{
            CombatState, Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, PowerUid,
        };

        let mut combat = CombatState::default();
        let owner_raw = NonZeroU32::new(1).expect("nz");
        combat.creatures.push(Creature {
            id: owner_raw,
            current_hp: 74,
            max_hp: 74,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: Some(MonsterKind::Fogmog),
            monster_id: MonsterId("Fogmog"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        combat.enemy_uids.push(CreatureUid(owner_raw));

        fogmog_swipe_effect(&mut combat, CreatureUid(owner_raw));

        let creature = combat
            .creatures
            .iter()
            .find(|c| c.id == owner_raw)
            .expect("fogmog present");
        let strength: i32 = creature
            .powers
            .iter()
            .filter_map(|puid| {
                combat
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::Strength)
            })
            .map(|p| p.amount)
            .sum();
        assert_eq!(strength, 1, "SWIPE must apply Strength 1 to Fogmog");
    }

    #[test]
    fn mawler_roar_effect_applies_vulnerable_three_to_player() {
        use sts2_core::{CombatState, CreatureUid};

        let mut state = CombatState::default();
        // Effect targets the player via the legacy surface; owner uid unused.
        mawler_roar_effect(&mut state, CreatureUid::new(1).expect("nz"));

        let vuln: i32 = state
            .player
            .powers
            .iter()
            .filter_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::Vulnerable)
                    .map(|p| p.amount)
            })
            .sum();
        assert_eq!(
            vuln, 3,
            "ROAR must apply Vulnerable 3 to the player (C# Mawler.RoarMove)"
        );
    }

    #[test]
    fn bygone_effigy_wake_effect_applies_strength_ten_to_self() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{
            CombatState, Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, PowerUid,
        };

        let mut combat = CombatState::default();
        let owner_raw = NonZeroU32::new(1).expect("nz");
        combat.creatures.push(Creature {
            id: owner_raw,
            current_hp: 84,
            max_hp: 84,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: Some(MonsterKind::BygoneEffigy),
            monster_id: MonsterId("BygoneEffigy"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        combat.enemy_uids.push(CreatureUid(owner_raw));

        bygone_effigy_wake_effect(&mut combat, CreatureUid(owner_raw));

        let creature = combat
            .creatures
            .iter()
            .find(|c| c.id == owner_raw)
            .expect("effigy present");
        let strength: i32 = creature
            .powers
            .iter()
            .filter_map(|puid| {
                combat
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::Strength)
            })
            .map(|p| p.amount)
            .sum();
        assert_eq!(
            strength, 10,
            "WAKE must apply Strength 10 to the effigy (C# WakeMove)"
        );
    }

    // ── Bead sts2-rs-pdsdl: telegraph-only Buff/Debuff power application ──

    /// Helper: sum the amount of a self-power `kind` on the creature `uid`.
    fn self_power_amount(combat: &CombatState, uid: u32, kind: PowerKind) -> i32 {
        let creature = combat
            .creatures
            .iter()
            .find(|c| c.id.get() == uid)
            .expect("creature present");
        creature
            .powers
            .iter()
            .filter_map(|puid| {
                combat
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == kind)
            })
            .map(|p| p.amount)
            .sum()
    }

    /// Helper: sum the amount of a player power `kind` on the legacy surface.
    fn player_power_amount(state: &CombatState, kind: PowerKind) -> i32 {
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

    /// Helper: build a `CombatState` with a single enemy creature at uid 1.
    fn combat_with_enemy(kind: MonsterKind) -> (CombatState, CreatureUid) {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, IntentHistory, MonsterId, PowerUid};

        let mut combat = CombatState::default();
        let owner_raw = NonZeroU32::new(1).expect("nz");
        combat.creatures.push(Creature {
            id: owner_raw,
            current_hp: 100,
            max_hp: 100,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: Some(kind),
            monster_id: MonsterId("test"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        combat.enemy_uids.push(CreatureUid(owner_raw));
        (combat, CreatureUid(owner_raw))
    }

    #[test]
    fn cubex_construct_charge_up_effect_applies_strength_two_to_self() {
        let (mut combat, owner) = combat_with_enemy(MonsterKind::CubexConstruct);
        cubex_construct_charge_up_effect(&mut combat, owner);
        assert_eq!(self_power_amount(&combat, 1, PowerKind::Strength), 2);
    }

    #[test]
    fn cubex_construct_repeater_effect_applies_strength_two_to_self() {
        let (mut combat, owner) = combat_with_enemy(MonsterKind::CubexConstruct);
        cubex_construct_repeater_effect(&mut combat, owner);
        assert_eq!(self_power_amount(&combat, 1, PowerKind::Strength), 2);
    }

    /// Bead `sts2-rs-owm9q`: AxeRubyRaider SWING's secondary
    /// `Intent::Defend{5}` is dropped by `perform_intent`, so the
    /// effect_fn grants the 5 block (C# `SwingBlock`=5).
    #[test]
    fn axe_ruby_raider_swing_gains_five_block() {
        let (mut combat, owner) = combat_with_enemy(MonsterKind::AxeRubyRaider);
        let block_of = |c: &CombatState| {
            c.creatures
                .iter()
                .find(|cr| cr.id == owner.0)
                .map_or(0, |cr| cr.block)
        };
        assert_eq!(block_of(&combat), 0, "no block before SWING");
        axe_ruby_raider_swing_effect(&mut combat, owner);
        assert_eq!(block_of(&combat), 5, "SWING grants 5 block to self");
    }

    #[test]
    fn brute_ruby_raider_roar_effect_applies_strength_three_to_self() {
        let (mut combat, owner) = combat_with_enemy(MonsterKind::BruteRubyRaider);
        brute_ruby_raider_roar_effect(&mut combat, owner);
        assert_eq!(self_power_amount(&combat, 1, PowerKind::Strength), 3);
    }

    #[test]
    fn tracker_ruby_raider_track_effect_applies_frail_two_to_player() {
        let mut state = CombatState::default();
        tracker_ruby_raider_track_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(
            player_power_amount(&state, PowerKind::Frail),
            2,
            "TRACK must apply Frail 2 to the player (C# TrackerRubyRaider.TrackMove)"
        );
    }

    #[test]
    fn ceremonial_beast_stamp_effect_applies_plow_to_self() {
        let (mut combat, owner) = combat_with_enemy(MonsterKind::CeremonialBeast);
        ceremonial_beast_stamp_effect(&mut combat, owner);
        assert_eq!(
            self_power_amount(&combat, 1, PowerKind::Plow),
            150,
            "STAMP must apply Plow 150 to self (C# CeremonialBeast.StampMove)"
        );
    }

    #[test]
    fn ceremonial_beast_plow_effect_applies_strength_two_to_self() {
        let (mut combat, owner) = combat_with_enemy(MonsterKind::CeremonialBeast);
        ceremonial_beast_plow_effect(&mut combat, owner);
        assert_eq!(self_power_amount(&combat, 1, PowerKind::Strength), 2);
    }

    #[test]
    fn ceremonial_beast_beast_cry_effect_applies_ringing_to_player() {
        let mut state = CombatState::default();
        ceremonial_beast_beast_cry_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(
            player_power_amount(&state, PowerKind::Ringing),
            1,
            "BEAST_CRY must apply Ringing 1 to the player (C# CeremonialBeast.BeastCryMove)"
        );
    }

    #[test]
    fn ceremonial_beast_crush_effect_applies_strength_three_to_self() {
        let (mut combat, owner) = combat_with_enemy(MonsterKind::CeremonialBeast);
        ceremonial_beast_crush_effect(&mut combat, owner);
        assert_eq!(self_power_amount(&combat, 1, PowerKind::Strength), 3);
    }

    #[test]
    fn kin_priest_orb_of_frailty_effect_applies_frail_one_to_player() {
        let mut state = CombatState::default();
        kin_priest_orb_of_frailty_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power_amount(&state, PowerKind::Frail), 1);
    }

    #[test]
    fn kin_priest_orb_of_weakness_effect_applies_weak_one_to_player() {
        let mut state = CombatState::default();
        kin_priest_orb_of_weakness_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power_amount(&state, PowerKind::Weak), 1);
    }

    #[test]
    fn kin_priest_ritual_effect_applies_strength_two_to_self() {
        let (mut combat, owner) = combat_with_enemy(MonsterKind::KinPriest);
        kin_priest_ritual_effect(&mut combat, owner);
        assert_eq!(self_power_amount(&combat, 1, PowerKind::Strength), 2);
    }

    // ── Mutation-coverage tests (cargo-mutants survivors) ──

    /// Bead `sts2-rs-r8x8h`: Vantom PREPARE applies +2 Strength to self.
    /// Kills `replace vantom_prepare_effect with ()`.
    #[test]
    fn vantom_prepare_effect_applies_strength_two_to_self() {
        let (mut combat, owner) = combat_with_enemy(MonsterKind::Vantom);
        vantom_prepare_effect(&mut combat, owner);
        assert_eq!(self_power_amount(&combat, 1, PowerKind::Strength), 2);
    }

    /// Bead `sts2-rs-o1mwv` (C.3): SnappingJaxfruit ENERGY_ORB applies +2
    /// Strength to self after the attack.
    /// Kills `replace snapping_jaxfruit_energy_orb_effect with ()`.
    #[test]
    fn snapping_jaxfruit_energy_orb_effect_applies_strength_two_to_self() {
        let (mut combat, owner) = combat_with_enemy(MonsterKind::SnappingJaxfruit);
        snapping_jaxfruit_energy_orb_effect(&mut combat, owner);
        assert_eq!(self_power_amount(&combat, 1, PowerKind::Strength), 2);
    }

    /// Bead `sts2-rs-p8n60` (o1mwv C.5): SlitheringStrangler CONSTRICT applies
    /// ConstrictPower(3) to the player AND rebinds the Constrict applier to the
    /// acting strangler (latest-applier-wins, for `ConstrictPower.AfterDeath`).
    /// Kills `replace slithering_strangler_constrict_effect with ()` plus the
    /// applier-rebind find predicate mutants (216: `==`→`!=`, `&&`→`||`).
    #[test]
    fn slithering_strangler_constrict_effect_applies_constrict_and_rebinds_applier() {
        let (mut state, owner) = combat_with_enemy(MonsterKind::SlitheringStrangler);

        slithering_strangler_constrict_effect(&mut state, owner);

        // 3 Constrict applied to the player (kills `with ()`).
        assert_eq!(player_power_amount(&state, PowerKind::Constrict), 3);

        // The Constrict power's applier is rebound to this strangler. The find
        // predicate `p.id == puid.0 && p.kind == Constrict` must match exactly
        // the player's Constrict power; mutating `==`→`!=` or `&&`→`||` picks
        // the wrong (or no) power, leaving the applier unset.
        let constrict_applier = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Constrict)
            .and_then(|p| p.applier);
        assert_eq!(
            constrict_applier,
            Some(owner),
            "Constrict applier must be rebound to the acting strangler",
        );
    }

    /// Distinct-power guard for the applier-rebind predicate: a non-Constrict
    /// player power must NOT have its applier rebound. Pins `&&`→`||` and the
    /// `kind == Constrict` half of the find predicate (216).
    #[test]
    fn slithering_strangler_constrict_effect_leaves_other_powers_applier_alone() {
        use std::num::NonZeroU32;
        let (mut state, owner) = combat_with_enemy(MonsterKind::SlitheringStrangler);

        // Pre-seed the player with a Weak power (applier = None) on the legacy
        // surface. After CONSTRICT, only Constrict's applier should change.
        let weak_id = NonZeroU32::new(900).expect("nz");
        state.powers.push(sts2_core::Power {
            id: weak_id,
            kind: PowerKind::Weak,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(sts2_core::PowerUid(weak_id));

        slithering_strangler_constrict_effect(&mut state, owner);

        let weak_applier = state
            .powers
            .iter()
            .find(|p| p.id == weak_id)
            .and_then(|p| p.applier);
        assert_eq!(
            weak_applier, None,
            "non-Constrict powers must keep their original (None) applier",
        );
        let constrict_applier = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Constrict)
            .and_then(|p| p.applier);
        assert_eq!(constrict_applier, Some(owner));
    }

    /// Bead `sts2-rs-p8n60` (o1mwv C.5): SlitheringStrangler TWACK gains 5 block
    /// after its attack. Kills `replace slithering_strangler_twack_effect with
    /// ()` and the `c.id == owner.0` find predicate (235:63 `==`→`!=`).
    #[test]
    fn slithering_strangler_twack_effect_gains_five_block() {
        let (mut state, owner) = combat_with_enemy(MonsterKind::SlitheringStrangler);
        let block_of = |c: &CombatState| {
            c.creatures
                .iter()
                .find(|cr| cr.id == owner.0)
                .map_or(-1, |cr| cr.block)
        };
        assert_eq!(block_of(&state), 0, "no block before TWACK");
        slithering_strangler_twack_effect(&mut state, owner);
        assert_eq!(
            block_of(&state),
            5,
            "TWACK grants exactly 5 block to the acting strangler",
        );
    }

    /// The gain goes through the shared `CreatureCmd.GainBlock` funnel, so it
    /// shows up in the combat log. It used to be a raw `c.block +=`, which
    /// left this move invisible in the log panel (and skipped the modify chain
    /// and both `after_block_gained` buses).
    #[test]
    fn slithering_strangler_twack_block_is_recorded_in_the_combat_log() {
        let (mut state, owner) = combat_with_enemy(MonsterKind::SlitheringStrangler);
        state.combat_log.event_log.set_recording(true);
        slithering_strangler_twack_effect(&mut state, owner);

        let rows = state.combat_log.event_log.rows();
        let gained = rows
            .iter()
            .find_map(|r| match r.entry {
                sts2_core::event_log::EventLogEntry::BlockGained { receiver, amount } => {
                    Some((receiver, amount))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("TWACK's block must be logged: {rows:?}"));
        assert_eq!(gained.1, 5);
        assert!(
            matches!(
                gained.0,
                sts2_core::event_log::LogCreature::Monster { uid, .. } if uid == owner.0.get()
            ),
            "the row names the strangler that gained it: {:?}",
            gained.0
        );
    }

    /// Bead `sts2-rs-fad0f`: Flyconid FRAIL_SPORES applies Frail(2) to the
    /// player. Kills `replace flyconid_frail_spores_effect with ()`.
    #[test]
    fn flyconid_frail_spores_effect_applies_frail_two_to_player() {
        let mut state = CombatState::default();
        flyconid_frail_spores_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power_amount(&state, PowerKind::Frail), 2);
    }

    /// Bead `sts2-rs-fad0f`: Flyconid VULNERABLE_SPORES applies Vulnerable(2)
    /// to the player. Kills `replace flyconid_vulnerable_spores_effect with ()`.
    #[test]
    fn flyconid_vulnerable_spores_effect_applies_vulnerable_two_to_player() {
        let mut state = CombatState::default();
        flyconid_vulnerable_spores_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power_amount(&state, PowerKind::Vulnerable), 2);
    }

    /// Bead `sts2-rs-g0gs9`: VineShambler GRASPING_VINES applies Tangled(1) to
    /// the player. Kills `replace vine_shambler_grasping_vines_effect with ()`.
    #[test]
    fn vine_shambler_grasping_vines_effect_applies_tangled_one_to_player() {
        let mut state = CombatState::default();
        vine_shambler_grasping_vines_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(player_power_amount(&state, PowerKind::Tangled), 1);
    }

    /// GRASPING_VINES' Tangled Entangles the player's Attacks as it lands
    /// (`TangledPower.AfterApplied`, `TangledPower.cs:23-30`), not their type.
    #[test]
    fn vine_shambler_grasping_vines_entangles_the_players_attacks() {
        use sts2_core::card_afflictions::CardAfflictionKind;
        use sts2_core::cards::{Card, CardType};
        let mut state = CombatState::default();
        for (i, ct) in [CardType::Attack, CardType::Skill].into_iter().enumerate() {
            let uid = sts2_core::CardUid::new(u32::try_from(i + 1).unwrap()).unwrap();
            state.cards.push(Card {
                uid,
                card_type: ct,
                ..Card::pod_defaults()
            });
            state.player.hand.push(uid);
        }
        vine_shambler_grasping_vines_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(
            state.cards[0].affliction.map(|a| a.kind),
            Some(CardAfflictionKind::Entangled)
        );
        assert_eq!(state.cards[1].affliction, None);
    }

    /// `apply_power_to_player_via_legacy_surface` stacks a repeated application
    /// of the same kind into the EXISTING power instance (rather than creating
    /// a second one). The stack-merge find predicate `p.id == puid.0 && p.kind
    /// == kind` (475) selects that existing instance; mutating `==`→`!=` or
    /// `&&`→`||` mis-selects, so the second apply spawns a duplicate power
    /// instead of summing. Asserting a single instance with the summed amount
    /// kills all three 475 mutants.
    #[test]
    fn apply_power_to_player_via_legacy_surface_stacks_into_existing_instance() {
        let mut state = CombatState::default();
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Frail, 2);
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Frail, 3);

        let frail_instances = state
            .powers
            .iter()
            .filter(|p| p.kind == PowerKind::Frail)
            .count();
        assert_eq!(
            frail_instances, 1,
            "repeated apply must stack into the single existing Frail instance",
        );
        assert_eq!(
            player_power_amount(&state, PowerKind::Frail),
            5,
            "stacked amount must be the sum (2 + 3)",
        );
        assert_eq!(
            state.player.powers.len(),
            1,
            "no duplicate PowerUid stacked onto the legacy surface",
        );
    }

    /// `ceremonial_beast_hp_at_plow_threshold` finds the beast's PLOW amount via
    /// `p.id == puid.0 && p.kind == Plow` (1398). Mutating `&&`→`||` makes the
    /// predicate match the FIRST power on the creature regardless of kind, so a
    /// non-Plow leading power would supply a bogus amount. Seed Strength FIRST
    /// (lower id) then Plow: with `&&` the real Plow(150) amount governs; with
    /// `||` the Strength(1) amount governs and the threshold flips.
    #[test]
    fn ceremonial_beast_hp_threshold_uses_plow_power_not_first_power() {
        use std::num::NonZeroU32;
        let mut combat = CombatState::default();
        let owner = NonZeroU32::new(1).expect("nz");

        // Strength(1) gets the lower id so it sorts first in the creature's
        // power list; Plow(150) is the real threshold.
        let str_id = NonZeroU32::new(2).expect("nz");
        let plow_id = NonZeroU32::new(3).expect("nz");
        combat.powers.push(sts2_core::Power {
            id: str_id,
            kind: PowerKind::Strength,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        combat.powers.push(sts2_core::Power {
            id: plow_id,
            kind: PowerKind::Plow,
            amount: 150,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        combat.creatures.push(sts2_core::Creature {
            id: owner,
            current_hp: 100,
            max_hp: 252,
            block: 0,
            powers: smallvec::smallvec![sts2_core::PowerUid(str_id), sts2_core::PowerUid(plow_id)],
            species: Some(MonsterKind::CeremonialBeast),
            monster_id: sts2_core::MonsterId("CeremonialBeast"),
            intent: None,
            intent_history: sts2_core::IntentHistory::default(),
            ai: None,
        });

        // HP 100: <= Plow(150) → true (at threshold), but > Strength(1) → false.
        // The `&&` predicate must select Plow(150).
        assert!(
            ceremonial_beast_hp_at_plow_threshold(&combat, CreatureUid(owner)),
            "HP 100 <= Plow 150 → at threshold; predicate must read the Plow amount",
        );
    }

    /// `plow_after_damage` must NOT strip when HP is strictly ABOVE the PLOW
    /// threshold. Kills `> `→`==` (1441): at hp=200 > plow=150, original returns
    /// NotImplemented (no strip); the `==` mutant (200 == 150 = false) would
    /// fall through and strip.
    #[test]
    fn plow_after_damage_does_not_strip_above_threshold() {
        let (mut combat, owner) = beast_combat(200, 150, 6);
        let has = |c: &CombatState, k: PowerKind| {
            c.creatures[0]
                .powers
                .iter()
                .any(|pu| c.powers.iter().any(|p| p.id == pu.0 && p.kind == k))
        };
        let res = plow_after_damage(
            &mut combat,
            owner,
            sts2_core::hooks::CreatureRef::Player,
            sts2_core::hooks::ValueProps::MOVE,
            1,
        );
        assert!(
            matches!(res, sts2_core::EffectResult::NotImplemented),
            "above-threshold HP must short-circuit (no strip)",
        );
        assert!(
            has(&combat, PowerKind::Plow),
            "Plow retained above threshold"
        );
        assert!(
            has(&combat, PowerKind::Strength),
            "Strength retained above threshold",
        );
    }

    /// `plow_after_damage` MUST strip exactly AT the threshold (hp == plow).
    /// Kills `> `→`>=` (1441): original `150 > 150` is false → strips; the `>=`
    /// mutant `150 >= 150` is true → returns NotImplemented and skips the strip.
    #[test]
    fn plow_after_damage_strips_exactly_at_threshold() {
        let (mut combat, owner) = beast_combat(150, 150, 6);
        let has = |c: &CombatState, k: PowerKind| {
            c.creatures[0]
                .powers
                .iter()
                .any(|pu| c.powers.iter().any(|p| p.id == pu.0 && p.kind == k))
        };
        let res = plow_after_damage(
            &mut combat,
            owner,
            sts2_core::hooks::CreatureRef::Player,
            sts2_core::hooks::ValueProps::MOVE,
            1,
        );
        assert!(matches!(res, sts2_core::EffectResult::Resolved));
        assert!(
            !has(&combat, PowerKind::Plow),
            "HP exactly at threshold must strip Plow",
        );
        assert!(
            !has(&combat, PowerKind::Strength),
            "HP exactly at threshold must strip Strength",
        );
    }

    /// `plow_after_damage` returns `NotImplemented` (no strip) when the creature
    /// carries NEITHER Strength nor Plow strip-targets — i.e. the
    /// `pids.is_empty()` guard (1462) fires. This also pins the `!pids.contains`
    /// retains (1465/1467, `delete !`) and the strip-filter predicate
    /// (1454 `==`→`!=`, 1455 `&&`→`||`): with no Plow present the function bails
    /// at the early `plow_amount` lookup, but with Plow present and HP at
    /// threshold the filter selects Strength+Plow and the negated retains drop
    /// them. The threshold test above already drives the positive path; here we
    /// assert the retains remove ONLY the strip-targets and keep an unrelated
    /// power (Slow), pinning the `delete !` mutants (which would instead KEEP
    /// the strip-targets and DROP everything else).
    #[test]
    fn plow_after_damage_retains_only_strip_targets() {
        use std::num::NonZeroU32;
        let (mut combat, owner) = beast_combat(100, 150, 6);

        // Add an unrelated Slow power that must SURVIVE the strip.
        let slow_id = NonZeroU32::new(500).expect("nz");
        combat.powers.push(sts2_core::Power {
            id: slow_id,
            kind: PowerKind::Slow,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        combat.creatures[0]
            .powers
            .push(sts2_core::PowerUid(slow_id));

        let res = plow_after_damage(
            &mut combat,
            owner,
            sts2_core::hooks::CreatureRef::Player,
            sts2_core::hooks::ValueProps::MOVE,
            1,
        );
        assert!(matches!(res, sts2_core::EffectResult::Resolved));

        let has = |c: &CombatState, k: PowerKind| {
            c.creatures[0]
                .powers
                .iter()
                .any(|pu| c.powers.iter().any(|p| p.id == pu.0 && p.kind == k))
        };
        // `delete !` on the retains would invert these: keep Plow/Strength and
        // drop Slow.
        assert!(!has(&combat, PowerKind::Plow), "Plow stripped");
        assert!(!has(&combat, PowerKind::Strength), "Strength stripped");
        assert!(
            has(&combat, PowerKind::Slow),
            "unrelated Slow power must survive the strip (pins the `!` retains)",
        );
        // The Slow power object itself is retained in the power pool.
        assert!(
            combat.powers.iter().any(|p| p.id == slow_id),
            "Slow power object retained in the pool",
        );
    }

    /// `install_plow_handlers` registers `plow_after_damage` under
    /// `PowerKind::Plow` on the global after-damage-received registry. Because
    /// the registry is a process-global that other tests may have populated,
    /// this test first overwrites the Plow slot with a distinct SENTINEL
    /// handler, then calls `install_plow_handlers()` and asserts the slot now
    /// holds `plow_after_damage`. Mutating the installer to `()` leaves the
    /// sentinel in place, flipping the fn-pointer assertion.
    #[test]
    fn install_plow_handlers_registers_plow_after_damage() {
        use sts2_core::monster_power_effects::{
            lookup_after_damage_received_for_power, register_after_damage_received_for_power,
        };

        type Handler = fn(
            &mut CombatState,
            CreatureUid,
            sts2_core::hooks::CreatureRef,
            sts2_core::hooks::ValueProps,
            i32,
        ) -> sts2_core::EffectResult;

        // A sentinel handler distinct from `plow_after_damage`.
        fn sentinel(
            _: &mut CombatState,
            _: CreatureUid,
            _: sts2_core::hooks::CreatureRef,
            _: sts2_core::hooks::ValueProps,
            _: i32,
        ) -> sts2_core::EffectResult {
            sts2_core::EffectResult::NotImplemented
        }

        register_after_damage_received_for_power(PowerKind::Plow, sentinel);
        // Precondition: the slot holds the sentinel, NOT plow_after_damage.
        let before =
            lookup_after_damage_received_for_power(PowerKind::Plow).expect("sentinel registered");
        assert_eq!(
            before as Handler as usize, sentinel as Handler as usize,
            "sentinel must occupy the Plow slot before install",
        );

        install_plow_handlers();

        let after = lookup_after_damage_received_for_power(PowerKind::Plow)
            .expect("install registers a Plow handler");
        assert_eq!(
            after as Handler as usize, plow_after_damage as Handler as usize,
            "install_plow_handlers must register plow_after_damage (overwriting the sentinel)",
        );
        assert_ne!(
            after as Handler as usize, sentinel as Handler as usize,
            "install must replace the sentinel",
        );
    }

    /// `plow_after_damage`'s PLOW-amount lookup find predicate
    /// `p.id == puid.0 && p.kind == Plow` (1435) must read the amount of the
    /// creature's OWN Plow power keyed by id. With ONLY a Plow power present and
    /// HP at threshold, the original strips; mutating `==`→`!=` makes the
    /// id-keyed lookup miss the sole Plow (no OTHER power shares the kind), so
    /// the function bails with NotImplemented and never strips.
    #[test]
    fn plow_after_damage_lookup_keys_on_plow_power_id() {
        // Plow-only beast (strength = 0): the sole power is Plow.
        let (mut combat, owner) = beast_combat(100, 150, 0);
        let has = |c: &CombatState, k: PowerKind| {
            c.creatures[0]
                .powers
                .iter()
                .any(|pu| c.powers.iter().any(|p| p.id == pu.0 && p.kind == k))
        };
        assert!(has(&combat, PowerKind::Plow));
        let res = plow_after_damage(
            &mut combat,
            owner,
            sts2_core::hooks::CreatureRef::Player,
            sts2_core::hooks::ValueProps::MOVE,
            1,
        );
        assert!(
            matches!(res, sts2_core::EffectResult::Resolved),
            "id-keyed Plow lookup must find the sole Plow and strip at threshold",
        );
        assert!(!has(&combat, PowerKind::Plow), "Plow stripped at threshold");
    }

    /// `plow_after_damage`'s PLOW-amount lookup must read the PLOW amount, not
    /// whichever power sorts first. Seed Strength(amount 1, lower id) BEFORE
    /// Plow(amount 150): with `&&` the id-keyed lookup yields the real Plow(150)
    /// so HP 100 <= 150 strips; mutating `&&`→`||` makes the lookup match the
    /// first power by id alone (Strength, amount 1), so HP 100 > 1 short-circuits
    /// and never strips.
    #[test]
    fn plow_after_damage_amount_lookup_reads_plow_not_first_power() {
        use std::num::NonZeroU32;
        let mut combat = CombatState::default();
        let owner = NonZeroU32::new(1).expect("nz");
        let str_id = NonZeroU32::new(2).expect("nz");
        let plow_id = NonZeroU32::new(3).expect("nz");
        combat.powers.push(sts2_core::Power {
            id: str_id,
            kind: PowerKind::Strength,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        combat.powers.push(sts2_core::Power {
            id: plow_id,
            kind: PowerKind::Plow,
            amount: 150,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        combat.creatures.push(sts2_core::Creature {
            id: owner,
            current_hp: 100,
            max_hp: 252,
            block: 0,
            powers: smallvec::smallvec![sts2_core::PowerUid(str_id), sts2_core::PowerUid(plow_id)],
            species: Some(MonsterKind::CeremonialBeast),
            monster_id: sts2_core::MonsterId("CeremonialBeast"),
            intent: None,
            intent_history: sts2_core::IntentHistory::default(),
            ai: None,
        });

        let res = plow_after_damage(
            &mut combat,
            CreatureUid(owner),
            sts2_core::hooks::CreatureRef::Player,
            sts2_core::hooks::ValueProps::MOVE,
            1,
        );
        assert!(
            matches!(res, sts2_core::EffectResult::Resolved),
            "lookup must read Plow(150): HP 100 <= 150 → strip",
        );
        let has = |c: &CombatState, k: PowerKind| {
            c.creatures[0]
                .powers
                .iter()
                .any(|pu| c.powers.iter().any(|p| p.id == pu.0 && p.kind == k))
        };
        assert!(!has(&combat, PowerKind::Plow), "Plow stripped");
        assert!(!has(&combat, PowerKind::Strength), "Strength stripped");
    }

    /// `ceremonial_beast_stun_effect`'s strip filter selects PLOW powers via
    /// `p.id == puid.0 && p.kind == Plow` (1537). Mutating `==`→`!=` makes the
    /// id-match select the WRONG powers (every power whose id differs from the
    /// puid), so the real Plow is left in place and an unrelated power is
    /// dropped. Seed Plow + a surviving Strength and assert ONLY Plow is removed.
    #[test]
    fn ceremonial_beast_stun_effect_strips_only_plow_by_id_match() {
        let (mut combat, owner) = beast_combat(100, 150, 2);
        let has = |c: &CombatState, k: PowerKind| {
            c.creatures[0]
                .powers
                .iter()
                .any(|pu| c.powers.iter().any(|p| p.id == pu.0 && p.kind == k))
        };
        assert!(has(&combat, PowerKind::Plow) && has(&combat, PowerKind::Strength));

        // Capture the Plow PowerUid currently on the beast's uid list so we can
        // assert it is pruned from the creature's `powers` vec (not just the
        // global power pool). The second `find(|c| c.id == owner.0)` (1537)
        // re-finds the beast to prune its uid list; mutating `==`→`!=` selects
        // the wrong creature (or none), leaving the stale Plow uid on the beast.
        let plow_uid = combat.creatures[0]
            .powers
            .iter()
            .copied()
            .find(|pu| {
                combat
                    .powers
                    .iter()
                    .any(|p| p.id == pu.0 && p.kind == PowerKind::Plow)
            })
            .expect("beast carries a Plow uid");

        ceremonial_beast_stun_effect(&mut combat, owner);

        assert!(!has(&combat, PowerKind::Plow), "Plow removed on stun");
        assert!(
            has(&combat, PowerKind::Strength),
            "Strength (different id) must NOT be removed — pins the `==` id match",
        );
        assert!(
            !combat.creatures[0].powers.contains(&plow_uid),
            "the beast's own power-uid list must be pruned of Plow \
             (pins the second `c.id == owner.0` find, 1537)",
        );
    }
}
