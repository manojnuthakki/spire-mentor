//! Uncommon-rarity relic dispatch — bead `sts2-rs-l22` (P7-3).
//!
//! Ported from `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py`
//! (40 relics). This file registers a Phase-7 on-trigger handler
//! for every uncommon-rarity [`RelicId`] variant. Handler bodies
//! are intentionally minimal markers (`EffectResult::Resolved`)
//! that satisfy the dispatch-table acceptance criterion: every
//! variant resolves through the registry instead of falling
//! through to `NotImplemented`. Concrete gameplay plumbing
//! (vigor application, vulnerable application, attack counters,
//! orb channelling, gold/HP gains, etc.) lands in downstream
//! Phase-7 plumbing beads once the corresponding `CombatState`
//! surface lands (player powers, combat round number, orb queue,
//! gold, run state, room context).
//!
//! ### Stateful relics
//!
//! Several uncommon relics carry per-card-trigger counter state
//! in Python (`Pen Nib`, `Letter Opener`, `Ornamental Fan`,
//! `Kusarigama`, `Nunchaku`, `Tuning Fork`, `Joss Paper`,
//! `Galactic Dust`, `Bowler Hat`, `Ripple Basin`, `Vambrace`).
//! Their per-instance counters live on the runtime `Relic` POD
//! (`sts2_core::Relic.counter` etc., not yet wired) — modelled in
//! this bead as comments on each handler to lock the Python pin
//! for the downstream plumbing bead.

use sts2_core::card_effects::{
    apply_power_to_creature, deal_damage_to_all_alive_enemies, EffectResult,
};
use sts2_core::hooks::{self, BlockCtx, BlockGainedCtx, CreatureRef, ValueProps};
use sts2_core::powers::PowerKind;
use sts2_core::relic_effects::{
    register_after_card_played, register_after_player_turn_start, register_after_side_turn_start,
    register_before_combat_start, register_before_turn_end, register_modify_damage_additive,
    register_modify_damage_multiplicative, register_owned_after_card_entered_combat,
    register_owned_after_potion_used, register_owned_before_hand_draw,
    register_owned_modify_vulnerable_multiplier, AfterCardEnteredCombatFn, AfterCardPlayedFn,
    AfterPlayerTurnStartFn, AfterPotionUsedFn, AfterSideTurnStartFn, BeforeCombatStartFn,
    BeforeHandDrawFn, BeforeTurnEndFn, CardEventCtx, ModifyDamageAdditiveFn, ModifyDamageCtx,
    ModifyDamageMultiplicativeFn, ModifyVulnerableMultiplierFn,
};
use sts2_core::relics::RelicId;
use sts2_core::run_effects::{
    register_after_obtained, register_after_room_entered, register_on_card_added_to_deck,
    register_on_gold_gained, register_should_gain_gold, AfterObtainedRunFn, AfterRoomEnteredRunFn,
    CardAddedRunCtx, OnCardAddedToDeckRunFn, OnGoldGainedRunFn, ShouldGainGoldRunFn,
};
use sts2_core::runstate::{RoomType, RunState};
use sts2_core::{CardUid, CombatState, RelicKind, Side};

use crate::relics::{RelicEffect, RelicRarity, RelicSpec};

// ---------------------------------------------------------------------------
// Pool installer
// ---------------------------------------------------------------------------

/// All 40 uncommon-rarity relic ids from
/// `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py`, in Python
/// `@register_relic` declaration order. Membership/identity data only —
/// live behaviour flows through the typed hook-channel listeners
/// registered by [`install_pool`], not through any dispatch marker.
const UNCOMMON_RELIC_IDS: &[RelicId] = &[
    RelicId::Akabeko,
    RelicId::BagOfMarbles,
    RelicId::Bellows,
    RelicId::BookRepairKnife,
    RelicId::BowlerHat,
    RelicId::Candelabra,
    RelicId::EternalFeather,
    RelicId::FuneraryMask,
    RelicId::GalacticDust,
    RelicId::GoldPlatedCables,
    RelicId::GremlinHorn,
    RelicId::HornCleat,
    RelicId::JossPaper,
    RelicId::Kusarigama,
    RelicId::LetterOpener,
    RelicId::LuckyFysh,
    RelicId::MercuryHourglass,
    RelicId::MiniatureCannon,
    RelicId::Nunchaku,
    RelicId::Orichalcum,
    RelicId::OrnamentalFan,
    RelicId::Pantograph,
    RelicId::PaperPhrog,
    RelicId::ParryingShield,
    RelicId::Pear,
    RelicId::PenNib,
    RelicId::PetrifiedToad,
    RelicId::Planisphere,
    RelicId::RedMask,
    RelicId::Regalite,
    RelicId::ReptileTrinket,
    RelicId::RippleBasin,
    RelicId::SelfFormingClay,
    RelicId::SparklingRouge,
    RelicId::StoneCracker,
    RelicId::SymbioticVirus,
    RelicId::Tingsha,
    RelicId::TuningFork,
    RelicId::TwistedFunnel,
    RelicId::Vambrace,
];

/// Install every uncommon-relic handler. Called from
/// [`crate::relics::dispatch::install_default_registry`]. Idempotent
/// (re-registering overwrites the same function pointer).
/// Every [`RelicId`] registered in this module's uncommon pool — the
/// droppable relics of this tier. Used to seed the run's relic grab
/// bag so combat/treasure/shop relic rewards are stocked (the run
/// loop's `populate_relic_grab_bag_by_rarity` source). Bead `sts2-rs-64mq2`.
#[must_use]
pub fn pool_relic_ids() -> Vec<RelicId> {
    UNCOMMON_RELIC_IDS.to_vec()
}

pub fn install_pool() {
    // GremlinHorn is implemented ONCE, in sts2-core
    // (`relic_effects::gremlin_horn`), on the C#-faithful relic `after_death`
    // bus (bead `sts2-rs-rmrzx`). The earlier content-side `after_damage_given`
    // port (bead `sts2-rs-c22x`, Python pin, `current_hp<=0` probe) was a
    // SECOND live registration on a different channel, so GremlinHorn fired
    // TWICE on every enemy death (double energy + double draw — the long-
    // standing `relic:GREMLIN_HORN` coverage-sweep divergence). The duplicate
    // has been removed; do NOT re-add a content-side GremlinHorn listener.
    // Bead `sts2-rs-gpsvo`.

    // Bead `sts2-rs-cr4y8`: PenNib doubles the 10th attack's damage.
    // C# `PenNib.ModifyDamageMultiplicative` returns `2m` — a TRUE
    // multiplicative on the FULLY-computed attack damage (base +
    // Strength + other additive bonuses). The prior port (`sts2-rs-w46z`)
    // routed it through `modify_damage_additive` returning `+base_damage`,
    // which doubled only the card's BASE and dropped Strength/flat
    // bonuses (base 10 + Str 3: C# `(10+3)*2 = 26` vs additive
    // `10+3+10 = 23`). The relic-side `modify_damage_multiplicative`
    // channel now exists (`sts2-rs-mcsra`), so PenNib composes a `×2`
    // alongside Vulnerable etc. and floors once — byte-faithful to C#.
    //
    // 1. `modify_damage_multiplicative` returns `×2` when the damage is
    //    a powered attack dealt by the player AND `(relic.counter + 1)
    //    % THRESHOLD == 0` (the attacks-played count anticipates the
    //    post-card-played increment, so the active window is
    //    `counter == 9`).
    // 2. `after_card_played` increments `relic.counter` when the just-
    //    played card was an Attack.
    register_modify_damage_multiplicative(
        RelicId::PenNib,
        listener_pen_nib_modify_damage as ModifyDamageMultiplicativeFn,
    );
    register_after_card_played(
        RelicId::PenNib,
        listener_pen_nib_after_card_played as AfterCardPlayedFn,
    );
    // Bead `sts2-rs-u8qa`: real `after_obtained` listener for Pear's
    // `MAX_HP = 10` (Python `uncommon.py::Pear.MAX_HP`).
    register_after_obtained(RelicId::Pear, listener_pear as AfterObtainedRunFn);
    // Bead `sts2-rs-b31s6`: BowlerHat — 25% bonus on every positive
    // gold gain (`sts2-rs-oim25` corrected the Python-pin's 20% to
    // C# `GoldIncrease = 1.25m`). Two-listener wire:
    // - should_gain_gold latches the integer-floored bonus into
    //   rs.relic_counters[BowlerHat] (never cancels the gain).
    // - on_gold_gained reads the latch and re-calls gain_gold(bonus),
    //   guarded by rs.bowler_hat_applying_bonus so other gold
    //   listeners (DragonFruit, ...) observe the bonus once while
    //   BowlerHat itself doesn't compound geometrically.
    register_should_gain_gold(
        RelicId::BowlerHat,
        listener_bowler_hat_should_gain_gold as ShouldGainGoldRunFn,
    );
    register_on_gold_gained(
        RelicId::BowlerHat,
        listener_bowler_hat_on_gold_gained as OnGoldGainedRunFn,
    );
    // Bead `sts2-rs-onyo`: real `after_room_entered` listener for
    // Pantograph's boss-room heal (Python `uncommon.py::Pantograph`).
    register_after_room_entered(
        RelicId::Pantograph,
        listener_pantograph as AfterRoomEnteredRunFn,
    );
    // Bead `sts2-rs-3wb0`: real `after_room_entered` listener for
    // Planisphere's unknown-room heal (`HEAL = 4`). Python
    // `uncommon.py::Planisphere`: on unknown-room entry, if alive,
    // heal 4. Same channel + alive-gate as Pantograph (onyo), with
    // `RoomType::Unknown` as the room-type predicate.
    register_after_room_entered(
        RelicId::Planisphere,
        listener_planisphere as AfterRoomEnteredRunFn,
    );
    // Bead `sts2-rs-9818`: real `on_card_added_to_deck` listener for
    // LuckyFysh's `GOLD = 15` per card added (Python
    // `uncommon.py::LuckyFysh.on_card_added_to_deck`). The combat-side
    // `on_trigger_lucky_fysh` stays a `resolved_marker`; this is the
    // run-state side that grants gold whenever any card hits the deck.
    register_on_card_added_to_deck(
        RelicId::LuckyFysh,
        listener_lucky_fysh as OnCardAddedToDeckRunFn,
    );
    // Bead `sts2-rs-g4ti` + `sts2-rs-axdgh`: MercuryHourglass (`DAMAGE = 3`).
    // C# fires this on `AfterPlayerTurnStart` (MercuryHourglass.cs), which runs
    // BEFORE the side-hook `AfterSideTurnStart` where an enemy LivingShield's
    // RampartPower arms the TurretOperator's block — so the 3 Unpowered damage
    // must land on bare HP, not the not-yet-granted shield. The Python pin
    // (`uncommon.py::MercuryHourglass.after_side_turn_start`) modeled it on the
    // side hook; the C# authority is the player hook. NB: this is registered on
    // the same global channel key as the sts2-core
    // `relic_effects::mercury_hourglass` handler — both target
    // `after_player_turn_start`, so the BTreeMap holds a single entry and the
    // relic fires exactly once.
    register_after_player_turn_start(
        RelicId::MercuryHourglass,
        listener_mercury_hourglass as AfterPlayerTurnStartFn,
    );
    // FuneraryMask. C# v0.107.1 `FuneraryMask.BeforeHandDraw` (TurnNumber == 1):
    // insert `CARDS = 3` Soul cards into the owner's draw pile at random
    // positions BEFORE the turn-1 hand is drawn — so a Soul can be drawn into
    // the opening hand. Registered on `before_hand_draw`, NOT
    // `after_side_turn_start` (the prior port, citing the OLD C#
    // `AfterSideTurnStart`): that hook fired AFTER the opening draw, so the
    // souls never reached the turn-1 hand and every later draw desynced
    // (DRAW_ORDER, 86/87 IC init divergences). Same move as ToastyMittens
    // (`sts2-rs-70anj.1`). Combat-side `on_trigger_funerary_mask` stays a
    // `resolved_marker`.
    register_owned_before_hand_draw(
        RelicId::FuneraryMask,
        RelicKind::FuneraryMask,
        listener_funerary_mask as BeforeHandDrawFn,
    );
    // Bead `sts2-rs-uo8t`: real listener for Bellows. C# `Bellows.cs`
    // overrides `AfterPlayerTurnStart` (NOT `AfterSideTurnStart` — the Python
    // pin `uncommon.py::Bellows` mis-named the hook): on round-1 player turn
    // start, call `CardCmd.Upgrade` for every card in hand. The Rust port
    // calls `card_effects::upgrade_card_uid` per uid. The hook channel matters:
    // `AfterPlayerTurnStart` fires inside `SetupPlayerTurn` — AFTER the opening
    // hand draw but BEFORE the outer-loop `AfterSideTurnStart`
    // (CombatManager.cs:429 SetupPlayerTurn precedes :437 AfterSideTurnStart).
    // So Bellows upgrades the DRAWN hand and never sees a SovereignBlade that
    // FencingManual forges at `AfterSideTurnStart` (sweep seed 287: a forged
    // SovereignBlade cost 2 in C#, was upgraded to 1 here). Combat-side
    // `on_trigger_bellows` stays a `resolved_marker`.
    register_after_player_turn_start(RelicId::Bellows, listener_bellows as AfterPlayerTurnStartFn);
    // Bead `sts2-rs-lr0y`: real `modify_damage_additive` listener for
    // MiniatureCannon (`EXTRA_DAMAGE = 3`). Python
    // `uncommon.py::MiniatureCannon.modify_damage_additive`: when the
    // dealer is the owner, the card is an upgraded Attack, and the
    // damage carries MOVE without UNPOWERED, contribute +3 damage.
    register_modify_damage_additive(
        RelicId::MiniatureCannon,
        listener_miniature_cannon as ModifyDamageAdditiveFn,
    );
    // Bead `sts2-rs-70anj.2` (Wave 2): real `after_side_turn_start`
    // listener for SymbioticVirus. Python
    // `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::SymbioticVirus`:
    // on round-1 player-side turn start, channel `DARK_ORBS = 1` Dark
    // orb. Same player-side + round-gate pattern as FuneraryMask
    // (`sts2-rs-hnd8`) / MercuryHourglass (`sts2-rs-g4ti`).
    // Combat-side `on_trigger_symbiotic_virus` stays a
    // `resolved_marker`.
    register_after_side_turn_start(
        RelicId::SymbioticVirus,
        listener_symbiotic_virus as AfterSideTurnStartFn,
    );
    // Bead `sts2-rs-u2jxd` (70anj.5.regalite): real
    // `after_card_entered_combat` listener for Regalite. Python pin
    // `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::Regalite`:
    // when a colorless card enters combat (mid-fight spawn), gain
    // REGALITE_BLOCK = 2. The channel landed at sts2-rs-x2rql and
    // fires post-spawn from `spawn_card_into_hand`; ownership is
    // implicit (only player spawns route through that path at the
    // current pin). Combat-side `on_trigger_regalite` stays a
    // `resolved_marker`.
    register_owned_after_card_entered_combat(
        RelicId::Regalite,
        RelicKind::Regalite,
        listener_regalite as AfterCardEnteredCombatFn,
    );
    // EternalFeather — rest-site heal `HEAL_PER * (deck // CARDS_PER)`
    // (Python `uncommon.py::EternalFeather`). Combat-side
    // `on_trigger_eternal_feather` stays a `resolved_marker`.
    register_after_room_entered(
        RelicId::EternalFeather,
        listener_eternal_feather as AfterRoomEnteredRunFn,
    );
    // StoneCracker — at the start of EVERY combat room (normal /
    // elite / boss), upgrade up to `STONE_CRACKER_CARDS = 2` random
    // not-already-upgraded cards in the player's draw pile. C#
    // `StoneCracker.cs::AfterRoomEntered` fires on `room is CombatRoom`
    // (no boss gate). Bead `sts2-rs-3r6hc` (follow-up to `sts2-rs-48htt`)
    // dropped the `state.is_boss` gate inherited from the Python pin
    // (`sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::StoneCracker`),
    // which is a pin-side fabrication. Combat-side
    // `on_trigger_stone_cracker` stays a `resolved_marker`.
    register_before_combat_start(
        RelicId::StoneCracker,
        listener_stone_cracker as BeforeCombatStartFn,
    );
    // GoldPlatedCables — first orb gets +1 passive trigger. Python pin
    // `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::GoldPlatedCables`:
    // after_side_turn_start fires `trigger_first_passive` if the front
    // orb IS Plasma (so the +1 energy arm), before_turn_end fires it if
    // the front orb is NOT Plasma. Both gate on `side == owner.side`.
    // Combat-side `on_trigger_gold_plated_cables` stays a
    // `resolved_marker`. Built on the
    // `orbs::trigger_first_orb_passive` primitive (bead
    // `sts2-rs-rt2ry`).
    register_after_side_turn_start(
        RelicId::GoldPlatedCables,
        listener_gold_plated_cables_after_side_turn_start as AfterSideTurnStartFn,
    );
    register_before_turn_end(
        RelicId::GoldPlatedCables,
        listener_gold_plated_cables_before_turn_end as BeforeTurnEndFn,
    );
    // ReptileTrinket — after the player uses a potion mid-combat, apply
    // a `ReptileTrinketPower(REPTILE_TRINKET_STRENGTH = 3)` (a
    // TemporaryStrengthPower) to the player: +3 Strength now, reverted
    // -3 at the player's turn end. C# `ReptileTrinket.cs::AfterPotionUsed`
    // gates on `potion.Owner == Owner && CombatManager.IsInProgress`.
    // The owner-side turn-end revert is wired in
    // `sts2-core::hooks::after_turn_end`. Bead `sts2-rs-ji7m7`.
    register_owned_after_potion_used(
        RelicId::ReptileTrinket,
        RelicKind::ReptileTrinket,
        listener_reptile_trinket as AfterPotionUsedFn,
    );
    // PaperPhrog — +PAPER_PHROG_EXTRA_VULNERABLE_PCT (25) to the Vulnerable
    // damage multiplier on the player's powered attacks against non-self
    // targets. C# `PaperPhrog.cs::ModifyVulnerableMultiplier` is consulted
    // from `VulnerablePower.ModifyDamageMultiplicative`, so the bonus only
    // lands when the target is actually Vulnerable (1.5 → 1.75). Combat-side
    // `on_trigger_paper_phrog` stays a `resolved_marker`. Bead `sts2-rs-0om8p`.
    register_owned_modify_vulnerable_multiplier(
        RelicId::PaperPhrog,
        RelicKind::PaperPhrog,
        listener_paper_phrog_vulnerable_multiplier as ModifyVulnerableMultiplierFn,
    );
}

/// `PaperPhrog::modify_vulnerable_multiplier` — additive
/// `+PAPER_PHROG_EXTRA_VULNERABLE_PCT` percent to the Vulnerable damage
/// multiplier. Mirrors C# `PaperPhrog.cs::ModifyVulnerableMultiplier`:
/// returns the bonus only when the target is NOT the relic owner
/// (`target != base.Owner.Creature`) and the hit is a powered attack
/// (`props.IsPoweredAttack()`). The relic lives on the player, so the
/// dealer must be the player (the owner) for the bonus to apply — a
/// powered player attack against a Vulnerable enemy goes 1.5× → 1.75×.
/// Returns `0` (no contribution) when any gate fails. Bead `sts2-rs-0om8p`.
fn listener_paper_phrog_vulnerable_multiplier(state: &CombatState, ctx: &hooks::DamageCtx) -> i32 {
    if !state.relics.owns(RelicKind::PaperPhrog) {
        return 0;
    }
    if !ctx.props.is_powered() {
        return 0;
    }
    // C#: `if (target == base.Owner.Creature) return amount;` — no bonus
    // when the owner (the player) is the target.
    if matches!(ctx.target, CreatureRef::Player) {
        return 0;
    }
    // The relic is player-owned, so only the player's attacks benefit
    // (C# fetches `dealer.Player?.GetRelic<PaperPhrog>()`).
    if !matches!(ctx.dealer, CreatureRef::Player) {
        return 0;
    }
    PAPER_PHROG_EXTRA_VULNERABLE_PCT
}

/// `ReptileTrinket::after_potion_used` — fold +`REPTILE_TRINKET_STRENGTH`
/// Strength into the player's visible `StrengthPower` and register a
/// `ReptileTrinket` temporary-strength counter that reverts the same
/// amount at the player's turn end. Mirrors the FlexPotion drink-time
/// fold (`drink_flex_potion`); the revert is handled by the
/// `PowerKind::ReptileTrinket` arm in `hooks::after_turn_end`.
///
/// C# `ReptileTrinket.cs::AfterPotionUsed` gates on `potion.Owner ==
/// Owner` (player-owned potion) and `CombatManager.IsInProgress`. At
/// the current pin only the player drinks potions through this channel,
/// so the relic-ownership probe (`state.relics` contains the relic) is
/// the only gate needed. Bead `sts2-rs-ji7m7`.
fn listener_reptile_trinket(
    state: &mut CombatState,
    _potion: sts2_core::potions::PotionKind,
    _target: Option<u8>,
) -> EffectResult {
    if !state.relics.owns(RelicKind::ReptileTrinket) {
        return EffectResult::NotImplemented;
    }
    let Some(uid) = state.player.creature_uid else {
        return EffectResult::NotImplemented;
    };
    apply_power_to_creature(state, uid, PowerKind::Strength, REPTILE_TRINKET_STRENGTH);
    apply_power_to_creature(
        state,
        uid,
        PowerKind::ReptileTrinket,
        REPTILE_TRINKET_STRENGTH,
    );
    EffectResult::Resolved
}

/// `SymbioticVirus::after_side_turn_start` — on the round-1 player
/// side-turn-start, channel `SYMBIOTIC_VIRUS_DARK_ORBS = 1` Dark orb
/// onto the player. Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::SymbioticVirus`
/// (`DARK_ORBS = 1`, `pool = DEFECT`). Ownership-gated; the
/// player-side + round-1 gates match Python's `side == PLAYER and
/// combat.round_number == 1`. Channel-only firing — uses
/// [`sts2_core::orbs::channel_orb`] which auto-grows the orb belt to
/// the default Defect capacity when the relic ends up on a fixture
/// build that didn't pre-allocate slots (mirrors Python's
/// `OrbQueue.channel` behavior). Bead `sts2-rs-70anj.2` (Wave 2).
fn listener_symbiotic_virus(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if state.round != 1 {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::SymbioticVirus) {
        return EffectResult::NotImplemented;
    }
    for _ in 0..SYMBIOTIC_VIRUS_DARK_ORBS {
        sts2_core::orbs::channel_orb(state, sts2_core::orbs::Orb::dark());
    }
    EffectResult::Resolved
}

/// `MiniatureCannon::modify_damage_additive` — `+EXTRA_DAMAGE` when
/// the player deals MOVE-tagged powered damage from an upgraded
/// Attack card (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::MiniatureCannon`,
/// `EXTRA_DAMAGE = 3`). Reads `ctx.card` (4nh5 surface) and locates
/// the matching `Card` in `state.cards` to inspect `upgraded` /
/// `card_type`. Returns `0` (no contribution) when any gate fails.
/// Bead `sts2-rs-lr0y`.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn listener_miniature_cannon(state: &CombatState, ctx: &ModifyDamageCtx) -> i32 {
    if !state.relics.owns(RelicKind::MiniatureCannon) {
        return 0;
    }
    let Some(player_uid) = state.player.creature_uid else {
        return 0;
    };
    if ctx.dealer != Some(player_uid) {
        return 0;
    }
    if !ctx.props.contains(ValueProps::MOVE) || ctx.props.contains(ValueProps::UNPOWERED) {
        return 0;
    }
    // Bead `sts2-rs-nwz1n` (R3 lever 1): C# `MiniatureCannon` reads
    // `cardSource.IsUpgraded` off the SPECIFIC played instance. Keying on the
    // instance uid (not `ctx.card`'s id) fixes the duplicate-id case — with
    // e.g. 3 TWIN_STRIKE copies the old `find(card_id == …)` read the FIRST
    // copy's (base) upgrade flag and silently dropped the +3/hit (parity-fuzz
    // seed 9: C#=24 vs Rust=16). Falls back to no contribution when the uid is
    // absent (non-card damage / mismatched proximate source).
    let Some(card_uid) = ctx.card_uid else {
        return 0;
    };
    let idx = card_uid.0.get() as usize - 1;
    let is_upgraded_attack = state
        .cards
        .get(idx)
        .is_some_and(|c| c.upgraded && c.card_type == sts2_core::cards::CardType::Attack);
    if !is_upgraded_attack {
        return 0;
    }
    MINIATURE_CANNON_EXTRA_DAMAGE
}

/// `MercuryHourglass::AfterPlayerTurnStart` — on player turn start, deal
/// `MERCURY_HOURGLASS_DAMAGE` to every alive enemy (C#
/// `MercuryHourglass.cs`: `CreatureCmd.Damage(HittableEnemies, 3, Unpowered)`;
/// pin `uncommon.py::MercuryHourglass`). Fires every player turn while owned.
/// Beads `sts2-rs-g4ti` (original) / `sts2-rs-axdgh` (hook correction: moved
/// off `after_side_turn_start` so it precedes the Rampart block grant).
fn listener_mercury_hourglass(state: &mut CombatState) -> EffectResult {
    if !state.relics.owns(RelicKind::MercuryHourglass) {
        return EffectResult::NotImplemented;
    }
    let _ = deal_damage_to_all_alive_enemies(
        state,
        MERCURY_HOURGLASS_DAMAGE,
        ValueProps::UNPOWERED,
        None,
    );
    EffectResult::Resolved
}

/// `FuneraryMask::before_hand_draw` — on the turn-1 hand draw, insert
/// `FUNERARY_MASK_CARDS` (= 3) Soul cards into the draw pile at random
/// positions, BEFORE the opening hand is drawn. Mirrors C# v0.107.1
/// `FuneraryMask.BeforeHandDraw` (`FuneraryMask.cs`):
/// `if (TurnNumber == 1) { for i in 0..Cards: AddGeneratedCardToCombat(Soul,
/// Draw, .., CardPilePosition.Random) }`. Bead `sts2-rs-hnd8`.
///
/// Timing (bead `sts2-rs-70anj.1` sibling): the prior port registered this on
/// `after_side_turn_start`, which fires AFTER the opening draw — so the souls
/// landed only in the post-draw pile and never in the turn-1 hand, and the
/// shifted Shuffle-stream consumption desynced every later draw (DRAW_ORDER,
/// 86/87 IC init divergences). `before_hand_draw` fires before the draw,
/// matching C#.
///
/// Random-position note: each Soul is added via
/// `AddGeneratedCardToCombat(card, Draw, .., CardPilePosition.Random)`,
/// i.e. `AddInternal(card, Rng.Shuffle.NextInt(Cards.Count + 1))`
/// (`CardPileCmd.cs:433`). Each is inserted at a fresh random depth and
/// consumes one Shuffle-stream draw, with the pile growing between iterations
/// — exactly what `spawn_card_into_draw_pile_random_position` does
/// (`next_int(0, len)` == `NextInt(len + 1)`). Bead `sts2-rs-n4sa4`.
fn listener_funerary_mask(state: &mut CombatState) -> EffectResult {
    if state.round != 1 {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::FuneraryMask) {
        return EffectResult::NotImplemented;
    }
    let cards = usize::try_from(FUNERARY_MASK_CARDS).unwrap_or(0);
    for _ in 0..cards {
        let soul =
            crate::cards::status::card_from_spec(&crate::cards::status_specs_generated::soul());
        let _ = sts2_core::card_effects::spawn_card_into_draw_pile_random_position(state, soul);
    }
    EffectResult::Resolved
}

/// `Bellows::AfterPlayerTurnStart` — on the round-1 player turn start,
/// upgrade every card currently in the player's hand (C#
/// `Bellows.cs::AfterPlayerTurnStart` → `CardCmd.Upgrade(Hand.Cards)`).
/// Already-upgraded cards are no-ops via `Card::upgrade` idempotency.
/// Registered on `after_player_turn_start` (not `after_side_turn_start`)
/// so it fires before FencingManual's forge — see the registration note.
/// Bead `sts2-rs-uo8t`.
fn listener_bellows(state: &mut CombatState) -> EffectResult {
    if state.round != 1 {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::Bellows) {
        return EffectResult::NotImplemented;
    }
    let hand: Vec<sts2_core::CardUid> = state.player.hand.iter().copied().collect();
    for uid in hand {
        let _ = sts2_core::card_effects::upgrade_card_uid(state, uid);
    }
    EffectResult::Resolved
}

/// `Pear::after_obtained` — `owner.gain_max_hp(10)` (Python
/// `uncommon.py::Pear`, `MAX_HP=10`). Bead `sts2-rs-u8qa`.
fn listener_pear(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::Pear) {
        return EffectResult::NotImplemented;
    }
    rs.gain_max_hp(PEAR_MAX_HP);
    EffectResult::Resolved
}

/// `BowlerHat::should_gain_gold` — stash the pending bonus
/// (`amount * MULTIPLIER`, integer-floored). Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::BowlerHat`:
/// ```text
/// def should_gain_gold(self, owner, amount):
///     if self._is_applying_bonus:
///         return True
///     self._pending_bonus_gold = int(amount * self.MULTIPLIER)
///     return True
/// ```
///
/// The Rust port stashes the pending bonus on
/// `rs.relic_counters[BowlerHat]` (per-relic state-on-RunState
/// idiom, same as PumpkinCandle's act snapshot in `f0438`). During
/// bonus application, the re-entry guard
/// [`RunState::bowler_hat_applying_bonus`] is `true` and the
/// listener short-circuits — matching Python's
/// `_is_applying_bonus` early-return so the bonus doesn't compound
/// geometrically. Ownership-gated. Never cancels the gain. Bead
/// `sts2-rs-b31s6`.
fn listener_bowler_hat_should_gain_gold(
    rs: &mut RunState,
    amount: i32,
    _cancel: &mut bool,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::BowlerHat) {
        return EffectResult::NotImplemented;
    }
    if rs.bowler_hat_applying_bonus {
        return EffectResult::Resolved;
    }
    if amount <= 0 {
        return EffectResult::NotImplemented;
    }
    // Integer-floor `amount * MULTIPLIER` exactly as Python's
    // `int(amount * 0.2)`. Using `amount * 20 / 100` keeps the
    // arithmetic in i32 and avoids the f64 round-trip; for the
    // canonical 20% pin the two are identical for all positive
    // i32 amounts.
    let bonus = amount.saturating_mul(BOWLER_HAT_MULTIPLIER_PCT) / 100;
    rs.relic_counters.insert(RelicId::BowlerHat, bonus);
    EffectResult::Resolved
}

/// `BowlerHat::on_gold_gained` — apply the stashed bonus by
/// calling [`RunState::gain_gold`] again, guarded by
/// [`RunState::bowler_hat_applying_bonus`] so the recursive
/// re-fire of the same listener is a no-op. Python pin:
/// ```text
/// def on_gold_gained(self, owner, amount):
///     if self._is_applying_bonus or self._pending_bonus_gold <= 0:
///         return
///     bonus = self._pending_bonus_gold
///     self._pending_bonus_gold = 0
///     self._is_applying_bonus = True
///     try:
///         owner.gain_gold(bonus)
///     finally:
///         self._is_applying_bonus = False
/// ```
///
/// The recursive `gain_gold(bonus)` is intentional: other gold-
/// channel listeners (e.g. DragonFruit's `+1 Max HP per positive
/// gold gain`) MUST observe the bonus too. The re-entry guard
/// only suppresses BowlerHat's own re-fire, matching Python.
/// Ownership-gated. Bead `sts2-rs-b31s6`.
fn listener_bowler_hat_on_gold_gained(rs: &mut RunState, _amount: i32) -> EffectResult {
    if !rs.owns_active_relic(RelicId::BowlerHat) {
        return EffectResult::NotImplemented;
    }
    if rs.bowler_hat_applying_bonus {
        return EffectResult::NotImplemented;
    }
    let bonus = rs
        .relic_counters
        .get(&RelicId::BowlerHat)
        .copied()
        .unwrap_or(0);
    if bonus <= 0 {
        return EffectResult::NotImplemented;
    }
    rs.relic_counters.insert(RelicId::BowlerHat, 0);
    rs.bowler_hat_applying_bonus = true;
    rs.gain_gold(bonus);
    rs.bowler_hat_applying_bonus = false;
    EffectResult::Resolved
}

/// `Pantograph::after_room_entered` — `if room_type.is_boss and
/// owner.current_hp > 0: owner.heal(25)`. Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::Pantograph`
/// (`HEAL = 25`). The alive-gate is load-bearing — Python's
/// `is_boss and current_hp > 0` is a no-op once the player has
/// already died, which matters because the runstate dispatcher
/// also fires `after_room_entered` on `RoomType::Boss` revivals.
/// Bead `sts2-rs-onyo`.
fn listener_pantograph(rs: &mut RunState, room_type: RoomType) -> EffectResult {
    if !rs.owns_active_relic(RelicId::Pantograph) {
        return EffectResult::NotImplemented;
    }
    if !matches!(room_type, RoomType::Boss) {
        return EffectResult::NotImplemented;
    }
    if rs.current_hp <= 0 {
        return EffectResult::NotImplemented;
    }
    rs.heal(PANTOGRAPH_HEAL);
    EffectResult::Resolved
}

/// `Planisphere::after_room_entered` — heal `PLANISPHERE_HEAL`
/// (= 4) on entry to an unknown room, gated on `current_hp > 0`
/// (Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::Planisphere`).
///
/// Python has a two-branch predicate: the `room_type.is_unknown`
/// fast path + a fallback via `run_state.visited_map_coords[-1]
/// .point_type.name == "UNKNOWN"`. The Rust port resolves both
/// to `matches!(room_type, RoomType::Unknown)` because RunState
/// doesn't track `visited_map_coords` or the act_map yet — when
/// that surface lands, the fallback branch can be wired separately.
/// Bead `sts2-rs-3wb0`.
fn listener_planisphere(rs: &mut RunState, room_type: RoomType) -> EffectResult {
    if !rs.owns_active_relic(RelicId::Planisphere) {
        return EffectResult::NotImplemented;
    }
    if !matches!(room_type, RoomType::Unknown) {
        return EffectResult::NotImplemented;
    }
    if rs.current_hp <= 0 {
        return EffectResult::NotImplemented;
    }
    rs.heal(PLANISPHERE_HEAL);
    EffectResult::Resolved
}

/// `LuckyFysh::on_card_added_to_deck` — `owner.gain_gold(15)` on every
/// card added to the deck (Python `uncommon.py::LuckyFysh`, `GOLD=15`).
/// No card-type gate (contrast with DarkstonePeriapt which keys on
/// `CardType::Curse`). Ownership-gated. Bead `sts2-rs-9818`.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn listener_lucky_fysh(rs: &mut RunState, _ctx: &CardAddedRunCtx) -> EffectResult {
    if !rs.owns_active_relic(RelicId::LuckyFysh) {
        return EffectResult::NotImplemented;
    }
    rs.gain_gold(LUCKY_FYSH_GOLD);
    EffectResult::Resolved
}

// ---------------------------------------------------------------------------
// RelicSpec factories — A–K wave (bead `sts2-rs-6ynt.5.5`)
// ---------------------------------------------------------------------------
//
// Static templates for the alphabetical A–K half of the uncommon
// pool (14 entries). Bodies are deliberately [`RelicEffect::Noop`]:
// the dispatch handlers below already register through
// [`register_many`] as `Resolved` markers; this bead's contribution
// is the spec-side scaffold so a Phase-7 wiring sub-bead can swap
// `Noop` for the right AST variant (likely reusing the variants
// added by sibling `sts2-rs-6ynt.5.3`:
// `ApplyPowerAtCombatStart`, `GainBlockAtCombatStart`,
// `HealAtPlayerSideTurnStart`, etc.) without re-touching the file
// structure. Matches the `faf5a2d` `common.rs` precedent (30 `Noop`
// factories first, body wiring in follow-ups like `sts2-rs-owus`
// and `sts2-rs-6ynt.5.3`).
//
// Pool-out-of-scope (NECROBINDER / REGENT / DEFECT) relics are still
// scaffolded here because pool restrictions are a future run-builder
// concern; the [`RelicSpec`] data itself is universal across pools.
// `RelicKind` variants for the 12 new uncommons are added by the
// same bead in `sts2-core::relics`.
//
// Constants below pin the load-bearing Python class attributes from
// `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py` so the wiring
// sub-bead does not need to re-grep them.

/// `Akabeko.VIGOR` — round-1 vigor stack on first attack
/// (`uncommon.py:41`).
pub const AKABEKO_VIGOR: i32 = 8;
/// `BagOfMarbles.VULNERABLE` — round-1 vulnerable per enemy
/// (`uncommon.py:54`).
pub const BAG_OF_MARBLES_VULNERABLE: i32 = 1;
/// `BookRepairKnife.HEAL_PER` — HP healed per enemy that died to Doom
/// (`uncommon.py:85`).
pub const BOOK_REPAIR_KNIFE_HEAL_PER: i32 = 3;
/// `BowlerHat` gold-gain bonus — C# `BowlerHat.cs`
/// `DynamicVar("GoldIncrease", 1.25m)` with
/// `PendingBonusGold = Math.Floor(amount * (GoldIncrease - 1))` →
/// 25% bonus, not 20%. Python pin `uncommon.py:107`'s 0.2 multiplier
/// drifted from the shipped game; corrected per bead `sts2-rs-oim25`.
/// Stored as integer percent to keep [`RelicSpec`] `Copy`-friendly.
pub const BOWLER_HAT_MULTIPLIER_PCT: i32 = 25;
/// `Candelabra.ENERGY` — round-2 energy gain (`uncommon.py:138`).
pub const CANDELABRA_ENERGY: i32 = 2;
/// `EternalFeather.HEAL_PER` — heal per `CARDS_PER` deck cards at
/// rest sites (`uncommon.py:152`).
pub const ETERNAL_FEATHER_HEAL_PER: i32 = 3;
/// `EternalFeather.CARDS_PER` — deck-size divisor (`uncommon.py:153`).
pub const ETERNAL_FEATHER_CARDS_PER: i32 = 5;
/// `FuneraryMask.CARDS` — round-1 Soul cards inserted
/// (`uncommon.py:168`).
pub const FUNERARY_MASK_CARDS: i32 = 3;
/// `GalacticDust.BLOCK` — block gained every 10 stars spent
/// (`uncommon.py:185`).
pub const GALACTIC_DUST_BLOCK: i32 = 10;
/// `GalacticDust.STARS_THRESHOLD` — accumulated stars-spend per
/// block trigger (`uncommon.py:184`).
pub const GALACTIC_DUST_STARS_THRESHOLD: i32 = 10;
/// `GoldPlatedCables.ENERGY` — passive orb-trigger budget
/// (`uncommon.py:231`).
pub const GOLD_PLATED_CABLES_ENERGY: i32 = 1;
/// `GoldPlatedCables.CARDS` — extra passive triggers granted to the
/// leftmost orb (`uncommon.py:232`).
pub const GOLD_PLATED_CABLES_CARDS: i32 = 1;
/// `GremlinHorn.ENERGY` — energy on enemy death (`uncommon.py`).
pub const GREMLIN_HORN_ENERGY: i32 = 1;
/// `GremlinHorn.CARDS` — cards drawn on enemy death.
pub const GREMLIN_HORN_CARDS: i32 = 1;
/// `HornCleat.BLOCK` — round-2 block on block-clear
/// (`uncommon.py:259`).
pub const HORN_CLEAT_BLOCK: i32 = 14;
/// `JossPaper.CARDS` — draw per `CARDS_PER` exhausted cards
/// (`uncommon.py:273`).
pub const JOSS_PAPER_CARDS: i32 = 1;
/// `Kusarigama.ATTACK_THRESHOLD` — Nth attack triggers
/// (`uncommon.py::Kusarigama`).
pub const KUSARIGAMA_ATTACK_THRESHOLD: i32 = 3;
/// `Kusarigama.DAMAGE` — damage dealt on Nth attack
/// (`uncommon.py::Kusarigama`).
pub const KUSARIGAMA_DAMAGE: i32 = 6;

macro_rules! relic_factory {
    ($vis:vis $fn_name:ident, $kind:ident, $name:literal, $rarity:ident, $effect:expr) => {
        #[must_use]
        $vis const fn $fn_name() -> RelicSpec {
            RelicSpec {
                id: RelicKind::$kind,
                name: $name,
                rarity: RelicRarity::$rarity,
                effect: $effect,
            }
        }
    };
}

relic_factory!(pub akabeko, Akabeko, "AKABEKO", Uncommon, RelicEffect::Noop);
relic_factory!(pub bag_of_marbles, BagOfMarbles, "BAG_OF_MARBLES", Common, RelicEffect::Noop);
// Bead `sts2-rs-uo8t`: real listener (`listener_bellows`) registered
// via `register_after_side_turn_start` — round-1 player-side upgrades
// every card in hand. Factory body stays `Noop`.
relic_factory!(pub bellows, Bellows, "BELLOWS", Uncommon, RelicEffect::Noop);
relic_factory!(pub book_repair_knife, BookRepairKnife, "BOOK_REPAIR_KNIFE", Uncommon, RelicEffect::Noop);
relic_factory!(pub bowler_hat, BowlerHat, "BOWLER_HAT", Uncommon, RelicEffect::Noop);
relic_factory!(pub candelabra, Candelabra, "CANDELABRA", Uncommon, RelicEffect::Noop);
relic_factory!(pub eternal_feather, EternalFeather, "ETERNAL_FEATHER", Uncommon, RelicEffect::Noop);
// Bead `sts2-rs-hnd8`: real listener (`listener_funerary_mask`)
// registered via `register_after_side_turn_start` — round-1 player-
// side inserts 3 Soul cards into draw. Factory body stays `Noop`.
relic_factory!(pub funerary_mask, FuneraryMask, "FUNERARY_MASK", Uncommon, RelicEffect::Noop);
relic_factory!(pub galactic_dust, GalacticDust, "GALACTIC_DUST", Uncommon, RelicEffect::Noop);
relic_factory!(pub gold_plated_cables, GoldPlatedCables, "GOLD_PLATED_CABLES", Uncommon, RelicEffect::Noop);
relic_factory!(pub gremlin_horn, GremlinHorn, "GREMLIN_HORN", Uncommon, RelicEffect::Noop);
relic_factory!(pub horn_cleat, HornCleat, "HORN_CLEAT", Uncommon, RelicEffect::Noop);
// Bead `sts2-rs-obpe`: real listener lives in
// `sts2-core::relic_effects` (every 5th card exhausted per combat
// draws 1; combat-scoped counter, no per-turn reset). Factory body
// stays `Noop` — the trigger fires via the listener bus.
relic_factory!(pub joss_paper, JossPaper, "JOSS_PAPER", Uncommon, RelicEffect::Noop);
// Bead `sts2-rs-owg8`: real listener lives in
// `sts2-core::relic_effects` (every 3rd ATTACK / turn deals 6 to a
// random alive enemy). Factory body stays `Noop` — the trigger fires
// via the listener bus, not the legacy `on_trigger_*` marker path.
relic_factory!(pub kusarigama, Kusarigama, "KUSARIGAMA", Uncommon, RelicEffect::Noop);

// ---------------------------------------------------------------------------
// RelicSpec factories — L–Z wave (bead `sts2-rs-6ynt.5.6`)
// ---------------------------------------------------------------------------
//
// Continuation of the A–K scaffold (bead `sts2-rs-6ynt.5.5`). Same
// staged-rollout shape: 26 `RelicEffect::Noop` factories + constants
// pinning Python class attributes from
// `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py`. Body wiring
// is deferred — the wiring sub-bead is expected to reuse the AST
// variants `sts2-rs-6ynt.5.3` added (`ApplyPowerAtCombatStart`,
// `GainBlockAtCombatStart`, `HealAtPlayerSideTurnStart`,
// `DrawCardsOnFirstDamagePerCombat`,
// `DealDamageToAllEnemiesOnSideTurnStart`,
// `GainEnergyEveryNPlayerSideTurns`, `AppendRewardGold`).

/// `LetterOpener.SKILL_THRESHOLD` — every Nth skill triggers the
/// AoE (`uncommon.py::LetterOpener`).
pub const LETTER_OPENER_SKILL_THRESHOLD: i32 = 3;
/// `LetterOpener.DAMAGE` — AoE damage on Nth skill
/// (`uncommon.py::LetterOpener`).
pub const LETTER_OPENER_DAMAGE: i32 = 5;
/// `LuckyFysh.GOLD` — gold gained each time a card is added to the
/// deck (`uncommon.py::LuckyFysh.on_card_added_to_deck`,
/// `GOLD = 15`). Real body lives on [`listener_lucky_fysh`].
pub const LUCKY_FYSH_GOLD: i32 = 15;
/// `MercuryHourglass.DAMAGE` — AoE damage at every player turn start
/// (`uncommon.py::MercuryHourglass`).
pub const MERCURY_HOURGLASS_DAMAGE: i32 = 3;
/// `MiniatureCannon.EXTRA_DAMAGE` — added damage on each attack
/// (`uncommon.py::MiniatureCannon`).
pub const MINIATURE_CANNON_EXTRA_DAMAGE: i32 = 3;
/// `Nunchaku.ATTACK_THRESHOLD` — energy granted every Nth attack
/// (`uncommon.py::Nunchaku`).
pub const NUNCHAKU_ATTACK_THRESHOLD: i32 = 10;
/// `Nunchaku.ENERGY` — energy gain on Nth attack
/// (`uncommon.py::Nunchaku`).
pub const NUNCHAKU_ENERGY: i32 = 1;
/// `Orichalcum.BLOCK` — end-of-turn block when block == 0
/// (`uncommon.py::Orichalcum`).
pub const ORICHALCUM_BLOCK: i32 = 6;
/// `OrnamentalFan.ATTACK_THRESHOLD` — block granted every Nth attack
/// (`uncommon.py::OrnamentalFan`).
pub const ORNAMENTAL_FAN_ATTACK_THRESHOLD: i32 = 3;
/// `OrnamentalFan.BLOCK` — block on Nth attack
/// (`uncommon.py::OrnamentalFan`).
pub const ORNAMENTAL_FAN_BLOCK: i32 = 4;
/// `Pantograph.HEAL` — heal before boss combat
/// (`uncommon.py::Pantograph`).
pub const PANTOGRAPH_HEAL: i32 = 25;
/// `PaperPhrog.EXTRA_VULNERABLE_PCT` — extra vulnerable damage on
/// non-vulnerable enemies (`uncommon.py::PaperPhrog`,
/// `EXTRA_VULNERABLE = 0.25`; stored as integer percent so
/// [`RelicSpec`] remains `Copy`-friendly).
pub const PAPER_PHROG_EXTRA_VULNERABLE_PCT: i32 = 25;
/// `ParryingShield.BLOCK_THRESHOLD` — block trigger
/// (`uncommon.py::ParryingShield`).
pub const PARRYING_SHIELD_BLOCK_THRESHOLD: i32 = 10;
/// `ParryingShield.DAMAGE` — damage on block trigger
/// (`uncommon.py::ParryingShield`).
pub const PARRYING_SHIELD_DAMAGE: i32 = 6;
/// `Pear.MAX_HP` — max-HP gained on obtain (`uncommon.py::Pear`).
pub const PEAR_MAX_HP: i32 = 10;
/// `PenNib.THRESHOLD` — Nth attack deals double damage
/// (`uncommon.py::PenNib`).
pub const PEN_NIB_THRESHOLD: i32 = 10;
/// `Planisphere` — heal on unknown-room entry. C#
/// `Planisphere.cs` `CanonicalVars = HealVar(5m)`; the Python pin's
/// `HEAL = 4` (`uncommon.py`) drifted from the shipped game. Bead
/// `sts2-rs-5ri31`.
pub const PLANISPHERE_HEAL: i32 = 5;
/// `RedMask.WEAK` — round-1 weak per enemy (`uncommon.py::RedMask`).
pub const RED_MASK_WEAK: i32 = 1;
/// `Regalite.BLOCK` — block on colorless card enter
/// (`uncommon.py::Regalite`).
pub const REGALITE_BLOCK: i32 = 2;
/// `ReptileTrinket.STRENGTH` — temp strength on potion use
/// (`uncommon.py::ReptileTrinket`).
pub const REPTILE_TRINKET_STRENGTH: i32 = 3;
/// `RippleBasin.BLOCK` — end-of-turn block when no attacks played
/// (`uncommon.py::RippleBasin`).
pub const RIPPLE_BASIN_BLOCK: i32 = 4;
/// `SelfFormingClay.BLOCK_NEXT_TURN` — queued block on unblocked
/// damage (`uncommon.py::SelfFormingClay`).
pub const SELF_FORMING_CLAY_BLOCK_NEXT_TURN: i32 = 3;
/// `SparklingRouge.STRENGTH` — strength on round-3 block clear
/// (`uncommon.py::SparklingRouge`).
pub const SPARKLING_ROUGE_STRENGTH: i32 = 1;
/// `SparklingRouge.DEXTERITY` — dexterity on round-3 block clear
/// (`uncommon.py::SparklingRouge`).
pub const SPARKLING_ROUGE_DEXTERITY: i32 = 1;
/// `StoneCracker` — random draw-pile upgrades on combat-room entry.
/// C# `StoneCracker.cs` `CanonicalVars = CardsVar(2)` (the Python
/// pin's `uncommon.py::StoneCracker.CARDS = 3` drifted from the
/// shipped game). Bead `sts2-rs-3r6hc` dropped the spurious
/// `state.is_boss` trigger gate; the relic now fires per
/// `room is CombatRoom` (normal / elite / boss) matching C#.
pub const STONE_CRACKER_CARDS: i32 = 2;
/// `SymbioticVirus.DARK_ORBS` — round-1 Dark orbs channeled
/// (`uncommon.py::SymbioticVirus`).
pub const SYMBIOTIC_VIRUS_DARK_ORBS: i32 = 1;
/// `Tingsha.DAMAGE` — damage per discarded card on player turn
/// (`uncommon.py::Tingsha`).
pub const TINGSHA_DAMAGE: i32 = 3;
/// `TuningFork.SKILL_THRESHOLD` — block trigger every Nth skill
/// (`uncommon.py::TuningFork`).
pub const TUNING_FORK_SKILL_THRESHOLD: i32 = 10;
/// `TuningFork.BLOCK` — block on Nth skill (`uncommon.py::TuningFork`).
pub const TUNING_FORK_BLOCK: i32 = 7;
/// `TwistedFunnel.POISON` — round-1 poison per enemy
/// (`uncommon.py::TwistedFunnel`).
pub const TWISTED_FUNNEL_POISON: i32 = 4;

relic_factory!(pub letter_opener, LetterOpener, "LETTER_OPENER", Uncommon, RelicEffect::Noop);
relic_factory!(pub lucky_fysh, LuckyFysh, "LUCKY_FYSH", Uncommon, RelicEffect::Noop);
relic_factory!(pub mercury_hourglass, MercuryHourglass, "MERCURY_HOURGLASS", Uncommon, RelicEffect::Noop);
relic_factory!(pub miniature_cannon, MiniatureCannon, "MINIATURE_CANNON", Uncommon, RelicEffect::Noop);
relic_factory!(pub nunchaku, Nunchaku, "NUNCHAKU", Uncommon, RelicEffect::Noop);
relic_factory!(pub orichalcum, Orichalcum, "ORICHALCUM", Uncommon, RelicEffect::Noop);
relic_factory!(pub ornamental_fan, OrnamentalFan, "ORNAMENTAL_FAN", Uncommon, RelicEffect::Noop);
relic_factory!(pub pantograph, Pantograph, "PANTOGRAPH", Uncommon, RelicEffect::Noop);
relic_factory!(pub paper_phrog, PaperPhrog, "PAPER_PHROG", Uncommon, RelicEffect::Noop);
relic_factory!(pub parrying_shield, ParryingShield, "PARRYING_SHIELD", Uncommon, RelicEffect::Noop);
relic_factory!(pub pear, Pear, "PEAR", Uncommon, RelicEffect::GainMaxHpOnObtained { amount: PEAR_MAX_HP });
relic_factory!(pub pen_nib, PenNib, "PEN_NIB", Uncommon, RelicEffect::Noop);
// Bead `sts2-rs-v5ku`: real listener
// (`on_before_combat_start_petrified_toad`) in sts2-core::relic_effects
// — at combat start, push PotionShapedRock onto state.player.potions
// (MAX_POTIONS cap respected). Factory body stays `Noop`.
relic_factory!(pub petrified_toad, PetrifiedToad, "PETRIFIED_TOAD", Uncommon, RelicEffect::Noop);
// Bead `sts2-rs-3wb0`: real listener (`listener_planisphere`)
// registered via `register_after_room_entered` — heal 4 on Unknown
// room entry (alive-gated). Factory body stays `Noop`.
relic_factory!(pub planisphere, Planisphere, "PLANISPHERE", Uncommon, RelicEffect::Noop);
relic_factory!(pub red_mask, RedMask, "RED_MASK", Common, RelicEffect::Noop);
relic_factory!(pub regalite, Regalite, "REGALITE", Uncommon, RelicEffect::Noop);
relic_factory!(pub reptile_trinket, ReptileTrinket, "REPTILE_TRINKET", Uncommon, RelicEffect::Noop);
relic_factory!(pub ripple_basin, RippleBasin, "RIPPLE_BASIN", Uncommon, RelicEffect::Noop);
relic_factory!(pub self_forming_clay, SelfFormingClay, "SELF_FORMING_CLAY", Uncommon, RelicEffect::Noop);
relic_factory!(pub sparkling_rouge, SparklingRouge, "SPARKLING_ROUGE", Uncommon, RelicEffect::Noop);
relic_factory!(pub stone_cracker, StoneCracker, "STONE_CRACKER", Uncommon, RelicEffect::Noop);
relic_factory!(pub symbiotic_virus, SymbioticVirus, "SYMBIOTIC_VIRUS", Uncommon, RelicEffect::Noop);
relic_factory!(pub tingsha, Tingsha, "TINGSHA", Uncommon, RelicEffect::Noop);
relic_factory!(pub tuning_fork, TuningFork, "TUNING_FORK", Uncommon, RelicEffect::Noop);
relic_factory!(pub twisted_funnel, TwistedFunnel, "TWISTED_FUNNEL", Uncommon, RelicEffect::Noop);
relic_factory!(pub vambrace, Vambrace, "VAMBRACE", Uncommon, RelicEffect::Noop);

// ---------------------------------------------------------------------------
// Per-relic handlers — one per Python class
// ---------------------------------------------------------------------------
//
// Naming convention: `on_trigger_<snake_case_id>` to mirror the
// `card_effects::on_play_<id>` convention from bead sts2-rs-ov4 (P6-1).
// Each handler is a `RelicEffectFn` that returns
// `EffectResult::Resolved` once dispatched. Gameplay plumbing lands
// in downstream beads — the Python listener method name is recorded in
// each doc comment so the wiring bead has a one-shot reference.

/// `EternalFeather::after_room_entered` — at rest-site entry, heal
/// `HEAL_PER * (deck_size // CARDS_PER) = 3 * (deck / 5)`. Mirrors
/// Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::EternalFeather`:
///
/// ```python
/// def after_room_entered(self, owner, room_type):
///     if hasattr(room_type, "is_rest_site") and room_type.is_rest_site:
///         deck_size = len(getattr(owner, "deck", []))
///         heal = self.HEAL_PER * (deck_size // self.CARDS_PER)
///         if heal > 0 and owner.current_hp > 0:
///             owner.heal(heal)
/// ```
///
/// Ownership-gated; matches the alive-gate from
/// [`listener_pantograph`] / [`listener_meal_ticket`].
fn listener_eternal_feather(rs: &mut RunState, room_type: RoomType) -> EffectResult {
    if !rs.owns_active_relic(RelicId::EternalFeather) {
        return EffectResult::NotImplemented;
    }
    if !matches!(room_type, RoomType::RestSite) {
        return EffectResult::NotImplemented;
    }
    if rs.current_hp <= 0 {
        return EffectResult::NotImplemented;
    }
    let deck_size = rs.master_deck.len() as i32;
    let heal = ETERNAL_FEATHER_HEAL_PER * (deck_size / ETERNAL_FEATHER_CARDS_PER);
    if heal <= 0 {
        return EffectResult::NotImplemented;
    }
    rs.heal(heal);
    EffectResult::Resolved
}

/// `PenNib::modify_damage_multiplicative` listener body — bead
/// `sts2-rs-cr4y8`. Faithful port of C#
/// `PenNib.ModifyDamageMultiplicative` (PenNib.cs): the 10th attack
/// deals double damage via a TRUE multiplicative on the fully-computed
/// attack damage (base + Strength + flat bonuses), composing with
/// Vulnerable etc. and flooring once.
///
/// Returns `DamageMul::new(2, 1)` (×2) when:
/// - player owns PenNib AND
/// - the damage is a powered attack (`props.is_powered()`, the Rust
///   analogue of C# `props.IsPoweredAttack()`) AND
/// - the dealer is the player (PenNib's owner) AND
/// - `(relic.counter + 1) % PEN_NIB_THRESHOLD == 0` — i.e. this damage
///   event belongs to the upcoming 10th attack-card play (the
///   `after_card_played` increment fires post-damage, so the active
///   window is `counter == 9`).
///
/// Returns `DamageMul::ONE` otherwise.
///
/// Faithfulness note: C# additionally gates on `cardSource != null`
/// and `cardSource == AttackToDouble` (the specific marked card). The
/// relic-side multiplicative channel carries no card identity
/// (`hooks::DamageCtx` has only dealer/target/props), so the card gate
/// is approximated by `is_powered()` + the player-dealer check + the
/// per-attack counter window. During the marked 10th attack's play the
/// only player powered-attack damage is that card's own hits, so the
/// two are equivalent for every realistic combat — the same card-less
/// multiplicative pattern used by [`listener_paper_krane`].
#[allow(clippy::trivially_copy_pass_by_ref)]
fn listener_pen_nib_modify_damage(state: &CombatState, ctx: &hooks::DamageCtx) -> hooks::DamageMul {
    use hooks::DamageMul;
    let Some(relic) = state
        .relics
        .iter()
        .find(|r| !r.is_melted && r.kind == RelicKind::PenNib)
    else {
        return DamageMul::ONE;
    };
    if !ctx.props.is_powered() {
        return DamageMul::ONE;
    }
    // C# `PenNib.cs:113` doubles damage dealt by the player OR the player's
    // Osty (`dealer == Owner || dealer == Owner.Osty`). Necrobinder card damage
    // dealt FromOsty (Flatten/Snap/Unleash) must still get the ×2 (RP134).
    let is_player_or_osty = matches!(ctx.dealer, CreatureRef::Player)
        || matches!(ctx.dealer, CreatureRef::Enemy(uid) if state.player.pet_uid == Some(uid));
    if !is_player_or_osty {
        return DamageMul::ONE;
    }
    if (relic.counter + 1) % PEN_NIB_THRESHOLD != 0 {
        return DamageMul::ONE;
    }
    DamageMul::new(2, 1)
}

/// `PenNib::after_card_played` listener body — bead `sts2-rs-w46z`.
/// Increments the attacks-played counter when the just-played card
/// was an `Attack`. Counter is the per-relic-instance scratch field
/// (`Relic.counter`) — wraps at i32::MAX (saturating add); the
/// `% THRESHOLD` predicate makes wrap-around behavior cosmetic in
/// practice.
fn listener_pen_nib_after_card_played(state: &mut CombatState, ctx: &CardEventCtx) -> EffectResult {
    let Some(uid) = ctx.card_uid else {
        return EffectResult::NotImplemented;
    };
    let is_attack = sts2_core::card_by_uid(&state.cards, uid)
        .is_some_and(|c| c.card_type == sts2_core::cards::CardType::Attack);
    if !is_attack {
        return EffectResult::NotImplemented;
    }
    if let Some(r) = state
        .relics
        .iter_mut()
        .find(|r| !r.is_melted && r.kind == RelicKind::PenNib)
    {
        r.counter = r.counter.saturating_add(1);
    } else {
        return EffectResult::NotImplemented;
    }
    EffectResult::Resolved
}

/// `Regalite::after_card_entered_combat` — gain
/// [`REGALITE_BLOCK`] (= 2) when a colorless card lands in the
/// player's hand mid-combat. Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py:638-657`:
///
/// ```python
/// def after_card_entered_combat(self, owner, card, combat):
///     if getattr(card, "owner", None) is not owner:
///         return
///     is_colorless = bool(
///         getattr(card, "is_colorless", False)
///         or getattr(visual_card_pool, "is_colorless", False)
///         or getattr(card, "card_id", None) in _colorless_card_ids()
///     )
///     if is_colorless:
///         owner.gain_block(self.BLOCK, unpowered=True)
/// ```
///
/// Rust port adapts the colorless gate to the static
/// [`crate::cards::colorless::is_colorless_card_id`] match (the only
/// "is_colorless" signal at the engine pin; visual_card_pool is a
/// Python-only attribute). Ownership is implicit — at the current pin
/// the channel only fires from `spawn_card_into_hand` (player-side
/// only). Block goes through the file-local
/// [`gain_block_player_unpowered`] helper — C# `Regalite.cs` declares
/// `BlockVar(2m, ValueProp.Unpowered)`, so Frail's 0.75x modifier is
/// skipped (bead `sts2-rs-w5dca`).
fn listener_regalite(
    state: &mut CombatState,
    card_uid: CardUid,
    added_by_player: bool,
) -> EffectResult {
    if !state.relics.owns(RelicKind::Regalite) {
        return EffectResult::NotImplemented;
    }
    // C# `Regalite.AfterCardGeneratedForCombat(card, Player? creator)`
    // (`v0.107.1/.../Relics/Regalite.cs`) grants the +2 Block ONLY when
    // `creator != null && creator == base.Owner` — i.e. the card was
    // generated BY THE PLAYER. An enemy-power generation (Aeonglass
    // `WitheringPresence` adds a Wither to the player's hand with
    // `creator == null`) must NOT grant block, even though it lands in a
    // player pile mid-turn. `added_by_player` carries that creator signal
    // (enemy generators route through `spawn_card_into_pile_with_origin`
    // with `false`). Bead `sts2-rs-regalite-creator`.
    if !added_by_player {
        return EffectResult::NotImplemented;
    }
    // The `card_uid` lookup doubles as the ownership / existence guard. The
    // prior `is_colorless_card_id` gate was a Python-pin fabrication
    // (`uncommon.py::Regalite`) and dropped block for every non-colorless card.
    if !state.cards.iter().any(|c| c.uid == card_uid) {
        return EffectResult::NotImplemented;
    }
    gain_block_player_unpowered(state, REGALITE_BLOCK);
    EffectResult::Resolved
}

/// Bead `sts2-rs-w5dca`: file-local block grant that bypasses
/// Frail/Dexterity — mirrors C# `BlockVar(N, ValueProp.Unpowered)`.
/// Same body as the canonical helper but with `ValueProps::UNPOWERED`
/// set so `modify_block` listeners gated on `is_powered()` skip the
/// grant.
fn gain_block_player_unpowered(state: &mut CombatState, base: i32) {
    let ctx = BlockCtx {
        target: CreatureRef::Player,
        props: ValueProps::MOVE.union(ValueProps::UNPOWERED),
        card_source: None,
    };
    let final_block = hooks::modify_block(state, base, &ctx);
    state.player.block = state.player.block.saturating_add(final_block);
    state.sync_creature_block_from_player();
    if final_block > 0 {
        let gained_ctx = BlockGainedCtx {
            target: CreatureRef::Player,
            amount: final_block,
            card_source: None,
        };
        hooks::dispatch_after_block_gained(state, &gained_ctx);
        let _ = sts2_core::relic_effects::dispatch_after_block_gained(state, &gained_ctx);
    }
}

/// `StoneCracker::before_combat_start` — at the start of every combat
/// room (normal / elite / boss), upgrade up to
/// `STONE_CRACKER_CARDS = 2` random not-already-upgraded cards in the
/// player's draw pile (C# `CanonicalVars = CardsVar(2)`). Mirrors C#
/// `StoneCracker.cs::AfterRoomEntered`
/// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Relics/StoneCracker.cs`):
///
/// [decompiled C# omitted — see the cited path]
///
/// Bead `sts2-rs-3r6hc` (follow-up to `sts2-rs-48htt`): dropped the
/// `state.is_boss` gate inherited from the Python pin
/// (`sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::StoneCracker`),
/// which is a Python-side fabrication not present in the shipped game.
///
/// Sampling reads the `combat_card_selection` stream via `stable_shuffle_by`
/// (faithful C# `StableShuffle` = Entry-ordinal/upgrade-level sort then
/// Fisher-Yates) — see the inline comment at the shuffle site (beads
/// `sts2-rs-4z5iw` / `sts2-rs-5aelq`).
fn listener_stone_cracker(state: &mut CombatState) -> EffectResult {
    use sts2_core::card_effects::upgrade_card_uid;

    if !state.relics.owns(RelicKind::StoneCracker) {
        return EffectResult::NotImplemented;
    }
    let mut candidates: Vec<CardUid> = state
        .player
        .draw
        .iter()
        .copied()
        .filter(|uid| {
            let idx = uid.0.get() as usize - 1;
            // C# `StoneCracker.cs::AfterRoomEntered` filters `Draw.Cards.Where(IsUpgradable)`.
            // `Card::is_upgradable()` = `!upgraded && card_type not Curse|Status`, matching
            // C# `IsUpgradable` for all combat-relevant `MaxUpgradeLevel==0` cards. The old
            // `!c.upgraded` predicate wrongly included non-upgradable Curse/Status cards
            // (e.g. SPORE_MIND), upgrading the wrong card AND lengthening the `StableShuffle`
            // candidate list — shifting the `combat_card_selection` Fisher-Yates read count.
            state
                .cards
                .get(idx)
                .is_some_and(sts2_core::cards::Card::is_upgradable)
        })
        .collect();
    if candidates.is_empty() {
        return EffectResult::NotImplemented;
    }
    // C# `StoneCracker.cs::AfterRoomEntered`:
    //   Draw.Cards.Where(IsUpgradable)
    //       .StableShuffle(RunState.Rng.CombatCardSelection)
    //       .Take(DynamicVars.Cards=2) -> CardCmd.Upgrade.
    // `StableShuffle` (`ListExtensions.cs:10`) = `Sort()` by
    // `CardModel.CompareTo` (Entry ordinal, then CurrentUpgradeLevel
    // ascending) THEN `UnstableShuffle` (Fisher-Yates from the end,
    // `NextInt(num+1)`), so it reads `combat_card_selection` exactly
    // `candidates.len() - 1` times. The prior port used the legacy
    // Xoshiro `state.rng` and a partial 2-swap, so it consumed ZERO
    // dotnet `combat_card_selection` reads — desyncing every later
    // draw of that stream (sweep: cs9 / rs0 for the 10-card starter).
    // Bead `sts2-rs-4z5iw` follow-up.
    // `list.Sort()` is .NET introsort (UNSTABLE for tied keys above 16
    // elements), so route the (Entry, upgraded) pre-sort through
    // `Rng::stable_shuffle_by` (faithful introsort + Fisher-Yates) rather
    // than a stable `sort_by`. Bead `sts2-rs-5aelq-followup`.
    let cmp = |a: CardUid, b: CardUid| {
        let ia = a.0.get() as usize - 1;
        let ib = b.0.get() as usize - 1;
        let (ca, cb) = (&state.cards[ia], &state.cards[ib]);
        ca.card_id
            .to_python_name()
            .cmp(cb.card_id.to_python_name())
            .then_with(|| u8::from(ca.upgraded).cmp(&u8::from(cb.upgraded)))
    };
    state
        .run_rng
        .combat_card_selection
        .stable_shuffle_by(candidates.as_mut_slice(), &cmp);
    for uid in candidates.into_iter().take(STONE_CRACKER_CARDS as usize) {
        upgrade_card_uid(state, uid);
    }
    EffectResult::Resolved
}

/// `GoldPlatedCables::after_side_turn_start` — on player-side turn
/// start, if the front-most filled orb IS Plasma, re-fire its passive
/// (Plasma's `+1 energy`). Mirrors Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py::GoldPlatedCables`:
///
/// ```python
/// def after_side_turn_start(self, owner, side, combat):
///     if side == owner.side:
///         self._trigger_first_passive_if(owner, combat, plasma=True)
/// ```
///
/// `_trigger_first_passive_if(plasma=True)` walks the owner's orb
/// queue, returns if empty, and calls `orb_queue.trigger_first_passive`
/// iff `first.orb_type == OrbType.PLASMA`. The Rust port routes through
/// [`sts2_core::orbs::trigger_first_orb_passive`] (`sts2-rs-rt2ry`),
/// which fires Plasma's `+1 energy` arm directly.
fn listener_gold_plated_cables_after_side_turn_start(
    state: &mut CombatState,
    side: Side,
) -> EffectResult {
    use sts2_core::orbs::{trigger_first_orb_passive, Orb};
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::GoldPlatedCables) {
        return EffectResult::NotImplemented;
    }
    let Some(idx) = state.player.orbs.slots.iter().position(|o| !o.is_empty()) else {
        return EffectResult::NotImplemented;
    };
    if !matches!(state.player.orbs.slots[idx], Orb::Plasma) {
        return EffectResult::NotImplemented;
    }
    trigger_first_orb_passive(state);
    EffectResult::Resolved
}

/// `GoldPlatedCables::before_turn_end` — on player-side turn end, if
/// the front-most filled orb is NOT Plasma, re-fire its passive.
/// Mirrors Python's `_trigger_first_passive_if(plasma=False)` arm —
/// covers Lightning, Frost, Dark, and Glass via
/// [`sts2_core::orbs::trigger_first_orb_passive`].
fn listener_gold_plated_cables_before_turn_end(
    state: &mut CombatState,
    side: Side,
) -> EffectResult {
    use sts2_core::orbs::{trigger_first_orb_passive, Orb};
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::GoldPlatedCables) {
        return EffectResult::NotImplemented;
    }
    let Some(idx) = state.player.orbs.slots.iter().position(|o| !o.is_empty()) else {
        return EffectResult::NotImplemented;
    };
    if matches!(state.player.orbs.slots[idx], Orb::Plasma) {
        return EffectResult::NotImplemented;
    }
    trigger_first_orb_passive(state);
    EffectResult::Resolved
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The id list covers exactly 40 entries — the count Python pin
    /// at `sts2-rl-agent@01c094d:sts2_env/relics/uncommon.py`
    /// (`grep -c '^@register_relic' uncommon.py == 40`).
    #[test]
    fn pool_has_forty_entries() {
        assert_eq!(UNCOMMON_RELIC_IDS.len(), 40);
    }

    /// Every `RelicId` in the list is distinct. Catches accidental
    /// duplicate paste-edits inside [`UNCOMMON_RELIC_IDS`].
    #[test]
    fn pool_entries_are_unique_by_relic_id() {
        let mut ids: Vec<RelicId> = UNCOMMON_RELIC_IDS.to_vec();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate RelicId in UNCOMMON_RELIC_IDS");
    }

    /// Bead `sts2-rs-cr4y8`: PenNib must register both a
    /// `modify_damage_multiplicative` listener (for the ×2 on the 10th
    /// attack) and an `after_card_played` listener (for the counter
    /// increment). The earlier `sts2-rs-w46z` port used the
    /// `modify_damage_additive` channel, which under-doubled flat
    /// bonuses; assert it is no longer registered there.
    #[test]
    fn pen_nib_registers_both_channels() {
        crate::relics::dispatch::ensure_installed();
        assert!(
            sts2_core::relic_effects::lookup_modify_damage_multiplicative(RelicId::PenNib)
                .is_some(),
            "PenNib must register modify_damage_multiplicative listener"
        );
        assert!(
            sts2_core::relic_effects::lookup_modify_damage_additive(RelicId::PenNib).is_none(),
            "PenNib must NOT register a modify_damage_additive listener (moved to multiplicative)"
        );
        assert!(
            sts2_core::relic_effects::lookup_after_card_played(RelicId::PenNib).is_some(),
            "PenNib must register after_card_played listener"
        );
    }

    /// Bead `sts2-rs-cr4y8`: counter pre-increment window — only the
    /// 10th (20th, …) attack returns `×2`; every other attack returns
    /// the identity multiplier. Drives the listener directly with a
    /// stub state. Mirrors C# `PenNib.ModifyDamageMultiplicative`'s
    /// `AttacksPlayed == 9` activation.
    #[test]
    fn pen_nib_modify_damage_returns_one_until_tenth_attack() {
        use std::num::NonZeroU32;
        use sts2_core::hooks::{CreatureRef, DamageMul, ValueProps};
        use sts2_core::relics::RelicId as CoreRelicId;
        use sts2_core::{Relic, RelicKind as CoreRelicKind};

        crate::relics::dispatch::ensure_installed();

        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        state.init_player_creature();
        state.sync_player_creature();

        state.relics.push(Relic {
            id: NonZeroU32::new(200).expect("non-zero"),
            kind: CoreRelicKind::PenNib,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        // Powered attack dealt by the player.
        let ctx = sts2_core::hooks::DamageCtx {
            dealer: CreatureRef::Player,
            target: CreatureRef::Player,
            props: ValueProps::MOVE,
        };

        let listener =
            sts2_core::relic_effects::lookup_modify_damage_multiplicative(CoreRelicId::PenNib)
                .expect("PenNib multiplicative listener registered");

        let set_counter = |state: &mut CombatState, n: i32| {
            state
                .relics
                .iter_mut()
                .find(|r| r.kind == CoreRelicKind::PenNib)
                .unwrap()
                .counter = n;
        };

        // counter=0 → next attack is #1, not divisible by 10 → ×1.
        assert_eq!(listener(&state, &ctx), DamageMul::ONE, "1st attack: ×1");

        // counter=9 → next attack is #10, threshold hit → ×2.
        set_counter(&mut state, 9);
        assert_eq!(
            listener(&state, &ctx),
            DamageMul::new(2, 1),
            "10th attack: ×2"
        );

        // counter=10 → next attack is #11, not divisible → ×1.
        set_counter(&mut state, 10);
        assert_eq!(listener(&state, &ctx), DamageMul::ONE, "11th attack: ×1");

        // counter=19 → next attack is #20, divisible → ×2 again.
        set_counter(&mut state, 19);
        assert_eq!(
            listener(&state, &ctx),
            DamageMul::new(2, 1),
            "20th attack: ×2"
        );

        // Unpowered attack on the 10th window → ×1 (C# IsPoweredAttack gate).
        set_counter(&mut state, 9);
        let unpowered = sts2_core::hooks::DamageCtx {
            dealer: CreatureRef::Player,
            target: CreatureRef::Player,
            props: ValueProps::MOVE.union(ValueProps::UNPOWERED),
        };
        assert_eq!(
            listener(&state, &unpowered),
            DamageMul::ONE,
            "unpowered attack: ×1 even on the 10th"
        );
    }

    /// Bead `sts2-rs-cr4y8`: the ×2 multiplies the FULLY-computed attack
    /// (base + Strength), not just the base. End-to-end through
    /// `hooks::modify_damage`: base 10 + Strength 3, 10th attack →
    /// `(10+3)×2 = 26` (the old additive port produced `10+3+10 = 23`).
    #[test]
    fn pen_nib_doubles_base_plus_strength_end_to_end() {
        use std::num::NonZeroU32;
        use sts2_core::hooks::{self, CreatureRef, DamageCtx, ValueProps};
        use sts2_core::{Relic, RelicKind as CoreRelicKind};

        crate::relics::dispatch::ensure_installed();

        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        state.init_player_creature();
        state.sync_player_creature();
        // Strength +3 on the player (the dealer).
        let player_uid = state.player.creature_uid.unwrap();
        apply_power_to_creature(&mut state, player_uid, PowerKind::Strength, 3);

        state.relics.push(Relic {
            id: NonZeroU32::new(201).expect("non-zero"),
            kind: CoreRelicKind::PenNib,
            counter: 9,
            is_wax: false,
            is_melted: false,
        });

        let ctx = DamageCtx {
            dealer: CreatureRef::Player,
            target: CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        assert_eq!(
            hooks::modify_damage(&state, 10, &ctx),
            26,
            "10th attack doubles base+Strength: (10+3)×2 = 26"
        );
    }

    // --- Spec-pin tests — A–K wave (bead `sts2-rs-6ynt.5.5`) -------------
    //
    // Each pin asserts (a) the factory emits the expected
    // `RelicEffect::Noop` shape with the right id/name/rarity, and
    // (b) the load-bearing Python constant is locked. Body wiring is
    // deferred — pin updates here are expected when a follow-up bead
    // swaps `Noop` for a real AST variant.

    #[test]
    fn akabeko_spec_pin() {
        let r = akabeko();
        assert_eq!(r.id, RelicKind::Akabeko);
        assert_eq!(r.name, "AKABEKO");
        assert_eq!(r.rarity, RelicRarity::Uncommon);
        assert_eq!(r.effect, RelicEffect::Noop);
        assert_eq!(r.name, RelicId::Akabeko.to_python_name());
        assert_eq!(AKABEKO_VIGOR, 8);
    }

    #[test]
    fn bag_of_marbles_spec_pin() {
        let r = bag_of_marbles();
        assert_eq!(r.id, RelicKind::BagOfMarbles);
        assert_eq!(r.name, "BAG_OF_MARBLES");
        // `Common` per C# `BagOfMarbles.cs:15` / grab_bag `SHARED_COMMON`
        // (bead `sts2-rs-kw0z9`). This module hosts the effect handler
        // (early-ported before the rarity correction); the factory tag now
        // matches the authoritative source.
        assert_eq!(r.rarity, RelicRarity::Common);
        assert_eq!(r.name, RelicId::BagOfMarbles.to_python_name());
        assert_eq!(BAG_OF_MARBLES_VULNERABLE, 1);
    }

    #[test]
    fn bellows_spec_pin() {
        let r = bellows();
        assert_eq!(r.id, RelicKind::Bellows);
        assert_eq!(r.name, "BELLOWS");
        assert_eq!(r.name, RelicId::Bellows.to_python_name());
    }

    #[test]
    fn book_repair_knife_spec_pin() {
        let r = book_repair_knife();
        assert_eq!(r.id, RelicKind::BookRepairKnife);
        assert_eq!(r.name, "BOOK_REPAIR_KNIFE");
        assert_eq!(r.name, RelicId::BookRepairKnife.to_python_name());
        assert_eq!(BOOK_REPAIR_KNIFE_HEAL_PER, 3);
    }

    #[test]
    fn bowler_hat_spec_pin() {
        let r = bowler_hat();
        assert_eq!(r.id, RelicKind::BowlerHat);
        assert_eq!(r.name, "BOWLER_HAT");
        assert_eq!(r.name, RelicId::BowlerHat.to_python_name());
        assert_eq!(BOWLER_HAT_MULTIPLIER_PCT, 25);
    }

    #[test]
    fn candelabra_spec_pin() {
        let r = candelabra();
        assert_eq!(r.id, RelicKind::Candelabra);
        assert_eq!(r.name, "CANDELABRA");
        assert_eq!(r.name, RelicId::Candelabra.to_python_name());
        assert_eq!(CANDELABRA_ENERGY, 2);
    }

    #[test]
    fn eternal_feather_spec_pin() {
        let r = eternal_feather();
        assert_eq!(r.id, RelicKind::EternalFeather);
        assert_eq!(r.name, "ETERNAL_FEATHER");
        assert_eq!(r.name, RelicId::EternalFeather.to_python_name());
        assert_eq!(ETERNAL_FEATHER_HEAL_PER, 3);
        assert_eq!(ETERNAL_FEATHER_CARDS_PER, 5);
    }

    #[test]
    fn funerary_mask_spec_pin() {
        let r = funerary_mask();
        assert_eq!(r.id, RelicKind::FuneraryMask);
        assert_eq!(r.name, "FUNERARY_MASK");
        assert_eq!(r.name, RelicId::FuneraryMask.to_python_name());
        assert_eq!(FUNERARY_MASK_CARDS, 3);
    }

    #[test]
    fn galactic_dust_spec_pin() {
        let r = galactic_dust();
        assert_eq!(r.id, RelicKind::GalacticDust);
        assert_eq!(r.name, "GALACTIC_DUST");
        assert_eq!(r.name, RelicId::GalacticDust.to_python_name());
        assert_eq!(GALACTIC_DUST_BLOCK, 10);
    }

    #[test]
    fn gold_plated_cables_spec_pin() {
        let r = gold_plated_cables();
        assert_eq!(r.id, RelicKind::GoldPlatedCables);
        assert_eq!(r.name, "GOLD_PLATED_CABLES");
        assert_eq!(r.name, RelicId::GoldPlatedCables.to_python_name());
        assert_eq!(GOLD_PLATED_CABLES_ENERGY, 1);
        assert_eq!(GOLD_PLATED_CABLES_CARDS, 1);
    }

    #[test]
    fn gremlin_horn_spec_pin() {
        let r = gremlin_horn();
        assert_eq!(r.id, RelicKind::GremlinHorn);
        assert_eq!(r.name, "GREMLIN_HORN");
        assert_eq!(r.name, RelicId::GremlinHorn.to_python_name());
        assert_eq!(GREMLIN_HORN_ENERGY, 1);
        assert_eq!(GREMLIN_HORN_CARDS, 1);
    }

    /// Regression guard for bead `sts2-rs-gpsvo`: GremlinHorn must be
    /// implemented EXACTLY ONCE — on the C#-faithful relic `after_death` bus
    /// in sts2-core (`relic_effects::gremlin_horn`). For months it was *also*
    /// registered here in content on `after_damage_given` (bead `sts2-rs-c22x`,
    /// a Python-pin port), so it fired TWICE on every enemy death (double
    /// energy + double draw — the long-standing `relic:GREMLIN_HORN`
    /// coverage-sweep divergence). Core-only unit tests could not catch it
    /// because the duplicate lived in a different crate; this test runs the
    /// FULL installed registry (core + content) via `ensure_installed`.
    #[test]
    fn gremlin_horn_registered_exactly_once_on_after_death() {
        use sts2_core::relic_effects::{
            lookup_after_damage_given, lookup_after_damage_received, lookup_after_death,
        };
        crate::relics::dispatch::ensure_installed();
        assert!(
            lookup_after_death(RelicId::GremlinHorn).is_some(),
            "GremlinHorn must register on the C#-faithful after_death bus",
        );
        assert!(
            lookup_after_damage_given(RelicId::GremlinHorn).is_none(),
            "GremlinHorn must NOT also be on after_damage_given (double-fire regression)",
        );
        assert!(
            lookup_after_damage_received(RelicId::GremlinHorn).is_none(),
            "GremlinHorn must NOT also be on after_damage_received (double-fire regression)",
        );
    }

    /// General guard for the GremlinHorn-class bug (bead `sts2-rs-gpsvo`): a
    /// relic registered on `after_death` AND on a damage channel double-fires
    /// on a killing blow (the kill hits the damage channel AND triggers the
    /// death channel). No relic should be on both. Runs the full registry so a
    /// future duplicate in EITHER crate fails here, in `cargo nextest`, rather
    /// than silently in a manual cross-engine sweep.
    #[test]
    fn no_relic_double_fires_on_death() {
        use sts2_core::relic_effects::{
            listeners_after_death, lookup_after_damage_given, lookup_after_damage_received,
        };
        crate::relics::dispatch::ensure_installed();
        for (id, _) in listeners_after_death() {
            assert!(
                lookup_after_damage_given(id).is_none()
                    && lookup_after_damage_received(id).is_none(),
                "{id:?} is registered on after_death AND a damage channel — it will \
                 double-fire on every killing blow (see gpsvo / GremlinHorn). Implement \
                 the relic on exactly one channel.",
            );
        }
    }

    #[test]
    fn horn_cleat_spec_pin() {
        let r = horn_cleat();
        assert_eq!(r.id, RelicKind::HornCleat);
        assert_eq!(r.name, "HORN_CLEAT");
        assert_eq!(r.name, RelicId::HornCleat.to_python_name());
        assert_eq!(HORN_CLEAT_BLOCK, 14);
    }

    #[test]
    fn joss_paper_spec_pin() {
        let r = joss_paper();
        assert_eq!(r.id, RelicKind::JossPaper);
        assert_eq!(r.name, "JOSS_PAPER");
        assert_eq!(r.name, RelicId::JossPaper.to_python_name());
        assert_eq!(JOSS_PAPER_CARDS, 1);
    }

    #[test]
    fn kusarigama_spec_pin() {
        let r = kusarigama();
        assert_eq!(r.id, RelicKind::Kusarigama);
        assert_eq!(r.name, "KUSARIGAMA");
        assert_eq!(r.name, RelicId::Kusarigama.to_python_name());
        assert_eq!(KUSARIGAMA_ATTACK_THRESHOLD, 3);
        assert_eq!(KUSARIGAMA_DAMAGE, 6);
    }

    // --- Spec-pin tests — L–Z wave (bead `sts2-rs-6ynt.5.6`) -------------

    macro_rules! l_z_spec_pin {
        ($test_name:ident, $factory:ident, $kind:ident, $name:literal) => {
            #[test]
            fn $test_name() {
                let r = $factory();
                assert_eq!(r.id, RelicKind::$kind);
                assert_eq!(r.name, $name);
                assert_eq!(r.rarity, RelicRarity::Uncommon);
                assert_eq!(r.effect, RelicEffect::Noop);
                assert_eq!(r.name, RelicId::$kind.to_python_name());
            }
        };
    }

    l_z_spec_pin!(
        letter_opener_spec_pin,
        letter_opener,
        LetterOpener,
        "LETTER_OPENER"
    );
    l_z_spec_pin!(lucky_fysh_spec_pin, lucky_fysh, LuckyFysh, "LUCKY_FYSH");
    l_z_spec_pin!(
        mercury_hourglass_spec_pin,
        mercury_hourglass,
        MercuryHourglass,
        "MERCURY_HOURGLASS"
    );
    l_z_spec_pin!(
        miniature_cannon_spec_pin,
        miniature_cannon,
        MiniatureCannon,
        "MINIATURE_CANNON"
    );
    l_z_spec_pin!(nunchaku_spec_pin, nunchaku, Nunchaku, "NUNCHAKU");
    l_z_spec_pin!(orichalcum_spec_pin, orichalcum, Orichalcum, "ORICHALCUM");
    l_z_spec_pin!(
        ornamental_fan_spec_pin,
        ornamental_fan,
        OrnamentalFan,
        "ORNAMENTAL_FAN"
    );
    l_z_spec_pin!(pantograph_spec_pin, pantograph, Pantograph, "PANTOGRAPH");
    l_z_spec_pin!(paper_phrog_spec_pin, paper_phrog, PaperPhrog, "PAPER_PHROG");
    l_z_spec_pin!(
        parrying_shield_spec_pin,
        parrying_shield,
        ParryingShield,
        "PARRYING_SHIELD"
    );
    // Pear's effect is `GainMaxHpOnObtained{PEAR_MAX_HP=10}` per
    // `sts2-rs-u8qa` — opts out of the `l_z_spec_pin` macro (which
    // asserts `RelicEffect::Noop`). Dedicated pin below mirrors
    // `rare.rs::mango_spec_pin`.
    #[test]
    fn pear_spec_pin() {
        let r = pear();
        assert_eq!(r.id, RelicKind::Pear);
        assert_eq!(r.name, "PEAR");
        assert_eq!(r.rarity, RelicRarity::Uncommon);
        assert_eq!(
            r.effect,
            RelicEffect::GainMaxHpOnObtained {
                amount: PEAR_MAX_HP,
            }
        );
        assert_eq!(r.name, RelicId::Pear.to_python_name());
    }
    l_z_spec_pin!(pen_nib_spec_pin, pen_nib, PenNib, "PEN_NIB");
    l_z_spec_pin!(
        petrified_toad_spec_pin,
        petrified_toad,
        PetrifiedToad,
        "PETRIFIED_TOAD"
    );
    l_z_spec_pin!(
        planisphere_spec_pin,
        planisphere,
        Planisphere,
        "PLANISPHERE"
    );
    // RedMask is `Common` per C# `RedMask.cs:15` / grab_bag `SHARED_COMMON`
    // (bead `sts2-rs-kw0z9`), so it opts out of the `l_z_spec_pin` macro
    // (which asserts `Uncommon`). This module still hosts its handler
    // registration.
    #[test]
    fn red_mask_spec_pin() {
        let r = red_mask();
        assert_eq!(r.id, RelicKind::RedMask);
        assert_eq!(r.name, "RED_MASK");
        assert_eq!(r.rarity, RelicRarity::Common);
        assert_eq!(r.effect, RelicEffect::Noop);
        assert_eq!(r.name, RelicId::RedMask.to_python_name());
    }
    l_z_spec_pin!(regalite_spec_pin, regalite, Regalite, "REGALITE");
    l_z_spec_pin!(
        reptile_trinket_spec_pin,
        reptile_trinket,
        ReptileTrinket,
        "REPTILE_TRINKET"
    );
    l_z_spec_pin!(
        ripple_basin_spec_pin,
        ripple_basin,
        RippleBasin,
        "RIPPLE_BASIN"
    );
    l_z_spec_pin!(
        self_forming_clay_spec_pin,
        self_forming_clay,
        SelfFormingClay,
        "SELF_FORMING_CLAY"
    );
    l_z_spec_pin!(
        sparkling_rouge_spec_pin,
        sparkling_rouge,
        SparklingRouge,
        "SPARKLING_ROUGE"
    );
    l_z_spec_pin!(
        stone_cracker_spec_pin,
        stone_cracker,
        StoneCracker,
        "STONE_CRACKER"
    );
    l_z_spec_pin!(
        symbiotic_virus_spec_pin,
        symbiotic_virus,
        SymbioticVirus,
        "SYMBIOTIC_VIRUS"
    );
    l_z_spec_pin!(tingsha_spec_pin, tingsha, Tingsha, "TINGSHA");
    l_z_spec_pin!(tuning_fork_spec_pin, tuning_fork, TuningFork, "TUNING_FORK");
    l_z_spec_pin!(
        twisted_funnel_spec_pin,
        twisted_funnel,
        TwistedFunnel,
        "TWISTED_FUNNEL"
    );
    l_z_spec_pin!(vambrace_spec_pin, vambrace, Vambrace, "VAMBRACE");

    /// Python constants are locked separately from the per-factory
    /// spec pins so the wiring sub-bead failing on a renamed constant
    /// surfaces at this test, not 26 of them.
    #[test]
    fn l_z_python_constants_match_pin() {
        assert_eq!(LETTER_OPENER_SKILL_THRESHOLD, 3);
        assert_eq!(LETTER_OPENER_DAMAGE, 5);
        assert_eq!(LUCKY_FYSH_GOLD, 15);
        assert_eq!(MERCURY_HOURGLASS_DAMAGE, 3);
        assert_eq!(MINIATURE_CANNON_EXTRA_DAMAGE, 3);
        assert_eq!(NUNCHAKU_ATTACK_THRESHOLD, 10);
        assert_eq!(NUNCHAKU_ENERGY, 1);
        assert_eq!(ORICHALCUM_BLOCK, 6);
        assert_eq!(ORNAMENTAL_FAN_ATTACK_THRESHOLD, 3);
        assert_eq!(ORNAMENTAL_FAN_BLOCK, 4);
        assert_eq!(PANTOGRAPH_HEAL, 25);
        assert_eq!(PAPER_PHROG_EXTRA_VULNERABLE_PCT, 25);
        assert_eq!(PARRYING_SHIELD_BLOCK_THRESHOLD, 10);
        assert_eq!(PARRYING_SHIELD_DAMAGE, 6);
        assert_eq!(PEAR_MAX_HP, 10);
        assert_eq!(PEN_NIB_THRESHOLD, 10);
        assert_eq!(PLANISPHERE_HEAL, 5);
        assert_eq!(RED_MASK_WEAK, 1);
        assert_eq!(REGALITE_BLOCK, 2);
        assert_eq!(REPTILE_TRINKET_STRENGTH, 3);
        assert_eq!(RIPPLE_BASIN_BLOCK, 4);
        assert_eq!(SELF_FORMING_CLAY_BLOCK_NEXT_TURN, 3);
        assert_eq!(SPARKLING_ROUGE_STRENGTH, 1);
        assert_eq!(SPARKLING_ROUGE_DEXTERITY, 1);
        assert_eq!(STONE_CRACKER_CARDS, 2);
        assert_eq!(SYMBIOTIC_VIRUS_DARK_ORBS, 1);
        assert_eq!(TINGSHA_DAMAGE, 3);
        assert_eq!(TUNING_FORK_SKILL_THRESHOLD, 10);
        assert_eq!(TUNING_FORK_BLOCK, 7);
        assert_eq!(TWISTED_FUNNEL_POISON, 4);
    }

    #[test]
    fn all_l_z_factories_are_pure() {
        assert_eq!(letter_opener(), letter_opener());
        assert_eq!(lucky_fysh(), lucky_fysh());
        assert_eq!(mercury_hourglass(), mercury_hourglass());
        assert_eq!(miniature_cannon(), miniature_cannon());
        assert_eq!(nunchaku(), nunchaku());
        assert_eq!(orichalcum(), orichalcum());
        assert_eq!(ornamental_fan(), ornamental_fan());
        assert_eq!(pantograph(), pantograph());
        assert_eq!(paper_phrog(), paper_phrog());
        assert_eq!(parrying_shield(), parrying_shield());
        assert_eq!(pear(), pear());
        assert_eq!(pen_nib(), pen_nib());
        assert_eq!(petrified_toad(), petrified_toad());
        assert_eq!(planisphere(), planisphere());
        assert_eq!(red_mask(), red_mask());
        assert_eq!(regalite(), regalite());
        assert_eq!(reptile_trinket(), reptile_trinket());
        assert_eq!(ripple_basin(), ripple_basin());
        assert_eq!(self_forming_clay(), self_forming_clay());
        assert_eq!(sparkling_rouge(), sparkling_rouge());
        assert_eq!(stone_cracker(), stone_cracker());
        assert_eq!(symbiotic_virus(), symbiotic_virus());
        assert_eq!(tingsha(), tingsha());
        assert_eq!(tuning_fork(), tuning_fork());
        assert_eq!(twisted_funnel(), twisted_funnel());
        assert_eq!(vambrace(), vambrace());
    }

    #[test]
    fn all_a_k_factories_are_pure() {
        // Spec factories are `const fn`, but `assert_eq` ensures the
        // returned data is value-equal across calls.
        assert_eq!(akabeko(), akabeko());
        assert_eq!(bag_of_marbles(), bag_of_marbles());
        assert_eq!(bellows(), bellows());
        assert_eq!(book_repair_knife(), book_repair_knife());
        assert_eq!(bowler_hat(), bowler_hat());
        assert_eq!(candelabra(), candelabra());
        assert_eq!(eternal_feather(), eternal_feather());
        assert_eq!(funerary_mask(), funerary_mask());
        assert_eq!(galactic_dust(), galactic_dust());
        assert_eq!(gold_plated_cables(), gold_plated_cables());
        assert_eq!(gremlin_horn(), gremlin_horn());
        assert_eq!(horn_cleat(), horn_cleat());
        assert_eq!(joss_paper(), joss_paper());
        assert_eq!(kusarigama(), kusarigama());
    }

    /// Every uncommon `RelicId` carries an upper-snake-case Python name.
    /// Locks the parity-output naming for the future exporter.
    #[test]
    fn every_uncommon_relic_has_python_name() {
        for &id in UNCOMMON_RELIC_IDS {
            let name = id.to_python_name();
            assert!(!name.is_empty(), "empty python_name for {id:?}");
            assert!(
                name.bytes()
                    .all(|b| b == b'_' || b.is_ascii_uppercase() || b.is_ascii_digit()),
                "non upper-snake-case python_name for {id:?}: {name:?}"
            );
        }
    }

    /// Bead `sts2-rs-u8qa`: Pear's `after_obtained` listener must add
    /// `MAX_HP=10` to the player's max HP (and current HP).
    #[test]
    fn listener_pear_gains_ten_max_hp_on_obtain() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        let before_max = rs.max_hp;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::Pear);
        rs.last_obtained_relic = Some(RelicId::Pear);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max + PEAR_MAX_HP);
        assert_eq!(rs.current_hp, before_hp + PEAR_MAX_HP);
    }

    // ── BowlerHat — bead b31s6 ────────────────────────────────────

    /// Bead `sts2-rs-b31s6` (corrected by `sts2-rs-oim25`): BowlerHat's
    /// two-listener wire grants 25% bonus on every positive gold gain
    /// (C# `GoldIncrease = 1.25m`). Pickup of 100 gold resolves to
    /// 100 + 25 = 125 final gold.
    #[test]
    fn listener_bowler_hat_grants_twenty_five_percent_bonus_on_positive_gain() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.gold = 0;
        rs.relics.push(RelicId::BowlerHat);
        rs.gain_gold(100);
        assert_eq!(rs.gold, 125, "100 + 25% bonus = 125");
    }

    /// Bonus is integer-floored — `gain_gold(3)` rolls bonus =
    /// `3 * 25 / 100 = 0`, no extra gold.
    #[test]
    fn listener_bowler_hat_floors_to_zero_bonus_for_small_gains() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.gold = 0;
        rs.relics.push(RelicId::BowlerHat);
        rs.gain_gold(3);
        assert_eq!(rs.gold, 3, "bonus floors to 0 below 4 gold");
    }

    /// Re-entry guard prevents geometric compounding — bonus fires
    /// once per outer gain_gold call, not on the bonus itself.
    #[test]
    fn listener_bowler_hat_bonus_does_not_compound() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.gold = 0;
        rs.relics.push(RelicId::BowlerHat);
        // 1000 → +250 bonus → if bonus compounded it'd recursively
        // grant +62 (and so on); the guard short-circuits the
        // recursion so the final total is exactly 1250.
        rs.gain_gold(1000);
        assert_eq!(rs.gold, 1250, "single-level bonus, no compounding");
        // Guard left clean for subsequent gains.
        assert!(!rs.bowler_hat_applying_bonus);
    }

    /// No-ownership: no bonus.
    #[test]
    fn listener_bowler_hat_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.gold = 0;
        rs.gain_gold(100);
        assert_eq!(rs.gold, 100, "no BowlerHat → no bonus");
    }

    /// Negative / zero delta short-circuits in `gain_gold` itself
    /// (before the channel fires) — BowlerHat sees nothing and the
    /// counter remains 0.
    #[test]
    fn listener_bowler_hat_skips_non_positive_gain() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.gold = 200;
        rs.relics.push(RelicId::BowlerHat);
        rs.gain_gold(-50);
        assert_eq!(rs.gold, 150);
        rs.gain_gold(0);
        assert_eq!(rs.gold, 150);
    }

    /// Parity-pin: the MULTIPLIER constant must match the Python
    /// canonical 0.2 expressed as integer percent.
    #[test]
    fn bowler_hat_multiplier_pct_parity() {
        assert_eq!(BOWLER_HAT_MULTIPLIER_PCT, 25);
    }

    /// Bead `sts2-rs-onyo`: Pantograph's `after_room_entered`
    /// listener heals `HEAL=25` on `RoomType::Boss` entry when the
    /// player is alive. Mirrors the Python pin `is_boss and
    /// current_hp > 0 → owner.heal(25)`.
    #[test]
    fn listener_pantograph_heals_twenty_five_on_boss_room_entry() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        // Drop the player below max so `heal` has room to move.
        rs.current_hp = rs.max_hp - 50;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::Pantograph);
        let _ = sts2_core::run_effects::dispatch_after_room_entered(
            &mut rs,
            sts2_core::runstate::RoomType::Boss,
        );
        assert_eq!(rs.current_hp, before_hp + PANTOGRAPH_HEAL);
    }

    /// No-op on non-boss room types (Monster, Elite, etc.).
    #[test]
    fn listener_pantograph_is_noop_on_non_boss_rooms() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.current_hp = rs.max_hp - 50;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::Pantograph);
        for rt in [
            sts2_core::runstate::RoomType::Monster,
            sts2_core::runstate::RoomType::Elite,
            sts2_core::runstate::RoomType::Shop,
            sts2_core::runstate::RoomType::RestSite,
            sts2_core::runstate::RoomType::Treasure,
            sts2_core::runstate::RoomType::Event,
        ] {
            let _ = sts2_core::run_effects::dispatch_after_room_entered(&mut rs, rt);
        }
        assert_eq!(rs.current_hp, before_hp);
    }

    /// No-op when the player is already dead. The Python pin's
    /// `current_hp > 0` gate matters because the runstate dispatcher
    /// fires `after_room_entered` even on revive-into-boss paths.
    #[test]
    fn listener_pantograph_is_noop_when_player_is_dead() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.current_hp = 0;
        rs.relics.push(RelicId::Pantograph);
        let _ = sts2_core::run_effects::dispatch_after_room_entered(
            &mut rs,
            sts2_core::runstate::RoomType::Boss,
        );
        assert_eq!(rs.current_hp, 0);
    }

    /// No-op when the relic is not yet owned.
    #[test]
    fn listener_pantograph_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.current_hp = rs.max_hp - 50;
        let before_hp = rs.current_hp;
        let _ = sts2_core::run_effects::dispatch_after_room_entered(
            &mut rs,
            sts2_core::runstate::RoomType::Boss,
        );
        assert_eq!(rs.current_hp, before_hp);
    }

    // -----------------------------------------------------------------
    // Planisphere listener tests — bead `sts2-rs-3wb0`
    // -----------------------------------------------------------------

    /// Happy path: Unknown room + alive + owned → +5 HP
    /// (C# `HealVar(5m)`, bead `sts2-rs-5ri31`).
    #[test]
    fn listener_planisphere_heals_five_on_unknown_room_entry() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.current_hp = rs.max_hp - 20;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::Planisphere);
        let _ = sts2_core::run_effects::dispatch_after_room_entered(
            &mut rs,
            sts2_core::runstate::RoomType::Unknown,
        );
        assert_eq!(rs.current_hp, before_hp + PLANISPHERE_HEAL);
    }

    /// No-op on non-Unknown room types.
    #[test]
    fn listener_planisphere_is_noop_on_other_rooms() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.current_hp = rs.max_hp - 20;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::Planisphere);
        for rt in [
            sts2_core::runstate::RoomType::Monster,
            sts2_core::runstate::RoomType::Elite,
            sts2_core::runstate::RoomType::Boss,
            sts2_core::runstate::RoomType::Shop,
            sts2_core::runstate::RoomType::RestSite,
            sts2_core::runstate::RoomType::Treasure,
            sts2_core::runstate::RoomType::Event,
        ] {
            let _ = sts2_core::run_effects::dispatch_after_room_entered(&mut rs, rt);
        }
        assert_eq!(rs.current_hp, before_hp);
    }

    /// No-op when the player is dead. Python's `current_hp <= 0`
    /// short-circuit.
    #[test]
    fn listener_planisphere_is_noop_when_player_is_dead() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.current_hp = 0;
        rs.relics.push(RelicId::Planisphere);
        let _ = sts2_core::run_effects::dispatch_after_room_entered(
            &mut rs,
            sts2_core::runstate::RoomType::Unknown,
        );
        assert_eq!(rs.current_hp, 0);
    }

    /// No-op when the relic is not owned.
    #[test]
    fn listener_planisphere_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        rs.current_hp = rs.max_hp - 20;
        let before_hp = rs.current_hp;
        let _ = sts2_core::run_effects::dispatch_after_room_entered(
            &mut rs,
            sts2_core::runstate::RoomType::Unknown,
        );
        assert_eq!(rs.current_hp, before_hp);
    }

    /// Bead `sts2-rs-9818`: LuckyFysh grants `LUCKY_FYSH_GOLD = 15` each
    /// time a card is added to the deck — no card-type gate (any add
    /// fires). Two sequential dispatches accumulate to +30 gold.
    #[test]
    fn listener_lucky_fysh_grants_fifteen_gold_per_card_added() {
        use sts2_core::cards::{CardId, CardType};
        use sts2_core::relic_effects::AddSource;
        use sts2_core::run_effects::{dispatch_on_card_added_to_deck, CardAddedRunCtx};
        use sts2_core::runstate::RunState;

        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_gold = rs.gold;
        rs.relics.push(RelicId::LuckyFysh);
        let ctx = CardAddedRunCtx {
            card_id: CardId::StrikeIronclad,
            card_type: CardType::Attack,
            source: AddSource::Reward,
        };
        let _ = dispatch_on_card_added_to_deck(&mut rs, &ctx);
        assert_eq!(rs.gold, before_gold + LUCKY_FYSH_GOLD);
        let _ = dispatch_on_card_added_to_deck(&mut rs, &ctx);
        assert_eq!(rs.gold, before_gold + 2 * LUCKY_FYSH_GOLD);
    }

    /// Bead `sts2-rs-9818`: LuckyFysh listener is a no-op when the
    /// player does not own the relic.
    #[test]
    fn listener_lucky_fysh_is_noop_without_ownership() {
        use sts2_core::cards::{CardId, CardType};
        use sts2_core::relic_effects::AddSource;
        use sts2_core::run_effects::{dispatch_on_card_added_to_deck, CardAddedRunCtx};
        use sts2_core::runstate::RunState;

        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_gold = rs.gold;
        let ctx = CardAddedRunCtx {
            card_id: CardId::StrikeIronclad,
            card_type: CardType::Attack,
            source: AddSource::Reward,
        };
        let _ = dispatch_on_card_added_to_deck(&mut rs, &ctx);
        assert_eq!(rs.gold, before_gold);
    }

    // -----------------------------------------------------------------
    // MercuryHourglass listener tests — bead `sts2-rs-g4ti`
    // -----------------------------------------------------------------
    //
    // Single-channel `after_side_turn_start` listener: on every
    // player-side turn-start, deal 3 unpowered damage to every alive
    // enemy. No latch, no round gate.

    #[cfg(test)]
    fn mercury_setup(state: &mut CombatState, equip: bool, enemy_hps: &[i32]) -> Vec<u32> {
        use smallvec::SmallVec;
        use std::num::NonZeroU32;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, Relic};
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::MercuryHourglass,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
        let mut uids = Vec::with_capacity(enemy_hps.len());
        let player_uid_raw = state.player.creature_uid.map_or(1, |u| u.0.get());
        for (i, &hp) in enemy_hps.iter().enumerate() {
            let raw = player_uid_raw + (i as u32) + 1;
            let id = NonZeroU32::new(raw).expect("non-zero");
            state.creatures.push(Creature {
                id,
                current_hp: hp,
                max_hp: hp.max(1),
                block: 0,
                powers: SmallVec::new(),
                species: Some(MonsterKind::Nibbit),
                monster_id: MonsterId(""),
                intent: None,
                intent_history: IntentHistory::default(),
                ai: None,
            });
            state.enemy_uids.push(CreatureUid(id));
            uids.push(raw);
        }
        uids
    }

    /// **MercuryHourglass #1** — happy path: every alive enemy takes
    /// 3 damage on player-side turn start.
    #[test]
    fn listener_mercury_hourglass_damages_all_alive_enemies() {
        use sts2_core::relic_effects::dispatch_after_player_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let uids = mercury_setup(&mut state, true, &[20, 30, 40]);
        let _ = dispatch_after_player_turn_start(&mut state);
        for raw in uids {
            let c = state
                .creatures
                .iter()
                .find(|c| c.id.get() == raw)
                .expect("enemy present");
            assert_eq!(c.max_hp - c.current_hp, MERCURY_HOURGLASS_DAMAGE);
        }
    }

    /// **MercuryHourglass #2** (bead `sts2-rs-axdgh`) — the relic moved off the
    /// `after_side_turn_start` channel onto `after_player_turn_start` (C#
    /// `AfterPlayerTurnStart`), so the SIDE hook no longer fires it on EITHER
    /// side. This guards against a regression that re-adds it there (which
    /// would double-fire alongside the player-turn-start dispatch).
    #[test]
    fn listener_mercury_hourglass_not_on_side_hook() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let _ = mercury_setup(&mut state, true, &[20]);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Enemy);
        let hp = state.creatures.last().expect("enemy").current_hp;
        assert_eq!(hp, 20, "side hook does not fire MercuryHourglass");
    }

    /// **MercuryHourglass #3** — no-op when the relic is not owned.
    #[test]
    fn listener_mercury_hourglass_is_noop_without_ownership() {
        use sts2_core::relic_effects::dispatch_after_player_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let _ = mercury_setup(&mut state, false, &[20]);
        let _ = dispatch_after_player_turn_start(&mut state);
        let hp = state.creatures.last().expect("enemy").current_hp;
        assert_eq!(hp, 20);
    }

    /// **MercuryHourglass #4** — listener resolves cleanly when
    /// there are zero alive enemies (no panic; the AoE helper
    /// gracefully returns).
    #[test]
    fn listener_mercury_hourglass_resolves_with_no_enemies() {
        use sts2_core::relic_effects::dispatch_after_player_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let _ = mercury_setup(&mut state, true, &[]);
        // Should not panic, should not mutate player.
        let before_hp = state.player.current_hp;
        let _ = dispatch_after_player_turn_start(&mut state);
        assert_eq!(state.player.current_hp, before_hp);
    }

    // -----------------------------------------------------------------
    // FuneraryMask listener tests — bead `sts2-rs-hnd8`
    // -----------------------------------------------------------------

    #[cfg(test)]
    fn funerary_mask_setup(state: &mut CombatState, equip: bool, round: u16) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        state.round = round;
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::FuneraryMask,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    fn count_soul_in_draw(state: &CombatState) -> usize {
        state
            .player
            .draw
            .iter()
            .filter(|uid| {
                let idx = uid.0.get() as usize - 1;
                state
                    .cards
                    .get(idx)
                    .is_some_and(|c| c.card_id == sts2_core::cards::CardId::Soul)
            })
            .count()
    }

    /// **FuneraryMask #1** — happy path: round-1 `before_hand_draw` adds
    /// `FUNERARY_MASK_CARDS` (= 3) Soul cards to the draw pile.
    #[test]
    fn listener_funerary_mask_round_one_player_inserts_three_souls() {
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        funerary_mask_setup(&mut state, true, 1);
        let before = count_soul_in_draw(&state);
        let _ = dispatch_before_hand_draw(&mut state);
        assert_eq!(
            count_soul_in_draw(&state) - before,
            usize::try_from(FUNERARY_MASK_CARDS).unwrap_or(0),
            "round-1 inserts 3 Souls"
        );
    }

    /// **FuneraryMask #2** — round-2 is gated out.
    #[test]
    fn listener_funerary_mask_round_two_no_souls() {
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        funerary_mask_setup(&mut state, true, 2);
        let before = count_soul_in_draw(&state);
        let _ = dispatch_before_hand_draw(&mut state);
        assert_eq!(
            count_soul_in_draw(&state),
            before,
            "round != 1 must not fire"
        );
    }

    /// **FuneraryMask #4** — no-op when relic not owned.
    #[test]
    fn listener_funerary_mask_no_souls_without_ownership() {
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        funerary_mask_setup(&mut state, false, 1);
        let before = count_soul_in_draw(&state);
        let _ = dispatch_before_hand_draw(&mut state);
        assert_eq!(count_soul_in_draw(&state), before, "no relic, no Souls");
    }

    // -----------------------------------------------------------------
    // Bellows listener tests — bead `sts2-rs-uo8t`
    // -----------------------------------------------------------------
    //
    // Round-1 player-side turn start upgrades every card in hand.
    // We push a few unupgraded cards into the hand, dispatch, and
    // assert each becomes upgraded. Other sides/rounds/no-ownership
    // are no-ops.

    #[cfg(test)]
    fn bellows_push_hand_card(state: &mut CombatState) -> sts2_core::CardUid {
        use std::num::NonZeroU32;
        use sts2_core::cards::{Card, CardId, CardTags, CardType, TargetType};
        let next_raw = state
            .cards
            .iter()
            .map(|c| c.uid.0.get())
            .max()
            .map_or(1, |m| m.saturating_add(1));
        let uid = sts2_core::CardUid(NonZeroU32::new(next_raw).expect("non-zero"));
        state.cards.push(Card {
            uid,
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            combat_vars: sts2_core::cards::CombatVars::new(),
            enchantments: sts2_core::cards::CardEnchantments::new(),
            tags: CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        state.player.hand.push(uid);
        uid
    }

    #[cfg(test)]
    fn bellows_setup(state: &mut CombatState, equip: bool, round: u16) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        state.round = round;
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::Bellows,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    #[cfg(test)]
    fn card_upgraded(state: &CombatState, uid: sts2_core::CardUid) -> bool {
        let idx = uid.0.get() as usize - 1;
        state.cards.get(idx).is_some_and(|c| c.upgraded)
    }

    /// **Bellows #1** — round-1 player turn start upgrades every hand card.
    #[test]
    fn listener_bellows_round_one_player_upgrades_hand() {
        use sts2_core::relic_effects::dispatch_after_player_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        bellows_setup(&mut state, true, 1);
        let a = bellows_push_hand_card(&mut state);
        let b = bellows_push_hand_card(&mut state);
        let c = bellows_push_hand_card(&mut state);
        let _ = dispatch_after_player_turn_start(&mut state);
        assert!(card_upgraded(&state, a));
        assert!(card_upgraded(&state, b));
        assert!(card_upgraded(&state, c));
    }

    /// **Bellows #1b** — a card that can't be upgraded stays as it is: C#
    /// `CardCmd.Upgrade` skips a card whose `IsUpgradable` is false
    /// (`CardCmd.cs:273`), so Bellows leaves Ascender's Bane (a Curse)
    /// un-upgraded while it upgrades the rest of the hand (A6c6).
    #[test]
    fn listener_bellows_leaves_ascenders_bane_unupgraded() {
        use sts2_core::relic_effects::dispatch_after_player_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        bellows_setup(&mut state, true, 1);
        let strike = bellows_push_hand_card(&mut state);
        let bane = bellows_push_hand_card(&mut state);
        {
            let c = &mut state.cards[bane.0.get() as usize - 1];
            c.card_id = sts2_core::cards::CardId::AscendersBane;
            c.card_type = sts2_core::cards::CardType::Curse;
            c.cost = -1;
            c.base_damage = 0;
        }
        let _ = dispatch_after_player_turn_start(&mut state);
        assert!(card_upgraded(&state, strike));
        assert!(
            !card_upgraded(&state, bane),
            "Ascender's Bane can't be upgraded"
        );
    }

    /// **Bellows #2** — round 2 inert.
    #[test]
    fn listener_bellows_round_two_no_upgrade() {
        use sts2_core::relic_effects::dispatch_after_player_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        bellows_setup(&mut state, true, 2);
        let a = bellows_push_hand_card(&mut state);
        let _ = dispatch_after_player_turn_start(&mut state);
        assert!(!card_upgraded(&state, a));
    }

    /// **Bellows #3** — moved OFF `after_side_turn_start`: that channel must
    /// no longer upgrade the hand (C# `Bellows.cs` is `AfterPlayerTurnStart`,
    /// which fires before FencingManual's `AfterSideTurnStart` forge).
    #[test]
    fn listener_bellows_not_on_after_side_turn_start() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        bellows_setup(&mut state, true, 1);
        let a = bellows_push_hand_card(&mut state);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert!(!card_upgraded(&state, a));
    }

    /// **Bellows #4** — no-op when relic not owned.
    #[test]
    fn listener_bellows_no_upgrade_without_ownership() {
        use sts2_core::relic_effects::dispatch_after_player_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        bellows_setup(&mut state, false, 1);
        let a = bellows_push_hand_card(&mut state);
        let _ = dispatch_after_player_turn_start(&mut state);
        assert!(!card_upgraded(&state, a));
    }

    // -----------------------------------------------------------------
    // MiniatureCannon listener tests — bead `sts2-rs-lr0y`
    // -----------------------------------------------------------------
    //
    // Single-channel `modify_damage_additive` listener: when the
    // player deals MOVE-tagged powered damage from an upgraded Attack
    // card, contribute +3. All other listeners are scoped by relic
    // ownership so the diff `equipped - unequipped` isolates
    // MiniatureCannon's contribution.

    #[cfg(test)]
    fn miniature_cannon_push_card(
        state: &mut CombatState,
        card_type: sts2_core::cards::CardType,
        upgraded: bool,
    ) -> sts2_core::CardUid {
        use std::num::NonZeroU32;
        use sts2_core::cards::{Card, CardId, CardTags, TargetType};
        let next_raw = state
            .cards
            .iter()
            .map(|c| c.uid.0.get())
            .max()
            .map_or(1, |m| m.saturating_add(1));
        let uid = sts2_core::CardUid(NonZeroU32::new(next_raw).expect("non-zero"));
        state.cards.push(Card {
            uid,
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type,
            target_type: TargetType::Enemy,
            upgraded,
            base_damage: 6,
            base_block: 0,
            combat_vars: sts2_core::cards::CombatVars::new(),
            enchantments: sts2_core::cards::CardEnchantments::new(),
            tags: CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        uid
    }

    #[cfg(test)]
    fn miniature_cannon_setup(state: &mut CombatState, equip: bool) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::MiniatureCannon,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    /// Dispatch the listener directly so we measure MiniatureCannon's
    /// contribution in isolation (the `dispatch_modify_damage_additive`
    /// bus sums across every listener, which would conflate other
    /// always-on contributions in a hand-rolled CombatState).
    #[cfg(test)]
    fn miniature_cannon_call_listener(
        state: &CombatState,
        ctx: &sts2_core::relic_effects::ModifyDamageCtx,
    ) -> i32 {
        crate::relics::dispatch::ensure_installed();
        super::listener_miniature_cannon(state, ctx)
    }

    /// **MiniatureCannon #1** — happy path: upgraded Attack by the
    /// player with MOVE props contributes +3.
    #[test]
    fn listener_miniature_cannon_adds_three_on_upgraded_attack() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::ModifyDamageCtx;
        let mut state = CombatState::default();
        miniature_cannon_setup(&mut state, true);
        let uid = miniature_cannon_push_card(&mut state, CardType::Attack, true);
        let card_id = state.cards[uid.0.get() as usize - 1].card_id;
        let ctx = ModifyDamageCtx {
            dealer: state.player.creature_uid,
            target: state.player.creature_uid.expect("player uid"),
            base_damage: 6,
            props: ValueProps::MOVE,
            card: Some(card_id),
            card_uid: Some(uid),
        };
        assert_eq!(
            miniature_cannon_call_listener(&state, &ctx),
            MINIATURE_CANNON_EXTRA_DAMAGE
        );
    }

    /// **MiniatureCannon #2** — non-upgraded Attack does not fire.
    #[test]
    fn listener_miniature_cannon_is_zero_on_non_upgraded_attack() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::ModifyDamageCtx;
        let mut state = CombatState::default();
        miniature_cannon_setup(&mut state, true);
        let uid = miniature_cannon_push_card(&mut state, CardType::Attack, false);
        let card_id = state.cards[uid.0.get() as usize - 1].card_id;
        let ctx = ModifyDamageCtx {
            dealer: state.player.creature_uid,
            target: state.player.creature_uid.expect("player uid"),
            base_damage: 6,
            props: ValueProps::MOVE,
            card: Some(card_id),
            card_uid: Some(uid),
        };
        assert_eq!(miniature_cannon_call_listener(&state, &ctx), 0);
    }

    /// **MiniatureCannon #3** — upgraded non-Attack (Skill) does not
    /// fire — Python's `card.card_type == ATTACK` gate.
    #[test]
    fn listener_miniature_cannon_is_zero_on_upgraded_skill() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::ModifyDamageCtx;
        let mut state = CombatState::default();
        miniature_cannon_setup(&mut state, true);
        let uid = miniature_cannon_push_card(&mut state, CardType::Skill, true);
        let card_id = state.cards[uid.0.get() as usize - 1].card_id;
        let ctx = ModifyDamageCtx {
            dealer: state.player.creature_uid,
            target: state.player.creature_uid.expect("player uid"),
            base_damage: 6,
            props: ValueProps::MOVE,
            card: Some(card_id),
            card_uid: Some(uid),
        };
        assert_eq!(miniature_cannon_call_listener(&state, &ctx), 0);
    }

    /// **MiniatureCannon #4** — UNPOWERED damage (e.g. environmental
    /// thorns, curse ticks) does not benefit from the +3.
    #[test]
    fn listener_miniature_cannon_is_zero_on_unpowered_damage() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::ModifyDamageCtx;
        let mut state = CombatState::default();
        miniature_cannon_setup(&mut state, true);
        let uid = miniature_cannon_push_card(&mut state, CardType::Attack, true);
        let card_id = state.cards[uid.0.get() as usize - 1].card_id;
        let ctx = ModifyDamageCtx {
            dealer: state.player.creature_uid,
            target: state.player.creature_uid.expect("player uid"),
            base_damage: 6,
            props: ValueProps::MOVE.union(ValueProps::UNPOWERED),
            card: Some(card_id),
            card_uid: Some(uid),
        };
        assert_eq!(miniature_cannon_call_listener(&state, &ctx), 0);
    }

    /// **MiniatureCannon #5** — when the dealer is not the player
    /// (e.g. enemy reflect damage), the listener returns 0 — Python's
    /// `dealer is owner` gate.
    #[test]
    fn listener_miniature_cannon_is_zero_when_dealer_is_not_player() {
        use std::num::NonZeroU32;
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::ModifyDamageCtx;
        use sts2_core::CreatureUid;
        let mut state = CombatState::default();
        miniature_cannon_setup(&mut state, true);
        let uid = miniature_cannon_push_card(&mut state, CardType::Attack, true);
        let card_id = state.cards[uid.0.get() as usize - 1].card_id;
        let fake_dealer = CreatureUid(NonZeroU32::new(99).expect("non-zero"));
        let ctx = ModifyDamageCtx {
            dealer: Some(fake_dealer),
            target: state.player.creature_uid.expect("player uid"),
            base_damage: 6,
            props: ValueProps::MOVE,
            card: Some(card_id),
            card_uid: Some(uid),
        };
        assert_eq!(miniature_cannon_call_listener(&state, &ctx), 0);
    }

    /// **MiniatureCannon #6** — no-op when the relic is not owned.
    #[test]
    fn listener_miniature_cannon_is_zero_without_ownership() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::ModifyDamageCtx;
        let mut state = CombatState::default();
        miniature_cannon_setup(&mut state, false);
        let uid = miniature_cannon_push_card(&mut state, CardType::Attack, true);
        let card_id = state.cards[uid.0.get() as usize - 1].card_id;
        let ctx = ModifyDamageCtx {
            dealer: state.player.creature_uid,
            target: state.player.creature_uid.expect("player uid"),
            base_damage: 6,
            props: ValueProps::MOVE,
            card: Some(card_id),
            card_uid: Some(uid),
        };
        assert_eq!(miniature_cannon_call_listener(&state, &ctx), 0);
    }

    /// **MiniatureCannon #7 (bead `sts2-rs-nwz1n`, parity-fuzz R3 lever 1)** —
    /// with duplicate card-ids the listener must read the upgrade flag off the
    /// PLAYED instance (`ctx.card_uid`), not the first same-id copy. Here a
    /// base copy is pushed FIRST (so the old `find(card_id == …)` would read
    /// its `upgraded == false` and drop the bonus) and the upgraded copy is the
    /// one actually played. The +3 must still fire. Repro of seed 9 (3
    /// TWIN_STRIKE copies; C#=24 vs the old Rust=16).
    #[test]
    fn listener_miniature_cannon_reads_played_instance_with_duplicate_ids() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::ModifyDamageCtx;
        let mut state = CombatState::default();
        miniature_cannon_setup(&mut state, true);
        // Base copy first in `state.cards` (the trap for `find(card_id == …)`).
        let _base_uid = miniature_cannon_push_card(&mut state, CardType::Attack, false);
        // Upgraded copy — the instance actually being played.
        let played_uid = miniature_cannon_push_card(&mut state, CardType::Attack, true);
        let card_id = state.cards[played_uid.0.get() as usize - 1].card_id;
        let ctx = ModifyDamageCtx {
            dealer: state.player.creature_uid,
            target: state.player.creature_uid.expect("player uid"),
            base_damage: 6,
            props: ValueProps::MOVE,
            card: Some(card_id),
            card_uid: Some(played_uid),
        };
        assert_eq!(
            miniature_cannon_call_listener(&state, &ctx),
            MINIATURE_CANNON_EXTRA_DAMAGE,
            "must read the played upgraded instance, not the first base copy"
        );
    }

    // -----------------------------------------------------------------
    // SymbioticVirus listener tests — bead `sts2-rs-70anj.2` (Wave 2)
    // -----------------------------------------------------------------
    //
    // Round-1 player-side turn start channels `DARK_ORBS = 1` Dark orb.
    // Other sides / other rounds / no-ownership must all be no-ops.

    #[cfg(test)]
    fn symbiotic_virus_setup(state: &mut CombatState, equip: bool, round: u16) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        state.round = round;
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::SymbioticVirus,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    fn count_dark_orbs(state: &CombatState) -> usize {
        state
            .player
            .orbs
            .slots
            .iter()
            .filter(|o| matches!(o, sts2_core::orbs::Orb::Dark { .. }))
            .count()
    }

    /// **SymbioticVirus #1** — happy path: round-1 player-side channels
    /// `SYMBIOTIC_VIRUS_DARK_ORBS` = 1 Dark orb onto the belt.
    #[test]
    fn listener_symbiotic_virus_round_one_player_channels_dark() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        symbiotic_virus_setup(&mut state, true, 1);
        let before = count_dark_orbs(&state);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(
            count_dark_orbs(&state) - before,
            usize::try_from(SYMBIOTIC_VIRUS_DARK_ORBS).unwrap_or(0),
            "round-1 channels 1 Dark orb",
        );
    }

    /// **SymbioticVirus #2** — round-2 player-side is gated out.
    #[test]
    fn listener_symbiotic_virus_round_two_no_orbs() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        symbiotic_virus_setup(&mut state, true, 2);
        let before = count_dark_orbs(&state);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(count_dark_orbs(&state), before, "round != 1 must not fire");
    }

    /// **SymbioticVirus #3** — enemy-side dispatch is a no-op on round 1.
    #[test]
    fn listener_symbiotic_virus_enemy_side_no_orbs() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        symbiotic_virus_setup(&mut state, true, 1);
        let before = count_dark_orbs(&state);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Enemy);
        assert_eq!(count_dark_orbs(&state), before, "Enemy side must not fire");
    }

    /// **SymbioticVirus #4** — no-op when relic not owned.
    #[test]
    fn listener_symbiotic_virus_no_orbs_without_ownership() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        symbiotic_virus_setup(&mut state, false, 1);
        let before = count_dark_orbs(&state);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(count_dark_orbs(&state), before, "no relic, no orbs");
    }

    // -----------------------------------------------------------------
    // ReptileTrinket listener tests — bead `sts2-rs-ji7m7`
    // -----------------------------------------------------------------

    #[cfg(test)]
    fn reptile_trinket_setup(state: &mut CombatState, equip: bool) -> sts2_core::CreatureUid {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let uid = state.init_player_creature();
        state.sync_player_creature();
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::ReptileTrinket,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
        uid
    }

    fn player_power_amount(
        state: &CombatState,
        uid: sts2_core::CreatureUid,
        kind: PowerKind,
    ) -> i32 {
        let Some(creature) = state.creatures.iter().find(|c| c.id == uid.0) else {
            return 0;
        };
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

    fn player_has_power(state: &CombatState, uid: sts2_core::CreatureUid, kind: PowerKind) -> bool {
        state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .is_some_and(|c| {
                c.powers.iter().any(|puid| {
                    state
                        .powers
                        .iter()
                        .any(|p| p.id == puid.0 && p.kind == kind)
                })
            })
    }

    /// **ReptileTrinket #1** — happy path: using a potion mid-combat
    /// folds +`REPTILE_TRINKET_STRENGTH` Strength into the player and
    /// registers the temporary-strength counter.
    #[test]
    fn listener_reptile_trinket_potion_use_grants_strength() {
        use sts2_core::potions::PotionKind;
        use sts2_core::relic_effects::dispatch_after_potion_used;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let uid = reptile_trinket_setup(&mut state, true);
        let before = player_power_amount(&state, uid, PowerKind::Strength);
        let _ = dispatch_after_potion_used(&mut state, PotionKind::BlockPotion, None);
        assert_eq!(
            player_power_amount(&state, uid, PowerKind::Strength) - before,
            REPTILE_TRINKET_STRENGTH,
            "potion use grants +REPTILE_TRINKET_STRENGTH Strength",
        );
        assert!(
            player_has_power(&state, uid, PowerKind::ReptileTrinket),
            "temporary-strength counter is registered",
        );
    }

    /// **ReptileTrinket #2** — turn-end reverts the temporary Strength
    /// and removes the counter (the `hooks::after_turn_end` arm).
    #[test]
    fn listener_reptile_trinket_turn_end_reverts() {
        use sts2_core::potions::PotionKind;
        use sts2_core::relic_effects::dispatch_after_potion_used;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let uid = reptile_trinket_setup(&mut state, true);
        let baseline = player_power_amount(&state, uid, PowerKind::Strength);
        let _ = dispatch_after_potion_used(&mut state, PotionKind::BlockPotion, None);
        sts2_core::hooks::dispatch_after_turn_end(
            &mut state,
            &sts2_core::hooks::TurnSideCtx { side: Side::Player },
        );
        assert_eq!(
            player_power_amount(&state, uid, PowerKind::Strength),
            baseline,
            "player-side turn end reverts the temporary Strength",
        );
        assert!(
            !player_has_power(&state, uid, PowerKind::ReptileTrinket),
            "temporary-strength counter is removed at turn end",
        );
    }

    /// **ReptileTrinket #3** — no-op when the relic is not owned.
    #[test]
    fn listener_reptile_trinket_no_strength_without_ownership() {
        use sts2_core::potions::PotionKind;
        use sts2_core::relic_effects::dispatch_after_potion_used;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let uid = reptile_trinket_setup(&mut state, false);
        let before = player_power_amount(&state, uid, PowerKind::Strength);
        let _ = dispatch_after_potion_used(&mut state, PotionKind::BlockPotion, None);
        assert_eq!(
            player_power_amount(&state, uid, PowerKind::Strength),
            before,
            "no relic, no Strength",
        );
    }

    // -----------------------------------------------------------------
    // PaperPhrog listener tests — bead `sts2-rs-0om8p`
    // -----------------------------------------------------------------

    /// Set up a player vs a single Vulnerable(2) enemy and return the
    /// enemy's `CreatureRef`. The damage pipeline applies Vulnerable's
    /// 1.5× to powered attacks targeting the enemy.
    #[cfg(test)]
    fn paper_phrog_vuln_enemy(state: &mut CombatState) -> CreatureRef {
        use std::num::NonZeroU32;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        let enemy_raw = state
            .creatures
            .iter()
            .map(|c| c.id.get())
            .max()
            .map_or(1, |m| m.saturating_add(1));
        let enemy_id = NonZeroU32::new(enemy_raw).expect("non-zero");
        state.creatures.push(sts2_core::Creature {
            id: enemy_id,
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: smallvec::smallvec![],
            species: Some(sts2_core::MonsterKind::Nibbit),
            intent: None,
            intent_history: sts2_core::IntentHistory::default(),
            monster_id: sts2_core::MonsterId("Nibbit"),
            ai: None,
        });
        let enemy_uid = sts2_core::CreatureUid(enemy_id);
        state.enemy_uids.push(enemy_uid);
        apply_power_to_creature(state, enemy_uid, PowerKind::Vulnerable, 2);
        CreatureRef::Enemy(enemy_uid)
    }

    /// **PaperPhrog #1** — a powered player attack against a Vulnerable
    /// enemy is amplified 1.5× → 1.75× when PaperPhrog is owned:
    /// `floor(6 × 1.5) = 9` → `floor(6 × 1.75) = 10`.
    #[test]
    fn listener_paper_phrog_bumps_vulnerable_multiplier() {
        use std::num::NonZeroU32;
        use sts2_core::hooks::{self, DamageCtx};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let enemy = paper_phrog_vuln_enemy(&mut state);
        let ctx = DamageCtx {
            dealer: CreatureRef::Player,
            target: enemy,
            props: ValueProps::MOVE,
        };
        assert_eq!(
            hooks::modify_damage(&state, 6, &ctx),
            9,
            "baseline Vulnerable 1.5× without PaperPhrog"
        );
        state.relics.push(sts2_core::Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::PaperPhrog,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        assert_eq!(
            hooks::modify_damage(&state, 6, &ctx),
            10,
            "PaperPhrog bumps Vulnerable to 1.75× (floor(6×1.75)=10)"
        );
    }

    /// **PaperPhrog #2** — no bonus on a non-Vulnerable target (the hook
    /// only fires inside the Vulnerable multiplier path).
    #[test]
    fn listener_paper_phrog_inert_without_vulnerable() {
        use std::num::NonZeroU32;
        use sts2_core::hooks::{self, DamageCtx};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        let enemy_id = NonZeroU32::new(2).expect("non-zero");
        state.creatures.push(sts2_core::Creature {
            id: enemy_id,
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: smallvec::smallvec![],
            species: Some(sts2_core::MonsterKind::Nibbit),
            intent: None,
            intent_history: sts2_core::IntentHistory::default(),
            monster_id: sts2_core::MonsterId("Nibbit"),
            ai: None,
        });
        let enemy_uid = sts2_core::CreatureUid(enemy_id);
        state.enemy_uids.push(enemy_uid);
        state.relics.push(sts2_core::Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::PaperPhrog,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let ctx = DamageCtx {
            dealer: CreatureRef::Player,
            target: CreatureRef::Enemy(enemy_uid),
            props: ValueProps::MOVE,
        };
        assert_eq!(
            hooks::modify_damage(&state, 6, &ctx),
            6,
            "no Vulnerable → no amplification, PaperPhrog contributes nothing"
        );
    }

    /// **PaperPhrog #3** — the listener returns 0 on an unpowered hit and
    /// when the player does not own the relic.
    #[test]
    fn listener_paper_phrog_gates() {
        use std::num::NonZeroU32;
        use sts2_core::hooks::{self, DamageCtx};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let enemy = paper_phrog_vuln_enemy(&mut state);
        state.relics.push(sts2_core::Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::PaperPhrog,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        // Unpowered hit: Vulnerable itself does not fire, so 6 stays 6.
        let unpowered = DamageCtx {
            dealer: CreatureRef::Player,
            target: enemy,
            props: ValueProps::MOVE.union(ValueProps::UNPOWERED),
        };
        assert_eq!(hooks::modify_damage(&state, 6, &unpowered), 6);
    }

    // -----------------------------------------------------------------
    // Regalite listener tests — bead `sts2-rs-u2jxd` (70anj.5.regalite)
    // -----------------------------------------------------------------

    #[cfg(test)]
    fn regalite_setup(state: &mut CombatState, equip: bool) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::Regalite,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    #[cfg(test)]
    fn push_card_with_id(state: &mut CombatState, card_id: sts2_core::cards::CardId) -> CardUid {
        use std::num::NonZeroU32;
        use sts2_core::cards::{Card, CardTags, CardType, TargetType};
        let next_raw = state
            .cards
            .iter()
            .map(|c| c.uid.0.get())
            .max()
            .map_or(1, |m| m.saturating_add(1));
        let uid = CardUid(NonZeroU32::new(next_raw).expect("non-zero"));
        state.cards.push(Card {
            uid,
            card_id,
            cost: 1,
            card_type: CardType::Skill,
            target_type: TargetType::None,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            combat_vars: sts2_core::cards::CombatVars::new(),
            enchantments: sts2_core::cards::CardEnchantments::new(),
            tags: CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        uid
    }

    /// **Regalite #1** — happy path: a colorless card entering combat
    /// grants `REGALITE_BLOCK` (= 2) to the player.
    #[test]
    fn listener_regalite_colorless_grants_block() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_after_card_entered_combat;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        regalite_setup(&mut state, true);
        let uid = push_card_with_id(&mut state, CardId::DarkShackles);
        let before = state.player.block;
        let _ = dispatch_after_card_entered_combat(&mut state, uid, true);
        assert_eq!(
            state.player.block - before,
            REGALITE_BLOCK,
            "colorless spawn grants {REGALITE_BLOCK} block"
        );
    }

    /// **Regalite #2** — ANY owned card entering combat grants block,
    /// including non-colorless cards (StrikeIronclad). C# `Regalite.cs:16-20`
    /// `AfterCardEnteredCombat` gates only on `card.Owner == base.Owner`;
    /// the prior `is_colorless_card_id` gate was a Python-pin fabrication
    /// (`uncommon.py::Regalite`). q6bix coverage-sweep fix — a SLIMED (a
    /// non-colorless status) entering combat granted 2 block in C# but 0
    /// in Rust (REGALITE hp cs=46 rust=44).
    #[test]
    fn listener_regalite_any_owned_card_grants_block() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_after_card_entered_combat;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        regalite_setup(&mut state, true);
        let uid = push_card_with_id(&mut state, CardId::StrikeIronclad);
        let before = state.player.block;
        let _ = dispatch_after_card_entered_combat(&mut state, uid, true);
        assert_eq!(
            state.player.block - before,
            REGALITE_BLOCK,
            "any owned card entering combat grants {REGALITE_BLOCK} block (no color gate)"
        );
    }

    /// **Regalite #3** — no-op when relic not owned, even on a
    /// colorless spawn.
    #[test]
    fn listener_regalite_no_block_without_ownership() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_after_card_entered_combat;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        regalite_setup(&mut state, false);
        let uid = push_card_with_id(&mut state, CardId::DarkShackles);
        let before = state.player.block;
        let _ = dispatch_after_card_entered_combat(&mut state, uid, true);
        assert_eq!(state.player.block, before, "no relic, no block");
    }

    /// **Regalite #5** — an ENEMY-generated card (`added_by_player == false`,
    /// C# `creator == null`, e.g. Aeonglass `WitheringPresence`'s Wither) grants
    /// NO block, even with the relic owned. Bead `sts2-rs-regalite-creator`.
    #[test]
    fn listener_regalite_no_block_for_enemy_generated_card() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_after_card_entered_combat;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        regalite_setup(&mut state, true);
        let uid = push_card_with_id(&mut state, CardId::Wither);
        let before = state.player.block;
        let _ = dispatch_after_card_entered_combat(&mut state, uid, false);
        assert_eq!(
            state.player.block, before,
            "enemy-generated (creator==null) card grants no Regalite block"
        );
    }

    /// **Regalite #4** — covers the colorless-modifier sub-pool
    /// (Finesse is in `eligible_colorless_modifier_card_ids`), to
    /// guard against an `is_colorless_card_id` match that drops the
    /// modifier subset on a future regen.
    #[test]
    fn listener_regalite_colorless_modifier_grants_block() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_after_card_entered_combat;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        regalite_setup(&mut state, true);
        let uid = push_card_with_id(&mut state, CardId::Finesse);
        let before = state.player.block;
        let _ = dispatch_after_card_entered_combat(&mut state, uid, true);
        assert_eq!(
            state.player.block - before,
            REGALITE_BLOCK,
            "Finesse (modifier-pool colorless) grants block"
        );
    }

    // -----------------------------------------------------------------
    // EternalFeather listener tests
    // -----------------------------------------------------------------

    #[cfg(test)]
    fn eternal_feather_setup(rs: &mut RunState, equip: bool, deck_size: usize) {
        use sts2_core::cards::Card;
        rs.current_hp = 50;
        rs.max_hp = 80;
        if equip {
            rs.relics.push(RelicId::EternalFeather);
        }
        for _ in 0..deck_size {
            rs.master_deck.push(Card::pod_defaults());
        }
    }

    /// **EternalFeather #1** — happy path: 25-card deck at rest site,
    /// heal = `3 * (25 // 5) = 15`.
    #[test]
    fn listener_eternal_feather_rest_site_heals() {
        use sts2_core::run_effects::dispatch_after_room_entered;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        eternal_feather_setup(&mut rs, true, 25);
        let before = rs.current_hp;
        let _ = dispatch_after_room_entered(&mut rs, RoomType::RestSite);
        assert_eq!(rs.current_hp - before, 15, "3 * (25 // 5) = 15 heal");
    }

    /// **EternalFeather #2** — non-rest-site room is a no-op.
    #[test]
    fn listener_eternal_feather_non_rest_no_op() {
        use sts2_core::run_effects::dispatch_after_room_entered;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        eternal_feather_setup(&mut rs, true, 25);
        let before = rs.current_hp;
        let _ = dispatch_after_room_entered(&mut rs, RoomType::Shop);
        assert_eq!(rs.current_hp, before, "Shop room must not heal");
    }

    /// **EternalFeather #3** — not owned: no-op.
    #[test]
    fn listener_eternal_feather_no_relic_no_op() {
        use sts2_core::run_effects::dispatch_after_room_entered;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        eternal_feather_setup(&mut rs, false, 25);
        let before = rs.current_hp;
        let _ = dispatch_after_room_entered(&mut rs, RoomType::RestSite);
        assert_eq!(rs.current_hp, before, "no relic, no heal");
    }

    /// **EternalFeather #4** — small deck (<5 cards) yields heal=0,
    /// silent skip.
    #[test]
    fn listener_eternal_feather_small_deck_no_heal() {
        use sts2_core::run_effects::dispatch_after_room_entered;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        eternal_feather_setup(&mut rs, true, 4);
        let before = rs.current_hp;
        let _ = dispatch_after_room_entered(&mut rs, RoomType::RestSite);
        assert_eq!(rs.current_hp, before, "4-card deck: 3 * (4 // 5) = 0");
    }

    // -----------------------------------------------------------------
    // StoneCracker listener tests
    // -----------------------------------------------------------------

    #[cfg(test)]
    fn stone_cracker_setup(state: &mut CombatState, equip: bool, is_boss: bool) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        state.is_boss = is_boss;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::StoneCracker,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    /// Push a Strike card (unupgraded) into the draw pile and return uid.
    #[cfg(test)]
    fn push_strike_to_draw(state: &mut CombatState) -> CardUid {
        use sts2_core::cards::CardId;
        let uid = push_card_with_id(state, CardId::StrikeIronclad);
        state.player.draw.push(uid);
        uid
    }

    /// **StoneCracker #1** — happy path: boss + owned + 5 cards in draw
    /// → exactly `STONE_CRACKER_CARDS` (= 2, C# `CardsVar(2)`)
    /// upgrades.
    #[test]
    fn listener_stone_cracker_boss_owned_upgrades_two() {
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        stone_cracker_setup(&mut state, true, true);
        for _ in 0..5 {
            push_strike_to_draw(&mut state);
        }
        let _ = dispatch_before_combat_start(&mut state);
        let upgraded_count = state
            .player
            .draw
            .iter()
            .filter(|uid| {
                let idx = uid.0.get() as usize - 1;
                state.cards[idx].upgraded
            })
            .count();
        assert_eq!(
            upgraded_count, STONE_CRACKER_CARDS as usize,
            "boss + owned: exactly {STONE_CRACKER_CARDS} cards upgraded"
        );
    }

    /// **StoneCracker #2** — bead `sts2-rs-3r6hc`: non-boss combat
    /// ALSO fires (C# `room is CombatRoom` covers normal / elite /
    /// boss). Was previously asserting no-op under the spurious
    /// `state.is_boss` gate.
    #[test]
    fn listener_stone_cracker_non_boss_combat_still_upgrades_two() {
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        stone_cracker_setup(&mut state, true, false);
        for _ in 0..5 {
            push_strike_to_draw(&mut state);
        }
        let _ = dispatch_before_combat_start(&mut state);
        let upgraded_count = state
            .player
            .draw
            .iter()
            .filter(|uid| {
                let idx = uid.0.get() as usize - 1;
                state.cards[idx].upgraded
            })
            .count();
        assert_eq!(
            upgraded_count, STONE_CRACKER_CARDS as usize,
            "non-boss combat: still upgrades {STONE_CRACKER_CARDS} cards (C# fires for any CombatRoom)"
        );
    }

    /// **StoneCracker #3** — not owned: no-op even in boss room.
    #[test]
    fn listener_stone_cracker_no_relic_no_op() {
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        stone_cracker_setup(&mut state, false, true);
        for _ in 0..5 {
            push_strike_to_draw(&mut state);
        }
        let _ = dispatch_before_combat_start(&mut state);
        let upgraded_count = state
            .player
            .draw
            .iter()
            .filter(|uid| {
                let idx = uid.0.get() as usize - 1;
                state.cards[idx].upgraded
            })
            .count();
        assert_eq!(upgraded_count, 0, "no relic, no upgrades");
    }

    /// **StoneCracker #4** — already-upgraded cards are skipped; only
    /// not-upgraded ones count toward the `min(CARDS, candidates.len())`.
    #[test]
    fn listener_stone_cracker_skips_already_upgraded() {
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        stone_cracker_setup(&mut state, true, true);
        // 2 unupgraded + 3 pre-upgraded → only 2 new upgrades possible.
        let unupgraded: Vec<CardUid> = (0..2).map(|_| push_strike_to_draw(&mut state)).collect();
        for _ in 0..3 {
            let uid = push_strike_to_draw(&mut state);
            let idx = uid.0.get() as usize - 1;
            state.cards[idx].upgraded = true;
        }
        let _ = dispatch_before_combat_start(&mut state);
        for uid in unupgraded {
            let idx = uid.0.get() as usize - 1;
            assert!(state.cards[idx].upgraded, "the 2 unupgraded must be picked");
        }
    }

    // -----------------------------------------------------------------
    // GoldPlatedCables listener tests
    // -----------------------------------------------------------------
    // After-side-turn-start arm fires Plasma's +1 energy iff front orb
    // IS Plasma; before-turn-end arm fires the non-Plasma passive iff
    // front orb is filled and is NOT Plasma. Both gate on player side
    // + relic ownership.

    #[cfg(test)]
    fn gold_plated_cables_setup(state: &mut CombatState, equip: bool) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        state.player.energy = 0;
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::GoldPlatedCables,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    /// **GoldPlatedCables #1** — happy path: Plasma front orb on
    /// player-side turn start fires `+1 energy`.
    #[test]
    fn gold_plated_cables_plasma_grants_energy_on_player_turn_start() {
        use sts2_core::orbs::Orb;
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        gold_plated_cables_setup(&mut state, true);
        sts2_core::orbs::channel_orb(&mut state, Orb::Plasma);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.player.energy, 1, "Plasma fires +1 energy");
    }

    /// **GoldPlatedCables #2** — non-Plasma front orb on after-side-
    /// turn-start is a no-op (that arm only fires for Plasma).
    #[test]
    fn gold_plated_cables_lightning_inert_at_turn_start_arm() {
        use sts2_core::orbs::Orb;
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        gold_plated_cables_setup(&mut state, true);
        // Single enemy so Lightning would otherwise deal damage if it
        // mistakenly fired here.
        let enemy = create_test_enemy(&mut state, 50);
        sts2_core::orbs::channel_orb(&mut state, Orb::Lightning);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        let hp = state
            .creatures
            .iter()
            .find(|c| c.id.get() == enemy)
            .expect("enemy")
            .current_hp;
        assert_eq!(hp, 50, "Lightning must NOT fire on the turn-start arm");
        assert_eq!(state.player.energy, 0, "no Plasma → no energy");
    }

    /// **GoldPlatedCables #3** — happy path: Lightning front orb on
    /// player-side turn end deals an extra `3 + focus` to a random
    /// enemy.
    #[test]
    fn gold_plated_cables_lightning_damages_on_player_turn_end() {
        use sts2_core::orbs::Orb;
        use sts2_core::relic_effects::dispatch_before_turn_end;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        gold_plated_cables_setup(&mut state, true);
        let enemy = create_test_enemy(&mut state, 50);
        sts2_core::orbs::channel_orb(&mut state, Orb::Lightning);
        let _ = dispatch_before_turn_end(&mut state, Side::Player);
        let hp = state
            .creatures
            .iter()
            .find(|c| c.id.get() == enemy)
            .expect("enemy")
            .current_hp;
        assert_eq!(hp, 47, "Lightning re-fire deals 3 damage");
    }

    /// **GoldPlatedCables #4** — Plasma at turn-end is a no-op (the
    /// `plasma=False` arm filters it out).
    #[test]
    fn gold_plated_cables_plasma_inert_at_turn_end_arm() {
        use sts2_core::orbs::Orb;
        use sts2_core::relic_effects::dispatch_before_turn_end;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        gold_plated_cables_setup(&mut state, true);
        sts2_core::orbs::channel_orb(&mut state, Orb::Plasma);
        let _ = dispatch_before_turn_end(&mut state, Side::Player);
        assert_eq!(
            state.player.energy, 0,
            "Plasma must NOT fire on the turn-end arm"
        );
    }

    /// **GoldPlatedCables #5** — enemy-side dispatch is a no-op on
    /// both arms (gated on `side == owner.side`).
    #[test]
    fn gold_plated_cables_enemy_side_inert() {
        use sts2_core::orbs::Orb;
        use sts2_core::relic_effects::{dispatch_after_side_turn_start, dispatch_before_turn_end};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        gold_plated_cables_setup(&mut state, true);
        sts2_core::orbs::channel_orb(&mut state, Orb::Plasma);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Enemy);
        let _ = dispatch_before_turn_end(&mut state, Side::Enemy);
        assert_eq!(state.player.energy, 0, "enemy-side fires nothing");
    }

    /// **GoldPlatedCables #6** — no ownership ⇒ both arms are no-ops
    /// even with a Plasma / Lightning front orb.
    #[test]
    fn gold_plated_cables_no_ownership_inert() {
        use sts2_core::orbs::Orb;
        use sts2_core::relic_effects::{dispatch_after_side_turn_start, dispatch_before_turn_end};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        // GoldPlatedCables is a Defect relic — Defect player gets the 3-slot
        // belt, so the Plasma + Lightning both fit without overflow-evoking the
        // Plasma (which would otherwise add +1 energy and mask the
        // no-ownership assertion; bead sts2-rs-dnxjt).
        state.player.character = Some(sts2_core::potions::CharacterId::Defect);
        gold_plated_cables_setup(&mut state, false);
        let enemy = create_test_enemy(&mut state, 50);
        sts2_core::orbs::channel_orb(&mut state, Orb::Plasma);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        sts2_core::orbs::channel_orb(&mut state, Orb::Lightning);
        let _ = dispatch_before_turn_end(&mut state, Side::Player);
        assert_eq!(state.player.energy, 0);
        let hp = state
            .creatures
            .iter()
            .find(|c| c.id.get() == enemy)
            .expect("enemy")
            .current_hp;
        assert_eq!(hp, 50);
    }

    /// **GoldPlatedCables #7** — empty belt is a no-op (no panic, no
    /// energy gain).
    #[test]
    fn gold_plated_cables_empty_belt_inert() {
        use sts2_core::relic_effects::{dispatch_after_side_turn_start, dispatch_before_turn_end};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        gold_plated_cables_setup(&mut state, true);
        // No orbs channelled — every slot stays Empty.
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        let _ = dispatch_before_turn_end(&mut state, Side::Player);
        assert_eq!(state.player.energy, 0);
    }

    /// Test helper: spawn an alive enemy with the given hp + return
    /// its raw uid. Same Creature-construction idiom as `mercury_setup`.
    #[cfg(test)]
    fn create_test_enemy(state: &mut CombatState, hp: i32) -> u32 {
        use smallvec::SmallVec;
        use std::num::NonZeroU32;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind};
        let player_uid_raw = state.player.creature_uid.map_or(1, |u| u.0.get());
        let raw = player_uid_raw + (state.enemy_uids.len() as u32) + 1;
        let id = NonZeroU32::new(raw).expect("non-zero");
        state.creatures.push(Creature {
            id,
            current_hp: hp,
            max_hp: hp.max(1),
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            monster_id: MonsterId(""),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(id));
        raw
    }
}
