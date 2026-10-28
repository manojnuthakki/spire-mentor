// Doc comments use upper-snake-case DSL state names (e.g. `CURSE_OF_KNOWLEDGE`,
// `LIQUIFY_GROUND`) that clippy flags as missing-backticks identifiers. They
// are state-id strings, not Rust paths.
#![allow(clippy::doc_markdown)]

//! Act-2 monster ports (bead sts2-rs-txk).
//!
//! Ported from sts2-rl-agent@01c094d:sts2_env/monsters/act2.py
//!
//! Ports the 27 monsters from
//! `sts2-rl-agent@01c094d:sts2_env/monsters/act2.py` (772 LOC) to
//! data-driven `MonsterAi` instances using the DSL skeleton from P5-1
//! (`sts2-rs-gqy`). Conventions match [`super::act1_overgrowth`]:
//!
//! - Effect bodies are no-ops; intent metadata is preserved.
//! - Constructors return `MonsterAi` only (not `(Creature, MonsterAi)`)
//!   until the dispatcher integration bead lands.
//! - `Intent::Summon` landed via bead `sts2-rs-47p5` and is wired here
//!   for Ovicopter LAY_EGGS (bead `sts2-rs-r9pl`). (LouseProgenitor's
//!   former SPAWN summon was a Python-pin fabrication, removed under
//!   bead `sts2-rs-ybtmu`; C# LouseProgenitor never summons.)
//! - `Intent::Heal` landed via bead `sts2-rs-va5r` and is wired here
//!   for DecimillipedeSegment `REATTACH`. The sibling `DEAD` state
//!   telegraphs nothing at all: C# constructs `DEAD_MOVE` with an
//!   empty `params AbstractIntent[]` (`DecimillipedeSegment.cs:152`).
//!   Python's `IntentType.UNKNOWN` at `act2.py:487` was a fabrication.
//!
//! # ConditionalBranchState predicate-evaluation contract
//!
//! `ConditionFn` is an enum with three variants:
//! `Stateless(fn(&CombatState) -> bool)`,
//! `OwnerAware(fn(&CombatState, CreatureUid) -> bool)` (bead `sts2-rs-msi1`), and
//! `CounterAware(fn(&CombatState, CreatureUid, i32) -> bool)` (bead `sts2-rs-976k`).
//! Together they cover the per-creature predicates Python expresses
//! via `lambda: _state["..."] < 3` closures. Content beads pick a
//! pattern by predicate shape:
//!
//! 1. **Stateless** — combat-global only. Read a field directly on
//!    `CombatState` (e.g. `combat.round`, `combat.enemy_uids.len()`).
//! 2. **OwnerAware** — read per-creature state via the owning uid
//!    (e.g. CeremonialBeast PLOW power amount).
//! 3. **CounterAware** — read the per-monster `i32` counter
//!    (`MonsterAi::counter`) alongside owner. KnowledgeDemon's
//!    `CURSE_CHECK` uses this (bead `sts2-rs-w4w7`):
//!    `counter < 3` is the parity port of Python's
//!    `_state["curse_counter"] < 3`. The counter increments inside
//!    CURSE_OF_KNOWLEDGE's `effect_fn`, so Stun-skipping the curse
//!    intent correctly delays Phase-2 escalation.
//!
//! Both patterns deliberately give up the multi-instance precision of
//! the Python `_state` capture. The trade-off — function-pointer
//! predicates are `Clone`-able, `Send`-safe, and pure — is acceptable
//! because:
//!
//! - The state machine is rolled per turn, not per instance: the
//!   coordinator already iterates over enemies and applies the rolled
//!   intent against the correct creature.
//! - For HP predicates the convention "first enemy is the boss" matches
//!   every Act-2..4 boss encounter in `sts2-rl-agent`.
//! - Test parity is preserved because tests construct a single-boss
//!   `CombatState` whose `round` / first-enemy HP they manipulate
//!   directly.

use std::collections::BTreeMap;

use smallvec::smallvec;
use sts2_core::ascension::{self, AscensionLevel};
use sts2_core::card_effects::{apply_power_to_creature, EffectResult};
use sts2_core::monster_ai::{
    ConditionFn, ConditionalBranch, ConditionalBranchState, MonsterAi, MonsterState,
    MoveRepeatType, MoveState, RandomBranchState,
};
use sts2_core::monsters::Intent;
use sts2_core::powers::PowerKind;
use sts2_core::{CombatState, CreatureKind, CreatureUid, MonsterKind};

use super::apply_power_to_player_via_legacy_surface;
use super::dsl::{always_true, br, cb, mv, mv_eff, proto_ai, proto_ais, ProtoAis};

// ─────────────────────────────────────────────────────────────────────────────
// Telegraph-only Buff/Debuff power-apply effects (bead sts2-rs-pdsdl).
//
// `perform_intent` runs ONLY the per-move `effect_fn` for non-attack intents;
// `Intent::Buff`/`Intent::Debuff` are telegraph metadata and apply NO power by
// themselves. The moves below declared a Buff/Debuff intent but ran
// `noop_effect`, silently applying nothing. Each effect mirrors the exact
// `PowerCmd.Apply<…>` from the authoritative C# decomp
// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/`).
//
// SELF-buffs route through `apply_power_to_creature(state, owner, …)`.
// PLAYER-debuffs route through `apply_power_to_player_via_legacy_surface`
// (mirrors act1_overgrowth — the Python parity harness reads
// `state.player.powers`, not the d03 mirror Creature).
// ─────────────────────────────────────────────────────────────────────────────

/// `Tunneler` BURROW_MOVE — `Tunneler.cs:80`
/// `PowerCmd.Apply<BurrowedPower>(base.Creature, 1m, …)` self-buff.
fn tunneler_burrow_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_power_to_creature(state, owner, PowerKind::Burrowed, 1);
    // Bead `sts2-rs-fzp1y`: C# `Tunneler.cs:78-83` `BurrowMove` also
    // does `GainBlock(BlockGain=32)` to self after the Burrowed apply.
    // The telegraph `Intent::Defend { block: 32 }` is dropped by
    // `perform_intent` because it is the SECOND intent (Buff is
    // first), so the block has to live here. 37 at ToughEnemies.
    let block = crate::ascension_scaling::tough_block(
        state.ascension,
        MonsterKind::Tunneler,
        "BURROW_MOVE",
        32,
    );
    gain_secondary_defend_block(state, owner, block);
}

/// `BowlbugNectar` BUFF — `BowlbugNectar.cs:68`
/// `PowerCmd.Apply<StrengthPower>(base.Creature, BuffStrengthGain=15, …)`
/// self-buff (Ascension-16 bumps to 16; identity at pin 01c094d).
fn bowlbug_nectar_buff_effect(state: &mut CombatState, owner: CreatureUid) {
    let amount =
        ascension::get_value_if_ascension(state.ascension, AscensionLevel::DeadlyEnemies, 16, 15);
    apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// `BowlbugSilk` TOXIC_SPIT — `BowlbugSilk.cs:84`
/// `PowerCmd.Apply<WeakPower>(targets, 1m, …)` player-debuff.
fn bowlbug_silk_toxic_spit_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 1);
}

/// `Exoskeleton` ENRAGE — `Exoskeleton.cs:98`
/// `PowerCmd.Apply<StrengthPower>(base.Creature, 2m, …)` self-buff.
fn exoskeleton_enrage_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// `TheObscura` SAIL_MOVE (`WailMove`, `TheObscura.cs:89-94`):
/// `PowerCmd.Apply<StrengthPower>(GetTeammatesOf(self), 3m, …)`.
/// `GetTeammatesOf` resolves to `GetCreaturesOnSide(Enemy)`
/// (`CombatState.cs:496-512`), i.e. ALL living enemy-side creatures
/// *including self* — so Obscura and every spawned Parafright each
/// gain Strength +3. Bead `sts2-rs-0q46p` (w1kh follow-up: the
/// `Intent::Buff{Strength,3}` telegraph was previously a `mv()` noop
/// because the teammate-target widening was deferred).
fn the_obscura_sail_effect(state: &mut CombatState, _owner: CreatureUid) {
    let targets: Vec<CreatureUid> = state
        .creatures
        .iter()
        .filter(|c| c.current_hp > 0)
        .map(|c| CreatureUid(c.id))
        .filter(|&uid| state.creature_kind(uid) == CreatureKind::Monster)
        .collect();
    for uid in targets {
        apply_power_to_creature(state, uid, PowerKind::Strength, 3);
    }
}

/// `TheObscura` ILLUSION_MOVE (`IllusionMove`, `TheObscura.cs:73-78`):
/// `CreatureCmd.Add<Parafright>(CombatState, "illusion")`. Spawns one
/// Parafright into the "illusion" slot, which precedes "obscura" in
/// `TheObscuraNormal.Slots = ["illusion", "obscura"]`, so the spawn
/// sorts AHEAD of Obscura in enemy order — front-insert to match
/// (same modelling as `fogmog_illusion_effect`, bead `sts2-rs-o1mwv`).
/// The fresh Parafright is paired with its real moveset via
/// [`create_parafright_ai`](crate::monsters::act1_overgrowth::create_parafright_ai)
/// so it acts on its own intents rather than the static
/// `MonsterKind::roll_intent` fallback (C#
/// `CreatureCmd.Add<Parafright>` constructs a fully-wired Parafright).
/// Bead `sts2-rs-cuocf` (AI attachment; the spawn itself was
/// `sts2-rs-3szrv`, previously a telegraph-only `mv()` noop).
fn the_obscura_illusion_effect(state: &mut CombatState, _owner: CreatureUid) {
    // Literal slot "illusion" = index 0 of
    // `TheObscuraNormal.Slots = ["illusion", "obscura"]`; the slot model
    // reproduces C#'s `SortEnemiesBySlotName` instead of the old hardcoded
    // front-insert (GH #552).
    let ai = crate::ascension_scaling::summoned_ai(
        state,
        MonsterKind::Parafright,
        crate::monsters::act1_overgrowth::create_parafright_ai(),
    );
    state.spawn_enemy_in_slot(MonsterKind::Parafright, THE_OBSCURA_ILLUSION_SLOT, ai);
}

/// `TheObscuraNormal.Slots = ["illusion", "obscura"]` — the Parafright's slot.
const THE_OBSCURA_ILLUSION_SLOT: u8 = 0;

/// `Chomper` SCREECH_MOVE (`ScreechMove`, `Chomper.cs:58,75-81`):
/// `StatusIntent(3)` →
/// `CardPileCmd.AddToCombatAndPreview<Dazed>(targets, Discard, 3)`.
/// Adds 3 Dazed to the player's discard pile. Bead `sts2-rs-ae8rc`
/// (previously a telegraph-only `mv()` noop — the `Intent::StatusCard`
/// was shown but no card was generated).
fn chomper_screech_effect(state: &mut CombatState, _owner: CreatureUid) {
    use sts2_core::card_effects::PileKind;
    for _ in 0..3 {
        let _ = crate::cards::status::spawn_dazed(state, PileKind::Discard);
    }
}

/// `HunterKiller` TENDERIZING_GOOP_MOVE — `HunterKiller.cs:54`
/// `PowerCmd.Apply<TenderPower>(targets, 1m, …)` player-debuff.
fn hunter_killer_goop_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Tender, 1);
}

/// `Wriggler` WRIGGLE — C# `Wriggler.cs::WriggleMove`:
/// `await CardPileCmd.AddToCombatAndPreview<Infection>(targets,
/// PileType.Discard, 1, addedByPlayer: false);`
/// `await PowerCmd.Apply<StrengthPower>(base.Creature, 2m, …)`.
/// The sibling `Intent::StatusCard` telegraphs the Infection-card
/// spawn; the StatusCard intent is telegraph-only and applies
/// nothing on its own, so the card-add must live here per the
/// per-monster effect_fn convention
/// (feedback_monster_buff_intent_is_telegraph_only). Bead
/// `sts2-rs-tasgy` (closing the "out of scope" gap previously
/// noted on this effect_fn).
fn wriggler_wriggle_effect(state: &mut CombatState, owner: CreatureUid) {
    use sts2_core::card_effects::PileKind;
    let _ = crate::cards::status::spawn_infection(state, PileKind::Discard);
    apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// `Myte` SUCK_MOVE — `Myte.cs:91`
/// `PowerCmd.Apply<StrengthPower>(base.Creature, SuckStrength=2, …)`
/// self-buff (Ascension-DeadlyEnemies bumps to 3; identity at pin).
/// The Attack 4 rider is handled by `Intent::Attack`.
fn myte_suck_effect(state: &mut CombatState, owner: CreatureUid) {
    let amount =
        ascension::get_value_if_ascension(state.ascension, AscensionLevel::DeadlyEnemies, 3, 2);
    apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// `Ovicopter` TENDERIZER_MOVE — `Ovicopter.cs:110`
/// `PowerCmd.Apply<VulnerablePower>(targets, 2m, …)` player-debuff.
/// The Attack 7 rider is handled by `Intent::Attack`.
fn ovicopter_tenderizer_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Vulnerable, 2);
}

/// `Ovicopter` NUTRITIONAL_PASTE_MOVE — `Ovicopter.cs:93`
/// `PowerCmd.Apply<StrengthPower>(base.Creature,
/// NutritionalPasteStrengthAmount=3, …)` self-buff (Ascension bumps to 4).
fn ovicopter_nutritional_paste_effect(state: &mut CombatState, owner: CreatureUid) {
    let amount =
        ascension::get_value_if_ascension(state.ascension, AscensionLevel::DeadlyEnemies, 4, 3);
    apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// `SpinyToad` SPINES — `SpinyToad.cs:81`
/// `PowerCmd.Apply<ThornsPower>(base.Creature, 5m, …)` self-buff. C# is a
/// hardcoded 5 (no Ascension scaling); the prior Rust intent declared 8 —
/// the effect applies the C#-authoritative 5. The intent metadata is left
/// untouched (intent-payload correction is a separate concern).
fn spiny_toad_spines_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_power_to_creature(state, owner, PowerKind::Thorns, 5);
}

/// `SpinyToad` EXPLOSION (`ExplosionMove`, `SpinyToad.cs:78-86`):
/// after the 23-damage attack lands, C# fires
/// `PowerCmd.Apply<ThornsPower>(base.Creature, -5m, …)` to strip the
/// +5 Thorns that SPINES applied the previous turn (`IsSpiny = false`
/// is the matching internal flag). The prior Rust port used bare
/// `mv()` with no effect_fn — the toad kept its Thorns and stacked
/// another +5 every cycle, drifting unboundedly from C#. Bead
/// `sts2-rs-cezr9`. Hook-routing intentionally bypassed (matches the
/// signed-power-mutation pattern in `apply_signed_power_to_player`,
/// act3_glory.rs:229).
///
/// C# `ThornsPower` is a `Counter`-stack, non-`AllowNegative` power, so
/// `PowerCmd.ModifyAmount(-5)` → `ShouldRemoveDueToAmount()` REMOVES it
/// once `Amount <= 0` (`PowerModel.cs:413`, `PowerCmd.cs:168`). The prior
/// Rust port clamped Thorns to 0 but kept the (now-zero) power instance,
/// so `monsters[0].powers.len` read 1 vs C#'s 0 on the cycle after
/// EXPLOSION (surfaced by the Act-2/3 coverage sweep, bead `sts2-rs-ybtmu`).
fn spiny_toad_explosion_effect(state: &mut CombatState, owner: CreatureUid) {
    let Some(idx) = state
        .creatures
        .iter()
        .position(|c| c.id == owner.0 && c.current_hp > 0)
    else {
        return;
    };
    let thorns_id = state.creatures[idx]
        .powers
        .iter()
        .copied()
        .find_map(|puid| {
            state
                .powers
                .iter()
                .find(|p| p.id == puid.0 && p.kind == PowerKind::Thorns)
                .map(|p| p.id)
        });
    if let Some(id) = thorns_id {
        let mut remove = false;
        if let Some(pow) = state.powers.get_mut_by_id(id) {
            pow.amount = pow.amount.saturating_sub(5).max(0);
            remove = pow.amount == 0;
        }
        if remove {
            state.powers.retain(|p| p.id != id);
            state.creatures[idx].powers.retain(|u| u.0 != id);
        }
    }
}

/// `TheInsatiable` SALIVATE — `TheInsatiable.cs:166`
/// `PowerCmd.Apply<StrengthPower>(base.Creature, SalivateStrength=2, …)`
/// self-buff (Ascension bumps to 3; identity at pin).
fn the_insatiable_salivate_effect(state: &mut CombatState, owner: CreatureUid) {
    let amount =
        ascension::get_value_if_ascension(state.ascension, AscensionLevel::DeadlyEnemies, 3, 2);
    apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// `TheInsatiable` LIQUIFY_GROUND — `TheInsatiable.cs` `LiquifyMove`:
/// apply `SandpitPower(4)` to ITSELF (the boss; C# `TheInsatiable.cs:121`
/// `PowerCmd.Apply(sandpitPower, base.Creature, 4, base.Creature, …)` — the
/// expiring-countdown that kills the player is a self-buff, NOT a player
/// debuff), then create 6 `FranticEscape`
/// status cards (the first 3 into the draw pile, the last 3 into the
/// discard pile — `StatusIntent(6)` / `_liquifyStatusDrawCount = 3` /
/// `_liquifyStatusDiscardCount = 3`). Bead `sts2-rs-asxdy`.
///
/// Scope: this lands the move's BuffIntent + StatusIntent payload (the
/// telegraph-only halves the dispatcher was skipping). The SandpitPower
/// *listener* — decrement on each enemy turn-start and kill the player
/// when it expires (`SandpitPower.AfterSideTurnStartLate` / `AfterRemoved`)
/// — plus FranticEscape's increment-countdown + cost-escalation body
/// (already a marker under `sts2-rs-kqpz`) are the boss-countdown
/// mechanic, split to follow-up `sts2-rs-ytgbq`.
fn the_insatiable_liquify_ground_effect(state: &mut CombatState, owner: CreatureUid) {
    use sts2_core::card_effects::PileKind;
    // C# `TheInsatiable.cs:121` applies SandpitPower(4) to `base.Creature`
    // (the boss itself), not the player. Bead `sts2-rs-ybtmu`.
    apply_power_to_creature(state, owner, PowerKind::Sandpit, 4);
    // C# `LiquifyMove` adds all 6 FranticEscape via
    // `CardPileCmd.AddGeneratedCardToCombat(card, pile, addedByPlayer:false,
    // CardPilePosition.Random)` (TheInsatiable.cs:127-133). `Random` resolves
    // to `Rng.Shuffle.NextInt(targetPile.Cards.Count + 1)` (CardPileCmd.cs:433)
    // — one Shuffle-RNG draw per card, inserting at a uniformly-random index.
    // The first 3 go to Draw, the last 3 to Discard.
    //
    // C# order: i=0..3 → Draw, i=3..6 → Discard, so the Shuffle stream is
    // consumed Draw×3 then Discard×3. Each card spawns then relocates to a
    // random slot via the `(_, true)` insert, whose `run_rng.shuffle
    // .next_int(0, len)` matches `Rng.Shuffle.NextInt(count + 1)` exactly.
    // Bead `sts2-rs-ybtmu`: both piles now route through the Shuffle stream —
    // previously the draw inserts used the legacy `rng` and the discard cards
    // were a plain back-`push` (zero RNG), leaving `run_rng.shuffle` 6 draws
    // short of C# and scrambling the draw order (the DRAW_ORDER divergence the
    // all-acts coverage sweep flagged for THE_INSATIABLE_BOSS).
    for _ in 0..3 {
        let uid = crate::cards::status::spawn_frantic_escape(state, PileKind::Draw);
        state.insert_card_into_draw_pile(uid, true);
    }
    for _ in 0..3 {
        let uid = crate::cards::status::spawn_frantic_escape(state, PileKind::Discard);
        state.insert_card_into_discard_pile(uid, true);
    }
}

/// `KnowledgeDemon` PONDER — `KnowledgeDemon.cs:208-217` (`PonderMove`):
/// `Damage(PonderDamage=11)` (the `Intent::Attack` rider) **then**
/// `CreatureCmd.Heal(base.Creature, 30 * Players.Count)` **then**
/// `PowerCmd.Apply<StrengthPower>(base.Creature, PonderStrength=2, …)`
/// self-buff (Ascension bumps PonderDamage→13 and PonderStrength→3).
/// Single-player sim → heal 30 (× player-count collapses to ×1), mirroring
/// `waterfall_giant_siphon_effect`. The heal was previously dropped.
fn knowledge_demon_ponder_effect(state: &mut CombatState, owner: CreatureUid) {
    state.creature_heal(owner, 30);
    let amount =
        ascension::get_value_if_ascension(state.ascension, AscensionLevel::DeadlyEnemies, 3, 2);
    apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// `Crusher` BUG_STING — `Crusher.cs:144-145`
/// `PowerCmd.Apply<WeakPower>(targets, 2m, …)` **and**
/// `PowerCmd.Apply<FrailPower>(targets, 2m, …)`, both player-debuffs.
/// The Rust intent declared only Weak; Frail was dropped entirely. Both
/// are applied here. The MultiAttack 6×2 rider is handled by the intents.
fn crusher_bug_sting_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 2);
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 2);
}

/// `Crusher` ADAPT — `Crusher.cs:151`
/// `PowerCmd.Apply<StrengthPower>(base.Creature, AdaptStrengthGain=2, …)`
/// self-buff (Ascension bumps to 3).
fn crusher_adapt_effect(state: &mut CombatState, owner: CreatureUid) {
    let amount =
        ascension::get_value_if_ascension(state.ascension, AscensionLevel::DeadlyEnemies, 3, 2);
    apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// `InfestedPrism` PULSATE (v0.107.1) — decomp
/// `v0.107.1/…/InfestedPrism.cs:107-114` `PulsateMove`:
/// `Attack(PulsateDamage)` then `GainBlock(PulsateBlock=20)` then
/// `PowerCmd.Apply<VitalSparkPower>(base.Creature, VitalSparkAmount, …)`.
/// `…wb8lk.11`: v0.107.1 replaced the old Strength self-buff with a
/// re-application of VitalSpark (the Tainted applier) plus the 20 Block.
/// The Attack 8 is the primary intent (handled by `perform_intent`); the
/// secondary Defend 20 and Buff are applied here. VitalSparkAmount =
/// GetValueIfAscension(DeadlyEnemies, 3, 2) ⇒ A0 2 / A9 3.
fn infested_prism_pulsate_effect(state: &mut CombatState, owner: CreatureUid) {
    let block = crate::ascension_scaling::tough_block(
        state.ascension,
        MonsterKind::InfestedPrism,
        "PULSATE",
        20,
    );
    gain_secondary_defend_block(state, owner, block);
    let amount =
        ascension::get_value_if_ascension(state.ascension, AscensionLevel::DeadlyEnemies, 3, 2);
    apply_power_to_creature(state, owner, PowerKind::VitalSpark, amount);
}

/// `DecimillipedeSegment` BULK — `DecimillipedeSegment.cs:194`
/// `PowerCmd.Apply<StrengthPower>(base.Creature, BulkStrength=2, …)`
/// self-buff. The Attack 6 rider is handled by `Intent::Attack`.
fn decimillipede_bulk_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// `DecimillipedeSegment` CONSTRICT — `DecimillipedeSegment.cs:203`
/// `PowerCmd.Apply<WeakPower>(targets, 1m, …)` player-debuff. The Attack 8
/// rider is handled by `Intent::Attack`.
fn decimillipede_constrict_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Weak, 1);
}

/// `DecimillipedeSegment.ReattachMove` (`DecimillipedeSegment.cs:230-235`) →
/// `ReattachPower.DoReattach` (`ReattachPower.cs:38-48`): the revive is
/// re-gated at EXECUTE time on another living segment still being up; when
/// the gate holds, the 0-HP corpse heals by its Reattach power amount (25)
/// and is back in the fight at its own roster slot. If every other segment
/// died between the death and this move, the heal is skipped and the corpse
/// stays down (C# leaves `isReviving` set and never heals). The headless VFX
/// tail (`ChangePhobiaModeTexture`) is intentionally not modeled. Bead
/// `sts2-rs-8lwnq`.
fn decimillipede_reattach_effect(state: &mut CombatState, owner: CreatureUid) {
    let carries_reattach = |state: &CombatState, c: &sts2_core::Creature| {
        c.powers.iter().any(|puid| {
            state
                .powers
                .iter()
                .any(|p| p.id == puid.0 && p.kind == PowerKind::Reattach)
        })
    };
    let another_segment_alive = state.enemy_uids.iter().any(|uid| {
        *uid != owner
            && state
                .creatures
                .iter()
                .any(|c| c.id == uid.0 && c.current_hp > 0 && carries_reattach(state, c))
    });
    if !another_segment_alive {
        return;
    }
    let amount = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .into_iter()
        .flat_map(|c| c.powers.iter())
        .find_map(|puid| {
            state
                .powers
                .iter()
                .find(|p| p.id == puid.0 && p.kind == PowerKind::Reattach)
                .map(|p| p.amount)
        })
        .unwrap_or(0);
    if amount <= 0 {
        return;
    }
    // `CreatureCmd.Heal` on the corpse: 0 → Amount, capped at max. The
    // shared `creature_heal` helper refuses dead targets, so mutate
    // directly (the Illusion revive in `combat_loop` does the same).
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
        if c.current_hp <= 0 {
            c.current_hp = amount.min(c.max_hp);
        }
    }
}

/// `ThievingHopper` THIEVERY_MOVE card-steal rider — C#
/// `ThievingHopper.cs:180` `ThieveryMove`. After (in C#, before) the
/// `TheftDamage` hit, the Hopper steals exactly one card from the
/// player's Draw+Discard piles and `CardPileCmd.RemoveFromCombat`s it,
/// shrinking that pile by one. This is the source of the
/// THIEVING_HOPPER_WEAK PILE_COUNT divergence: the prior port encoded
/// only the attack damage and dropped the steal, so the player's
/// draw/discard counts never decremented vs the C# reference.
///
/// Faithful selection (`ThievingHopper.cs:195-219`):
///   1. Candidate list = `CardPile.GetCards(player, Draw, Discard)`
///      filtered to `DeckVersion != null` — i.e. real deck cards, in
///      pile order (Draw first, then Discard). Combat-generated Status
///      cards (Dazed/Wound/…) have a null `DeckVersion`, so they are
///      excluded; we proxy that with `card_type != Status` (the engine
///      does not model `DeckVersion`).
///   2. Walk the four `_stealPriorities` predicates
///      (`ThievingHopper.cs:27-65`) in order; the first predicate that
///      matches ≥1 candidate narrows the list to that subset. If none
///      match, the full candidate list is kept.
///   3. `RunRng.CombatCardGeneration.NextItem(items)` →
///      `NextInt(0, n)` (max-exclusive) selects the index. Exactly one
///      `combat_card_generation` draw is consumed, so downstream RNG
///      stays in lockstep with C#.
///
/// `SwipePower` (C# `ThievingHopper.cs:234-236` applies one stack per stolen
/// card via `PowerCmd.Apply<SwipePower>(Creature, 1, …)`) IS applied to the
/// owner after the steal. Its only behavioural hook is `BeforeDeath`
/// (out-of-combat deck restoration, `SwipePower.cs:53-67`) — zero combat /
/// PILE_COUNT impact — but its PRESENCE is observable in `monsters[*].powers`,
/// so it must be modelled for state parity (bead `sts2-rs-ybtmu`).
fn thieving_hopper_thievery_effect(state: &mut CombatState, owner: CreatureUid) {
    use crate::cards::spec_index::rarity_for_card_id;
    use crate::cards::CardRarity;
    use sts2_core::cards::{CardType, EnchantmentId};

    // Real-deck cards in Draw then Discard pile order (`DeckVersion != null`,
    // `ThievingHopper.cs:203`). ANY combat-generated card has a null
    // `DeckVersion` in C# — not just Statuses: a Calamity/Stoke/JackOfAll-
    // Trades-generated MEMENTO_MORI / CINDER / BATTLE_TRANCE is excluded
    // too. The prior Status-only proxy inflated the candidate count `n`
    // feeding `NextItem`, shifting the stolen index (sweep seeds 4043/5342:
    // same RNG sample, C# n=8 → CATASTROPHE vs Rust n=9 → HEMOKINESIS).
    // The `GeneratedInCombat` marker is the engine's `DeckVersion == null`.
    let candidates: Vec<sts2_core::CardUid> = state
        .player
        .draw
        .iter()
        .chain(state.player.discard.iter())
        .copied()
        .filter(|uid| {
            let card = &state.cards[uid.get() as usize - 1];
            card.card_type != CardType::Status
                && card.combat_var(sts2_core::cards::VarKey::GeneratedInCombat) == 0
        })
        .collect();
    if candidates.is_empty() {
        return;
    }

    // `_stealPriorities` (`ThievingHopper.cs:27-65`): Imbued cards are
    // demoted to the last bucket; rarity drives the rest. Resolve the
    // narrowed `items` list inside a block so the read-only `state.cards`
    // borrows are dropped before the steal mutates the pile.
    let items: Vec<sts2_core::CardUid> = {
        let not_imbued = |uid: &sts2_core::CardUid| -> bool {
            state.cards[uid.get() as usize - 1].enchantment(EnchantmentId::Imbued) == 0
        };
        let rarity_of = |uid: &sts2_core::CardUid| -> Option<CardRarity> {
            rarity_for_card_id(state.cards[uid.get() as usize - 1].card_id)
        };
        let predicates: [Box<dyn Fn(&sts2_core::CardUid) -> bool>; 4] = [
            Box::new(|u| not_imbued(u) && rarity_of(u) == Some(CardRarity::Uncommon)),
            Box::new(|u| {
                not_imbued(u)
                    && matches!(
                        rarity_of(u),
                        Some(CardRarity::Common | CardRarity::Rare | CardRarity::Event)
                    )
            }),
            Box::new(|u| {
                not_imbued(u) && matches!(rarity_of(u), Some(CardRarity::Basic | CardRarity::Quest))
            }),
            Box::new(|u| rarity_of(u) == Some(CardRarity::Ancient) || !not_imbued(u)),
        ];

        let mut items = candidates.clone();
        for predicate in &predicates {
            let filtered: Vec<sts2_core::CardUid> = candidates
                .iter()
                .copied()
                .filter(|u| predicate(u))
                .collect();
            if !filtered.is_empty() {
                items = filtered;
                break;
            }
        }
        items
    };

    // `RunRng.CombatCardGeneration.NextItem(items)` — one max-exclusive draw.
    let len = i32::try_from(items.len()).unwrap_or(i32::MAX);
    let idx = state
        .run_rng
        .combat_card_generation
        .next_int_exclusive(0, len) as usize;
    let stolen = items[idx];

    // `CardPileCmd.RemoveFromCombat(cardToSteal)` — removes the card from its
    // (Draw/Discard) combat pile entirely (`CardPileCmd.cs:78-106`). C# ALSO
    // does `SwipePower.Steal(card)` → `CardPileCmd.RemoveFromDeck(card
    // .DeckVersion)` (`SwipePower.cs:70`): the steal PERMANENTLY removes the
    // card from the RUN DECK, not just the combat pile. Record the stolen
    // card's master-deck identity (`(card_id, upgraded)`) here so `exit_combat`
    // removes it from `master_deck`; a killed thief reclaims it
    // (`SwipePower.BeforeDeath` → `CombatState::reclaim_swipe_on_death`). The
    // candidate filter above already excludes combat-generated cards
    // (`DeckVersion == null`), so the stolen card always has a master-deck
    // counterpart. Bead `sts2-rs-RP13`.
    let (stolen_card_id, stolen_upgraded) = {
        let c = &state.cards[stolen.get() as usize - 1];
        (c.card_id, c.upgraded)
    };
    let _ = sts2_core::card_effects::remove_card_from_any_pile(state, stolen);
    state.record_swipe_steal(owner, stolen_card_id, stolen_upgraded);

    // `ThievingHopper.cs:234-236`: one `SwipePower` stack per stolen card.
    // Exactly one card is stolen per THIEVERY, so apply a single stack. The
    // spawn-time EscapeArtist is already present, so this lands at `powers[1]`,
    // keeping EscapeArtist at `powers[0]` (matches the C# snapshot order).
    apply_power_to_creature(state, owner, PowerKind::Swipe, 1);
}

/// `ThievingHopper` FLUTTER_MOVE — `ThievingHopper.cs:266`
/// `PowerCmd.Apply<FlutterPower>(base.Creature, 5m, …)` self-buff.
fn thieving_hopper_flutter_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_power_to_creature(state, owner, PowerKind::Flutter, 5);
}

/// `LouseProgenitor` WEB_CANNON_MOVE applies `Frail(2)` to the player after
/// its attack. Mirrors C# `LouseProgenitor.cs` `WebMove`:
/// `DamageCmd.Attack(WebDamage) ; await PowerCmd.Apply<FrailPower>(targets, 2m, …)`.
/// Bead `sts2-rs-ybtmu` (Act-2/3 parity vein): the prior port fabricated a
/// 2-Wriggler SPAWN move off the Python pin — C# never summons (solo
/// `LouseProgenitorNormal` encounter, no summon in the moveset or CurlUp).
fn louse_progenitor_web_effect(state: &mut CombatState, _owner: CreatureUid) {
    apply_power_to_player_via_legacy_surface(state, PowerKind::Frail, 2);
}

/// `LouseProgenitor` CURL_AND_GROW_MOVE applies `Strength(5)` to self; its
/// primary `Intent::Defend { block: 14 }` (CurlBlock non-asc) is granted by
/// the engine's block-gain path before this effect runs. Mirrors C#
/// `LouseProgenitor.cs` `CurlAndGrowMove`:
/// `GainBlock(CurlBlock=14, Move) ; await PowerCmd.Apply<StrengthPower>(base.Creature, 5m, …)`.
fn louse_progenitor_curl_and_grow_effect(state: &mut CombatState, owner: CreatureUid) {
    apply_power_to_creature(state, owner, PowerKind::Strength, 5);
}

/// Bead `sts2-rs-7qd1`: Ovicopter LAY_EGGS spawns 3
/// [`MonsterKind::ToughEgg`]s per C# decomp
/// `MegaCrit.Sts2.Core.Models.Monsters/Ovicopter.cs::LayEggsMove`
/// (`for (int i = 0; i < 3; i++) { ... CreatureCmd.Add<ToughEgg>(...) }`).
/// Python pin `01c094d:sts2_env/monsters/act2.py:342-344` spawns
/// just 1; we deliberately diverge per the monster-stat audit
/// (kgwj umbrella) — see `docs/perf/2026-05-18-monster-stat-audit.md`.
/// Originally landed as 1-spawn under `sts2-rs-vn0m`. Bead
/// `sts2-rs-ybtmu` (Group B) added the per-egg `MinionPower(1)` tag the
/// C# applies at the summon site (the summon-gap fix).
fn ovicopter_lay_eggs_effect(state: &mut CombatState, _owner: CreatureUid) {
    // C# `OvicopterNormal.Slots = [egg1, egg2, egg3, egg4, egg5,
    // ovicopter]` — five egg slots (indices 0..=4) ahead of the
    // Ovicopter (slot index 5). Bead `sts2-rs-c8why` (Ovicopter
    // multi-lay slot-sort, superseding the `sts2-rs-z4jsg` front-insert).
    //
    // FIVE, not six: C#'s `LastOrDefault` scans all six and only skips
    // "ovicopter" because the living Ovicopter holds it. Bounding the search
    // at the egg slots gets the same answer while staying correct on paths
    // that never stamped the Ovicopter's own slot (the CLI scenario loader —
    // see the note in `sim::scenario::starter`). An Ovicopter dead at the
    // moment it lays is not reachable.
    const EGG_SLOT_COUNT: u8 = 5;
    for _ in 0..3 {
        // C# `LayEggsMove` assigns each egg
        // `Encounter.Slots.LastOrDefault(s => Enemies.All(c => c.SlotName
        // != s))` — the HIGHEST-index slot not held by a LIVING enemy —
        // then `CombatManager.AddCreature` calls `SortEnemiesBySlotName`,
        // re-sorting the active `Enemies` list by slot index on EVERY add.
        //
        // A flat front-insert only reproduced that for a single lay with no
        // prior egg deaths: once a hatchling in a low slot dies, a freed slot
        // can sit BETWEEN surviving hatchlings, so the second lay mis-ordered
        // (coverage seeds 1350/2161/2481 — a new egg taking a freed `egg5`
        // belongs AFTER the `egg3`/`egg4` survivors, not at the front).
        //
        // This used to be ~35 lines of hand-rolled free-slot search and
        // insert-position scan right here. Both are now the shared slot model
        // (GH #552), which is the same rule every other slotted encounter
        // needs — `last_free_enemy_slot` skips corpses for us (C# drops them
        // from `Enemies`; Rust retains them in `enemy_uids`), and the
        // Ovicopter's own slot 5 is stamped at combat entry so it no longer
        // needs the species-filtered special case the old scan used.
        let Some(slot) = state.last_free_enemy_slot(EGG_SLOT_COUNT) else {
            continue;
        };
        // Pair each summon with its real moveset (bead `sts2-rs-wrklt`,
        // follow-up to `sts2-rs-cuocf`); without this the fresh ToughEgg
        // falls back to the static `MonsterKind::roll_intent` path. The
        // eggs act before the Ovicopter on subsequent turns (slot < 5) —
        // load-bearing for the hatch-roll Niche RNG order.
        let ai = crate::ascension_scaling::summoned_ai(
            state,
            MonsterKind::ToughEgg,
            create_tough_egg_ai(),
        );
        let uid = state.spawn_enemy_in_slot(MonsterKind::ToughEgg, slot, ai);
        // Bead `sts2-rs-z4jsg`: C# `ToughEgg.AfterAddedToRoom` (fired
        // inside `CreatureCmd.Add`, BEFORE the summon-site Minion apply
        // below) grants `HatchPower((CurrentSide != Enemy) ? 1 : 2)`.
        // LAY_EGGS only ever runs mid-ENEMY-turn → 2; the same turn's
        // `after_turn_end(Enemy)` Hatch arm ticks it to 1 (HatchPower.cs
        // ticks on every enemy turn end). The egg hatches when its
        // HATCH_MOVE performs (see `tough_egg_hatch_effect`), not when
        // the counter empties — the power is the visible countdown.
        apply_power_to_creature(state, uid, PowerKind::Hatch, 2);
        // Bead `sts2-rs-ybtmu` (Group B): C# `Ovicopter.LayEggsMove` does
        // `PowerCmd.Apply<MinionPower>(egg, 1m, base.Creature, null)`
        // (Ovicopter.cs:85). MinionPower tags the egg as a secondary enemy
        // (`Creature.IsSecondaryEnemy`), which (a) makes the leader-death
        // cascade in `check_combat_end` kill the eggs when the Ovicopter
        // dies (C# `Kill(teammates)` when the last primary falls,
        // CreatureCmd.cs:392) and (b) excludes them from the non-minion
        // enemy HP / pressure leaf-eval terms. ToughEgg does NOT self-apply
        // MinionPower (its `AfterAddedToRoom` only grants HatchPower), so it
        // is not in the auto-spawn `on_spawn_self_buffs` table and must be
        // tagged here at the summon site — same as the Fabricator's bots
        // (`fabricator_spawn_bot`, bead `sts2-rs-dygv1`).
        apply_power_to_creature(state, uid, PowerKind::Minion, 1);
    }
}

/// Bead `sts2-rs-z4jsg`: ToughEgg HATCH_MOVE body. C# `ToughEgg.cs::
/// HatchMove` → `Hatch()`:
///   1. `PowerCmd.Remove<HatchPower>(Owner)` + remove every remaining
///      non-Minion power (`Powers.Where(p => !(p is MinionPower))`) —
///      a hatching egg sheds player-applied debuffs (Vulnerable, …) but
///      keeps its Minion tag.
///   2. Re-roll HP: `RunRng.Niche.NextInt(HatchlingMinHp=19,
///      HatchlingMaxHp=22)` (non-ascension; C# `Rng.NextInt(min, max)`
///      is max-EXCLUSIVE — `Rng.cs:57` — so the band is 19..=21; PLAIN
///      NextInt, no unique-HP dedup) wrapped in `ScaleHpForMultiplayer`
///      (identity at playerCount == 1), then
///      `CreatureCmd.SetMaxAndCurrentHp` — max AND current both set,
///      i.e. the hatchling emerges at full (new) HP regardless of chip
///      damage on the shell. Block is untouched.
/// The Title flips to HATCHLING.name but `Id` stays TOUGH_EGG (cosmetic
/// only — the bridge `kind` field never changes). The NIBBLE follow-up
/// telegraph rolls through the normal end-of-enemy-turn path.
fn tough_egg_hatch_effect(state: &mut CombatState, owner: CreatureUid) {
    let shed: smallvec::SmallVec<[std::num::NonZeroU32; 4]> = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .map(|c| {
            c.powers
                .iter()
                .filter_map(|pu| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == pu.0 && p.kind != PowerKind::Minion)
                        .map(|p| p.id)
                })
                .collect()
        })
        .unwrap_or_default();
    for pid in shed {
        state.powers.retain(|p| p.id != pid);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
            c.powers.retain(|u| u.0 != pid);
        }
    }
    // C# `Rng.NextInt(19, 22)` is max-EXCLUSIVE → band 19..=21.
    let hp = state.run_rng.niche.next_int_exclusive(19, 22);
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
        c.max_hp = hp;
        c.current_hp = hp;
    }
}

/// Bead `sts2-rs-baxi5`: Myte INFEST adds 2 [`CardId::Toxic`] to the
/// player's hand per C# `Myte.cs:ToxicMove`:
/// `await CardPileCmd.AddToCombatAndPreview<Toxic>(targets, PileType.Hand, 2, addedByPlayer: false)`.
/// The Hand placement is mechanically load-bearing — Toxic's
/// `OnTurnEndInHand` fires `CreatureCmd.Damage(owner, DamageVar(5))`
/// per copy in hand at turn end, so two Toxics is 10 unpowered
/// self-damage per turn until they exhaust/play. The prior body
/// (bead `sts2-rs-h82l`) followed the Python pin
/// (`sts2-rl-agent@01c094d:sts2_env/monsters/act2.py:291-317`,
/// `combat.add_card_to_discard(make_parasite())`) — wrong card,
/// wrong count, wrong pile. C# is the source of truth.
fn myte_infest_effect(state: &mut CombatState, _owner: CreatureUid) {
    use sts2_core::card_effects::PileKind;
    let _ = crate::cards::status::spawn_toxic(state, PileKind::Hand);
    let _ = crate::cards::status::spawn_toxic(state, PileKind::Hand);
}

/// `Entomancer` PHEROMONE_SPIT effect. Per C#
/// `Entomancer.cs:SpitMove`: if `PersonalHive` amount is below 3, grow
/// the hive by 1 AND apply Strength +1 to self; otherwise (hive maxed)
/// apply Strength +2. The Python pin
/// (`sts2-rl-agent@01c094d:sts2_env/monsters/act2.py:542-548`) dropped
/// the Strength +1 in the sub-3 branch — corrected here per the
/// C#-is-source-of-truth rule. Bead `sts2-rs-pnv5` / `sts2-rs-ybtmu`.
fn entomancer_pheromone_spit_effect(state: &mut CombatState, uid: CreatureUid) {
    let hive = entomancer_personal_hive_amount(state, uid);
    if hive < 3 {
        apply_power_to_creature(state, uid, PowerKind::PersonalHive, 1);
        apply_power_to_creature(state, uid, PowerKind::Strength, 1);
    } else {
        apply_power_to_creature(state, uid, PowerKind::Strength, 2);
    }
}

/// Read the `PersonalHive` counter on `owner` (0 if absent). Shared by
/// PHEROMONE_SPIT (ramp gate) and the AfterDamageReceived hook (Dazed
/// count).
fn entomancer_personal_hive_amount(state: &CombatState, owner: CreatureUid) -> i32 {
    state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().find_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::PersonalHive)
                    .map(|p| p.amount)
            })
        })
        .unwrap_or(0)
}

/// C# `PersonalHivePower.AfterDamageReceived` (PersonalHivePower.cs:23-42):
/// when the Entomancer (owner) takes a POWERED attack from the player,
/// shove `Amount` `Dazed` cards into the player's DRAW pile at random
/// positions. The C# body ignores the `DamageResult` — it fires even on
/// a fully-blocked powered hit. `CardPilePosition.Random` resolves to
/// `Rng.Shuffle.NextInt(count + 1)`, which [`CombatState::
/// insert_card_into_draw_pile`]`(_, true)` mirrors (same shuffle stream
/// `ShiningStrike` uses). The hive amount ramps 1→3 via PHEROMONE_SPIT,
/// so the flood grows from 1 to 3 Dazed per hit. Bead `sts2-rs-ybtmu`.
fn entomancer_personal_hive_after_damage(
    state: &mut CombatState,
    owner: CreatureUid,
    dealer: sts2_core::hooks::CreatureRef,
    props: sts2_core::hooks::ValueProps,
    // C# `PersonalHivePower.AfterDamageReceived` ignores the `DamageResult`
    // — a fully-blocked powered hit still floods Dazed.
    _unblocked_damage: i32,
) -> EffectResult {
    use sts2_core::card_effects::PileKind;
    use sts2_core::hooks::CreatureRef;
    // C# gate: `dealer != null && props.IsPoweredAttack()`, with an Osty-pet
    // redirect — `if (dealer.Monster is Osty) dealer = dealer.PetOwner.Creature`
    // (PersonalHivePower.cs:27-30) — so a Necrobinder Osty attack (SQUEEZE,
    // POKE, …) floods Dazed too, attributed to the pet's player owner. Rust
    // models the Osty as `CreatureRef::Enemy(pet_uid)`, so accept the player OR
    // the player's living Osty pet as the dealer. (Bead `sts2-rs-1jqu3`: the
    // old plain Player-only gate dropped the flood on every Osty attack —
    // Necrobinder fuzz seed 406, SQUEEZE vs ENTOMANCER: draw cs=2 rs=0 Dazed.)
    let dealer_is_player_side = matches!(dealer, CreatureRef::Player)
        || matches!(dealer, CreatureRef::Enemy(uid) if Some(uid) == state.player.pet_uid);
    if !props.is_powered() || !dealer_is_player_side {
        return EffectResult::NotImplemented;
    }
    let amount = entomancer_personal_hive_amount(state, owner);
    if amount <= 0 {
        return EffectResult::NotImplemented;
    }
    for _ in 0..amount {
        // spawn inserts at the draw-pile top; relocate each copy to a random
        // slot (one Shuffle-RNG draw per Dazed, matching the C# per-card `Add`
        // loop). C# `PersonalHivePower` passes `creator: null` to
        // `AddGeneratedCardToCombat`, so this is NOT a player generation — pass
        // `added_by_player = false` so Smokestack/Regalite do not fire on it
        // (they gate on `creator == Owner`). The default `spawn_dazed` would
        // infer `true` from the non-empty Play pile mid-player-attack (seed 5696:
        // Rust Rend +5 Smokestack that C# never dealt).
        let uid = crate::cards::status::spawn_dazed_with_origin(state, PileKind::Draw, false);
        state.insert_card_into_draw_pile(uid, true);
    }
    EffectResult::Resolved
}

/// Idempotent installer for PersonalHive's AfterDamageReceived hook.
/// Called via a `Once` guard from [`create_entomancer_ai`] (every
/// Entomancer spawn flows through it, so the handler is present before
/// any combat damage event). Safe to call from multiple boot paths.
pub fn install_personal_hive_handlers() {
    sts2_core::monster_power_effects::register_after_damage_received_for_power(
        PowerKind::PersonalHive,
        entomancer_personal_hive_after_damage,
    );
}

/// C# `CurlUpPower` (CurlUpPower.cs:34-70) is a reactive Counter buff: when
/// the owner takes a POWERED attack sourced from a card, it latches that card
/// (`AfterDamageReceived`) and, once the card finishes resolving
/// (`AfterCardPlayed`), grants `Amount` block to the owner and removes itself.
/// `LouseProgenitor` spawns with `CurlUp(14)` (`LouseProgenitor.cs:69`); the
/// first powered player card grants it 14 block and consumes the power, so the
/// post-strike snapshot is `block = 14, powers.len = 0`.
///
/// C# is two-phase: `AfterDamageReceived` only LATCHES the card; the block is
/// granted in `AfterCardPlayed`, AFTER the card fully resolves — so a multi-hit
/// card's later hits are NOT absorbed by the freshly-granted block. The prior
/// Rust port collapsed both phases into the damage hook (granting on hit 1),
/// which a 2-hit card like THRASH then partially absorbed (coverage seed 2060:
/// LouseProgenitor block cs=14 rs=8 / hp cs=123 rs=129). Now this hook only
/// sets the CurlUp `counter` to 1 (latch); the grant + self-remove run in the
/// core `dispatch_after_card_played` CurlUp arm. Bead
/// `sts2-rs-curlup-after-card`.
fn curl_up_after_damage(
    state: &mut CombatState,
    owner: CreatureUid,
    dealer: sts2_core::hooks::CreatureRef,
    props: sts2_core::hooks::ValueProps,
    // C# `CurlUpPower.AfterDamageReceived` latches without reading the
    // `DamageResult` — blocked hits still arm the latch.
    _unblocked_damage: i32,
) -> EffectResult {
    use sts2_core::hooks::CreatureRef;
    // C# gate: powered attack (`IsPoweredAttack()`) sourced from a card
    // (`cardSource != null` ≈ a player-card dealer).
    if !props.is_powered() || !matches!(dealer, CreatureRef::Player) {
        return EffectResult::NotImplemented;
    }
    // Latch the CurlUp on the owner (counter = 1) — granted + removed once the
    // card finishes resolving, in `dispatch_after_card_played`.
    let pid: Option<std::num::NonZeroU32> = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().find_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::CurlUp && p.amount > 0)
                    .map(|p| p.id)
            })
        });
    let Some(pid) = pid else {
        return EffectResult::NotImplemented;
    };
    if let Some(p) = state.powers.get_mut_by_id(pid) {
        p.counter = 1;
    }
    EffectResult::Resolved
}

/// Idempotent installer for CurlUp's AfterDamageReceived hook. Called via a
/// `Once` guard from [`create_louse_progenitor_ai`]. Safe to call from
/// multiple boot paths.
pub fn install_curl_up_handlers() {
    sts2_core::monster_power_effects::register_after_damage_received_for_power(
        PowerKind::CurlUp,
        curl_up_after_damage,
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Weak encounters
// ────────────────────────────────────────────────────────────────────────────

/// `ThievingHopper` — deterministic 5-move chain
/// THIEVERY → FLUTTER → HAT_TRICK → NAB → ESCAPE (self-loop) per C#
/// decomp `MegaCrit.Sts2.Core.Models.Monsters/ThievingHopper.cs`
/// v0.103.2 (`GenerateMoveStateMachine`). Bead `sts2-rs-l8yi`,
/// parent `sts2-rs-kgwj` (Pattern A — Python pin `01c094d` ships a
/// stale 1-move MUG variant we deliberately diverge from per the
/// monster-stat audit).
///
/// Move payloads track the C# constants verbatim:
/// - THIEVERY → `SingleAttackIntent(TheftDamage=17)` + `CardDebuffIntent`
///   (card-steal). The attack damage is the `Intent::Attack`; the
///   card-steal rider lives in `thieving_hopper_thievery_effect` (bead
///   `sts2-rs-ybtmu`, PILE_COUNT fix) — it removes one Draw/Discard card
///   from combat per `ThievingHopper.cs:180` `ThieveryMove`.
/// - FLUTTER → `BuffIntent` → `FlutterPower(5)` self-buff. The reactive
///   listener IS wired (bead `sts2-rs-ybtmu`): `FlutterPower` halves incoming
///   powered attacks (`modify_damage_multiplicative` ×0.5) and decrements one
///   stack per unblocked powered hit (`tick_flutter_and_maybe_stun` in
///   `hooks::dispatch_after_damage_received`). The at-0 stun + un-hover is
///   deferred (bead `sts2-rs-r9075`).
/// - HAT_TRICK → `SingleAttackIntent(HatTrickDamage=21)`.
/// - NAB → `SingleAttackIntent(NabDamage=14)`.
/// - ESCAPE → `EscapeIntent` → `Intent::Escape` (bead `sts2-rs-ybtmu`;
///   the dispatcher landed under `sts2-rs-8f336`). C# `EscapeMove` ends with
///   `CreatureCmd.Escape(base.Creature)`, removing the hopper from combat
///   (it flees with the stolen card); the Rust handler models removal as
///   `current_hp = 0` (corpse-retention pin — `sts2-rs-c8why`). The C# graph
///   self-loops ESCAPE → ESCAPE so a not-yet-removed hopper re-telegraphs
///   escape. NOTE: exact combat-end TURN still depends on the deferred
///   FlutterPower at-0 stun (`sts2-rs-r9075`), which burns a turn before the
///   chain reaches ESCAPE.
///
/// `AfterAddedToRoom` applies `EscapeArtistPower(5)`: wired via the on-spawn
/// self-buff table (`monsters.rs` `on_spawn_self_buffs`), and the power
/// decrements one stack per owner turn-end while `Amount > 1`
/// (`hooks.rs` `after_turn_end` `EscapeArtist` arm).
pub fn create_thieving_hopper_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "THIEVERY_MOVE",
        // Bead `sts2-rs-ybtmu`: the `CardDebuffIntent` card-steal rider is
        // no longer a noop — `thieving_hopper_thievery_effect` removes one
        // Draw/Discard card from combat per `ThievingHopper.cs:180`.
        MonsterState::Move(MoveState {
            id: "THIEVERY_MOVE",
            effect: thieving_hopper_thievery_effect,
            // `ThievingHopper.cs:166`: `SingleAttackIntent` + `CardDebuffIntent`
            // (the steal, held by the thief's `SwipePower`; telegraph only).
            intents: smallvec![
                Intent::Attack { damage: 17 },
                Intent::Debuff {
                    kind: PowerKind::Swipe,
                    amount: 1
                }
            ],
            follow_up: Some("FLUTTER_MOVE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "FLUTTER_MOVE",
        // Bead `sts2-rs-pdsdl`: telegraph-only Buff now applies FlutterPower
        // +5 to self per `ThievingHopper.cs:266`.
        MonsterState::Move(MoveState {
            id: "FLUTTER_MOVE",
            effect: thieving_hopper_flutter_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Flutter,
                amount: 5,
            }],
            follow_up: Some("HAT_TRICK_MOVE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "HAT_TRICK_MOVE",
        mv(
            "HAT_TRICK_MOVE",
            smallvec![Intent::Attack { damage: 21 }],
            "NAB_MOVE",
        ),
    );
    s.insert(
        "NAB_MOVE",
        mv(
            "NAB_MOVE",
            smallvec![Intent::Attack { damage: 14 }],
            "ESCAPE_MOVE",
        ),
    );
    s.insert(
        "ESCAPE_MOVE",
        mv("ESCAPE_MOVE", smallvec![Intent::Escape], "ESCAPE_MOVE"),
    );
    // FlutterPower at-0 stun target (bead `sts2-rs-r9075`). C#
    // `FlutterPower.AfterDamageReceived` calls `Stun(Owner, StunnedMove,
    // nextState)` when Flutter depletes: a no-op turn, then resume at the move
    // the hopper had telegraphed. `tick_flutter_and_maybe_stun`
    // (`hooks.rs`) drives the hopper here via `force_current_state` and
    // re-queues the saved telegraph via `force_next_state`, so this state's
    // static `follow_up` is only a fallback (never normally consumed).
    s.insert(
        "STUNNED_MOVE",
        mv("STUNNED_MOVE", smallvec![Intent::Stun], "ESCAPE_MOVE"),
    );
    MonsterAi::new(s, "THIEVERY_MOVE")
}

/// `Tunneler` — BITE → BURROW → BELOW (self-loop). Mirrors C# decomp
/// `MegaCrit.Sts2.Core.Models.Monsters/Tunneler.cs` v0.103.2 (bead
/// `sts2-rs-ii8x`, parent `sts2-rs-kgwj`). The Python pin `01c094d`
/// ships a stale 24-28 HP / 2-move (BURROW↔BITE, 7/8 dmg) variant
/// that diverges from current C#; we follow C# for engine fidelity
/// per the monster-stat audit (`docs/perf/2026-05-18-monster-stat-audit.md`).
///
/// `DIZZY_MOVE` (Stun intent, follow_up `BITE_MOVE`) is the faithful
/// analogue of the C# dynamic `STUNNED` state that `BurrowedPower`'s
/// `AfterBlockBroken` installs via `Stun(Owner, "BITE_MOVE")`. Bead
/// `sts2-rs-ybtmu` (Group A) wired that block-break redirection: when
/// the player breaks the burrowed Tunneler's block, the power-side
/// `after_block_broken` channel forces this state immediately
/// (`MonsterAi::force_current_state`) and removes Burrowed, so the
/// Tunneler unburrows — stunned for a turn, then bites.
pub fn create_tunneler_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "BITE_MOVE",
        mv(
            "BITE_MOVE",
            smallvec![Intent::Attack { damage: 13 }],
            "BURROW_MOVE",
        ),
    );
    s.insert(
        "BURROW_MOVE",
        // Bead `sts2-rs-pdsdl`: telegraph-only Buff now applies BurrowedPower
        // +1 to self per `Tunneler.cs:80`. The Defend 32 rider is the intent.
        MonsterState::Move(MoveState {
            id: "BURROW_MOVE",
            effect: tunneler_burrow_effect,
            intents: smallvec![
                Intent::Buff {
                    kind: PowerKind::Burrowed,
                    amount: 1,
                },
                Intent::Defend { block: 32 },
            ],
            follow_up: Some("BELOW_MOVE_1"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "BELOW_MOVE_1",
        mv(
            "BELOW_MOVE_1",
            smallvec![Intent::Attack { damage: 23 }],
            "BELOW_MOVE_1",
        ),
    );
    s.insert(
        "DIZZY_MOVE",
        mv("DIZZY_MOVE", smallvec![Intent::Stun], "BITE_MOVE"),
    );
    MonsterAi::new(s, "BITE_MOVE")
}

/// Grant `amount` block to the acting monster. Used by moves whose
/// `Intent::Defend { block }` is SECONDARY (after the Attack): bead
/// `sts2-rs-qk2du`'s block-gain only fires for a move's primary
/// `intents.first()`, so a secondary Defend is otherwise dropped. Bead
/// `sts2-rs-97w3y` (same class as `sts2-rs-71kog` / `sts2-rs-viwkh`).
fn gain_secondary_defend_block(state: &mut CombatState, owner: CreatureUid, amount: i32) {
    sts2_core::hooks::apply_effects(
        state,
        &[sts2_core::hooks::Effect::GainBlockCreature {
            target: sts2_core::hooks::CreatureRef::Enemy(owner),
            amount,
        }],
    );
}

/// BowlbugEgg BITE secondary block — C# `BiteMove` `GainBlock(ProtectBlock)`,
/// `BowlbugEgg.cs:25` `(.., 8, 7)`.
fn bowlbug_egg_bite_effect(state: &mut CombatState, owner: CreatureUid) {
    let block = crate::ascension_scaling::deadly_block(
        state.ascension,
        sts2_core::MonsterKind::BowlbugEgg,
        "BITE",
        7,
    );
    gain_secondary_defend_block(state, owner, block);
}

/// TheObscura HARDENING_STRIKE secondary block — C# `HardeningStrikeMove`
/// `GainBlock(HardeningStrikeBlock)`, `TheObscura.cs:44` `(.., 7, 6)`.
fn the_obscura_hardening_strike_effect(state: &mut CombatState, owner: CreatureUid) {
    let block = crate::ascension_scaling::deadly_block(
        state.ascension,
        sts2_core::MonsterKind::TheObscura,
        "HARDENING_STRIKE_MOVE",
        6,
    );
    gain_secondary_defend_block(state, owner, block);
}

/// InfestedPrism RADIATE secondary block — C# `RadiateMove`
/// `GainBlock(RadiateBlock=16)`.
fn infested_prism_radiate_effect(state: &mut CombatState, owner: CreatureUid) {
    // `…wb8lk.11`: v0.107.1 RadiateBlock 16→11 at A0 (decomp
    // `InfestedPrism.cs:48 GetValueIfAscension(DeadlyEnemies, 13, 11)`).
    let block = crate::ascension_scaling::deadly_block(
        state.ascension,
        sts2_core::MonsterKind::InfestedPrism,
        "RADIATE",
        11,
    );
    gain_secondary_defend_block(state, owner, block);
}

/// Crusher GUARDED_STRIKE secondary block — C# `GuardedStrikeMove`
/// `GainBlock(18)`.
fn crusher_guarded_strike_effect(state: &mut CombatState, owner: CreatureUid) {
    gain_secondary_defend_block(state, owner, 18);
}

/// `BowlbugEgg` — BITE self-loop (Attack+Defend intent on the same
/// state).
pub fn create_bowlbug_egg_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "BITE",
        mv_eff(
            "BITE",
            smallvec![Intent::Attack { damage: 7 }, Intent::Defend { block: 7 }],
            "BITE",
            bowlbug_egg_bite_effect,
        ),
    );
    MonsterAi::new(s, "BITE")
}

/// `BowlbugNectar` — THRASH → BUFF → THRASH2 (self-loop on THRASH2).
pub fn create_bowlbug_nectar_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "THRASH",
        mv("THRASH", smallvec![Intent::Attack { damage: 3 }], "BUFF"),
    );
    s.insert(
        "BUFF",
        // Bead `sts2-rs-pdsdl`: telegraph-only Buff now applies StrengthPower
        // +15 to self per `BowlbugNectar.cs:68` (BuffStrengthGain=15).
        MonsterState::Move(MoveState {
            id: "BUFF",
            effect: bowlbug_nectar_buff_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 15
            }],
            follow_up: Some("THRASH2"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "THRASH2",
        mv(
            "THRASH2",
            smallvec![Intent::Attack { damage: 3 }],
            "THRASH2",
        ),
    );
    MonsterAi::new(s, "THRASH")
}

/// `BowlbugRock` — HEADBUTT → CHECK(off_balance? DIZZY : HEADBUTT).
///
/// C# `BowlbugRock` + `ImbalancedPower`: when a HEADBUTT is FULLY BLOCKED,
/// `ImbalancedPower.AfterDamageGiven` sets `BowlbugRock.IsOffBalance = true`;
/// the `POST_HEADBUTT` branch then telegraphs `DIZZY_MOVE` (a `StunIntent`,
/// no damage) instead of a second headbutt, and `DizzyMove` clears the flag.
/// The flag is the AI's per-instance `counter` (Rust analogue of the C#
/// bool), set by the `Imbalanced` arm of `dispatch_after_damage_given`
/// (sts2-core) on a fully-blocked owner attack and reset to 0 here by the
/// DIZZY effect. Bead `sts2-rs-bowlbug-offbalance`. (Previously `CHECK`
/// collapsed to always-`HEADBUTT`, so Rust dealt a second 15-dmg headbutt
/// where C# was dizzy — coverage seed 1220, player.hp cs=23 rs=8.)
pub fn create_bowlbug_rock_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "HEADBUTT",
        mv(
            "HEADBUTT",
            smallvec![Intent::Attack { damage: 15 }],
            "CHECK",
        ),
    );
    s.insert(
        "CHECK",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "CHECK",
            branches: smallvec![
                ConditionalBranch {
                    condition: ConditionFn::CounterAware(bowlbug_rock_is_off_balance),
                    state_id: "DIZZY",
                },
                cb(always_true, "HEADBUTT"),
            ],
        }),
    );
    s.insert(
        "DIZZY",
        MonsterState::Move(MoveState {
            id: "DIZZY",
            effect: bowlbug_rock_dizzy_effect,
            intents: smallvec![Intent::Stun],
            follow_up: Some("HEADBUTT"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "HEADBUTT")
}

/// `BowlbugRock` off-balance predicate: the `counter` carries the C#
/// `IsOffBalance` bool (1 = off-balance, set by the `Imbalanced` damage arm
/// on a fully-blocked headbutt). Bead `sts2-rs-bowlbug-offbalance`.
fn bowlbug_rock_is_off_balance(_: &CombatState, _: sts2_core::CreatureUid, counter: i32) -> bool {
    counter != 0
}

/// `DizzyMove` effect: clear the off-balance flag (C# `DizzyMove`:
/// `IsOffBalance = false`). Bead `sts2-rs-bowlbug-offbalance`.
fn bowlbug_rock_dizzy_effect(state: &mut CombatState, owner: sts2_core::CreatureUid) {
    if let Some(creature) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
        if let Some(ai) = creature.ai.as_mut() {
            ai.set_counter(0);
        }
    }
}

/// `BowlbugSilk` — TOXIC_SPIT ↔ THRASH (2-hit multi-attack).
pub fn create_bowlbug_silk_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "TOXIC_SPIT",
        // Bead `sts2-rs-pdsdl`: telegraph-only Debuff now applies WeakPower
        // +1 to the player per `BowlbugSilk.cs:84`.
        MonsterState::Move(MoveState {
            id: "TOXIC_SPIT",
            effect: bowlbug_silk_toxic_spit_effect,
            intents: smallvec![Intent::Debuff {
                kind: PowerKind::Weak,
                amount: 1
            }],
            follow_up: Some("THRASH"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "THRASH",
        mv(
            "THRASH",
            smallvec![Intent::MultiAttack { damage: 4, hits: 2 }],
            "TOXIC_SPIT",
        ),
    );
    MonsterAi::new(s, "TOXIC_SPIT")
}

/// `Exoskeleton` — RAND(SKITTER | MANDIBLE→ENRAGE) with slot-dependent
/// initial state (matches Python `slot_map`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExoskeletonSlot {
    First,
    Second,
    Third,
    Fourth,
}

pub fn create_exoskeleton_ai(slot: ExoskeletonSlot) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            branches: smallvec![
                br("SKITTER", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("MANDIBLE", MoveRepeatType::CannotRepeat, 1, 1.0),
            ],
        }),
    );
    s.insert(
        "SKITTER",
        mv(
            "SKITTER",
            smallvec![Intent::MultiAttack { damage: 1, hits: 3 }],
            "RAND",
        ),
    );
    s.insert(
        "MANDIBLE",
        mv(
            "MANDIBLE",
            smallvec![Intent::Attack { damage: 8 }],
            "ENRAGE",
        ),
    );
    s.insert(
        "ENRAGE",
        // Bead `sts2-rs-pdsdl`: telegraph-only Buff now applies StrengthPower
        // +2 to self per `Exoskeleton.cs:98`.
        MonsterState::Move(MoveState {
            id: "ENRAGE",
            effect: exoskeleton_enrage_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2
            }],
            follow_up: Some("RAND"),
            must_perform_once: false,
        }),
    );
    let initial = match slot {
        ExoskeletonSlot::First => "SKITTER",
        ExoskeletonSlot::Second => "MANDIBLE",
        ExoskeletonSlot::Third => "ENRAGE",
        ExoskeletonSlot::Fourth => "RAND",
    };
    MonsterAi::new(s, initial)
}

// ────────────────────────────────────────────────────────────────────────────
// Normal encounters
// ────────────────────────────────────────────────────────────────────────────

/// `Chomper` — CLAMP ↔ SCREECH. `scream_first` flips the starter.
pub fn create_chomper_ai(scream_first: bool) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "CLAMP",
        mv(
            "CLAMP",
            smallvec![Intent::MultiAttack { damage: 8, hits: 2 }],
            "SCREECH",
        ),
    );
    s.insert(
        "SCREECH",
        mv_eff(
            "SCREECH",
            smallvec![Intent::StatusCard],
            "CLAMP",
            chomper_screech_effect,
        ),
    );
    let initial = if scream_first { "SCREECH" } else { "CLAMP" };
    MonsterAi::new(s, initial)
}

/// `HunterKiller` — TENDERIZING_GOOP → RAND(BITE CannotRepeat | PUNCTURE w=2).
///
/// Rebased off Python pin `01c094d` to C# v0.103.2 canonical
/// (`MegaCrit.Sts2.Core.Models.Monsters/HunterKiller.cs`). Bead
/// `sts2-rs-2srb`, parent `sts2-rs-kgwj`. Python pin ships a stale
/// 60-65 HP / 2-move (HUNT 2×4+Vuln ↔ KILL 15) variant we diverge
/// from per the monster-stat audit
/// (`docs/perf/2026-05-18-monster-stat-audit.md`).
///
/// C# chain: initial TENDERIZING_GOOP_MOVE (DebuffIntent applying
/// TenderPower 1) → RandomBranchState picks between BITE_MOVE
/// (SingleAttack 17, CannotRepeat) and PUNCTURE_MOVE (MultiAttack
/// 7×3, weight 2). TenderPower's two-phase semantics (each player card
/// played this turn drops Str/Dex by 1, restored at the player's turn end)
/// are ported in `sts2_core::hooks` — `dispatch_after_card_played` +
/// `after_turn_end` Tender arms (bead `sts2-rs-ybtmu`, Group C).
pub fn create_hunter_killer_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "TENDERIZING_GOOP_MOVE",
        // Bead `sts2-rs-pdsdl`: telegraph-only Debuff now applies TenderPower
        // +1 to the player per `HunterKiller.cs:54`.
        MonsterState::Move(MoveState {
            id: "TENDERIZING_GOOP_MOVE",
            effect: hunter_killer_goop_effect,
            intents: smallvec![Intent::Debuff {
                kind: PowerKind::Tender,
                amount: 1,
            }],
            follow_up: Some("RAND"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            // C# `HunterKiller.cs`: AddBranch(BITE, CannotRepeat) +
            // AddBranch(PUNCTURE, 2). The `2` is the maxRepeats overload
            // (CanRepeatXTimes(2), weight 1f), NOT weight 2 — both weights
            // are 1 (total 2, not 3). Bead `sts2-rs-pp5ld`.
            branches: smallvec![
                br("BITE_MOVE", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("PUNCTURE_MOVE", MoveRepeatType::CanRepeatXTimes, 2, 1.0),
            ],
        }),
    );
    s.insert(
        "BITE_MOVE",
        mv(
            "BITE_MOVE",
            smallvec![Intent::Attack { damage: 17 }],
            "RAND",
        ),
    );
    s.insert(
        "PUNCTURE_MOVE",
        mv(
            "PUNCTURE_MOVE",
            smallvec![Intent::MultiAttack { damage: 7, hits: 3 }],
            "RAND",
        ),
    );
    MonsterAi::new(s, "TENDERIZING_GOOP_MOVE")
}

/// `Wriggler` — SPAWNED_MOVE (Stun) → alternating NASTY_BITE (6) /
/// WRIGGLE (Buff Str+2 + StatusCard). Spawned by LouseProgenitor with
/// StartStunned=true, so the initial state is the spawn-turn stun.
///
/// Port-pin: aligned to C# decomp
/// `MegaCrit.Sts2.Core.Models.Monsters/Wriggler.cs::GenerateMoveStateMachine`,
/// not Python `sts2_env/monsters/act2.py:create_wriggler` (which is a
/// content bug — 8-10 HP / single-BITE / 5 dmg). See bead
/// `sts2-rs-n76j` and `docs/perf/2026-05-18-monster-stat-audit.md`.
pub fn create_wriggler_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SPAWNED_MOVE",
        mv("SPAWNED_MOVE", smallvec![Intent::Stun], "NASTY_BITE"),
    );
    s.insert(
        "NASTY_BITE",
        mv(
            "NASTY_BITE",
            smallvec![Intent::Attack { damage: 6 }],
            "WRIGGLE",
        ),
    );
    s.insert(
        "WRIGGLE",
        // Bead `sts2-rs-pdsdl`: telegraph-only Buff now applies StrengthPower
        // +2 to self per `Wriggler.cs:99`. The sibling `Intent::StatusCard`
        // telegraphs the Infection card-add (Discard, 1) — landed by
        // `sts2-rs-tasgy` inside `wriggler_wriggle_effect`.
        MonsterState::Move(MoveState {
            id: "WRIGGLE",
            effect: wriggler_wriggle_effect,
            intents: smallvec![
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2,
                },
                Intent::StatusCard,
            ],
            follow_up: Some("NASTY_BITE"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "SPAWNED_MOVE")
}

/// `LouseProgenitor` — WEB_CANNON (Attack 9 + Frail 2) → CURL_AND_GROW
/// (Defend 14 + Strength 5) → POUNCE (Attack 14) → loop. Mirrors C#
/// `LouseProgenitor.cs` `GenerateMoveStateMachine`, which returns
/// `new MonsterMoveStateMachine(list, moveState)` where `moveState =
/// WEB_CANNON_MOVE` (the chain is WEB → CURL_AND_GROW → POUNCE → WEB).
/// On spawn the engine applies `CurlUp(14)` via `on_spawn_self_buffs`
/// (`monsters.rs`), matching C# `AfterAddedToRoom` `PowerCmd.Apply<CurlUpPower>`.
///
/// Bead `sts2-rs-ybtmu` (Act-2/3 parity vein): replaces the fabricated
/// Python-pin SPAWN→SCRATCH moveset (2-Wriggler summon, 8-dmg loop) — C#
/// LouseProgenitor never summons (solo encounter; no summon move).
pub fn create_louse_progenitor_ai() -> MonsterAi {
    // Defensive install: every LouseProgenitor spawn flows through here, so
    // guarding the CurlUp AfterDamageReceived hook registration at this gate
    // ensures the reactive block-grant listener is present even when a test
    // bypasses the full content-boot path. Bead `sts2-rs-ybtmu`.
    static INSTALL_CURL_UP: std::sync::Once = std::sync::Once::new();
    INSTALL_CURL_UP.call_once(install_curl_up_handlers);
    let mut s = BTreeMap::new();
    s.insert(
        "WEB_CANNON_MOVE",
        MonsterState::Move(MoveState {
            id: "WEB_CANNON_MOVE",
            effect: louse_progenitor_web_effect,
            intents: smallvec![
                Intent::Attack { damage: 9 },
                Intent::Debuff {
                    kind: PowerKind::Frail,
                    amount: 2,
                },
            ],
            follow_up: Some("CURL_AND_GROW_MOVE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "CURL_AND_GROW_MOVE",
        MonsterState::Move(MoveState {
            id: "CURL_AND_GROW_MOVE",
            effect: louse_progenitor_curl_and_grow_effect,
            intents: smallvec![
                Intent::Defend { block: 14 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 5,
                },
            ],
            follow_up: Some("POUNCE_MOVE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "POUNCE_MOVE",
        mv(
            "POUNCE_MOVE",
            smallvec![Intent::Attack { damage: 14 }],
            "WEB_CANNON_MOVE",
        ),
    );
    MonsterAi::new(s, "WEB_CANNON_MOVE")
}

/// `Myte`'s starting move depends on its slot in the encounter:
/// `"first"` opens at TOXIC_MOVE, `"second"` opens at SUCK_MOVE
/// (C# `Myte.cs` `ConditionalBranchState("INIT_MOVE")`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MyteSlot {
    First,
    Second,
}

/// `Myte` — TOXIC_MOVE → BITE_MOVE → SUCK_MOVE → TOXIC (linear cycle).
/// Mirrors C# decomp `MegaCrit.Sts2.Core.Models.Monsters/Myte.cs`
/// v0.103.2 (bead `sts2-rs-t85w`, parent `sts2-rs-kgwj`). The Python
/// pin `01c094d` ships a stale 22-26 HP / RAND(BITE | INFEST) variant
/// we deliberately diverge from per the monster-stat audit
/// (`docs/perf/2026-05-18-monster-stat-audit.md`).
///
/// C# gates the initial state on `Creature.SlotName` via an
/// `INIT_MOVE` ConditionalBranchState: `"first"` starts at TOXIC_MOVE,
/// `"second"` starts at SUCK_MOVE (the two `MytesNormal` slots). The
/// follow-up cycle is identical for both slots — only the entry point
/// differs — so we select the opener from [`MyteSlot`] rather than
/// model the branch state (bead `sts2-rs-ybtmu`). This matters for the
/// 2-Myte encounter: only the `"first"` Myte casts TOXIC on turn 1, so
/// the player receives exactly 2 Toxic cards, not 4.
///
/// TOXIC_MOVE injects 2 `Toxic` status cards into the player's hand via
/// `myte_infest_effect` (bead `sts2-rs-baxi5`), matching C#
/// `CardPileCmd.AddToCombatAndPreview<Toxic>(targets, PileType.Hand, 2)`.
pub fn create_myte_ai(slot: MyteSlot) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "TOXIC_MOVE",
        mv_eff(
            "TOXIC_MOVE",
            smallvec![Intent::StatusCard],
            "BITE_MOVE",
            myte_infest_effect,
        ),
    );
    s.insert(
        "BITE_MOVE",
        mv(
            "BITE_MOVE",
            smallvec![Intent::Attack { damage: 13 }],
            "SUCK_MOVE",
        ),
    );
    s.insert(
        "SUCK_MOVE",
        // Bead `sts2-rs-pdsdl`: telegraph-only Buff now applies StrengthPower
        // +2 to self per `Myte.cs:91` (SuckStrength=2). Attack 4 = intent.
        MonsterState::Move(MoveState {
            id: "SUCK_MOVE",
            effect: myte_suck_effect,
            intents: smallvec![
                Intent::Attack { damage: 4 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2,
                },
            ],
            follow_up: Some("TOXIC_MOVE"),
            must_perform_once: false,
        }),
    );
    let initial = match slot {
        MyteSlot::First => "TOXIC_MOVE",
        MyteSlot::Second => "SUCK_MOVE",
    };
    MonsterAi::new(s, initial)
}

/// `ToughEgg` — 2-move chain per C# `ToughEgg.cs`
/// `GenerateMoveStateMachine` (bead `sts2-rs-z4jsg`; replaces the prior
/// WAIT-loop stub):
/// - HATCH_MOVE → `SummonIntent`; sheds non-Minion powers + re-rolls HP
///   to the 19-22 Hatchling band ([`tough_egg_hatch_effect`]);
///   follow=NIBBLE_MOVE.
/// - NIBBLE_MOVE → `SingleAttackIntent(NibbleDamage=4)` self-loop.
/// The machine STARTS at HATCH_MOVE: an egg laid mid-enemy-turn rolls
/// it as its first telegraph at that turn's end and hatches on its
/// first acting turn. Spawned by Ovicopter LAY_EGGS, so a clone of a
/// per-thread prototype (`proto_ai`).
pub fn create_tough_egg_ai() -> MonsterAi {
    thread_local! {
        static PROTO: ProtoAis<1> = const { proto_ais() };
    }
    proto_ai(&PROTO, 0, build_tough_egg_ai)
}

pub(crate) fn build_tough_egg_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "HATCH_MOVE",
        mv_eff(
            "HATCH_MOVE",
            smallvec![Intent::Summon],
            "NIBBLE_MOVE",
            tough_egg_hatch_effect,
        ),
    );
    s.insert(
        "NIBBLE_MOVE",
        mv(
            "NIBBLE_MOVE",
            smallvec![Intent::Attack { damage: 4 }],
            "NIBBLE_MOVE",
        ),
    );
    MonsterAi::new(s, "HATCH_MOVE")
}

/// `Ovicopter` — 4-move chain with conditional re-summon per C# decomp
/// `MegaCrit.Sts2.Core.Models.Monsters/Ovicopter.cs` v0.103.2
/// (`GenerateMoveStateMachine`). Bead `sts2-rs-7qd1`, parent
/// `sts2-rs-kgwj` (Pattern A — Python pin `01c094d` ships a stale
/// 2-move LAY_EGGS↔DIVE 14 variant we deliberately diverge from per
/// the monster-stat audit). Originally landed under `sts2-rs-r9pl` /
/// `sts2-rs-47p5`.
///
/// Move payloads track the C# constants verbatim:
/// - LAY_EGGS_MOVE → `SummonIntent` (spawns 3 `ToughEgg`s via
///   [`ovicopter_lay_eggs_effect`]); follow=SMASH_MOVE.
/// - SMASH_MOVE → `SingleAttackIntent(SmashDamage=16)`; follow=TENDERIZER_MOVE.
/// - TENDERIZER_MOVE → `SingleAttackIntent(TenderizerDamage=7)` +
///   `DebuffIntent` (Vulnerable 2 to player); follow=SUMMON_BRANCH_STATE.
/// - NUTRITIONAL_PASTE_MOVE → `BuffIntent` (Strength
///   `NutritionalPasteStrengthAmount=3` self-buff); follow=SMASH_MOVE.
/// - SUMMON_BRANCH_STATE — C# `CanLay = teammates.Count(c =>
///   c.IsAlive) <= 3`. If true → LAY_EGGS; else → NUTRITIONAL_PASTE.
///   Encoded here as [`ConditionFn::OwnerAware`]
///   [`ovicopter_can_lay`] which counts alive enemies excluding the
///   owner.
pub fn create_ovicopter_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "LAY_EGGS_MOVE",
        mv_eff(
            "LAY_EGGS_MOVE",
            smallvec![Intent::Summon],
            "SMASH_MOVE",
            ovicopter_lay_eggs_effect,
        ),
    );
    s.insert(
        "SMASH_MOVE",
        mv(
            "SMASH_MOVE",
            smallvec![Intent::Attack { damage: 16 }],
            "TENDERIZER_MOVE",
        ),
    );
    s.insert(
        "TENDERIZER_MOVE",
        // Bead `sts2-rs-pdsdl`: telegraph-only Debuff now applies
        // VulnerablePower +2 to the player per `Ovicopter.cs:110`. Attack 7 =
        // intent.
        MonsterState::Move(MoveState {
            id: "TENDERIZER_MOVE",
            effect: ovicopter_tenderizer_effect,
            intents: smallvec![
                Intent::Attack { damage: 7 },
                Intent::Debuff {
                    kind: PowerKind::Vulnerable,
                    amount: 2,
                },
            ],
            follow_up: Some("SUMMON_BRANCH"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "NUTRITIONAL_PASTE_MOVE",
        // Bead `sts2-rs-pdsdl`: telegraph-only Buff now applies StrengthPower
        // +3 to self per `Ovicopter.cs:93` (NutritionalPasteStrengthAmount=3).
        MonsterState::Move(MoveState {
            id: "NUTRITIONAL_PASTE_MOVE",
            effect: ovicopter_nutritional_paste_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 3,
            }],
            follow_up: Some("SMASH_MOVE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "SUMMON_BRANCH",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "SUMMON_BRANCH",
            branches: smallvec![
                ConditionalBranch {
                    condition: ConditionFn::OwnerAware(ovicopter_can_lay),
                    state_id: "LAY_EGGS_MOVE",
                },
                ConditionalBranch {
                    condition: ConditionFn::Stateless(always_true),
                    state_id: "NUTRITIONAL_PASTE_MOVE",
                },
            ],
        }),
    );
    MonsterAi::new(s, "LAY_EGGS_MOVE")
}

/// Bead `sts2-rs-7qd1` / `sts2-rs-z4jsg`: C# `Ovicopter.CanLay =
/// GetTeammatesOf(Creature).Count(c => c.IsAlive) <= 3`.
/// `CombatState.GetTeammatesOf` returns `GetCreaturesOnSide(side)` —
/// the ENTIRE enemy side INCLUDING the Ovicopter itself
/// (CombatState.cs:509-512; there is no self-exclusion). So with 3
/// live hatchlings the side count is 4 and the branch takes
/// NUTRITIONAL_PASTE; a re-lay needs an egg/hatchling to have died.
/// The prior owner-excluding count re-laid one cycle early (coverage
/// `encounter:OVICOPTER_NORMAL` monsters.len 4 vs 7 at turn 3).
/// Alive-filtered because Rust keeps corpses in `enemy_uids`
/// (`sts2-rs-c8why`) while C# `IsAlive` gates the count.
fn ovicopter_can_lay(state: &CombatState, owner: CreatureUid) -> bool {
    let _ = owner;
    let alive_side = state
        .enemy_uids
        .iter()
        .filter(|&&uid| {
            state
                .creatures
                .iter()
                .any(|c| c.id == uid.0 && c.current_hp > 0)
        })
        .count();
    alive_side <= 3
}

/// `SlumberingBeetle` WAKE_UP_MOVE body — C# `SlumberingBeetle.WakeUpMove`
/// (SlumberingBeetle.cs:95-107): remove Plating, nothing else (no attack, no
/// Strength). This is the `CreatureCmd.Stun(WakeUpMove, "ROLL_OUT_MOVE")` stun
/// body on the **damage-path** wake — it runs as the beetle's no-op stun turn
/// (a `SleepIntent` telegraph → 0 damage), then the AI advances to
/// ROLL_OUT_MOVE the turn after. The turn-end-path wake removes Plating inside
/// `tick_slumber_and_maybe_wake` (refresh_telegraph=true) and forces ROLL_OUT
/// directly, so this state is reached only via the damage path. Bead
/// `sts2-rs-7i736`.
fn slumbering_beetle_wake_effect(state: &mut CombatState, owner: CreatureUid) {
    if let Some(plating_power) = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().copied().find_map(|u| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == u.0 && p.kind == PowerKind::Plating)
                    .map(|_| u.0)
            })
        })
    {
        state.powers.retain(|p| p.id != plating_power);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != plating_power);
        }
    }
}

/// `SlumberingBeetle` ROLL_OUT_MOVE self-buff — `SlumberingBeetle.cs:145`
/// `PowerCmd.Apply<StrengthPower>(base.Creature, 2m, …)` after the rollout
/// attack resolves. The `Intent::Attack { 16 }` is performed by
/// `perform_intent`; this effect applies the trailing +2 Strength (the
/// telegraph carries a `BuffIntent`, which is display-only).
fn slumbering_beetle_rollout_effect(state: &mut CombatState, owner: CreatureUid) {
    // C# WakeUpMove (SlumberingBeetle.cs:95-107) executes as the Stun body and
    // removes Plating. On the damage-path wake, Plating removal was deferred from
    // tick_slumber_and_maybe_wake (refresh_telegraph=false) to here (the move body);
    // on the turn-end path, it already happened in tick_slumber_and_maybe_wake
    // (refresh_telegraph=true) before the move redirect, so this is a no-op there.
    if let Some(plating_power) = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| {
            c.powers.iter().copied().find_map(|u| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == u.0 && p.kind == PowerKind::Plating)
                    .map(|_| u.0)
            })
        })
    {
        state.powers.retain(|p| p.id != plating_power);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != plating_power);
        }
    }
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 2);
}

/// `SlumberingBeetle` — SNORE_MOVE (Sleep, self-loop while asleep) → wake →
/// ROLL_OUT_MOVE (Attack 16 + Strength +2, self-loop).
///
/// Rebased off the fabricated Python-pin `SLEEP → CHECK(always SLEEP) →
/// GORE(9)` — a degraded loop that never woke and whose phantom 9-damage
/// GORE was unreachable — to C# v0.103.2 `SlumberingBeetle.cs`. On spawn the
/// engine applies Plating(15) + Slumber(3) via `on_spawn_self_buffs`. The
/// beetle SNOREs (`SleepIntent`, no-op) while it holds SlumberPower;
/// SlumberPower decrements at each of the beetle's own turn-ends AND on each
/// unblocked-damage hit (C# `SlumberPower.AfterTurnEnd` / `AfterDamageReceived`).
/// At Slumber 0 the beetle wakes: Plating is stripped and its move is forced
/// to `ROLL_OUT_MOVE` — the C# `Stun(WakeUpMove, "ROLL_OUT_MOVE")` analogue —
/// wired in `sts2_core::hooks` (the turn-end + damage-received Slumber arms),
/// reusing the Group-A `Effect::ForceMonsterMove` redirect.
///
/// Validated against the C# headless harness (bead `sts2-rs-ybtmu`): with the
/// player not breaking the Plating shell, Slumber ticks 3→2→1→0 over the
/// beetle's first three turn-ends and it ROLL_OUTs on the fourth. RolloutDamage
/// = 16 (non-asc; 18 @ DeadlyEnemies).
pub fn create_slumbering_beetle_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SNORE_MOVE",
        mv("SNORE_MOVE", smallvec![Intent::Sleep], "SNORE_MOVE"),
    );
    // Damage-path stun body: C# `CreatureCmd.Stun(WakeUpMove, "ROLL_OUT_MOVE")`
    // runs WakeUpMove (remove Plating) as a no-op stun turn, THEN advances to
    // ROLL_OUT_MOVE the turn after. Forced from `tick_slumber_and_maybe_wake`
    // on the unblocked-damage wake. Telegraph is `Intent::Stun`, NOT Sleep:
    // C# `Creature.StunInternal` builds `MoveState("STUNNED", …, new
    // StunIntent())` and `SetMoveImmediate` re-points `NextMove` at it, so
    // the wire shows a Stun icon the moment the beetle wakes (9 v28wr intent
    // events were the mirror still telegraphing Sleep). The stun turn still
    // deals 0 and applies no Strength. Bead `sts2-rs-7i736`.
    s.insert(
        "WAKE_UP_MOVE",
        mv_eff(
            "WAKE_UP_MOVE",
            smallvec![Intent::Stun],
            "ROLL_OUT_MOVE",
            slumbering_beetle_wake_effect,
        ),
    );
    s.insert(
        "ROLL_OUT_MOVE",
        mv_eff(
            "ROLL_OUT_MOVE",
            smallvec![
                Intent::Attack { damage: 16 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2,
                },
            ],
            "ROLL_OUT_MOVE",
            slumbering_beetle_rollout_effect,
        ),
    );
    MonsterAi::new(s, "SNORE_MOVE")
}

/// `SpinyToad` — LASH → SPINES → LASH2 → EXPLOSION → LASH …
pub fn create_spiny_toad_ai() -> MonsterAi {
    // Bead `sts2-rs-or4a6`: C# `SpinyToad.cs:55-65`
    // `GenerateMoveStateMachine` ships a 3-state cycle starting at
    // PROTRUDING_SPIKES (Rust "SPINES"): SPINES → EXPLOSION → LASH →
    // SPINES. The prior Rust port had a 4-state LASH → SPINES → LASH2 →
    // EXPLOSION cycle starting at LASH — both an extra duplicated LASH
    // and a wrong initial state. Telegraph payload also corrected from
    // Thorns 8 → 5 to match the `SpikesMove` apply amount
    // (`PowerCmd.Apply<ThornsPower>(self, 5m)`); the effect already
    // applied 5.
    let mut s = BTreeMap::new();
    s.insert(
        "SPINES",
        MonsterState::Move(MoveState {
            id: "SPINES",
            effect: spiny_toad_spines_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Thorns,
                amount: 5
            }],
            follow_up: Some("EXPLOSION"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "EXPLOSION",
        // Bead `sts2-rs-cezr9`: C# `SpinyToad.cs:78-86` ExplosionMove
        // fires `PowerCmd.Apply<ThornsPower>(self, -5m)` after the
        // attack — strip the +5 Thorns SPINES applied last cycle.
        // The prior `mv()` body leaked +5 Thorns per full cycle.
        mv_eff(
            "EXPLOSION",
            smallvec![Intent::Attack { damage: 23 }],
            "LASH",
            spiny_toad_explosion_effect,
        ),
    );
    s.insert(
        "LASH",
        mv("LASH", smallvec![Intent::Attack { damage: 17 }], "SPINES"),
    );
    MonsterAi::new(s, "SPINES")
}

/// `TheObscura` — ILLUSION_MOVE → RAND{PIERCING_GAZE | SAIL |
/// HARDENING_STRIKE}, each branch `CannotRepeat`. Mirrors C# decomp
/// `MegaCrit.Sts2.Core.Models.Monsters/TheObscura.cs` v0.103.2 (bead
/// `sts2-rs-w1kh`, parent `sts2-rs-kgwj`). The Python pin `01c094d`
/// ships a stale 36-39 HP / DARKNESS-CHOMP-STRIKE variant we
/// deliberately diverge from per the monster-stat audit
/// (`docs/perf/2026-05-18-monster-stat-audit.md`).
///
/// ILLUSION_MOVE is `SummonIntent` (spawn `Parafright`) in C#; wired
/// via `the_obscura_illusion_effect` (bead `sts2-rs-3szrv`), which
/// front-inserts a Parafright per `TheObscuraNormal.Slots`. AI
/// attachment for the spawn is a follow-up. All three random branches
/// return to RAND, matching C# `RandomBranchState` with
/// `MoveRepeatType.CannotRepeat`.
///
/// SAIL_MOVE in C# applies `StrengthPower(3)` to teammates via
/// `GetTeammatesOf(Creature)` → `GetCreaturesOnSide(Enemy)` (all
/// living enemy-side creatures, including self). Wired via
/// `the_obscura_sail_effect` in bead `sts2-rs-0q46p` (w1kh
/// follow-up); previously a telegraph-only `mv()` noop.
pub fn create_the_obscura_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "ILLUSION_MOVE",
        mv_eff(
            "ILLUSION_MOVE",
            smallvec![Intent::Summon],
            "RAND",
            the_obscura_illusion_effect,
        ),
    );
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            branches: smallvec![
                br("PIERCING_GAZE_MOVE", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("SAIL_MOVE", MoveRepeatType::CannotRepeat, 1, 1.0),
                br(
                    "HARDENING_STRIKE_MOVE",
                    MoveRepeatType::CannotRepeat,
                    1,
                    1.0
                ),
            ],
        }),
    );
    s.insert(
        "PIERCING_GAZE_MOVE",
        mv(
            "PIERCING_GAZE_MOVE",
            smallvec![Intent::Attack { damage: 10 }],
            "RAND",
        ),
    );
    s.insert(
        "SAIL_MOVE",
        mv_eff(
            "SAIL_MOVE",
            smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 3,
            }],
            "RAND",
            the_obscura_sail_effect,
        ),
    );
    s.insert(
        "HARDENING_STRIKE_MOVE",
        mv_eff(
            "HARDENING_STRIKE_MOVE",
            smallvec![Intent::Attack { damage: 6 }, Intent::Defend { block: 6 }],
            "RAND",
            the_obscura_hardening_strike_effect,
        ),
    );
    MonsterAi::new(s, "ILLUSION_MOVE")
}

// ────────────────────────────────────────────────────────────────────────────
// Elite encounters
// ────────────────────────────────────────────────────────────────────────────

/// `DecimillipedeSegment` — RAND(WRITHE | BULK | CONSTRICT) with linear
/// post-move chain `WRITHE→CONSTRICT→BULK→WRITHE`. Initial state
/// depends on the segment's `starter_idx` (0=WRITHE, 1=BULK,
/// 2=CONSTRICT). Bead `sts2-rs-va5r`: REATTACH emits `Intent::Heal`;
/// DEAD telegraphs nothing (C# `DEAD_MOVE` is constructed with no
/// intents — `DecimillipedeSegment.cs:152`).
pub fn create_decimillipede_segment_ai(starter_idx: u8) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "RAND",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RAND",
            branches: smallvec![
                br("WRITHE", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("BULK", MoveRepeatType::CannotRepeat, 1, 1.0),
                br("CONSTRICT", MoveRepeatType::CannotRepeat, 1, 1.0),
            ],
        }),
    );
    s.insert(
        "WRITHE",
        mv(
            "WRITHE",
            smallvec![Intent::MultiAttack { damage: 5, hits: 2 }],
            "CONSTRICT",
        ),
    );
    s.insert(
        "CONSTRICT",
        // Bead `sts2-rs-pdsdl`: telegraph-only Debuff now applies WeakPower +1
        // to the player per `DecimillipedeSegment.cs:203`. Attack 8 = intent.
        MonsterState::Move(MoveState {
            id: "CONSTRICT",
            effect: decimillipede_constrict_effect,
            intents: smallvec![
                Intent::Attack { damage: 8 },
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 1
                }
            ],
            follow_up: Some("BULK"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "BULK",
        // Bead `sts2-rs-pdsdl`: telegraph-only Buff now applies StrengthPower
        // +2 to self per `DecimillipedeSegment.cs:194` (BulkStrength=2).
        // Attack 6 = intent.
        MonsterState::Move(MoveState {
            id: "BULK",
            effect: decimillipede_bulk_effect,
            intents: smallvec![
                Intent::Attack { damage: 6 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2
                }
            ],
            follow_up: Some("WRITHE"),
            must_perform_once: false,
        }),
    );
    // Bead `sts2-rs-va5r`: REATTACH now emits `Intent::Heal` (matches
    // Python pin `act2.py:488`). DEAD telegraphs NOTHING: C#
    // `DeadState = new MoveState("DEAD_MOVE", DeadMove)`
    // (`DecimillipedeSegment.cs:152`) passes no `params AbstractIntent[]`,
    // so `Intents` is empty. The Python pin's `IntentType.UNKNOWN`
    // (`act2.py:487`) was a fabrication — the game has no `UnknownIntent`
    // type at all, and the corpse's empty telegraph is observable on the
    // wire, so the fabricated entry read as a divergence on every board a
    // dead segment was on (51 records on the v16 sweep).
    //
    // Bead `sts2-rs-8lwnq`: the revive is real. A dead segment is armed
    // onto DEAD by `ReattachPower.AfterDeath` (core
    // `arm_reattach_dead_state`), no-ops one enemy turn, then performs
    // REATTACH at its own roster slot — whose move body heals it back
    // (`DecimillipedeSegment.ReattachMove` → `ReattachPower.DoReattach`).
    s.insert("DEAD", mv("DEAD", smallvec![], "REATTACH"));
    s.insert(
        "REATTACH",
        MonsterState::Move(MoveState {
            id: "REATTACH",
            effect: decimillipede_reattach_effect,
            intents: smallvec![Intent::Heal],
            follow_up: Some("RAND"),
            must_perform_once: true,
        }),
    );
    let initial = match starter_idx {
        1 => "BULK",
        2 => "CONSTRICT",
        _ => "WRITHE",
    };
    MonsterAi::new(s, initial)
}

/// `DecimillipedeSegmentFront` — wrapper around
/// [`create_decimillipede_segment_ai`] preserving the Python C# class
/// distinction (visual only).
pub fn create_decimillipede_segment_front_ai(starter_idx: u8) -> MonsterAi {
    create_decimillipede_segment_ai(starter_idx)
}

/// `DecimillipedeSegmentMiddle` — wrapper around
/// [`create_decimillipede_segment_ai`] (visual only).
pub fn create_decimillipede_segment_middle_ai(starter_idx: u8) -> MonsterAi {
    create_decimillipede_segment_ai(starter_idx)
}

/// `DecimillipedeSegmentBack` — wrapper around
/// [`create_decimillipede_segment_ai`] (visual only).
pub fn create_decimillipede_segment_back_ai(starter_idx: u8) -> MonsterAi {
    create_decimillipede_segment_ai(starter_idx)
}

/// `Entomancer` — BEES (3×7) → SPEAR (18) → PHEROMONE_SPIT (buff or
/// hive-stack — function-pointer predicate can't see per-instance hive
/// counter, so PHEROMONE_SPIT always emits the buff metadata; the
/// dispatcher bead picks which power to actually apply).
pub fn create_entomancer_ai() -> MonsterAi {
    // Defensive install: every Entomancer spawn flows through here, so
    // guarding the PersonalHive AfterDamageReceived hook registration at
    // this gate ensures the Dazed-flood listener is present even when a
    // test bypasses the full content-boot path. Bead `sts2-rs-ybtmu`.
    static INSTALL_PERSONAL_HIVE: std::sync::Once = std::sync::Once::new();
    INSTALL_PERSONAL_HIVE.call_once(install_personal_hive_handlers);
    let mut s = BTreeMap::new();
    s.insert(
        "BEES",
        mv(
            "BEES",
            smallvec![Intent::MultiAttack { damage: 3, hits: 7 }],
            "SPEAR",
        ),
    );
    s.insert(
        "SPEAR",
        mv(
            "SPEAR",
            smallvec![Intent::Attack { damage: 18 }],
            "PHEROMONE_SPIT",
        ),
    );
    // Bead `sts2-rs-pnv5`: PHEROMONE_SPIT effect mirrors Python pin —
    // increment `PERSONAL_HIVE` (cap 3) or apply STRENGTH +2 when capped.
    s.insert(
        "PHEROMONE_SPIT",
        MonsterState::Move(MoveState {
            id: "PHEROMONE_SPIT",
            effect: entomancer_pheromone_spit_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2
            }],
            follow_up: Some("BEES"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "BEES")
}

/// `InfestedPrism` — 4-move ring JAB → RADIATE → WHIRLWIND →
/// PULSATE → JAB. Per C# decomp `InfestedPrism.cs`. HP 161 at A0
/// (was 200) in core monsters.rs.
///
/// Move ring (v0.107.1 decomp, A0 base values):
/// - JAB_MOVE       — Attack 15
/// - RADIATE_MOVE   — Attack 11 + Defend 11
/// - WHIRLWIND_MOVE — MultiAttack 5x3
/// - PULSATE_MOVE   — Attack 8 + Buff (VitalSpark) + Defend 20
///
/// `…wb8lk.11`: all four moves were cut and PULSATE became an attack that
/// re-applies VitalSpark (the Tainted applier) instead of granting Strength.
///
/// AfterAddedToRoom: applies VitalSparkPower (VitalSparkAmount, A0 2). The
/// on-spawn power is registered in core `monsters.rs`
/// (`MonsterKind::InfestedPrism => VitalSpark(2)`). v0.107.1 reworked
/// VitalSpark into the **Tainted**-keyword
/// applier (bead `…wb8lk.4`): at `BeforeCombatStart` it taints every player
/// Skill card, and when a Tainted card is played it stacks `TaintedPower` on the
/// player (extra damage from powered attacks until the enemy turn ends). The
/// v0.103.2 energy-on-powered-hit behavior was removed. The amount stays 1 here;
/// the v0.107.1 `VitalSparkAmount` 5→3 stat change is bead `…wb8lk.11`.
///
/// `slot` parameter retained for backward compat with existing
/// spawn-args plumbing. The 4-move ring is slot-insensitive in
/// the C# spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfestedPrismSlot {
    First,
    Second,
    Other,
}

pub fn create_infested_prism_ai(_slot: InfestedPrismSlot) -> MonsterAi {
    // Register VitalSpark's v0.107.1 registry hook bodies (AfterCardEnteredCombat
    // + AfterRemoved) the first time an Infested Prism spawns. The other two legs
    // (BeforeCombatStart taint, AfterCardPlayed → TaintedPower) are dedicated core
    // functions and need no registration. Bead `…wb8lk.4`.
    static INSTALL_VITAL_SPARK: std::sync::Once = std::sync::Once::new();
    INSTALL_VITAL_SPARK
        .call_once(sts2_core::monster_power_effects::install_vital_spark_power_handlers);
    let mut s = BTreeMap::new();
    // `…wb8lk.11`: v0.107.1 stat cuts. Jab 22→15, Radiate 16→11 (+block 16→11),
    // Whirlwind 9→5, Pulsate is now an attack (8) + Block 20 + VitalSpark.
    s.insert(
        "JAB",
        mv("JAB", smallvec![Intent::Attack { damage: 15 }], "RADIATE"),
    );
    s.insert(
        "RADIATE",
        mv_eff(
            "RADIATE",
            smallvec![Intent::Attack { damage: 11 }, Intent::Defend { block: 11 }],
            "WHIRLWIND",
            infested_prism_radiate_effect,
        ),
    );
    s.insert(
        "WHIRLWIND",
        mv(
            "WHIRLWIND",
            smallvec![Intent::MultiAttack { damage: 5, hits: 3 }],
            "PULSATE",
        ),
    );
    s.insert(
        "PULSATE",
        // v0.107.1 `PulsateMove`: Attack 8 (primary) + Buff (VitalSpark, the
        // Tainted applier) + Defend 20 (both secondary, applied by
        // `infested_prism_pulsate_effect`). Decomp `InfestedPrism.cs:68,107-114`.
        MonsterState::Move(MoveState {
            id: "PULSATE",
            effect: infested_prism_pulsate_effect,
            intents: smallvec![
                Intent::Attack { damage: 8 },
                Intent::Buff {
                    kind: PowerKind::VitalSpark,
                    amount: 2,
                },
                Intent::Defend { block: 20 },
            ],
            follow_up: Some("JAB"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "JAB")
}

// ────────────────────────────────────────────────────────────────────────────
// Boss encounters
// ────────────────────────────────────────────────────────────────────────────

/// `TheInsatiable` — LIQUIFY_GROUND → THRASH_1 → LUNGING_BITE →
/// SALIVATE → THRASH_2 → THRASH_1 …
pub fn create_the_insatiable_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    // Bead `sts2-rs-asxdy`: LIQUIFY_GROUND's telegraph-only Buff +
    // StatusCard now apply `SandpitPower(4)` to the player and add 6
    // `FranticEscape` status cards (3 Draw / 3 Discard) via the move's
    // `effect_fn`, per C# `TheInsatiable.cs` `LiquifyMove`. (Supersedes
    // the `sts2-rs-09k9` `(Strength, 0)` placeholder — `PowerKind::Sandpit`
    // does exist.)
    s.insert(
        "LIQUIFY_GROUND",
        MonsterState::Move(MoveState {
            id: "LIQUIFY_GROUND",
            effect: the_insatiable_liquify_ground_effect,
            intents: smallvec![
                Intent::Buff {
                    kind: PowerKind::Sandpit,
                    amount: 4
                },
                Intent::StatusCard
            ],
            follow_up: Some("THRASH_1"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "THRASH_1",
        mv(
            "THRASH_1",
            smallvec![Intent::MultiAttack { damage: 8, hits: 2 }],
            "LUNGING_BITE",
        ),
    );
    s.insert(
        "LUNGING_BITE",
        mv(
            "LUNGING_BITE",
            smallvec![Intent::Attack { damage: 28 }],
            "SALIVATE",
        ),
    );
    s.insert(
        "SALIVATE",
        // Bead `sts2-rs-pdsdl`: telegraph-only Buff now applies StrengthPower
        // +2 to self per `TheInsatiable.cs:166` (SalivateStrength=2).
        MonsterState::Move(MoveState {
            id: "SALIVATE",
            effect: the_insatiable_salivate_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2
            }],
            follow_up: Some("THRASH_2"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "THRASH_2",
        mv(
            "THRASH_2",
            smallvec![Intent::MultiAttack { damage: 8, hits: 2 }],
            "THRASH_1",
        ),
    );
    MonsterAi::new(s, "LIQUIFY_GROUND")
}

/// `KnowledgeDemon` — CURSE_OF_KNOWLEDGE → SLAP → KNOWLEDGE_OVERWHELMING
/// → PONDER → CURSE_CHECK → {CURSE_OF_KNOWLEDGE | SLAP}.
///
/// Bead `sts2-rs-w4w7`: ported from a `combat.round < 3` proxy to a
/// proper per-monster counter mirroring Python's
/// `_state["curse_counter"] < 3` (`act2.py:692`). The counter
/// increments inside CURSE_OF_KNOWLEDGE's effect_fn (via the
/// k27e dispatcher extension that fires effect_fn for non-attack
/// intents), so Stun-skipping the curse intent now correctly delays
/// Phase-2 escalation — the round-proxy could not express that.
fn knowledge_demon_curse_counter_lt_three(
    _combat: &CombatState,
    _owner: sts2_core::CreatureUid,
    counter: i32,
) -> bool {
    counter < 3
}

/// CURSE_OF_KNOWLEDGE effect (beads `sts2-rs-w4w7` + `sts2-rs-ybtmu`).
///
/// C# `KnowledgeDemon.CurseOfKnowledge` (KnowledgeDemon.cs:153-200) shows
/// each player a blocking 2-card choose screen — option 0 is always
/// `Disintegration`, option 1 is `MindRot`/`Sloth`/`WasteAway` keyed by the
/// curse counter. The chosen card's `OnChosen()` applies its *power* to the
/// player (no card enters the deck), then `CurseOfKnowledgeCounter++`.
///
/// Rust models this as a real player choice raised at C#'s timing — MID enemy
/// turn. This effect stamps the pre-increment counter onto
/// [`CombatState::pending_curse_choice`]; the core tail of
/// `combat_loop::perform_intent` (`open_monster_curse_select`) turns that stamp
/// into the 2-option [`ChoiceKind::MonsterCurseSelect`] and
/// `run_enemy_perform_walk` parks the remainder of the enemy turn on
/// `CombatState::pending_enemy_exec_resume` until it resolves.
/// `resolve_monster_curse_select` applies the chosen curse power to the player,
/// then the enemy turn continues where it stopped. The prior port DEFERRED the
/// screen to the next `start_player_turn` on the theory that the four curse
/// powers only act at player-turn boundaries; that is true of the powers but
/// not of the pause — the live game halts at the same round/phase while the
/// deferred engine had already run a whole enemy turn, which is what the v27wr
/// sweep saw as ~81 intent + 95 `$.round` + 37 `$.phase` divergences and 35
/// unanswerable `card_select` injections. The headless parity harness resolves
/// the choice by picking index 0 (`Disintegration`), so the sweep stays
/// deterministic.
///
/// The counter is advanced here (not in the resolver): the only reader
/// before the next curse is CURSE_CHECK, rolled later THIS enemy turn, so
/// incrementing now advances the phase gate while the *pre*-increment value
/// rides on `pending_curse_choice` for the resolver's Disintegration-amount
/// lookup. A stun-skipped curse never runs this effect, so the counter and
/// the deferred choice both correctly stay put.
fn knowledge_demon_curse_of_knowledge_effect(
    state: &mut CombatState,
    owner: sts2_core::CreatureUid,
) {
    let counter = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .and_then(|c| c.ai.as_ref())
        .map_or(0, sts2_core::monster_ai::MonsterAi::counter);
    // CURSE_CHECK only re-enters CURSE_OF_KNOWLEDGE while counter < 3, so a
    // live combat never exceeds index 2; guard so a preset/overflowed counter
    // can't open a curse set C# wouldn't have.
    if counter < 3 {
        if let Ok(c) = u8::try_from(counter) {
            state.pending_curse_choice = Some(c);
        }
    }
    if let Some(creature) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
        if let Some(ai) = creature.ai.as_mut() {
            ai.increment_counter();
        }
    }
}

pub fn create_knowledge_demon_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        // CURSE_OF_KNOWLEDGE telegraphs a generic `DebuffIntent` (C#
        // `KnowledgeDemon.cs:139`). The concrete delivery — applying
        // `DisintegrationPower(6/7/8)` to the player, the harness's index-0
        // curse pick — happens in `knowledge_demon_curse_of_knowledge_effect`,
        // not the intent (telegraph intents apply nothing). The `(Weak, 0)`
        // payload is a neutral debuff-icon placeholder, not a real Weak apply.
        "CURSE_OF_KNOWLEDGE",
        MonsterState::Move(MoveState {
            id: "CURSE_OF_KNOWLEDGE",
            effect: knowledge_demon_curse_of_knowledge_effect,
            intents: smallvec![Intent::Debuff {
                kind: PowerKind::Weak,
                amount: 0
            }],
            follow_up: Some("SLAP"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "SLAP",
        mv(
            "SLAP",
            smallvec![Intent::Attack { damage: 17 }],
            "KNOWLEDGE_OVERWHELMING",
        ),
    );
    s.insert(
        "KNOWLEDGE_OVERWHELMING",
        mv(
            "KNOWLEDGE_OVERWHELMING",
            smallvec![Intent::MultiAttack { damage: 8, hits: 3 }],
            "PONDER",
        ),
    );
    s.insert(
        "PONDER",
        // Bead `sts2-rs-pdsdl`: telegraph-only Buff now applies StrengthPower
        // +2 to self per `KnowledgeDemon.cs:215` (PonderStrength=2).
        // Attack 11 = intent; Heal 30 (self) added per `PonderMove` — see
        // `knowledge_demon_ponder_effect`.
        MonsterState::Move(MoveState {
            id: "PONDER",
            effect: knowledge_demon_ponder_effect,
            intents: smallvec![
                Intent::Attack { damage: 11 },
                Intent::Heal,
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2
                }
            ],
            follow_up: Some("CURSE_CHECK"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "CURSE_CHECK",
        MonsterState::ConditionalBranch(ConditionalBranchState {
            id: "CURSE_CHECK",
            branches: smallvec![
                ConditionalBranch {
                    condition: ConditionFn::CounterAware(knowledge_demon_curse_counter_lt_three),
                    state_id: "CURSE_OF_KNOWLEDGE",
                },
                cb(always_true, "SLAP"),
            ],
        }),
    );
    MonsterAi::new(s, "CURSE_OF_KNOWLEDGE")
}

/// `Crusher` (KaiserCrab left) — THRASH → ENLARGING_STRIKE → BUG_STING
/// → ADAPT → GUARDED_STRIKE → THRASH …
pub fn create_crusher_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "THRASH",
        mv(
            "THRASH",
            smallvec![Intent::Attack { damage: 12 }],
            "ENLARGING_STRIKE",
        ),
    );
    s.insert(
        "ENLARGING_STRIKE",
        mv(
            "ENLARGING_STRIKE",
            smallvec![Intent::Attack { damage: 4 }],
            "BUG_STING",
        ),
    );
    s.insert(
        "BUG_STING",
        // Bead `sts2-rs-pdsdl`: telegraph-only Debuff now applies WeakPower +2
        // AND FrailPower +2 to the player per `Crusher.cs:144-145`. The Rust
        // intent declared only Weak; Frail was dropped — both apply here.
        // MultiAttack 6×2 = intent.
        MonsterState::Move(MoveState {
            id: "BUG_STING",
            effect: crusher_bug_sting_effect,
            intents: smallvec![
                Intent::MultiAttack { damage: 6, hits: 2 },
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 2
                }
            ],
            follow_up: Some("ADAPT"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "ADAPT",
        // Bead `sts2-rs-pdsdl`: telegraph-only Buff now applies StrengthPower
        // +2 to self per `Crusher.cs:151` (AdaptStrengthGain=2).
        MonsterState::Move(MoveState {
            id: "ADAPT",
            effect: crusher_adapt_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2
            }],
            follow_up: Some("GUARDED_STRIKE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "GUARDED_STRIKE",
        mv_eff(
            "GUARDED_STRIKE",
            smallvec![Intent::Attack { damage: 12 }, Intent::Defend { block: 18 }],
            "THRASH",
            crusher_guarded_strike_effect,
        ),
    );
    MonsterAi::new(s, "THRASH")
}

/// `Rocket` CHARGE_UP_MOVE: `BuffIntent` → `PowerCmd.Apply<StrengthPower>
/// (base.Creature, ChargeUpStrengthGain)` self. `ChargeUpStrengthGain` = 2
/// at Asc-0 (DeadlyEnemies = 3, ascension identity at the pin). Bead
/// `sts2-rs-i0niw`.
fn rocket_charge_up_effect(state: &mut CombatState, owner: CreatureUid) {
    let amount =
        ascension::get_value_if_ascension(state.ascension, AscensionLevel::DeadlyEnemies, 3, 2);
    apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// `Rocket` (KaiserCrab right) — five-state linear chain rebased to C#
/// v0.103.2 (`MegaCrit.Sts2.Core.Models.Monsters/Rocket.cs`
/// `GenerateMoveStateMachine`): TARGETING_RETICLE → PRECISION_BEAM →
/// CHARGE_UP → LASER → RECHARGE → TARGETING_RETICLE …
///
/// - TARGETING_RETICLE: `SingleAttackIntent(3)` — a plain 3-damage attack.
///   The prior port fabricated a `Vulnerable 2` debuff here; the C# move
///   body is `DamageCmd.Attack(TargetingReticleDamage)` with NO
///   `PowerCmd.Apply`, so the debuff is removed.
/// - PRECISION_BEAM: `SingleAttackIntent(18)`.
/// - CHARGE_UP:       `BuffIntent` → Strength 2 (self).
/// - LASER:           `SingleAttackIntent(31)`.
/// - RECHARGE:        `SleepIntent` (skip turn).
///
/// The prior port omitted CHARGE_UP and RECHARGE entirely and looped
/// TARGETING_RETICLE → PRECISION_BEAM → LASER. Damage amounts use Asc-0
/// values (DeadlyEnemies 4/20/35 are ascension identity at the pin). HP
/// (199/209) and the `AfterAddedToRoom` spawn powers (Surrounded /
/// BackAttackRight / CrabRage) live outside this moveset file and are
/// tracked separately. Bead `sts2-rs-i0niw`.
pub fn create_rocket_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "TARGETING_RETICLE_MOVE",
        mv(
            "TARGETING_RETICLE_MOVE",
            smallvec![Intent::Attack { damage: 3 }],
            "PRECISION_BEAM_MOVE",
        ),
    );
    s.insert(
        "PRECISION_BEAM_MOVE",
        mv(
            "PRECISION_BEAM_MOVE",
            smallvec![Intent::Attack { damage: 18 }],
            "CHARGE_UP_MOVE",
        ),
    );
    s.insert(
        "CHARGE_UP_MOVE",
        mv_eff(
            "CHARGE_UP_MOVE",
            smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2
            }],
            "LASER_MOVE",
            rocket_charge_up_effect,
        ),
    );
    s.insert(
        "LASER_MOVE",
        mv(
            "LASER_MOVE",
            smallvec![Intent::Attack { damage: 31 }],
            "RECHARGE_MOVE",
        ),
    );
    s.insert(
        "RECHARGE_MOVE",
        mv(
            "RECHARGE_MOVE",
            smallvec![Intent::Sleep],
            "TARGETING_RETICLE_MOVE",
        ),
    );
    MonsterAi::new(s, "TARGETING_RETICLE_MOVE")
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Bead `sts2-rs-z4jsg`: end-to-end LAY_EGGS → hatch lifecycle
    /// through real enemy turns. Locks three behaviors at once:
    ///   1. Mid-turn-summoned eggs do NOT get a phantom
    ///      `on_move_performed` advance (the step-3.5 walk is scoped to
    ///      `acting_enemies`) — they telegraph their never-performed
    ///      initial HATCH_MOVE (Summon) after the lay turn, exactly as
    ///      C#'s `FindNextMoveState` early-return keeps the machine on
    ///      the initial move until it performs.
    ///   2. The Ovicopter advances LAY_EGGS → SMASH normally.
    ///   3. On their first acting turn the eggs perform HATCH_MOVE:
    ///      HP re-rolls into the 19..=21 Hatchling band, the Hatch
    ///      power is shed (Minion kept), and the follow-up NIBBLE
    ///      (attack 4) telegraph rolls.
    #[test]
    fn z4jsg_lay_eggs_then_hatch_across_enemy_turns() {
        use sts2_core::CombatPhase;
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let ovi = state.spawn_enemy(MonsterKind::Ovicopter);
        state.set_creature_ai(ovi, create_ovicopter_ai());
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == ovi.0) {
            c.intent = Some(Intent::Summon);
            if let Some(ai) = c.ai.as_mut() {
                ai.force_current_state("LAY_EGGS_MOVE");
            }
        }
        // Lay turn: Ovicopter performs LAY_EGGS mid-enemy-turn.
        // Bead `sts2-rs-c8why`: the next-intent roll moved to `enemy_turn_end`
        // (Ovicopter LAY_EGGS→SMASH; mid-turn-summoned eggs stay on their
        // initial HATCH_MOVE via `roll_move`'s `performed_first_move` guard).
        state.phase = CombatPhase::EnemyTurnExec;
        let _ = sts2_core::combat_loop::enemy_turn_exec(&mut state);
        sts2_core::combat_loop::enemy_turn_end(&mut state);
        let eggs: Vec<CreatureUid> = state
            .enemy_uids
            .iter()
            .copied()
            .filter(|u| {
                state
                    .creatures
                    .iter()
                    .any(|c| c.id == u.0 && c.species == Some(MonsterKind::ToughEgg))
            })
            .collect();
        assert_eq!(eggs.len(), 3, "3 eggs laid");
        for uid in &eggs {
            let c = state.creatures.iter().find(|c| c.id == uid.0).unwrap();
            assert_eq!(
                c.ai.as_ref().map(MonsterAi::current_state_id),
                Some("HATCH_MOVE"),
                "no phantom advance — egg still on its initial HATCH_MOVE"
            );
            assert_eq!(c.intent, Some(Intent::Summon), "HATCH telegraph rolled");
            assert!(
                (14..=18).contains(&c.current_hp),
                "shell HP band 14..=18 (got {})",
                c.current_hp
            );
        }
        let ovi_c = state.creatures.iter().find(|c| c.id == ovi.0).unwrap();
        assert_eq!(
            ovi_c.ai.as_ref().map(MonsterAi::current_state_id),
            Some("SMASH_MOVE"),
            "Ovicopter advanced LAY_EGGS → SMASH"
        );

        // Hatch turn: eggs act first (front slots) and perform HATCH_MOVE.
        // Bead `sts2-rs-c8why`: roll (HATCH→NIBBLE) now in `enemy_turn_end`.
        state.phase = CombatPhase::EnemyTurnExec;
        let _ = sts2_core::combat_loop::enemy_turn_exec(&mut state);
        sts2_core::combat_loop::enemy_turn_end(&mut state);
        for uid in &eggs {
            let c = state.creatures.iter().find(|c| c.id == uid.0).unwrap();
            assert!(
                (19..=21).contains(&c.max_hp),
                "hatchling HP band 19..=21 (got {})",
                c.max_hp
            );
            assert_eq!(c.current_hp, c.max_hp, "hatchling at full new HP");
            assert_eq!(
                c.intent,
                Some(Intent::Attack { damage: 4 }),
                "NIBBLE follow-up telegraph"
            );
            assert!(
                !c.powers.iter().any(|pu| state
                    .powers
                    .iter()
                    .any(|p| p.id == pu.0 && p.kind == PowerKind::Hatch)),
                "Hatch power shed on hatch"
            );
        }
    }

    /// Smoke test: every ctor produces an AI whose initial move resolves
    /// without panicking.
    #[test]
    fn every_ctor_has_resolvable_initial_move() {
        let smokes: Vec<MonsterAi> = vec![
            create_thieving_hopper_ai(),
            create_tunneler_ai(),
            create_bowlbug_egg_ai(),
            create_bowlbug_nectar_ai(),
            create_bowlbug_rock_ai(),
            create_bowlbug_silk_ai(),
            create_exoskeleton_ai(ExoskeletonSlot::First),
            create_exoskeleton_ai(ExoskeletonSlot::Second),
            create_exoskeleton_ai(ExoskeletonSlot::Third),
            create_exoskeleton_ai(ExoskeletonSlot::Fourth),
            create_chomper_ai(false),
            create_chomper_ai(true),
            create_hunter_killer_ai(),
            create_wriggler_ai(),
            create_louse_progenitor_ai(),
            create_myte_ai(MyteSlot::First),
            create_tough_egg_ai(),
            create_ovicopter_ai(),
            create_slumbering_beetle_ai(),
            create_spiny_toad_ai(),
            create_the_obscura_ai(),
            create_decimillipede_segment_ai(0),
            create_decimillipede_segment_ai(1),
            create_decimillipede_segment_ai(2),
            create_decimillipede_segment_front_ai(0),
            create_decimillipede_segment_middle_ai(1),
            create_decimillipede_segment_back_ai(2),
            create_entomancer_ai(),
            create_infested_prism_ai(InfestedPrismSlot::First),
            create_infested_prism_ai(InfestedPrismSlot::Second),
            create_infested_prism_ai(InfestedPrismSlot::Other),
            create_the_insatiable_ai(),
            create_the_rocket_helper(),
            create_crusher_ai(),
        ];
        for ai in smokes {
            let _ = ai.current_move();
        }
    }

    // Helper used by the smoke test so the `Rocket` ctor is part of the
    // vector (the `create_` name clashes with `create_decimillipede_*` in
    // doc tables; this is just a typed alias).
    fn create_the_rocket_helper() -> MonsterAi {
        create_rocket_ai()
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
    fn tunneler_bite_burrow_below_chain() {
        // Per C# decomp: BITE_MOVE → BURROW_MOVE → BELOW_MOVE_1 (self-loops).
        let mut ai = create_tunneler_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["BITE_MOVE", "BURROW_MOVE", "BELOW_MOVE_1"]
        );
        // BELOW self-loops thereafter.
        for _ in 0..3 {
            ai.on_move_performed();
            assert_eq!(ai.roll_move_seeded(0).id, "BELOW_MOVE_1");
        }
    }

    #[test]
    fn bowlbug_egg_self_loop() {
        let mut ai = create_bowlbug_egg_ai();
        for _ in 0..5 {
            assert_eq!(ai.current_move().id, "BITE");
            ai.on_move_performed();
            ai.roll_move_seeded(0);
        }
    }

    #[test]
    fn bowlbug_nectar_thrash_chain_then_self_loop() {
        let mut ai = create_bowlbug_nectar_ai();
        assert_eq!(run_three_moves(&mut ai), vec!["THRASH", "BUFF", "THRASH2"]);
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(99).id, "THRASH2");
    }

    #[test]
    fn bowlbug_rock_check_branches_on_off_balance_counter() {
        // CHECK conditional: counter==0 (not off-balance) → HEADBUTT self-loop;
        // counter==1 (off-balance flag set by the Imbalanced fully-blocked arm)
        // → DIZZY (a Stun telegraph that clears the flag and follows up to
        // HEADBUTT). The off-balance flag is now supplied by the
        // `dispatch_after_damage_given` `was_fully_blocked` gate (bead
        // `sts2-rs-0l7dd`).
        let combat = CombatState::default();

        // Not off-balance: self-loops to HEADBUTT.
        let mut ai = create_bowlbug_rock_ai();
        for seed in 0..10 {
            assert_eq!(ai.current_move().id, "HEADBUTT");
            ai.on_move_performed();
            assert_eq!(
                ai.roll_move_with_combat_seeded(seed, &combat).id,
                "HEADBUTT",
                "counter==0 CHECK should self-loop (seed {seed})"
            );
        }

        // Off-balance: CHECK routes to DIZZY. The `CounterAware` predicate
        // needs an owner uid bound (the off-balance fn ignores its value).
        let mut ai = create_bowlbug_rock_ai();
        ai.set_owner(CreatureUid::new(1).expect("nonzero"));
        assert_eq!(ai.current_move().id, "HEADBUTT");
        ai.on_move_performed();
        ai.set_counter(1);
        assert_eq!(
            ai.roll_move_with_combat_seeded(7, &combat).id,
            "DIZZY",
            "counter==1 (off-balance) CHECK should route to DIZZY"
        );
    }

    #[test]
    fn bowlbug_silk_alternates() {
        let mut ai = create_bowlbug_silk_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["TOXIC_SPIT", "THRASH", "TOXIC_SPIT"]
        );
    }

    #[test]
    fn exoskeleton_slot_dependent_initial() {
        assert_eq!(
            create_exoskeleton_ai(ExoskeletonSlot::First)
                .current_move()
                .id,
            "SKITTER"
        );
        assert_eq!(
            create_exoskeleton_ai(ExoskeletonSlot::Second)
                .current_move()
                .id,
            "MANDIBLE"
        );
        assert_eq!(
            create_exoskeleton_ai(ExoskeletonSlot::Third)
                .current_move()
                .id,
            "ENRAGE"
        );
        // Fourth starts at RAND, which resolves to SKITTER or MANDIBLE.
        let leaf = create_exoskeleton_ai(ExoskeletonSlot::Fourth)
            .current_move()
            .id;
        assert!(leaf == "SKITTER" || leaf == "MANDIBLE", "got {leaf}");
    }

    #[test]
    fn chomper_scream_first_flips_initial() {
        assert_eq!(create_chomper_ai(false).current_move().id, "CLAMP");
        assert_eq!(create_chomper_ai(true).current_move().id, "SCREECH");
    }

    #[test]
    fn hunter_killer_goop_then_random_bite_or_puncture() {
        // C# v0.103.2 chain: TENDERIZING_GOOP_MOVE → RAND picks
        // BITE_MOVE (CannotRepeat) | PUNCTURE_MOVE (weight 2). Bead
        // `sts2-rs-2srb`.
        let mut ai = create_hunter_killer_ai();
        assert_eq!(ai.current_move().id, "TENDERIZING_GOOP_MOVE");
        ai.on_move_performed();
        // After the opener, every subsequent move must be one of the
        // two attacks; the RAND state is transparent.
        let mut saw_bite = false;
        let mut saw_puncture = false;
        for seed in 0..200 {
            let id = ai.roll_move_seeded(seed).id;
            assert!(
                id == "BITE_MOVE" || id == "PUNCTURE_MOVE",
                "post-GOOP move must be BITE or PUNCTURE, got {id} (seed {seed})"
            );
            saw_bite |= id == "BITE_MOVE";
            saw_puncture |= id == "PUNCTURE_MOVE";
            ai.on_move_performed();
        }
        assert!(
            saw_bite && saw_puncture,
            "both RAND branches must fire across 200 seeds"
        );
    }

    #[test]
    fn hunter_killer_bite_cannot_repeat() {
        // BITE_MOVE is CannotRepeat — two consecutive BITE_MOVEs must
        // never appear across many seeds.
        let mut ai = create_hunter_killer_ai();
        ai.on_move_performed(); // consume GOOP opener
        let mut last = "";
        for seed in 0..500 {
            let id = ai.roll_move_seeded(seed).id;
            assert!(
                !(id == "BITE_MOVE" && last == "BITE_MOVE"),
                "BITE_MOVE repeated at seed {seed} (CannotRepeat broken)"
            );
            last = id;
            ai.on_move_performed();
        }
    }

    /// Bead `sts2-rs-n76j`: post-fix Wriggler is a 3-move state machine —
    /// SPAWNED_MOVE (Stun, turn-1 only) → NASTY_BITE → WRIGGLE →
    /// NASTY_BITE → WRIGGLE … alternating thereafter. Mirrors the C#
    /// decomp `Wriggler.cs` machine with `StartStunned=true` (the
    /// LouseProgenitor-spawned context).
    #[test]
    fn wriggler_spawn_then_alternating_cycle() {
        let mut ai = create_wriggler_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["SPAWNED_MOVE", "NASTY_BITE", "WRIGGLE"]
        );
        // Continue past the first three to confirm the bite/wriggle
        // alternation is stable. After run_three_moves the current
        // (un-performed) move is WRIGGLE.
        assert_eq!(ai.current_move().id, "WRIGGLE");
        ai.on_move_performed();
        ai.roll_move_seeded(0);
        assert_eq!(ai.current_move().id, "NASTY_BITE");
        ai.on_move_performed();
        ai.roll_move_seeded(0);
        assert_eq!(ai.current_move().id, "WRIGGLE");
    }

    #[test]
    fn louse_progenitor_web_curl_pounce_chain_loops() {
        // C# `LouseProgenitor.cs` chain: WEB_CANNON → CURL_AND_GROW →
        // POUNCE → WEB_CANNON. Bead `sts2-rs-ybtmu` replaced the
        // fabricated SPAWN→SCRATCH Python-pin moveset.
        let mut ai = create_louse_progenitor_ai();
        let mut got = Vec::new();
        for _ in 0..4 {
            got.push(ai.current_move().id);
            ai.on_move_performed();
            ai.roll_move_seeded(0);
        }
        assert_eq!(
            got,
            vec![
                "WEB_CANNON_MOVE",
                "CURL_AND_GROW_MOVE",
                "POUNCE_MOVE",
                "WEB_CANNON_MOVE",
            ]
        );
    }

    #[test]
    fn myte_toxic_bite_suck_chain() {
        // Per C# decomp: first-slot Myte opens at TOXIC_MOVE →
        // BITE_MOVE → SUCK_MOVE → TOXIC.
        let mut ai = create_myte_ai(MyteSlot::First);
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["TOXIC_MOVE", "BITE_MOVE", "SUCK_MOVE"]
        );
        // SUCK → TOXIC closes the cycle.
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "TOXIC_MOVE");
    }

    #[test]
    fn myte_second_slot_opens_at_suck() {
        // C# `Myte.cs` `INIT_MOVE` ConditionalBranchState: the "second"
        // slot opens at SUCK_MOVE, then follows SUCK → TOXIC → BITE →
        // SUCK. Only the entry point differs from the first slot, so the
        // second Myte does NOT cast TOXIC on turn 1 (the 2-Myte
        // encounter therefore adds exactly 2 Toxic to hand, not 4).
        let mut ai = create_myte_ai(MyteSlot::Second);
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["SUCK_MOVE", "TOXIC_MOVE", "BITE_MOVE"]
        );
    }

    #[test]
    fn ovicopter_four_move_chain_with_resummon_branch() {
        // Bead sts2-rs-7qd1: chain is LAY_EGGS → SMASH → TENDERIZER →
        // {LAY_EGGS if alive teammates ≤ 3 else NUTRITIONAL_PASTE} per
        // C# `Ovicopter.cs::GenerateMoveStateMachine`. With no other
        // enemies in the state, `alive teammates = 0` which is ≤ 3, so
        // SUMMON_BRANCH resolves to LAY_EGGS_MOVE.
        let mut ai = create_ovicopter_ai();
        let combat = CombatState::default();
        ai.set_owner(CreatureUid::new(1).expect("nz"));
        let expected = ["LAY_EGGS_MOVE", "SMASH_MOVE", "TENDERIZER_MOVE"];
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(ai.current_move().id, *want, "step {i}");
            ai.on_move_performed();
            ai.roll_move_with_combat_seeded(0, &combat);
        }
        // SUMMON_BRANCH should select LAY_EGGS_MOVE because alive
        // teammates == 0 ≤ 3.
        assert_eq!(ai.current_move().id, "LAY_EGGS_MOVE");
    }

    #[test]
    fn slumbering_beetle_snores_then_rolls_out_self_loop() {
        // C# `SlumberingBeetle.cs`: SNORE_MOVE self-loops while asleep; once
        // woken (Slumber stripped + force_current_state by the core Slumber
        // hooks) the beetle ROLL_OUT_MOVEs and self-loops. The AI itself only
        // owns the two self-looping states; the SNORE→ROLL_OUT transition is
        // driven by `force_current_state("ROLL_OUT_MOVE")` (bead `sts2-rs-ybtmu`).
        let mut ai = create_slumbering_beetle_ai();
        let combat = CombatState::default();
        assert_eq!(ai.current_move().id, "SNORE_MOVE");
        for seed in 0..5 {
            ai.on_move_performed();
            assert_eq!(
                ai.roll_move_with_combat_seeded(seed, &combat).id,
                "SNORE_MOVE",
                "snores while asleep (no wake force applied)"
            );
        }
        // After a wake-force redirect, ROLL_OUT self-loops.
        ai.force_current_state("ROLL_OUT_MOVE");
        assert_eq!(ai.current_move().id, "ROLL_OUT_MOVE");
        for seed in 0..5 {
            ai.on_move_performed();
            assert_eq!(
                ai.roll_move_with_combat_seeded(seed, &combat).id,
                "ROLL_OUT_MOVE",
                "rolls out every turn once awake"
            );
        }
    }

    #[test]
    fn spiny_toad_three_step_chain() {
        // C# `SpinyToad.cs:55-65`: initial = PROTRUDING_SPIKES
        // (Rust "SPINES"); rotation SPINES → EXPLOSION → LASH → SPINES.
        // Bead `sts2-rs-or4a6` corrected the prior fabricated 4-state
        // chain.
        let mut ai = create_spiny_toad_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["SPINES", "EXPLOSION", "LASH"]
        );
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "SPINES");
    }

    #[test]
    fn the_obscura_illusion_then_cannot_repeat() {
        // Per C# decomp: ILLUSION_MOVE → RAND{PIERCING_GAZE_MOVE |
        // SAIL_MOVE | HARDENING_STRIKE_MOVE} with CannotRepeat. Initial
        // move is ILLUSION; thereafter no branch fires twice in a row.
        let mut ai = create_the_obscura_ai();
        assert_eq!(ai.current_move().id, "ILLUSION_MOVE");
        ai.on_move_performed(); // past ILLUSION
        let mut last = "";
        for seed in 0..300 {
            let id = ai.roll_move_seeded(seed).id;
            assert_ne!(id, last, "CannotRepeat violation at seed {seed}");
            last = id;
            ai.on_move_performed();
        }
    }

    #[test]
    fn decimillipede_segment_starter_idx_dispatch() {
        assert_eq!(
            create_decimillipede_segment_ai(0).current_move().id,
            "WRITHE"
        );
        assert_eq!(create_decimillipede_segment_ai(1).current_move().id, "BULK");
        assert_eq!(
            create_decimillipede_segment_ai(2).current_move().id,
            "CONSTRICT"
        );
    }

    #[test]
    fn entomancer_three_chain() {
        let mut ai = create_entomancer_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["BEES", "SPEAR", "PHEROMONE_SPIT"]
        );
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(99).id, "BEES");
    }

    /// Bead `sts2-rs-pnv5`: PHEROMONE_SPIT effect mirrors Python pin
    /// `sts2-rl-agent@01c094d:sts2_env/monsters/act2.py:542-548`.
    /// `PERSONAL_HIVE` at 1 → 2, at 2 → 3, at 3 (cap) → STRENGTH +2.
    #[test]
    fn entomancer_pheromone_spit_pumps_hive_then_strength() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::powers::PowerKind;
        use sts2_core::CombatState;
        use sts2_core::CreatureUid;

        fn enemy_power_amount(state: &CombatState, uid: CreatureUid, kind: PowerKind) -> i32 {
            let puids = state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .map(|c| c.powers.clone())
                .unwrap_or_default();
            puids
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

        let mut state = CombatState {
            rng: sts2_core::dotnet_rng::Rng::new(0_i32),
            ..Default::default()
        };
        // Spawn a minimal Entomancer creature.
        let uid = CreatureUid::new(1).expect("nonzero");
        state.creatures.push(sts2_core::Creature {
            id: uid.0,
            current_hp: 145,
            max_hp: 145,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(sts2_core::monsters::MonsterKind::Entomancer),
            intent: None,
            intent_history: sts2_core::monsters::IntentHistory::default(),
            monster_id: sts2_core::monsters::MonsterId(
                sts2_core::monsters::MonsterKind::Entomancer.python_name(),
            ),
            ai: Some(create_entomancer_ai()),
        });
        state.enemy_uids.push(uid);
        // Spawn-time PERSONAL_HIVE x1 per the Python ctor.
        apply_power_to_creature(&mut state, uid, PowerKind::PersonalHive, 1);
        assert_eq!(enemy_power_amount(&state, uid, PowerKind::PersonalHive), 1);

        // Hive 1 → 2, AND Strength +1 (C# sub-3 branch grants both).
        entomancer_pheromone_spit_effect(&mut state, uid);
        assert_eq!(enemy_power_amount(&state, uid, PowerKind::PersonalHive), 2);
        assert_eq!(enemy_power_amount(&state, uid, PowerKind::Strength), 1);

        // Hive 2 → 3, Strength +1 again.
        entomancer_pheromone_spit_effect(&mut state, uid);
        assert_eq!(enemy_power_amount(&state, uid, PowerKind::PersonalHive), 3);
        assert_eq!(enemy_power_amount(&state, uid, PowerKind::Strength), 2);

        // Hive capped at 3 → Strength +2 per spit (4, then 6).
        entomancer_pheromone_spit_effect(&mut state, uid);
        assert_eq!(enemy_power_amount(&state, uid, PowerKind::PersonalHive), 3);
        assert_eq!(enemy_power_amount(&state, uid, PowerKind::Strength), 4);
        entomancer_pheromone_spit_effect(&mut state, uid);
        assert_eq!(enemy_power_amount(&state, uid, PowerKind::Strength), 6);
    }

    /// Bead `sts2-rs-w4gq`: post-rebaseline 4-move ring is
    /// slot-insensitive. Asserts the chain JAB → RADIATE →
    /// WHIRLWIND → PULSATE → JAB across every slot variant.
    #[test]
    fn infested_prism_four_move_ring_slot_insensitive() {
        for slot in [
            InfestedPrismSlot::First,
            InfestedPrismSlot::Second,
            InfestedPrismSlot::Other,
        ] {
            let mut ai = create_infested_prism_ai(slot);
            assert_eq!(ai.current_move().id, "JAB", "slot {slot:?} initial");
            assert_eq!(
                run_three_moves(&mut ai),
                vec!["JAB", "RADIATE", "WHIRLWIND"],
                "slot {slot:?} first three"
            );
            ai.on_move_performed();
            assert_eq!(ai.roll_move_seeded(0).id, "PULSATE", "slot {slot:?} 4th");
            ai.on_move_performed();
            assert_eq!(
                ai.roll_move_seeded(0).id,
                "JAB",
                "slot {slot:?} ring wraps to JAB"
            );
        }
    }

    /// `…wb8lk.11`: intent payload spot-checks against the v0.107.1 decomp
    /// `InfestedPrism.cs` (A0 base values; Jab 15, Radiate 11+11, Whirlwind
    /// 5x3, Pulsate Attack 8 + VitalSpark buff + Defend 20).
    #[test]
    fn infested_prism_intent_payloads_match_csharp_decomp() {
        let mut ai = create_infested_prism_ai(InfestedPrismSlot::First);
        // JAB — Attack 15
        assert_eq!(
            ai.current_move().intents.as_slice(),
            &[Intent::Attack { damage: 15 }]
        );
        ai.on_move_performed();
        // RADIATE — Attack 11 + Defend 11
        assert_eq!(
            ai.roll_move_seeded(0).intents.as_slice(),
            &[Intent::Attack { damage: 11 }, Intent::Defend { block: 11 }]
        );
        ai.on_move_performed();
        // WHIRLWIND — MultiAttack 5x3
        assert_eq!(
            ai.roll_move_seeded(0).intents.as_slice(),
            &[Intent::MultiAttack { damage: 5, hits: 3 }]
        );
        ai.on_move_performed();
        // PULSATE — Attack 8 + VitalSpark buff + Defend 20
        assert_eq!(
            ai.roll_move_seeded(0).intents.as_slice(),
            &[
                Intent::Attack { damage: 8 },
                Intent::Buff {
                    kind: PowerKind::VitalSpark,
                    amount: 2
                },
                Intent::Defend { block: 20 }
            ]
        );
    }

    #[test]
    fn the_insatiable_five_step_chain() {
        let mut ai = create_the_insatiable_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["LIQUIFY_GROUND", "THRASH_1", "LUNGING_BITE"]
        );
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "SALIVATE");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "THRASH_2");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "THRASH_1");
    }

    /// KnowledgeDemon — the bead's parity gate (bead `sts2-rs-w4w7`):
    /// walk ≥10 turns and exercise both branches of CURSE_CHECK using
    /// the per-monster counter (Python `_state["curse_counter"]`).
    /// The counter is incremented manually here to simulate the
    /// dispatcher firing the CURSE_OF_KNOWLEDGE effect_fn — the
    /// AI-only `roll_move_with_combat` path doesn't run effect_fn,
    /// and a full combat-loop fixture is overkill for branch coverage.
    #[test]
    fn knowledge_demon_curse_check_both_branches() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, PowerUid};

        let mut combat = CombatState::default();
        let owner_uid_raw = NonZeroU32::new(1).expect("nz");
        let powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        combat.creatures.push(Creature {
            id: owner_uid_raw,
            current_hp: 379,
            max_hp: 379,
            block: 0,
            powers,
            species: Some(MonsterKind::KnowledgeDemon),
            monster_id: MonsterId("KnowledgeDemon"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(create_knowledge_demon_ai()),
        });
        let owner = CreatureUid(owner_uid_raw);

        // Walk a full cycle with counter < 3 — CURSE_CHECK should
        // re-route back to CURSE_OF_KNOWLEDGE.
        let mut ai = combat.creatures[0].ai.take().expect("ai");
        ai.set_owner(owner);
        let mut trace = Vec::new();
        for turn in 0u64..10 {
            let id = ai.current_move().id;
            trace.push(id);
            if id == "CURSE_OF_KNOWLEDGE" {
                // Simulate dispatcher firing the effect_fn after the
                // Debuff intent resolves — increments counter by 1.
                ai.increment_counter();
            }
            ai.on_move_performed();
            ai.roll_move_with_combat_seeded(turn, &combat);
        }
        // First 3 CURSE_OF_KNOWLEDGE firings increment counter to 3;
        // 4th visit to CURSE_CHECK with counter==3 takes the SLAP
        // branch (CURSE → SLAP → OVER → PONDER × 3, then SLAP loop).
        assert_eq!(
            trace,
            vec![
                "CURSE_OF_KNOWLEDGE",
                "SLAP",
                "KNOWLEDGE_OVERWHELMING",
                "PONDER",
                "CURSE_OF_KNOWLEDGE",
                "SLAP",
                "KNOWLEDGE_OVERWHELMING",
                "PONDER",
                "CURSE_OF_KNOWLEDGE",
                "SLAP",
            ],
            "first three CURSE_OF_KNOWLEDGE firings should re-enter via counter < 3"
        );

        // Walk a second AI with counter preset to 3 — CURSE_CHECK
        // should immediately jump to SLAP (always-true fallback).
        let mut ai2 = create_knowledge_demon_ai();
        ai2.set_owner(owner);
        ai2.set_counter(3);
        let mut trace2 = Vec::new();
        for turn in 0u64..10 {
            trace2.push(ai2.current_move().id);
            ai2.on_move_performed();
            ai2.roll_move_with_combat_seeded(turn, &combat);
        }
        // Counter starts at 3 so even the initial CURSE_OF_KNOWLEDGE
        // (initial state) fires once, then CURSE_CHECK jumps to SLAP
        // loop. Note: initial CURSE_OF_KNOWLEDGE fires regardless of
        // counter because it's the initial state; the gate is read
        // only at CURSE_CHECK.
        assert_eq!(
            trace2,
            vec![
                "CURSE_OF_KNOWLEDGE",
                "SLAP",
                "KNOWLEDGE_OVERWHELMING",
                "PONDER",
                "SLAP",
                "KNOWLEDGE_OVERWHELMING",
                "PONDER",
                "SLAP",
                "KNOWLEDGE_OVERWHELMING",
                "PONDER",
            ],
            "counter>=3 should jump to SLAP and loop SLAP→OVER→PONDER"
        );
    }

    /// Bead sts2-rs-w4w7: stun-skipped CURSE_OF_KNOWLEDGE moves do NOT
    /// increment the counter (Python parity — Stun is player-side
    /// mitigation that prevents the curse effect_fn from running).
    /// The old `combat.round < 3` proxy could not express this.
    #[test]
    fn knowledge_demon_stun_skipped_curse_does_not_advance_phase() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, PowerUid};

        let mut combat = CombatState::default();
        let owner_uid_raw = NonZeroU32::new(1).expect("nz");
        let powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        combat.creatures.push(Creature {
            id: owner_uid_raw,
            current_hp: 379,
            max_hp: 379,
            block: 0,
            powers,
            species: Some(MonsterKind::KnowledgeDemon),
            monster_id: MonsterId("KnowledgeDemon"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(create_knowledge_demon_ai()),
        });
        let owner = CreatureUid(owner_uid_raw);

        let mut ai = combat.creatures[0].ai.take().expect("ai");
        ai.set_owner(owner);
        // Walk one full cycle WITHOUT incrementing counter — simulates
        // Stun preventing the effect_fn from firing.
        for turn in 0u64..4 {
            ai.on_move_performed();
            ai.roll_move_with_combat_seeded(turn, &combat);
        }
        // After one cycle counter still 0; CURSE_CHECK re-routes back
        // to CURSE_OF_KNOWLEDGE because counter < 3.
        assert_eq!(
            ai.current_move().id,
            "CURSE_OF_KNOWLEDGE",
            "Stun-skipped curses must not advance the gate"
        );
    }

    #[test]
    fn crusher_five_step_chain() {
        let mut ai = create_crusher_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec!["THRASH", "ENLARGING_STRIKE", "BUG_STING"]
        );
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "ADAPT");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "GUARDED_STRIKE");
    }

    #[test]
    fn rocket_five_step_chain() {
        // Rebased to C# Rocket.cs: TARGETING_RETICLE → PRECISION_BEAM →
        // CHARGE_UP → LASER → RECHARGE → TARGETING_RETICLE … (linear loop).
        let mut ai = create_rocket_ai();
        assert_eq!(
            run_three_moves(&mut ai),
            vec![
                "TARGETING_RETICLE_MOVE",
                "PRECISION_BEAM_MOVE",
                "CHARGE_UP_MOVE"
            ]
        );
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "LASER_MOVE");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "RECHARGE_MOVE");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "TARGETING_RETICLE_MOVE");
    }

    #[test]
    fn thieving_hopper_five_move_chain_then_escape_self_loop() {
        // Bead sts2-rs-l8yi: chain is
        // THIEVERY → FLUTTER → HAT_TRICK → NAB → ESCAPE → ESCAPE (self-loop)
        // per C# `ThievingHopper.cs::GenerateMoveStateMachine`.
        let mut ai = create_thieving_hopper_ai();
        let expected = [
            "THIEVERY_MOVE",
            "FLUTTER_MOVE",
            "HAT_TRICK_MOVE",
            "NAB_MOVE",
            "ESCAPE_MOVE",
        ];
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(ai.current_move().id, *want, "step {i}");
            ai.on_move_performed();
            ai.roll_move_seeded(0);
        }
        // ESCAPE_MOVE self-loops forever.
        for _ in 0..3 {
            assert_eq!(ai.current_move().id, "ESCAPE_MOVE");
            ai.on_move_performed();
            ai.roll_move_seeded(0);
        }
    }

    /// Bead `sts2-rs-ybtmu` (PILE_COUNT fix): THIEVERY_MOVE's card-steal
    /// rider removes exactly one Draw/Discard card from combat, applying
    /// the `_stealPriorities` rarity ordering. With a Basic Strike in
    /// draw and an Uncommon FlameBarrier in discard, the first priority
    /// bucket (Uncommon) wins, so the discard card is stolen — draw is
    /// untouched, discard drops to zero. One `combat_card_generation`
    /// draw is consumed (`NextItem`), matching `ThievingHopper.cs:216`.
    #[test]
    fn thieving_hopper_thievery_steals_one_card_by_rarity_priority() {
        use crate::cards::spec_index::card_pod_from_id;
        use sts2_core::cards::CardId;

        let mut state = CombatState::default();

        // Push two real deck cards into the arena with index-aligned uids.
        let push_card = |state: &mut CombatState, id: CardId| -> sts2_core::CardUid {
            let mut card = card_pod_from_id(id, false).expect("spec");
            let uid = sts2_core::CardUid::new(state.cards.len() as u32 + 1).expect("nz");
            card.uid = uid;
            state.cards.push(card);
            uid
        };
        let strike = push_card(&mut state, CardId::StrikeIronclad); // Basic
        let flame = push_card(&mut state, CardId::FlameBarrierCard); // Uncommon
        state.player.draw.push(strike);
        state.player.discard.push(flame);

        let rng_before = state.run_rng.combat_card_generation.counter();
        thieving_hopper_thievery_effect(&mut state, CreatureUid::new(1).expect("nz"));

        // Uncommon priority bucket → the discard FlameBarrier is stolen.
        assert_eq!(state.player.draw.len(), 1, "Basic draw card untouched");
        assert_eq!(state.player.draw[0], strike);
        assert!(
            state.player.discard.is_empty(),
            "Uncommon discard card removed from combat"
        );
        assert_eq!(
            state.run_rng.combat_card_generation.counter(),
            rng_before + 1,
            "exactly one CombatCardGeneration draw consumed"
        );
    }

    /// Mutation coverage (issue #65): THIEVERY's `_stealPriorities` bucket 2
    /// (Common/Rare/Event) and the candidate filter. With a Common Headbutt
    /// and a Basic Strike (no Uncommon), bucket 1 is empty and bucket 2
    /// narrows to the single Common — so the Common is stolen deterministically
    /// (single-item `NextItem` index 0). A combat-generated Common
    /// (`GeneratedInCombat`) is EXCLUDED from candidates. Kills the candidate
    /// filter `&&` (557) and the bucket-2 `&&` (579).
    #[test]
    fn thieving_hopper_thievery_bucket_two_picks_common_excludes_generated() {
        use crate::cards::spec_index::{card_pod_from_id, rarity_for_card_id};
        use crate::cards::CardRarity;
        use sts2_core::cards::{CardId, VarKey};

        let mut state = CombatState::default();
        let push_card = |state: &mut CombatState, id: CardId| -> sts2_core::CardUid {
            let mut card = card_pod_from_id(id, false).expect("spec");
            let uid = sts2_core::CardUid::new(state.cards.len() as u32 + 1).expect("nz");
            card.uid = uid;
            state.cards.push(card);
            uid
        };
        // Sanity: rarity assumptions hold.
        assert_eq!(
            rarity_for_card_id(CardId::Headbutt),
            Some(CardRarity::Common)
        );
        assert_eq!(
            rarity_for_card_id(CardId::StrikeIronclad),
            Some(CardRarity::Basic)
        );

        let strike = push_card(&mut state, CardId::StrikeIronclad); // Basic
        let headbutt = push_card(&mut state, CardId::Headbutt); // Common
                                                                // A combat-generated Common that must be EXCLUDED from candidates.
        let generated = push_card(&mut state, CardId::Headbutt);
        state.cards[generated.get() as usize - 1].set_combat_var(VarKey::GeneratedInCombat, 1);

        state.player.draw.push(strike);
        state.player.draw.push(generated);
        state.player.discard.push(headbutt);

        thieving_hopper_thievery_effect(&mut state, CreatureUid::new(1).expect("nz"));

        // Bucket 2 narrowed to the single real Common (Headbutt in discard).
        assert!(
            state.player.discard.is_empty(),
            "Common Headbutt stolen from discard"
        );
        assert_eq!(
            state.player.draw.len(),
            2,
            "Basic Strike + generated Common remain in draw (neither stolen)"
        );
        assert!(
            state.player.draw.contains(&strike) && state.player.draw.contains(&generated),
            "the Basic and the combat-generated Common are untouched"
        );
    }

    /// Mutation coverage (issue #65): THIEVERY bucket 3 (Basic/Quest, NOT
    /// imbued). Two Basic Strikes — one Imbued, one not. Buckets 1/2 are
    /// empty; bucket 3's `not_imbued && Basic` narrows to the single
    /// non-imbued Strike, which is stolen deterministically. Kills the
    /// bucket-3 `&&` (585) — under `||` the imbued Strike re-enters the bucket.
    #[test]
    fn thieving_hopper_thievery_bucket_three_excludes_imbued_basic() {
        use crate::cards::spec_index::card_pod_from_id;
        use sts2_core::cards::{CardId, EnchantmentId};

        let mut state = CombatState::default();
        let push_card = |state: &mut CombatState, id: CardId, imbued: bool| -> sts2_core::CardUid {
            let mut card = card_pod_from_id(id, false).expect("spec");
            let uid = sts2_core::CardUid::new(state.cards.len() as u32 + 1).expect("nz");
            card.uid = uid;
            if imbued {
                card.set_enchantment(EnchantmentId::Imbued, 1);
            }
            state.cards.push(card);
            uid
        };
        let imbued_strike = push_card(&mut state, CardId::StrikeIronclad, true);
        let plain_strike = push_card(&mut state, CardId::StrikeIronclad, false);
        state.player.draw.push(imbued_strike);
        state.player.draw.push(plain_strike);

        thieving_hopper_thievery_effect(&mut state, CreatureUid::new(1).expect("nz"));

        assert!(
            state.player.draw.contains(&imbued_strike),
            "Imbued Basic must NOT be stolen (bucket 3 requires not_imbued)"
        );
        assert!(
            !state.player.draw.contains(&plain_strike),
            "non-imbued Basic Strike is the bucket-3 pick and is stolen"
        );
    }

    /// Mutation coverage (issue #65): THIEVERY bucket 4 (Ancient OR imbued —
    /// the last-resort bucket). Candidates are an Ancient (non-imbued) card
    /// and an Imbued Common; buckets 1-3 are all empty, so bucket 4 keeps both
    /// and the deterministic `NextItem` picks the Ancient. Kills the bucket-4
    /// `rarity == Ancient` (587:39), `||` (587:68) and `!` (587:71) mutants:
    /// each drops the Ancient (or the imbued) card from the bucket, changing
    /// the chosen index.
    #[test]
    fn thieving_hopper_thievery_bucket_four_ancient_and_imbued() {
        use crate::cards::spec_index::{card_pod_from_id, rarity_for_card_id};
        use crate::cards::CardRarity;
        use sts2_core::cards::{CardId, EnchantmentId};

        // Sanity: Corruption is Ancient.
        assert_eq!(
            rarity_for_card_id(CardId::CorruptionCard),
            Some(CardRarity::Ancient)
        );

        let mut state = CombatState::default();
        let push_card = |state: &mut CombatState, id: CardId, imbued: bool| -> sts2_core::CardUid {
            let mut card = card_pod_from_id(id, false).expect("spec");
            let uid = sts2_core::CardUid::new(state.cards.len() as u32 + 1).expect("nz");
            card.uid = uid;
            if imbued {
                card.set_enchantment(EnchantmentId::Imbued, 1);
            }
            state.cards.push(card);
            uid
        };
        let ancient = push_card(&mut state, CardId::CorruptionCard, false); // Ancient
        let imbued_common = push_card(&mut state, CardId::Headbutt, true); // Imbued Common
        state.player.draw.push(ancient);
        state.player.discard.push(imbued_common);

        thieving_hopper_thievery_effect(&mut state, CreatureUid::new(1).expect("nz"));

        // Clean behavior: bucket 4 = {ancient, imbued_common}; NextItem(0)
        // resolves to the Ancient (draw-pile, first in candidate order).
        assert!(
            !state.player.draw.contains(&ancient),
            "Ancient card is the bucket-4 pick and is stolen"
        );
        assert!(
            state.player.discard.contains(&imbued_common),
            "Imbued Common stays (it was in the bucket but not chosen)"
        );
    }

    /// Mutation coverage (issue #65): THIEVERY bucket 4 `!not_imbued` clause
    /// (act2_hive:587:71). An Imbued Common sits at candidate index 0 (draw)
    /// and an Ancient card in discard. Clean bucket 4 keeps BOTH (`Ancient ||
    /// !not_imbued`) and `NextItem(0)` steals the Imbued Common. Deleting the
    /// `!` makes the clause `not_imbued`, dropping the imbued card from the
    /// bucket so the Ancient is stolen instead. Kills `delete !` (587:71).
    #[test]
    fn thieving_hopper_thievery_bucket_four_keeps_imbued_at_index_zero() {
        use crate::cards::spec_index::card_pod_from_id;
        use sts2_core::cards::{CardId, EnchantmentId};

        let mut state = CombatState::default();
        let push_card = |state: &mut CombatState, id: CardId, imbued: bool| -> sts2_core::CardUid {
            let mut card = card_pod_from_id(id, false).expect("spec");
            let uid = sts2_core::CardUid::new(state.cards.len() as u32 + 1).expect("nz");
            card.uid = uid;
            if imbued {
                card.set_enchantment(EnchantmentId::Imbued, 1);
            }
            state.cards.push(card);
            uid
        };
        let imbued_common = push_card(&mut state, CardId::Headbutt, true);
        let ancient = push_card(&mut state, CardId::CorruptionCard, false);
        // Imbued card is candidate index 0 (draw precedes discard).
        state.player.draw.push(imbued_common);
        state.player.discard.push(ancient);

        thieving_hopper_thievery_effect(&mut state, CreatureUid::new(1).expect("nz"));

        assert!(
            !state.player.draw.contains(&imbued_common),
            "Imbued Common (index 0) is kept in bucket 4 via `!not_imbued` and stolen"
        );
        assert!(
            state.player.discard.contains(&ancient),
            "Ancient card stays (not the index-0 pick)"
        );
    }

    /// Mutation coverage (issue #65): THIEVERY bucket 4 `||` disjunction
    /// (act2_hive:587:68). An Ancient-only card at candidate index 0 (draw)
    /// and an Ancient+Imbued card in discard. Clean bucket 4 (`Ancient ||
    /// !not_imbued`) keeps BOTH and `NextItem(0)` steals the Ancient-only
    /// card. Replacing `||` with `&&` keeps only the Ancient-AND-imbued card,
    /// so it is stolen instead. Kills `||`→`&&` (587:68).
    #[test]
    fn thieving_hopper_thievery_bucket_four_or_keeps_ancient_only_at_index_zero() {
        use crate::cards::spec_index::{card_pod_from_id, rarity_for_card_id};
        use crate::cards::CardRarity;
        use sts2_core::cards::{CardId, EnchantmentId};

        assert_eq!(
            rarity_for_card_id(CardId::Break),
            Some(CardRarity::Ancient),
            "Break is Ancient"
        );

        let mut state = CombatState::default();
        let push_card = |state: &mut CombatState, id: CardId, imbued: bool| -> sts2_core::CardUid {
            let mut card = card_pod_from_id(id, false).expect("spec");
            let uid = sts2_core::CardUid::new(state.cards.len() as u32 + 1).expect("nz");
            card.uid = uid;
            if imbued {
                card.set_enchantment(EnchantmentId::Imbued, 1);
            }
            state.cards.push(card);
            uid
        };
        let ancient_only = push_card(&mut state, CardId::Break, false); // Ancient, not imbued
        let ancient_imbued = push_card(&mut state, CardId::CorruptionCard, true); // Ancient + Imbued
                                                                                  // Ancient-only card is candidate index 0 (draw precedes discard).
        state.player.draw.push(ancient_only);
        state.player.discard.push(ancient_imbued);

        thieving_hopper_thievery_effect(&mut state, CreatureUid::new(1).expect("nz"));

        assert!(
            !state.player.draw.contains(&ancient_only),
            "Ancient-only card (index 0) kept via the `Ancient ||` disjunct and stolen"
        );
        assert!(
            state.player.discard.contains(&ancient_imbued),
            "Ancient+Imbued card stays (not the index-0 pick)"
        );
    }

    /// Bead `sts2-rs-ybtmu`: combat-generated Status cards (null
    /// `DeckVersion` in C#) are NOT eligible to be stolen, and with no
    /// real deck cards present the steal is a no-op that consumes no RNG.
    #[test]
    fn thieving_hopper_thievery_skips_status_cards_and_consumes_no_rng() {
        use sts2_core::card_effects::PileKind;

        let mut state = CombatState::default();
        // Only a generated Status card (Dazed) in the draw pile.
        let _ = crate::cards::status::spawn_dazed(&mut state, PileKind::Draw);
        let pre_draw = state.player.draw.len();
        let rng_before = state.run_rng.combat_card_generation.counter();

        thieving_hopper_thievery_effect(&mut state, CreatureUid::new(1).expect("nz"));

        assert_eq!(
            state.player.draw.len(),
            pre_draw,
            "Status card not stolen (null DeckVersion proxy)"
        );
        assert_eq!(
            state.run_rng.combat_card_generation.counter(),
            rng_before,
            "no candidates → no RNG draw"
        );
    }

    /// Bead sts2-rs-vn0m: LAY_EGGS's `effect_fn` spawns 1 ToughEgg.
    /// Bead sts2-rs-va5r: DecimillipedeSegment REATTACH now emits
    /// `Intent::Heal` (matches Python pin `act2.py:488`). DEAD telegraphs
    /// NOTHING — C# builds `DEAD_MOVE` with an empty `params` intent array
    /// (`DecimillipedeSegment.cs:152`), and the wire shows the difference.
    #[test]
    fn decimillipede_reattach_telegraphs_heal() {
        let ai = create_decimillipede_segment_ai(0);
        let states = &ai.states;
        let reattach = states.get("REATTACH").expect("REATTACH present");
        match reattach {
            sts2_core::monster_ai::MonsterState::Move(m) => {
                assert_eq!(m.intents.as_slice(), &[Intent::Heal][..]);
            }
            _ => panic!("REATTACH must be a MoveState"),
        }
        let dead = states.get("DEAD").expect("DEAD present");
        match dead {
            sts2_core::monster_ai::MonsterState::Move(m) => {
                assert!(
                    m.intents.is_empty(),
                    "DEAD_MOVE carries no intents in C#; a fabricated one diverges \
                     against every wire board the corpse is on"
                );
            }
            _ => panic!("DEAD must be a MoveState"),
        }
    }

    /// Bead `sts2-rs-8lwnq`: end-to-end Reattach revive lifecycle through
    /// real enemy turns. C# `ReattachPower.AfterDeath` arms the corpse onto
    /// DEAD (`SetMoveImmediate`); the corpse no-ops one enemy turn at its own
    /// roster slot, rolls DEAD→REATTACH at turn boundary
    /// (`PrepareForNextTurn` has no IsDead gate), then `ReattachMove` →
    /// `DoReattach` heals it back to its Reattach amount (25) on the NEXT
    /// enemy turn — re-gated on a sibling still being alive.
    #[test]
    fn reattach_segment_revives_at_own_slot_across_enemy_turns() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};
        use sts2_core::CombatPhase;

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let front = state.spawn_enemy(MonsterKind::DecimillipedeSegmentFront);
        let back = state.spawn_enemy(MonsterKind::DecimillipedeSegmentBack);
        state.set_creature_ai(front, create_decimillipede_segment_front_ai(0));
        state.set_creature_ai(back, create_decimillipede_segment_back_ai(2));
        for uid in [front, back] {
            if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
                let telegraph =
                    c.ai.as_ref()
                        .and_then(|ai| ai.current_move().intents.first().copied());
                c.intent = telegraph;
            }
        }
        let hp_of = |s: &CombatState, uid: CreatureUid| {
            s.creatures
                .iter()
                .find(|c| c.id == uid.0)
                .map(|c| c.current_hp)
                .unwrap()
        };
        let ai_state = |s: &CombatState, uid: CreatureUid| {
            s.creatures
                .iter()
                .find(|c| c.id == uid.0)
                .and_then(|c| c.ai.as_ref())
                .map(sts2_core::MonsterAi::current_state_id)
                .unwrap()
        };

        // Kill the front segment during the player turn — the death path
        // must arm DEAD + telegraph the no-op (arm_reattach_dead_state).
        let front_hp = hp_of(&state, front);
        apply_damage(
            &mut state,
            CreatureRef::Player,
            front,
            front_hp + 50,
            ValueProps::NONE,
            None,
        );
        assert_eq!(hp_of(&state, front), 0, "front segment downed");
        assert_eq!(
            ai_state(&state, front),
            "DEAD",
            "death arms SetMoveImmediate(DEAD)"
        );
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == front.0)
                .unwrap()
                .intent,
            None,
            "DEAD is intent-less, so the corpse telegraphs nothing"
        );
        assert!(!state.is_over, "sibling alive — death is not fatal");

        // Enemy turn 1: the corpse performs DEAD (no-op) at its slot; the
        // boundary roll advances DEAD→REATTACH (Heal telegraph), still 0 HP.
        state.phase = CombatPhase::EnemyTurnExec;
        let _ = sts2_core::combat_loop::enemy_turn_exec(&mut state);
        sts2_core::combat_loop::enemy_turn_end(&mut state);
        assert_eq!(hp_of(&state, front), 0, "no heal on the DEAD turn");
        assert_eq!(
            ai_state(&state, front),
            "REATTACH",
            "boundary roll advances the corpse"
        );
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == front.0)
                .unwrap()
                .intent,
            Some(Intent::Heal),
            "REATTACH telegraphs Heal"
        );

        // Enemy turn 2: REATTACH executes at the corpse's slot — heals to 25
        // (the Reattach amount), and the follow-up RAND telegraphs a real move.
        state.phase = CombatPhase::EnemyTurnExec;
        let _ = sts2_core::combat_loop::enemy_turn_exec(&mut state);
        sts2_core::combat_loop::enemy_turn_end(&mut state);
        assert_eq!(hp_of(&state, front), 25, "DoReattach heals 0 → 25");
        assert!(
            matches!(ai_state(&state, front), "WRITHE" | "BULK" | "CONSTRICT"),
            "post-revive RAND rolled a real move, got {}",
            ai_state(&state, front)
        );
    }

    /// Bead `sts2-rs-8lwnq`: `DoReattach` re-gates at EXECUTE time — if every
    /// other segment died between the death and the REATTACH move, the heal
    /// is skipped and the corpse stays down (C# `AreAllOtherSegmentsDead()`).
    #[test]
    fn reattach_effect_skips_heal_without_living_sibling() {
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let front = state.spawn_enemy(MonsterKind::DecimillipedeSegmentFront);
        let back = state.spawn_enemy(MonsterKind::DecimillipedeSegmentBack);
        for uid in [front, back] {
            if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
                c.current_hp = 0;
            }
        }
        decimillipede_reattach_effect(&mut state, front);
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == front.0)
                .unwrap()
                .current_hp,
            0,
            "no living sibling → no heal"
        );
        // With the sibling restored, the same move body heals to 25.
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == back.0) {
            c.current_hp = 10;
        }
        decimillipede_reattach_effect(&mut state, front);
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == front.0)
                .unwrap()
                .current_hp,
            25,
            "living sibling → DoReattach heals by the Reattach amount"
        );
    }

    #[test]
    fn ovicopter_lay_eggs_effect_pushes_three_tough_eggs() {
        // Bead sts2-rs-7qd1: C# `LayEggsMove` loops `for (int i = 0; i < 3; i++)`,
        // spawning 3 ToughEggs. Python pin only spawned 1.
        let mut state = CombatState::default();
        let pre = state.creatures.len();
        ovicopter_lay_eggs_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(state.creatures.len(), pre + 3);
        for new_egg in &state.creatures[pre..] {
            assert_eq!(new_egg.species, Some(MonsterKind::ToughEgg));
            // Bead sts2-rs-7twl bumped ToughEgg starting HP to 14..=18
            // per C# `ToughEgg.cs` MinInitialHp/MaxInitialHp.
            assert!(new_egg.current_hp >= 14 && new_egg.current_hp <= 18);
            // Bead sts2-rs-ybtmu (Group B): each laid egg carries
            // MinionPower(1) per C# `PowerCmd.Apply<MinionPower>(egg, 1m,
            // base.Creature)` (Ovicopter.cs:85), so it is a secondary enemy
            // (auto-killed when the Ovicopter dies, excluded from the
            // non-minion leaf-eval).
            let is_minion = new_egg.powers.iter().any(|puid| {
                state
                    .powers
                    .iter()
                    .any(|p| p.id == puid.0 && p.kind == PowerKind::Minion)
            });
            assert!(is_minion, "laid egg must be tagged MinionPower");
            // Bead sts2-rs-z4jsg: C# `ToughEgg.AfterAddedToRoom` grants
            // HatchPower(2) when spawned mid-enemy-turn, BEFORE the
            // summon-site Minion apply — so power order is [Hatch, Minion].
            let kinds: Vec<PowerKind> = new_egg
                .powers
                .iter()
                .filter_map(|puid| state.powers.get_by_id(puid.0).map(|p| p.kind))
                .collect();
            assert_eq!(
                kinds,
                vec![PowerKind::Hatch, PowerKind::Minion],
                "egg powers are [Hatch, Minion] in C# apply order"
            );
        }
        // Bead sts2-rs-z4jsg: C# slot-sorts each egg to the last unused
        // egg slot (all BEFORE the ovicopter slot), netting array order
        // [spawn3, spawn2, spawn1, ...]. Front-insert reproduces it:
        // enemy_uids must list the eggs in reverse spawn order, ahead of
        // any pre-existing enemy.
        let egg_ids: Vec<u32> = state.creatures[pre..].iter().map(|c| c.id.get()).collect();
        let lead: Vec<u32> = state.enemy_uids[..3].iter().map(|u| u.0.get()).collect();
        assert_eq!(
            lead,
            vec![egg_ids[2], egg_ids[1], egg_ids[0]],
            "eggs front-inserted in reverse spawn order (slot-sort parity)"
        );
    }

    /// Bead `sts2-rs-z4jsg`: full ToughEgg hatch lifecycle. C#
    /// `ToughEgg.HatchMove` removes HatchPower + every other non-Minion
    /// power and re-rolls max+current HP to the Hatchling band via
    /// `Niche.NextInt(19, 22)`; the machine then loops NIBBLE_MOVE
    /// (attack 4).
    #[test]
    fn tough_egg_hatch_sheds_powers_and_rerolls_hatchling_hp() {
        let mut state = CombatState::default();
        let egg = state.spawn_enemy(MonsterKind::ToughEgg);
        state.set_creature_ai(egg, create_tough_egg_ai());
        apply_power_to_creature(&mut state, egg, PowerKind::Hatch, 1);
        apply_power_to_creature(&mut state, egg, PowerKind::Minion, 1);
        // A player-applied debuff the hatch must shed (C# removes every
        // non-Minion power, not just Hatch).
        apply_power_to_creature(&mut state, egg, PowerKind::Vulnerable, 2);

        tough_egg_hatch_effect(&mut state, egg);

        let c = state
            .creatures
            .iter()
            .find(|c| c.id == egg.0)
            .expect("egg alive");
        assert!(
            (19..=21).contains(&c.max_hp),
            "hatchling HP re-rolled into 19..=21 (got {})",
            c.max_hp
        );
        assert_eq!(c.current_hp, c.max_hp, "SetMaxAndCurrentHp — full HP");
        let kinds: Vec<PowerKind> = c
            .powers
            .iter()
            .filter_map(|puid| state.powers.get_by_id(puid.0).map(|p| p.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![PowerKind::Minion],
            "hatch sheds Hatch + Vulnerable, keeps the Minion tag"
        );
        // Machine shape: HATCH_MOVE (Summon intent) → NIBBLE_MOVE
        // (attack 4) self-loop.
        let ai = create_tough_egg_ai();
        match ai.states.get("NIBBLE_MOVE").expect("NIBBLE present") {
            sts2_core::monster_ai::MonsterState::Move(m) => {
                assert_eq!(m.intents.as_slice(), &[Intent::Attack { damage: 4 }][..]);
                assert_eq!(m.follow_up, Some("NIBBLE_MOVE"), "nibble self-loops");
            }
            _ => panic!("NIBBLE_MOVE must be a MoveState"),
        }
    }

    /// **End-to-end Ovicopter minion cascade** (`sts2-rs-ybtmu`, Group B):
    /// tagging the eggs as minions makes the C# leader-death rule fire — when
    /// the Ovicopter (the lone primary) dies, the surviving egg-minions are
    /// killed and combat resolves as a win, instead of the player having to
    /// mop up three inert eggs. Proves the summon-gap fix integrates with
    /// `check_combat_end`'s `Kill(teammates)` collapse.
    #[test]
    fn ovicopter_death_collapses_egg_minions_to_victory() {
        use sts2_core::StepResult;

        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        let _ = state.init_player_creature();

        let ovicopter = state.spawn_enemy(MonsterKind::Ovicopter);
        ovicopter_lay_eggs_effect(&mut state, ovicopter);
        // Fixture sanity: 3 live egg-minions alongside the live Ovicopter.
        let egg_uids: Vec<_> = state
            .creatures
            .iter()
            .filter(|c| c.species == Some(MonsterKind::ToughEgg))
            .map(|c| c.id)
            .collect();
        assert_eq!(egg_uids.len(), 3, "fixture: 3 eggs laid");

        // The Ovicopter (the only primary) dies; eggs stay alive.
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == ovicopter.0) {
            c.current_hp = 0;
        }

        let result = sts2_core::combat_loop::check_combat_end(&mut state);

        assert_eq!(
            result,
            StepResult::CombatWon,
            "last primary (Ovicopter) dead + only egg-minions left ⇒ victory",
        );
        for egg in egg_uids {
            let hp = state
                .creatures
                .iter()
                .find(|c| c.id == egg)
                .map_or(99, |c| c.current_hp);
            assert!(hp <= 0, "egg-minion killed when the Ovicopter dies");
        }
    }

    /// Bead `sts2-rs-cezr9`: SpinyToad EXPLOSION strips the +5 Thorns
    /// that SPINES applied — C# `ExplosionMove`:
    /// `PowerCmd.Apply<ThornsPower>(self, -5m)`.
    #[test]
    fn spiny_toad_explosion_strips_five_thorns_from_self() {
        let mut state = CombatState::default();
        let owner = state.spawn_enemy(sts2_core::MonsterKind::SpinyToad);
        let id = owner.0;
        // Apply +5 Thorns (mirroring SPINES), then EXPLOSION strips them.
        spiny_toad_spines_effect(&mut state, owner);
        let thorns_after_spines = state
            .creatures
            .iter()
            .find(|c| c.id == id)
            .expect("toad")
            .powers
            .iter()
            .find_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::Thorns)
                    .map(|p| p.amount)
            })
            .unwrap_or(0);
        assert_eq!(thorns_after_spines, 5, "SPINES gives +5 Thorns");

        spiny_toad_explosion_effect(&mut state, owner);
        // C# `ShouldRemoveDueToAmount()` REMOVES the Thorns power at 0 — it
        // must not linger as a zero-amount instance (else `powers.len`
        // drifts +1 vs C#). Assert the Thorns power is gone from both the
        // global `powers` table and the creature's `powers` list.
        let thorns_pid = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Thorns)
            .map(|p| p.id);
        assert!(
            thorns_pid.is_none(),
            "EXPLOSION removes the Thorns power at 0, not clamp-to-0"
        );
        let toad_powers = state
            .creatures
            .iter()
            .find(|c| c.id == id)
            .expect("toad")
            .powers
            .len();
        assert_eq!(toad_powers, 0, "no zero-amount Thorns left on the toad");
    }

    /// Bead `sts2-rs-tasgy`: Wriggler WRIGGLE's `effect_fn` adds one
    /// Infection status card to the player's DISCARD per C#
    /// `Wriggler.cs::WriggleMove`
    /// (`AddToCombatAndPreview<Infection>(targets, PileType.Discard, 1)`).
    /// The accompanying +2 Strength self-buff is exercised by the
    /// existing `monster_buff_intent_applies_power` table-driven test
    /// in this module (Wriggler/Strength/2 row).
    #[test]
    fn wriggler_wriggle_effect_adds_one_infection_to_discard() {
        use sts2_core::cards::CardId;

        let mut state = CombatState::default();
        let pre_discard = state.player.discard.len();
        let pre_cards = state.cards.len();
        wriggler_wriggle_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(
            state.player.discard.len(),
            pre_discard + 1,
            "WRIGGLE must push exactly one card into discard"
        );
        assert_eq!(state.cards.len(), pre_cards + 1, "card arena grew by one");
        let last = *state.player.discard.last().expect("spawned");
        assert_eq!(
            state.cards[last.0.get() as usize - 1].card_id,
            CardId::Infection
        );
    }

    /// Bead `sts2-rs-baxi5`: Myte INFEST's `effect_fn` adds two Toxic
    /// status cards to the player's HAND per C# `Myte.cs:ToxicMove`
    /// (`AddToCombatAndPreview<Toxic>(targets, PileType.Hand, 2)`).
    /// Toxic's `OnTurnEndInHand` deals 5 unpowered self-damage per
    /// copy in hand at turn end — Hand placement is mechanically
    /// load-bearing. Replaces the prior Python-pin port that pushed
    /// one Parasite to Discard (`sts2-rs-h82l`).
    #[test]
    fn myte_infest_effect_adds_two_toxic_to_hand() {
        use sts2_core::cards::CardId;

        let mut state = CombatState::default();
        let pre_hand = state.player.hand.len();
        let pre_cards = state.cards.len();
        myte_infest_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(
            state.player.hand.len(),
            pre_hand + 2,
            "INFEST must push exactly two cards into hand"
        );
        assert_eq!(state.cards.len(), pre_cards + 2, "card arena grew by two");
        let h = &state.player.hand;
        let last = &state.cards[h[h.len() - 1].0.get() as usize - 1];
        let prev = &state.cards[h[h.len() - 2].0.get() as usize - 1];
        assert_eq!(last.card_id, CardId::Toxic);
        assert_eq!(prev.card_id, CardId::Toxic);
    }

    /// Bead `sts2-rs-asxdy`: LIQUIFY_GROUND applies `SandpitPower(4)` to the
    /// player and adds 6 `FranticEscape` status cards (3 → draw, 3 →
    /// discard) per C# `TheInsatiable.cs` `LiquifyMove`.
    #[test]
    fn the_insatiable_liquify_ground_applies_sandpit_and_six_frantic_escapes() {
        use sts2_core::cards::CardId;

        let mut state = CombatState::default();
        // SandpitPower is a self-buff on the boss, so spawn the boss creature
        // as the move owner (C# `base.Creature`). Bead `sts2-rs-ybtmu`.
        let owner = spawn_monster(&mut state, MK::TheInsatiable);
        let pre_draw = state.player.draw.len();
        let pre_discard = state.player.discard.len();
        let pre_shuffle = state.run_rng.shuffle.counter();
        let pre_rng = state.rng.counter();
        the_insatiable_liquify_ground_effect(&mut state, owner);

        // C# `CardPilePosition.Random` consumes one `Rng.Shuffle.NextInt` per
        // card across BOTH piles (3 draw + 3 discard = 6 total), and consumes
        // NOTHING from the legacy combat `rng`. Bead `sts2-rs-ybtmu`.
        assert_eq!(
            state.run_rng.shuffle.counter() - pre_shuffle,
            6,
            "6 random-position inserts (3 draw + 3 discard) each consume one Shuffle-RNG draw"
        );
        assert_eq!(
            state.rng.counter() - pre_rng,
            0,
            "the legacy combat rng stream is untouched"
        );

        assert_eq!(
            state.player.draw.len(),
            pre_draw + 3,
            "3 FranticEscape cards into the draw pile"
        );
        assert_eq!(
            state.player.discard.len(),
            pre_discard + 3,
            "3 FranticEscape cards into the discard pile"
        );
        for &uid in state
            .player
            .draw
            .iter()
            .skip(pre_draw)
            .chain(state.player.discard.iter().skip(pre_discard))
        {
            assert_eq!(
                state.cards[uid.0.get() as usize - 1].card_id,
                CardId::FranticEscape,
                "spawned cards are FranticEscape"
            );
        }

        // C# applies SandpitPower(4) to the boss itself, NOT the player.
        let owner_power_uids: Vec<_> = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .expect("boss creature present")
            .powers
            .iter()
            .copied()
            .collect();
        let sandpit = owner_power_uids.iter().find_map(|u| {
            state
                .powers
                .iter()
                .find(|p| p.id == u.0 && p.kind == PowerKind::Sandpit)
                .map(|p| p.amount)
        });
        assert_eq!(
            sandpit,
            Some(4),
            "SandpitPower(4) applied to the boss itself"
        );
        let player_has_sandpit = state.player.powers.iter().any(|u| {
            state
                .powers
                .iter()
                .any(|p| p.id == u.0 && p.kind == PowerKind::Sandpit)
        });
        assert!(!player_has_sandpit, "Sandpit is NOT applied to the player");
    }

    // ─────────────────────────────────────────────────────────────────────
    // Bead `sts2-rs-pdsdl`: telegraph-only Buff/Debuff power-apply effects.
    // ─────────────────────────────────────────────────────────────────────

    use sts2_core::{Creature, CreatureUid as Cuid, IntentHistory, MonsterId, MonsterKind as MK};

    /// Spawn a minimal monster Creature at the next free uid (`creatures.len()
    /// + 1`, as the engine mints them — uid 1 unless the player's mirror was
    /// pushed first) and return its uid.
    fn spawn_monster(state: &mut CombatState, species: MK) -> Cuid {
        let uid =
            Cuid::new(u32::try_from(state.creatures.len() + 1).expect("fits")).expect("nonzero");
        state.creatures.push(Creature {
            id: uid.0,
            current_hp: 100,
            max_hp: 100,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(species),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId(species.python_name()),
            ai: None,
        });
        state.enemy_uids.push(uid);
        uid
    }

    fn creature_power_amount(state: &CombatState, uid: Cuid, kind: PowerKind) -> i32 {
        let puids = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .map(|c| c.powers.clone())
            .unwrap_or_default();
        puids
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

    #[test]
    fn pdsdl_self_buff_effects_apply_strength_thorns_burrowed_flutter() {
        // (monster, effect, kind, expected) — all self-buffs vs the C# decomp.
        let cases: Vec<(MK, fn(&mut CombatState, Cuid), PowerKind, i32)> = vec![
            (MK::Tunneler, tunneler_burrow_effect, PowerKind::Burrowed, 1),
            (
                MK::BowlbugNectar,
                bowlbug_nectar_buff_effect,
                PowerKind::Strength,
                15,
            ),
            (
                MK::Exoskeleton,
                exoskeleton_enrage_effect,
                PowerKind::Strength,
                2,
            ),
            (
                MK::Wriggler,
                wriggler_wriggle_effect,
                PowerKind::Strength,
                2,
            ),
            (MK::Myte, myte_suck_effect, PowerKind::Strength, 2),
            (
                MK::Ovicopter,
                ovicopter_nutritional_paste_effect,
                PowerKind::Strength,
                3,
            ),
            (
                MK::SpinyToad,
                spiny_toad_spines_effect,
                PowerKind::Thorns,
                5,
            ),
            (
                MK::TheInsatiable,
                the_insatiable_salivate_effect,
                PowerKind::Strength,
                2,
            ),
            (
                MK::KnowledgeDemon,
                knowledge_demon_ponder_effect,
                PowerKind::Strength,
                2,
            ),
            (MK::Crusher, crusher_adapt_effect, PowerKind::Strength, 2),
            // `…wb8lk.11`: v0.107.1 PULSATE re-applies VitalSpark (A0 2), not
            // Strength. (The effect also grants 20 Block, checked elsewhere.)
            (
                MK::InfestedPrism,
                infested_prism_pulsate_effect,
                PowerKind::VitalSpark,
                2,
            ),
            (
                MK::DecimillipedeSegment,
                decimillipede_bulk_effect,
                PowerKind::Strength,
                2,
            ),
            (
                MK::ThievingHopper,
                thieving_hopper_flutter_effect,
                PowerKind::Flutter,
                5,
            ),
        ];
        for (species, effect, kind, expected) in cases {
            let mut state = CombatState::default();
            let uid = spawn_monster(&mut state, species);
            assert_eq!(creature_power_amount(&state, uid, kind), 0);
            effect(&mut state, uid);
            assert_eq!(
                creature_power_amount(&state, uid, kind),
                expected,
                "{species:?} self-buff {kind:?} should be {expected}"
            );
        }
    }

    /// `KnowledgeDemon` PONDER heals the boss 30 (× player-count, ×1 in
    /// single-player) per `KnowledgeDemon.cs:213` `CreatureCmd.Heal(
    /// base.Creature, 30 * Players.Count)`. The heal was previously
    /// dropped; the `pdsdl` table above already covers the Strength+2
    /// rider, so this isolates the heal on a damaged boss.
    #[test]
    fn knowledge_demon_ponder_heals_thirty() {
        let mut state = CombatState::default();
        let uid = spawn_monster(&mut state, MK::KnowledgeDemon);
        // spawn_monster opens at 100/100; chip it down so the heal is
        // observable below the max-HP clamp.
        state
            .creatures
            .iter_mut()
            .find(|c| c.id == uid.0)
            .expect("boss")
            .current_hp = 50;
        knowledge_demon_ponder_effect(&mut state, uid);
        let hp = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .map(|c| c.current_hp)
            .expect("boss");
        assert_eq!(hp, 80, "PONDER should heal 30 (50 → 80)");
        assert_eq!(
            creature_power_amount(&state, uid, PowerKind::Strength),
            2,
            "PONDER still applies Strength +2 alongside the heal"
        );
    }

    /// C#-timing gate for CURSE_OF_KNOWLEDGE (`KnowledgeDemon.cs:149-189`).
    /// The blocking card screen opens INSIDE the enemy turn — `CurseOfKnowledge`
    /// `await`s `CardSelectCmd.FromChooseACardScreen(new
    /// BlockingPlayerChoiceContext(), …)` before `CurseOfKnowledgeCounter++` and
    /// before the turn continues. The port used to defer it to the next
    /// `start_player_turn`, landing a full turn ahead of the live game at the
    /// same decision.
    ///
    /// Drives a real `Action::EndTurn` against a live KnowledgeDemon and locks
    /// the whole shape: the dispatch suspends mid-enemy-turn at the round it
    /// started, with the 2-option `MonsterCurseSelect` open and the demon still
    /// telegraphing / standing on CURSE_OF_KNOWLEDGE; resolving option 0 lands
    /// `DisintegrationPower(6)` on the player and lets the parked enemy turn
    /// finish (round advances, the move machine advances to SLAP and rolls its
    /// attack telegraph).
    #[test]
    fn knowledge_demon_curse_screen_opens_mid_enemy_turn() {
        use sts2_core::{apply, Action, ChoiceKind, CombatPhase, Side, StepResult};

        let kd_state_id = |state: &CombatState, uid: Cuid| -> Option<&'static str> {
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .and_then(|c| c.ai.as_ref())
                .map(MonsterAi::current_state_id)
        };

        let mut state = CombatState::default();
        state.player.current_hp = 200;
        state.player.max_hp = 200;
        let _ = state.init_player_creature();
        let kd = state.spawn_enemy(MonsterKind::KnowledgeDemon);
        state.set_creature_ai(kd, create_knowledge_demon_ai());
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == kd.0) {
            // CURSE_OF_KNOWLEDGE's telegraph (C# `DebuffIntent`).
            c.intent = Some(Intent::Debuff {
                kind: PowerKind::Weak,
                amount: 0,
            });
        }
        state.round = 1;
        state.side = Side::Player;
        state.phase = CombatPhase::PlayerPlayPhase;

        // ── The enemy turn suspends ON the curse screen. ──────────────────
        let result = apply(&mut state, Action::EndTurn);
        assert_eq!(
            result,
            StepResult::PendingChoice,
            "EndTurn must stop on the blocking curse screen, not run past it"
        );
        assert_eq!(
            state.round, 1,
            "the round does NOT advance while the screen is open (C# pauses in place)"
        );
        assert_eq!(
            state.phase,
            CombatPhase::EnemyTurnExec,
            "the pause is mid-enemy-turn"
        );
        assert_eq!(state.side, Side::Enemy, "still the enemy's turn");
        let pc = state
            .pending_choice
            .as_ref()
            .expect("curse screen must be open");
        assert_eq!(pc.kind, ChoiceKind::MonsterCurseSelect);
        assert_eq!(
            pc.options
                .iter()
                .map(|o| o.card_id)
                .collect::<Vec<sts2_core::cards::CardId>>(),
            vec![
                sts2_core::cards::CardId::Disintegration,
                sts2_core::cards::CardId::MindRot
            ],
            "counter 0 offers [Disintegration, MindRot]"
        );
        assert_eq!(
            kd_state_id(&state, kd),
            Some("CURSE_OF_KNOWLEDGE"),
            "the move machine has NOT advanced while the screen is open"
        );
        assert_eq!(
            player_power_amount(&state, PowerKind::Disintegration),
            0,
            "no curse power lands before the pick"
        );

        // ── Picking Disintegration applies it and resumes the enemy turn. ──
        let mut sel: smallvec::SmallVec<[u8; 4]> = smallvec::SmallVec::new();
        sel.push(0);
        let _ = apply(&mut state, Action::ResolveChoice { selection: sel });
        assert_eq!(
            player_power_amount(&state, PowerKind::Disintegration),
            6,
            "counter 0 → DisintegrationPower(6) (`_disintegrationDamageValues[0]`)"
        );
        assert!(
            state.pending_choice.is_none() && state.pending_enemy_exec_resume.is_none(),
            "the screen and the parked enemy turn are both consumed"
        );
        assert_eq!(
            state.round, 2,
            "the enemy turn completes on resolution and the round advances"
        );
        assert_eq!(
            state.phase,
            CombatPhase::PlayerPlayPhase,
            "the whole EndTurn settles back to the resting player phase"
        );
        assert_eq!(
            kd_state_id(&state, kd),
            Some("SLAP"),
            "CURSE_OF_KNOWLEDGE → SLAP follow-up"
        );
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == kd.0)
                .and_then(|c| c.intent),
            Some(Intent::Attack { damage: 17 }),
            "SLAP telegraph rolled after the resumed turn ended"
        );
    }

    /// `sts2-rs-ybtmu`: CURSE_OF_KNOWLEDGE's `effect_fn` does NOT apply a power
    /// directly — it stamps the pre-increment curse counter onto
    /// `state.pending_curse_choice` (which the core `perform_intent` tail turns
    /// into the blocking card screen, mid-enemy-turn) and advances the AI
    /// counter. The full curse → screen → power flow is exercised by
    /// `knowledge_demon_curse_screen_opens_mid_enemy_turn` above and by the core
    /// `combat_loop` `drive_curse_choice` tests.
    #[test]
    fn knowledge_demon_curse_defers_choice_and_advances_counter() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;
        use sts2_core::{Creature, CreatureUid, IntentHistory, MonsterId, PowerUid};

        let mut state = CombatState::default();
        let raw = NonZeroU32::new(1).expect("nz");
        let powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        state.creatures.push(Creature {
            id: raw,
            current_hp: 379,
            max_hp: 379,
            block: 0,
            powers,
            species: Some(MK::KnowledgeDemon),
            monster_id: MonsterId("KnowledgeDemon"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(create_knowledge_demon_ai()),
        });
        let owner = CreatureUid(raw);
        if let Some(ai) = state.creatures[0].ai.as_mut() {
            ai.set_owner(owner);
        }

        // First curse: stamps the pre-increment counter (0), advances to 1.
        knowledge_demon_curse_of_knowledge_effect(&mut state, owner);
        assert_eq!(state.pending_curse_choice, Some(0));
        assert_eq!(
            state.creatures[0]
                .ai
                .as_ref()
                .map(sts2_core::monster_ai::MonsterAi::counter),
            Some(1)
        );
        // No power is applied by the effect itself (deferred to the choice).
        assert_eq!(player_power_amount(&state, PowerKind::Disintegration), 0);

        // Second curse stamps counter 1; third stamps counter 2.
        knowledge_demon_curse_of_knowledge_effect(&mut state, owner);
        assert_eq!(state.pending_curse_choice, Some(1));
        knowledge_demon_curse_of_knowledge_effect(&mut state, owner);
        assert_eq!(state.pending_curse_choice, Some(2));
        assert_eq!(
            state.creatures[0]
                .ai
                .as_ref()
                .map(sts2_core::monster_ai::MonsterAi::counter),
            Some(3)
        );
    }

    #[test]
    fn obscura_sail_buffs_all_teammates_including_self() {
        // `TheObscura.WailMove` applies StrengthPower(3) to
        // `GetTeammatesOf(self)` → all living enemy-side creatures,
        // INCLUDING the caster (`CombatState.GetCreaturesOnSide`).
        // Bead `sts2-rs-0q46p`.
        let mut state = CombatState::default();
        let obscura = spawn_monster(&mut state, MK::TheObscura);
        // Second enemy-side creature (a spawned Parafright stand-in) at uid 2.
        let teammate = Cuid::new(2).expect("nonzero");
        state.creatures.push(Creature {
            id: teammate.0,
            current_hp: 30,
            max_hp: 30,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MK::TheObscura),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId(MK::TheObscura.python_name()),
            ai: None,
        });
        state.enemy_uids.push(teammate);

        assert_eq!(
            creature_power_amount(&state, obscura, PowerKind::Strength),
            0
        );
        assert_eq!(
            creature_power_amount(&state, teammate, PowerKind::Strength),
            0
        );

        the_obscura_sail_effect(&mut state, obscura);

        assert_eq!(
            creature_power_amount(&state, obscura, PowerKind::Strength),
            3,
            "WailMove buffs self (teammates includes caster)"
        );
        assert_eq!(
            creature_power_amount(&state, teammate, PowerKind::Strength),
            3,
            "WailMove buffs every enemy-side creature"
        );
    }

    #[test]
    fn chomper_screech_adds_three_dazed_to_discard() {
        // `Chomper.ScreechMove` = `StatusIntent(3)` → 3 Dazed to the
        // player's discard pile. Bead `sts2-rs-ae8rc`.
        use sts2_core::cards::CardId;
        let mut state = CombatState::default();
        let pre = state.player.discard.len();
        chomper_screech_effect(&mut state, Cuid::new(1).expect("nonzero"));
        assert_eq!(
            state.player.discard.len(),
            pre + 3,
            "SCREECH adds 3 Dazed to discard"
        );
        for &uid in state.player.discard.iter().skip(pre) {
            assert_eq!(
                state.cards[uid.0.get() as usize - 1].card_id,
                CardId::Dazed,
                "spawned cards are Dazed"
            );
        }
    }

    #[test]
    fn obscura_illusion_spawns_parafright_before_obscura() {
        // `TheObscura.IllusionMove` adds a Parafright into the "illusion"
        // slot, which precedes "obscura" in `TheObscuraNormal.Slots`, so
        // the spawn sorts ahead of Obscura. Bead `sts2-rs-3szrv`.
        let mut state = CombatState::default();
        let obscura = state.spawn_enemy(MonsterKind::TheObscura);
        // The slot rides on the creature's `MonsterAi`, so it needs one.
        state.set_creature_ai(obscura, create_the_obscura_ai());
        // Stamp the encounter's slots the way combat entry does: Obscura sits
        // in "obscura" (index 1), leaving "illusion" (0) for the summon.
        // Without the stamp the slot model has nothing to sort against and
        // degrades to append (GH #552).
        state.assign_encounter_slots(&[1]);
        let pre = state.creatures.len();

        the_obscura_illusion_effect(&mut state, obscura);

        assert_eq!(state.creatures.len(), pre + 1, "one Parafright spawned");
        assert_eq!(state.enemy_uids.len(), 2, "Obscura + spawned Parafright");
        let kind_of = |uid: Cuid| {
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .and_then(|c| c.species)
        };
        assert_eq!(
            kind_of(state.enemy_uids[0]),
            Some(MonsterKind::Parafright),
            "illusion slot precedes obscura — Parafright must be enemy_uids[0]",
        );
        assert_eq!(kind_of(state.enemy_uids[1]), Some(MonsterKind::TheObscura));

        // Bead `sts2-rs-cuocf`: the spawned Parafright must carry its
        // real `create_parafright_ai()` moveset, not fall back to the
        // static `MonsterKind::roll_intent` path (which fires only when
        // `Creature::ai` is `None`).
        let parafright = state
            .creatures
            .iter()
            .find(|c| c.id == state.enemy_uids[0].0)
            .expect("spawned Parafright is in the arena");
        assert!(
            parafright.ai.is_some(),
            "Parafright must have its AI attached (cuocf), not the roll_intent fallback",
        );
    }

    #[test]
    fn wrklt_summoner_spawns_carry_their_ai() {
        // Bead `sts2-rs-wrklt` (cuocf follow-up): Ovicopter LAY_EGGS
        // (3x ToughEgg) must attach each summon's real moveset, not leave
        // it on the static `roll_intent` fallback (`Creature::ai == None`).
        // (LouseProgenitor's fabricated 2-Wriggler SPAWN was removed under
        // bead `sts2-rs-ybtmu` — C# LouseProgenitor never summons.)
        let cases: Vec<(fn(&mut CombatState, Cuid), MonsterKind, MonsterKind, usize)> = vec![(
            ovicopter_lay_eggs_effect,
            MonsterKind::Ovicopter,
            MonsterKind::ToughEgg,
            3,
        )];
        for (effect, summoner_kind, summon_kind, count) in cases {
            let mut state = CombatState::default();
            // Plain `spawn_enemy` leaves the summoner on `ai: None`; the
            // effect_fn does the summoning + AI attachment under test.
            let owner = state.spawn_enemy(summoner_kind);
            let pre = state.creatures.len();

            effect(&mut state, owner);

            assert_eq!(
                state.creatures.len(),
                pre + count,
                "{summoner_kind:?}: {count} summons spawned",
            );
            let summons_with_ai = state
                .creatures
                .iter()
                .filter(|c| c.species == Some(summon_kind) && c.ai.is_some())
                .count();
            assert_eq!(
                summons_with_ai, count,
                "{summoner_kind:?}: all {count} {summon_kind:?} summons must carry an AI (wrklt)",
            );
        }
    }

    /// Bead `sts2-rs-97w3y`: four moves carry a SECONDARY `Intent::Defend`
    /// (dropped by `perform_intent`), so their effect_fns must grant the
    /// block. Verified C# block amounts (non-asc).
    #[test]
    fn w97w3y_secondary_defend_moves_grant_block() {
        let cases: Vec<(fn(&mut CombatState, Cuid), MK, i32)> = vec![
            (bowlbug_egg_bite_effect, MK::BowlbugEgg, 7),
            (the_obscura_hardening_strike_effect, MK::TheObscura, 6),
            (infested_prism_radiate_effect, MK::InfestedPrism, 11),
            (crusher_guarded_strike_effect, MK::Crusher, 18),
        ];
        for (effect, kind, expected) in cases {
            let mut state = CombatState::default();
            let uid = state.spawn_enemy(kind);
            let before = state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .map_or(0, |c| c.block);
            effect(&mut state, uid);
            let after = state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .map_or(0, |c| c.block);
            assert_eq!(after - before, expected, "{kind:?} secondary-Defend block");
        }
    }

    #[test]
    fn pdsdl_player_debuff_effects_apply_weak_tender_vulnerable() {
        // (effect, kind, expected) — single-debuff player effects.
        let cases: Vec<(fn(&mut CombatState, Cuid), PowerKind, i32)> = vec![
            (bowlbug_silk_toxic_spit_effect, PowerKind::Weak, 1),
            (hunter_killer_goop_effect, PowerKind::Tender, 1),
            (ovicopter_tenderizer_effect, PowerKind::Vulnerable, 2),
            (decimillipede_constrict_effect, PowerKind::Weak, 1),
        ];
        for (effect, kind, expected) in cases {
            let mut state = CombatState::default();
            let uid = spawn_monster(&mut state, MK::Tunneler);
            assert_eq!(player_power_amount(&state, kind), 0);
            effect(&mut state, uid);
            assert_eq!(
                player_power_amount(&state, kind),
                expected,
                "player debuff {kind:?} should be {expected}"
            );
        }
    }

    #[test]
    fn pdsdl_crusher_bug_sting_applies_weak_and_frail() {
        // `Crusher.cs:144-145` applies BOTH WeakPower 2 and FrailPower 2.
        let mut state = CombatState::default();
        let uid = spawn_monster(&mut state, MK::Crusher);
        crusher_bug_sting_effect(&mut state, uid);
        assert_eq!(player_power_amount(&state, PowerKind::Weak), 2);
        assert_eq!(player_power_amount(&state, PowerKind::Frail), 2);
    }

    /// **End-to-end Burrowed block-break** (`sts2-rs-ybtmu`, Group A): drive
    /// the *real* `create_tunneler_ai` + Burrowed power + block through the
    /// damage pipeline. A player attack that breaks the burrowed shell must
    /// fire the `after_block_broken` chokepoint → redirect the Tunneler to
    /// `DIZZY_MOVE` (the C# `Stun(Owner, "BITE_MOVE")` analogue) and strip
    /// Burrowed. This also pins the cross-crate string contract: the
    /// `"DIZZY_MOVE"` id forced from `sts2-core` must exist in this AI.
    #[test]
    fn burrowed_block_break_unburrows_and_stuns_real_tunneler() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();

        // Spawn the Tunneler with its real AI, burrowed shell (Burrowed + 32
        // block), telegraphing BELOW (its burrowed self-loop).
        let uid = spawn_monster(&mut state, MK::Tunneler);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.ai = Some(create_tunneler_ai());
            c.ai.as_mut().unwrap().set_owner(uid);
            c.block = 32;
        }
        apply_power_to_creature(&mut state, uid, PowerKind::Burrowed, 1);
        // Walk the AI to its burrowed BELOW self-loop so the redirect has a
        // non-DIZZY move to replace.
        {
            let c = state.creatures.iter_mut().find(|c| c.id == uid.0).unwrap();
            let ai = c.ai.as_mut().unwrap();
            ai.on_move_performed(); // performed BITE (initial)
            ai.roll_move_seeded(0); // → BURROW
            ai.on_move_performed();
            ai.roll_move_seeded(0); // → BELOW
        }
        let cur = |s: &CombatState| {
            s.creatures
                .iter()
                .find(|c| c.id == uid.0)
                .and_then(|c| c.ai.as_ref())
                .map(sts2_core::MonsterAi::current_state_id)
                .unwrap()
        };
        assert_eq!(cur(&state), "BELOW_MOVE_1", "burrowed, looping BELOW");

        // A 20-damage chip does NOT break the 32 block → no stun, still
        // burrowed (block absorbs, 12 remains).
        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            20,
            ValueProps::NONE,
            None,
        );
        assert_eq!(
            cur(&state),
            "BELOW_MOVE_1",
            "partial chip does not unburrow"
        );
        assert_eq!(creature_power_amount(&state, uid, PowerKind::Burrowed), 1);

        // A 20-damage hit now breaks the remaining 12 block → unburrow + stun.
        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            20,
            ValueProps::NONE,
            None,
        );
        assert_eq!(
            cur(&state),
            "DIZZY_MOVE",
            "broken shell → immediate redirect to DIZZY_MOVE (stun)",
        );
        // Bead `sts2-rs-gpdng`: the redirect must ALSO re-point the
        // telegraphed `creature.intent` — `enemy_turn_exec` performs the
        // snapshot intent, so a stale BELOW `Attack{23}` telegraph would
        // still land on the very turn C# has the Tunneler STUNNED (the
        // `encounter:TUNNELER_WEAK` coverage divergence: player.hp cs=21
        // rs=3).
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .and_then(|c| c.intent),
            Some(sts2_core::Intent::Stun),
            "block-break redirect re-telegraphs the stun (C# Stun replaces \
             the intent immediately)",
        );
        assert_eq!(
            creature_power_amount(&state, uid, PowerKind::Burrowed),
            0,
            "Burrowed is removed when the shell breaks",
        );

        // Confirm the stun→bite cadence: performing DIZZY then rolling yields
        // BITE_MOVE (its follow-up).
        {
            let c = state.creatures.iter_mut().find(|c| c.id == uid.0).unwrap();
            let ai = c.ai.as_mut().unwrap();
            ai.on_move_performed();
            assert_eq!(
                ai.roll_move_seeded(0).id,
                "BITE_MOVE",
                "after the stun turn the Tunneler bites",
            );
        }
    }

    /// **EXPOSE breaks the burrowed shell** (issue #179, `MONSTER_STATE|EXPOSE`,
    /// TUNNELER_WEAK seed 851). C# `Expose.OnPlay` strips block via
    /// `CreatureCmd.LoseBlock`, which fires `Hook.AfterBlockBroken` on a >0→0
    /// transition. The Rust port zeroed `block` directly, so a burrowed Tunneler
    /// whose 11-block shell EXPOSE broke kept `BurrowedPower` (and never
    /// unburrowed/stunned) where C# removed it. Drive the REAL `play_expose`
    /// against a real burrowed Tunneler and confirm the unburrow fires.
    #[test]
    fn expose_block_strip_unburrows_real_tunneler() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::cards::{CardId, CardType, TargetType};
        use sts2_core::{Card, CardUid};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;

        let uid = spawn_monster(&mut state, MK::Tunneler);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.ai = Some(create_tunneler_ai());
            c.ai.as_mut().unwrap().set_owner(uid);
            c.block = 11; // a partial shell EXPOSE will strip to 0
        }
        apply_power_to_creature(&mut state, uid, PowerKind::Burrowed, 1);
        // Walk the AI into its burrowed BELOW self-loop (the move the unburrow
        // redirect replaces).
        {
            let ai = state
                .creatures
                .iter_mut()
                .find(|c| c.id == uid.0)
                .unwrap()
                .ai
                .as_mut()
                .unwrap();
            ai.on_move_performed();
            ai.roll_move_seeded(0); // → BURROW
            ai.on_move_performed();
            ai.roll_move_seeded(0); // → BELOW
        }

        // Card-play context for `play_expose` (`current_card_upgraded` reads the
        // top of the Play pile): a non-upgraded EXPOSE.
        let card_uid = std::num::NonZeroU32::new(state.cards.len() as u32 + 1).expect("nz");
        state.cards.push(Card {
            uid: CardUid(card_uid),
            card_id: CardId::Expose,
            cost: 0,
            card_type: CardType::Skill,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            ..Card::pod_defaults()
        });
        state.player.play.push(CardUid(card_uid));

        crate::cards::silent::expose::play_expose(&mut state, Some(uid));

        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .map(|c| c.block),
            Some(0),
            "EXPOSE strips the shell block to 0",
        );
        assert_eq!(
            creature_power_amount(&state, uid, PowerKind::Burrowed),
            0,
            "breaking the shell via EXPOSE's LoseBlock unburrows the Tunneler",
        );
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .and_then(|c| c.ai.as_ref())
                .map(sts2_core::MonsterAi::current_state_id),
            Some("DIZZY_MOVE"),
            "unburrow redirects the Tunneler to its stun (DIZZY_MOVE)",
        );
    }

    /// **FlutterPower reactive behavior** (`sts2-rs-ybtmu` / `sts2-rs-r9075`):
    /// drives the real damage pipeline against a ThievingHopper carrying
    /// `Flutter`. Verifies (1) powered attacks are halved while Flutter is
    /// active, (2) each unblocked powered hit decrements Flutter, and (3) at 0
    /// the power is removed AND the hopper is stunned for one turn (redirected
    /// to `STUNNED_MOVE`, then resuming the telegraphed move).
    #[test]
    fn flutter_halves_decrements_and_stuns_real_hopper() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};

        // NB: do NOT init a player creature here — the test-local
        // `spawn_monster` hardcodes the enemy at uid 1, which would collide
        // with the player mirror (also uid 1 in a fresh arena) and make
        // `owner_of_listener` resolve the hopper's Flutter as player-owned.
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;

        let uid = spawn_monster(&mut state, MK::ThievingHopper);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.ai = Some(create_thieving_hopper_ai());
            c.ai.as_mut().unwrap().set_owner(uid);
            c.current_hp = 79;
            c.max_hp = 79;
        }
        // Walk the chain to NAB_MOVE so the stun has a real telegraph to
        // resume to (THIEVERY → FLUTTER → HAT_TRICK → NAB).
        {
            let c = state.creatures.iter_mut().find(|c| c.id == uid.0).unwrap();
            let ai = c.ai.as_mut().unwrap();
            for _ in 0..3 {
                ai.on_move_performed();
                ai.roll_move_seeded(0);
            }
        }
        let cur = |s: &CombatState| {
            s.creatures
                .iter()
                .find(|c| c.id == uid.0)
                .and_then(|c| c.ai.as_ref())
                .map(sts2_core::MonsterAi::current_state_id)
                .unwrap()
        };
        assert_eq!(cur(&state), "NAB_MOVE", "walked to the telegraphed NAB");

        // Flutter(2): a powered 8-damage hit is HALVED to 4, Flutter 2→1.
        apply_power_to_creature(&mut state, uid, PowerKind::Flutter, 2);
        let hp_before = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .unwrap()
            .current_hp;
        let loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            8,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(loss, 4, "powered attack halved by Flutter (8 → 4)");
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .unwrap()
                .current_hp,
            hp_before - 4,
        );
        assert_eq!(
            creature_power_amount(&state, uid, PowerKind::Flutter),
            1,
            "Flutter decremented 2 → 1 on the powered hit",
        );
        // Not yet stunned (Flutter still > 0).
        assert_eq!(cur(&state), "NAB_MOVE");

        // Second powered hit: Flutter 1→0 → removed AND hopper stunned.
        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            8,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            creature_power_amount(&state, uid, PowerKind::Flutter),
            0,
            "Flutter removed at 0 (Counter power)",
        );
        assert_eq!(
            cur(&state),
            "STUNNED_MOVE",
            "depleted Flutter stuns the hopper"
        );
        // The STORED intent telegraph must also flip to Stun — the enemy-turn
        // perform loop reads `creature.intent`, not the live `current_move()`,
        // so without this the stale telegraph would perform anyway (the bug
        // that left the THIEVING_HOPPER_WEAK sweep bucket flagged). Bead
        // `sts2-rs-r9075`.
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .unwrap()
                .intent,
            Some(Intent::Stun),
            "stored intent telegraph flips to Stun so the perform loop honors the stun",
        );

        // After the wasted STUNNED turn, the hopper resumes at the FOLLOW-UP
        // of the telegraphed NAB — the stun consumes that telegraph
        // (C# `FlutterPower.cs:47` passes `StateLog.Last().GetNextState()`).
        {
            let c = state.creatures.iter_mut().find(|c| c.id == uid.0).unwrap();
            let ai = c.ai.as_mut().unwrap();
            ai.on_move_performed();
            assert_eq!(
                ai.roll_move_seeded(0).id,
                "ESCAPE_MOVE",
                "stun consumes the NAB telegraph; resumes at its follow-up (C# FlutterPower.cs:47)",
            );
        }
    }

    /// **End-to-end HunterKiller Tender** (`sts2-rs-ybtmu`, Group C): drive
    /// the *real* `hunter_killer_goop_effect` (applies TenderPower to the
    /// player) and then a player card play through the real
    /// `dispatch_after_card_played` bus. Pins the cross-crate seam: the goop's
    /// Tender (applied to the legacy player-power surface) must be found by the
    /// after-card-played drain, dropping the player's Strength/Dexterity by 1.
    /// The first card's -1 lands on the mirror creature even from a 0 base
    /// (Str/Dex are AllowNegative). Restores at the player's turn end.
    #[test]
    fn hunter_killer_goop_tender_drains_and_restores_through_real_buses() {
        use sts2_core::hooks::{dispatch_after_card_played, dispatch_after_turn_end, CardCtx};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let player_uid = state.init_player_creature();
        let hk = spawn_monster(&mut state, MK::HunterKiller);

        // Goop applies TenderPower(1) to the player (telegraphed Debuff).
        hunter_killer_goop_effect(&mut state, hk);
        assert_eq!(player_power_amount(&state, PowerKind::Tender), 1);
        assert_eq!(
            creature_power_amount(&state, player_uid, PowerKind::Strength),
            0
        );

        // Two player card plays → Str/Dex each drop to -2 on the mirror.
        let ctx = CardCtx::default();
        dispatch_after_card_played(&mut state, &ctx);
        dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(
            creature_power_amount(&state, player_uid, PowerKind::Strength),
            -2,
            "Tender drains the player's Strength by 1 per card played",
        );
        assert_eq!(
            creature_power_amount(&state, player_uid, PowerKind::Dexterity),
            -2,
        );

        // Player turn end restores both to the 0 base.
        dispatch_after_turn_end(
            &mut state,
            &sts2_core::hooks::TurnSideCtx {
                side: sts2_core::Side::Player,
            },
        );
        assert_eq!(
            creature_power_amount(&state, player_uid, PowerKind::Strength),
            0,
            "Tender restores the drained Strength at the player's turn end",
        );
        assert_eq!(
            creature_power_amount(&state, player_uid, PowerKind::Dexterity),
            0,
        );
    }

    /// Build a real SlumberingBeetle with its AI + Plating(15) + Slumber(3),
    /// the way `spawn_enemy` + `on_spawn_self_buffs` do in real combat.
    fn spawn_real_slumbering_beetle(state: &mut CombatState) -> Cuid {
        use sts2_core::card_effects::apply_power_to_creature;
        let uid = spawn_monster(state, MK::SlumberingBeetle);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.ai = Some(create_slumbering_beetle_ai());
            c.ai.as_mut().unwrap().set_owner(uid);
        }
        apply_power_to_creature(state, uid, PowerKind::Plating, 15);
        apply_power_to_creature(state, uid, PowerKind::Slumber, 3);
        uid
    }

    fn beetle_move(state: &CombatState, uid: Cuid) -> &'static str {
        state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .and_then(|c| c.ai.as_ref())
            .map(sts2_core::MonsterAi::current_state_id)
            .unwrap()
    }

    /// **End-to-end SlumberingBeetle wake (turn-end path)** (`sts2-rs-ybtmu`):
    /// drive the *real* AI + Slumber(3)/Plating(15) through the real
    /// `dispatch_after_turn_end(Enemy)`. Pins the C#-headless-harness
    /// observable: Slumber ticks 3→2→1→0 over three beetle turn-ends; at 0 the
    /// beetle wakes (Slumber + Plating both stripped) and its move is forced to
    /// ROLL_OUT_MOVE. Also pins the cross-crate "ROLL_OUT_MOVE" string contract
    /// (the id core's `force_current_state` targets must exist in this AI).
    #[test]
    fn slumbering_beetle_turn_end_ticks_then_wakes_to_rollout() {
        use sts2_core::hooks::{dispatch_after_turn_end, TurnSideCtx};
        use sts2_core::Side;

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let uid = spawn_real_slumbering_beetle(&mut state);
        assert_eq!(beetle_move(&state, uid), "SNORE_MOVE");

        let end = |s: &mut CombatState| {
            dispatch_after_turn_end(s, &TurnSideCtx { side: Side::Enemy });
        };

        end(&mut state); // turn-end 1: 3→2
        assert_eq!(creature_power_amount(&state, uid, PowerKind::Slumber), 2);
        assert_eq!(beetle_move(&state, uid), "SNORE_MOVE", "still asleep");
        end(&mut state); // turn-end 2: 2→1
        assert_eq!(creature_power_amount(&state, uid, PowerKind::Slumber), 1);
        assert_eq!(beetle_move(&state, uid), "SNORE_MOVE");
        // Bead `sts2-rs-gpdng`: in real combat `enemy_turn_exec` step 4 has
        // already telegraphed next turn's intent (Sleep — the beetle still
        // held Slumber when it rolled) BEFORE this turn-end hook runs. The
        // wake must re-point that stale telegraph at the rollout, else next
        // phase `perform_intent` deals the telegraphed 0 damage while still
        // firing ROLL_OUT's effect (+2 Strength early) — the
        // `encounter:SLUMBERING_BEETLE_NORMAL` coverage divergence (C# kills
        // the player through the 16-damage rollout; Rust left them at 4 hp).
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.intent = Some(sts2_core::Intent::Sleep);
        }
        end(&mut state); // turn-end 3: 1→0 → wake
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Slumber),
            "Slumber stripped at 0",
        );
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Plating),
            "Plating stripped on wake (C# WakeUpMove)",
        );
        assert_eq!(
            beetle_move(&state, uid),
            "ROLL_OUT_MOVE",
            "woke at Slumber 0 → forced to ROLL_OUT_MOVE",
        );
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .and_then(|c| c.intent),
            Some(sts2_core::Intent::Attack { damage: 16 }),
            "turn-end wake re-points the stale Sleep telegraph at the rollout \
             (C# telegraphs AFTER the AfterTurnEnd hooks)",
        );
    }

    /// **SlumberingBeetle wake (unblocked-damage path)** (`sts2-rs-ybtmu`):
    /// C# `SlumberPower.AfterDamageReceived` ticks Slumber on each unblocked
    /// hit. With no Plating shell (block 0) three 5-damage hits drive Slumber
    /// 3→2→1→0 and wake the beetle. Pins the `dispatch_after_damage_received`
    /// inline arm.
    #[test]
    fn slumbering_beetle_unblocked_damage_ticks_then_wakes() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let uid = spawn_real_slumbering_beetle(&mut state);
        // Drop the Plating shell so the hits land unblocked (the shell would
        // otherwise absorb them — exactly why the harness attack-run didn't
        // wake the beetle faster).
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.block = 0;
        }

        for expected in [2, 1] {
            apply_damage(
                &mut state,
                CreatureRef::Player,
                uid,
                5,
                ValueProps::NONE,
                None,
            );
            assert_eq!(
                creature_power_amount(&state, uid, PowerKind::Slumber),
                expected,
            );
            assert_eq!(beetle_move(&state, uid), "SNORE_MOVE", "asleep until 0");
        }
        // Third unblocked hit drives Slumber to 0 → wake.
        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            5,
            ValueProps::NONE,
            None,
        );
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Slumber),
            "Slumber stripped at 0 via the damage path",
        );
        // C# `SlumberPower.AfterDamageReceived` → `CreatureCmd.Stun(WakeUpMove,
        // "ROLL_OUT_MOVE")`: the beetle's NEXT turn is a no-op stun running
        // WakeUpMove (remove Plating, no Strength/attack); only the turn AFTER
        // is ROLL_OUT_MOVE. So the damage-path wake forces the WAKE_UP_MOVE stun
        // state (which follow-ups to ROLL_OUT_MOVE), NOT ROLL_OUT directly —
        // otherwise the +2 Strength and 16 attack land one turn early. Bead
        // `sts2-rs-7i736`.
        assert_eq!(
            beetle_move(&state, uid),
            "WAKE_UP_MOVE",
            "unblocked-damage wake forces the WAKE_UP_MOVE stun turn first",
        );
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .and_then(|c| c.ai.as_ref())
                .and_then(|ai| ai.current_move().follow_up),
            Some("ROLL_OUT_MOVE"),
            "WAKE_UP_MOVE advances to ROLL_OUT_MOVE the turn after the stun",
        );
        // Plating is still present immediately after the damage hit — C# Stun
        // runs WakeUpMove (the Plating removal) on the stun turn, not now.
        assert!(
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .is_some_and(|c| c.powers.iter().any(|u| state
                    .powers
                    .iter()
                    .any(|p| p.id == u.0 && p.kind == PowerKind::Plating))),
            "Plating removal is deferred to the WAKE_UP_MOVE stun turn",
        );
    }

    /// **Plating shell absorbs → no damage-tick** (`sts2-rs-ybtmu`): mirrors
    /// the harness attack-run — hits fully absorbed by the Plating shell
    /// (UnblockedDamage == 0) do NOT tick Slumber.
    #[test]
    fn slumbering_beetle_plating_absorbed_hit_does_not_tick_slumber() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let uid = spawn_real_slumbering_beetle(&mut state);
        // Plating-equivalent shell of 15 block; a 6-damage Strike is absorbed.
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.block = 15;
        }
        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            6,
            ValueProps::NONE,
            None,
        );
        assert_eq!(
            creature_power_amount(&state, uid, PowerKind::Slumber),
            3,
            "fully-absorbed hit (0 unblocked) must NOT tick Slumber",
        );
        assert_eq!(beetle_move(&state, uid), "SNORE_MOVE");
    }

    /// Build a real Entomancer with its AI (which installs the
    /// PersonalHive AfterDamageReceived hook) + the on-spawn
    /// `PersonalHivePower(1)` that C# `Entomancer.AfterAddedToRoom`
    /// applies.
    fn spawn_real_entomancer(state: &mut CombatState) -> Cuid {
        use sts2_core::card_effects::apply_power_to_creature;
        let uid = spawn_monster(state, MK::Entomancer);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.ai = Some(create_entomancer_ai());
            c.ai.as_mut().unwrap().set_owner(uid);
        }
        apply_power_to_creature(state, uid, PowerKind::PersonalHive, 1);
        uid
    }

    fn draw_dazed_count(state: &CombatState) -> usize {
        use sts2_core::cards::CardId;
        state
            .player
            .draw
            .iter()
            .filter(|cu| state.cards[cu.0.get() as usize - 1].card_id == CardId::Dazed)
            .count()
    }

    /// **PersonalHive Dazed flood (powered player hit)** (`sts2-rs-ybtmu`):
    /// C# `PersonalHivePower.AfterDamageReceived` shoves `Amount` Dazed into
    /// the player's draw pile when the Entomancer takes a powered attack.
    /// At spawn the hive is 1 → exactly one Dazed. Pins the new
    /// monster-power `after_damage_received` lane end-to-end through the
    /// real `apply_damage` pipeline.
    #[test]
    fn entomancer_personal_hive_floods_one_dazed_on_powered_player_hit() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let uid = spawn_real_entomancer(&mut state);
        let pre_draw = state.player.draw.len();

        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            10,
            ValueProps::MOVE,
            None,
        );

        assert_eq!(
            state.player.draw.len(),
            pre_draw + 1,
            "hive(1) → exactly one Dazed into draw",
        );
        assert_eq!(draw_dazed_count(&state), 1, "the new draw card is Dazed");
    }

    /// **PersonalHive floods Dazed on an OSTY (pet) attack too**
    /// (`sts2-rs-1jqu3`): C# `PersonalHivePower.AfterDamageReceived` redirects
    /// an Osty dealer to its player owner (`dealer.Monster is Osty → PetOwner`)
    /// and still floods. A Necrobinder Osty attack (SQUEEZE/POKE/…) deals with
    /// `CreatureRef::Enemy(pet_uid)` as the dealer, so the gate must accept the
    /// player's living pet. Regression for Necrobinder fuzz seed 406 (SQUEEZE
    /// vs ENTOMANCER: draw cs=2 rs=0 before the fix).
    #[test]
    fn entomancer_personal_hive_floods_dazed_on_osty_hit() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let uid = spawn_real_entomancer(&mut state);
        // Summon the player's Osty pet — the dealer for SQUEEZE et al.
        let pet = state.summon_osty(1).expect("osty summoned");
        let pre_draw = state.player.draw.len();

        apply_damage(
            &mut state,
            CreatureRef::Enemy(pet),
            uid,
            10,
            ValueProps::MOVE,
            None,
        );

        assert_eq!(
            state.player.draw.len(),
            pre_draw + 1,
            "an Osty powered hit floods Dazed too (hive(1) → one Dazed)",
        );
        assert_eq!(draw_dazed_count(&state), 1, "the new draw card is Dazed");
    }

    /// **PersonalHive Dazed count scales with the hive counter**
    /// (`sts2-rs-ybtmu`): two PHEROMONE_SPITs ramp the hive 1→3, so the
    /// next powered hit floods 3 Dazed (C# `for i in 0..Amount`).
    #[test]
    fn entomancer_personal_hive_dazed_count_scales_with_hive() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let uid = spawn_real_entomancer(&mut state);
        // Ramp hive 1 → 3 (each sub-3 spit also grants the Entomancer
        // Strength +1, which is irrelevant to an incoming player attack).
        entomancer_pheromone_spit_effect(&mut state, uid);
        entomancer_pheromone_spit_effect(&mut state, uid);
        assert_eq!(
            creature_power_amount(&state, uid, PowerKind::PersonalHive),
            3
        );
        let pre_draw = state.player.draw.len();

        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            10,
            ValueProps::MOVE,
            None,
        );

        assert_eq!(
            state.player.draw.len(),
            pre_draw + 3,
            "hive(3) → three Dazed into draw",
        );
        assert_eq!(draw_dazed_count(&state), 3);
    }

    /// **PersonalHive ignores unpowered hits** (`sts2-rs-ybtmu`): C# gates
    /// the flood on `props.IsPoweredAttack()`, so an unpowered hit (Thorns/
    /// reflection, `ValueProp.Unpowered`) shoves no Dazed.
    #[test]
    fn entomancer_personal_hive_ignores_unpowered_hit() {
        use sts2_core::damage::apply_damage;
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let uid = spawn_real_entomancer(&mut state);
        let pre_draw = state.player.draw.len();

        // Unpowered hit (no MOVE flag) → is_powered() == false → no flood.
        apply_damage(
            &mut state,
            CreatureRef::Player,
            uid,
            5,
            ValueProps::NONE,
            None,
        );

        assert_eq!(
            state.player.draw.len(),
            pre_draw,
            "unpowered hit must not flood Dazed",
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // Mutation-coverage tests (issue #65). Each targets a specific surviving
    // mutant in a lookup/guard predicate by seeding a decoy power/creature so
    // the && / == / > operator becomes load-bearing.
    // ─────────────────────────────────────────────────────────────────────

    /// `apply_power_to_player_via_legacy_surface` (act2_hive:111): the
    /// existing-power lookup `p.id == puid.0 && p.kind == kind` must stack
    /// into the SAME-kind instance and ignore a decoy of a different kind.
    /// Kills the `==`/`&&` mutants at line 111.
    #[test]
    fn legacy_surface_stacks_same_kind_past_decoy() {
        let mut state = CombatState::default();
        // Decoy player power of a DIFFERENT kind, applied first.
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Vulnerable, 2);
        // First Weak: creates a new instance (no existing Weak).
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Weak, 1);
        // Second Weak: must find + stack the existing Weak, not create a 2nd.
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Weak, 3);

        assert_eq!(
            player_power_amount(&state, PowerKind::Weak),
            4,
            "second Weak stacks into the existing instance (1+3)"
        );
        // Exactly one Weak instance — a mistargeted lookup would push a 2nd.
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
        assert_eq!(weak_instances, 1, "no duplicate Weak instance created");
        // Decoy untouched.
        assert_eq!(player_power_amount(&state, PowerKind::Vulnerable), 2);
    }

    /// `apply_power_to_player_via_legacy_surface` (act2_hive:111): the
    /// existing-instance id-match `p.id == puid.0`. With NO decoy power, the
    /// SAME-kind stack must reuse the one instance — under `id != puid.0` the
    /// lookup finds nothing and pushes a duplicate. Kills `==`→`!=` (111:32).
    #[test]
    fn legacy_surface_stacks_into_single_instance_no_decoy() {
        let mut state = CombatState::default();
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Weak, 1);
        apply_power_to_player_via_legacy_surface(&mut state, PowerKind::Weak, 3);
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
            "stacking reuses the single Weak instance (id match)"
        );
        assert_eq!(player_power_amount(&state, PowerKind::Weak), 4);
        assert_eq!(
            state.player.powers.len(),
            1,
            "exactly one player power total"
        );
    }

    /// `the_obscura_sail_effect` (act2_hive): every LIVING enemy-side creature
    /// gains Strength +3. (The `c.current_hp > 0` filter at 186 is EQUIVALENT
    /// to mutate — `apply_power_to_creature` itself no-ops on a dead target —
    /// but this still locks the living-target buff.)
    #[test]
    fn obscura_sail_skips_dead_enemy() {
        let mut state = CombatState::default();
        let alive = state.spawn_enemy(MonsterKind::TheObscura);
        let dead = state.spawn_enemy(MonsterKind::Parafright);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == dead.0) {
            c.current_hp = 0;
        }
        the_obscura_sail_effect(&mut state, alive);
        assert_eq!(
            creature_power_amount(&state, alive, PowerKind::Strength),
            3,
            "living enemy gains Strength +3"
        );
        assert_eq!(
            creature_power_amount(&state, dead, PowerKind::Strength),
            0,
            "dead (hp==0) enemy must NOT gain Strength (hp > 0 gate)"
        );
    }

    /// `spiny_toad_explosion_effect` (act2_hive:308): the owner lookup
    /// `c.id == owner.0 && c.current_hp > 0` must skip a dead owner. With a
    /// dead owner carrying Thorns, EXPLOSION strips nothing. Kills `>`→`>=`
    /// (308:55) and the dead-creature path of `&&`→`||` (308:39).
    #[test]
    fn spiny_toad_explosion_skips_dead_owner_keeps_thorns() {
        let mut state = CombatState::default();
        let owner = state.spawn_enemy(MonsterKind::SpinyToad);
        spiny_toad_spines_effect(&mut state, owner); // +5 Thorns
                                                     // Kill the owner.
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
            c.current_hp = 0;
        }
        spiny_toad_explosion_effect(&mut state, owner);
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Thorns),
            5,
            "dead owner (hp==0) is not found → Thorns NOT stripped"
        );
    }

    /// `spiny_toad_explosion_effect` (act2_hive:320): the Thorns-power lookup
    /// `p.id == puid.0 && p.kind == Thorns` must ignore a decoy power of a
    /// different kind. With a Strength decoy present, EXPLOSION still strips
    /// exactly the Thorns. Kills `&&`→`||` (320:42).
    #[test]
    fn spiny_toad_explosion_strips_thorns_past_strength_decoy() {
        let mut state = CombatState::default();
        let owner = state.spawn_enemy(MonsterKind::SpinyToad);
        // Decoy of a different kind first, then Thorns.
        apply_power_to_creature(&mut state, owner, PowerKind::Strength, 9);
        spiny_toad_spines_effect(&mut state, owner); // +5 Thorns
        spiny_toad_explosion_effect(&mut state, owner);
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Thorns),
            0,
            "Thorns stripped to 0 (removed)"
        );
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Thorns),
            "Thorns power removed entirely"
        );
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Strength),
            9,
            "Strength decoy untouched"
        );
    }

    /// `decimillipede_reattach_effect` (act2_hive:464/487): both the
    /// sibling-liveness scan and the owner Reattach-amount read must match
    /// the Reattach power by kind and skip a decoy of another kind. A sibling
    /// carrying a non-Reattach power does NOT count as a living segment.
    /// Kills `==`/`&&` at 464 and the `==`/`&&` at 487.
    #[test]
    fn reattach_requires_sibling_with_reattach_power() {
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let front = state.spawn_enemy(MonsterKind::DecimillipedeSegmentFront);
        let sibling = state.spawn_enemy(MonsterKind::DecimillipedeSegmentBack);
        // Strip every on-spawn power from both so the only Reattach powers in
        // play are the ones this test grants explicitly.
        let clear_powers = |state: &mut CombatState, uid: Cuid| {
            let pids: Vec<std::num::NonZeroU32> = state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .map(|c| c.powers.iter().map(|u| u.0).collect())
                .unwrap_or_default();
            state.powers.retain(|p| !pids.contains(&p.id));
            if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
                c.powers.clear();
            }
        };
        clear_powers(&mut state, front);
        clear_powers(&mut state, sibling);
        // Owner carries a Strength(99) decoy BEFORE its Reattach(20) so the
        // owner amount-read `p.id == puid.0 && p.kind == Reattach` (487) is
        // load-bearing: under `&&`→`||` the decoy's id-match returns 99 →
        // heal to 99; under a kind-mismatch the sibling's Reattach(25) is
        // grabbed → heal to 25. Clean must heal to the owner's own 20. (Apply
        // both while the owner is still alive — apply_power no-ops on a corpse.)
        apply_power_to_creature(&mut state, front, PowerKind::Strength, 99);
        apply_power_to_creature(&mut state, front, PowerKind::Reattach, 20);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == front.0) {
            c.current_hp = 0;
            c.max_hp = 100;
        }
        // Sibling is ALIVE but carries only a non-Reattach (Strength) power —
        // it must NOT satisfy the `carries_reattach` gate.
        apply_power_to_creature(&mut state, sibling, PowerKind::Strength, 1);

        decimillipede_reattach_effect(&mut state, front);
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == front.0)
                .unwrap()
                .current_hp,
            0,
            "sibling lacks Reattach → no living segment → no heal"
        );

        // Now grant the sibling a Reattach(25) power: the gate holds and the
        // owner's OWN Reattach(20) amount drives the heal (not the sibling's).
        apply_power_to_creature(&mut state, sibling, PowerKind::Reattach, 25);
        decimillipede_reattach_effect(&mut state, front);
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == front.0)
                .unwrap()
                .current_hp,
            20,
            "heal uses the OWNER's Reattach amount (20), not the sibling's (25)"
        );
    }

    /// `decimillipede_reattach_effect` (act2_hive:487:32): the owner
    /// amount-read id-match `p.id == puid.0`. Owner carries ONLY Reattach(20)
    /// (no decoy that could re-find it via the wrong puid), while the living
    /// sibling carries a DISTINCT Reattach(25). Clean reads the owner's own
    /// 20; under `id != puid.0` the read falls through to the sibling's 25.
    /// Kills `==`→`!=` (487:32).
    #[test]
    fn reattach_reads_owner_amount_not_sibling_amount() {
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let _ = state.init_player_creature();
        let front = state.spawn_enemy(MonsterKind::DecimillipedeSegmentFront);
        let sibling = state.spawn_enemy(MonsterKind::DecimillipedeSegmentBack);
        let clear_powers = |state: &mut CombatState, uid: Cuid| {
            let pids: Vec<std::num::NonZeroU32> = state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .map(|c| c.powers.iter().map(|u| u.0).collect())
                .unwrap_or_default();
            state.powers.retain(|p| !pids.contains(&p.id));
            if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
                c.powers.clear();
            }
        };
        clear_powers(&mut state, front);
        clear_powers(&mut state, sibling);
        // Owner: ONLY Reattach(20). Sibling: a DISTINCT Reattach(25).
        apply_power_to_creature(&mut state, front, PowerKind::Reattach, 20);
        apply_power_to_creature(&mut state, sibling, PowerKind::Reattach, 25);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == front.0) {
            c.current_hp = 0;
            c.max_hp = 100;
        }
        decimillipede_reattach_effect(&mut state, front);
        assert_eq!(
            state
                .creatures
                .iter()
                .find(|c| c.id == front.0)
                .unwrap()
                .current_hp,
            20,
            "owner heals by its OWN Reattach(20), not the sibling's Reattach(25)"
        );
    }

    /// `tough_egg_hatch_effect` (act2_hive:802): the shed scan keeps Minion
    /// (`p.kind != Minion`) and sheds every other power. With both a Minion
    /// and a Vulnerable decoy on the egg, only Vulnerable is shed. Kills the
    /// `!=`→`==` mutant at 802:40 and 810:67 (`c.id == owner.0`).
    #[test]
    fn tough_egg_hatch_keeps_minion_sheds_others() {
        let mut state = CombatState::default();
        let egg = state.spawn_enemy(MonsterKind::ToughEgg);
        let decoy = state.spawn_enemy(MonsterKind::ToughEgg);
        apply_power_to_creature(&mut state, egg, PowerKind::Minion, 1);
        apply_power_to_creature(&mut state, egg, PowerKind::Vulnerable, 2);
        // Decoy egg keeps its own Vulnerable — proves the owner-id filter
        // (810:67) only touches `egg`, not `decoy`.
        apply_power_to_creature(&mut state, decoy, PowerKind::Vulnerable, 3);

        tough_egg_hatch_effect(&mut state, egg);

        assert_eq!(
            creature_power_amount(&state, egg, PowerKind::Minion),
            1,
            "Minion kept on hatch (kind != Minion filter)"
        );
        assert_eq!(
            creature_power_amount(&state, egg, PowerKind::Vulnerable),
            0,
            "Vulnerable shed on hatch"
        );
        assert_eq!(
            creature_power_amount(&state, decoy, PowerKind::Vulnerable),
            3,
            "decoy egg's powers untouched (owner-id filter)"
        );
        // The egg's OWN powers vec must drop the shed Vulnerable puid (not just
        // the global power table) — under `c.id != owner.0` the retain runs on
        // the wrong creature, leaving a dangling puid on the egg. So the egg
        // keeps exactly one puid (Minion).
        let egg_puids = state
            .creatures
            .iter()
            .find(|c| c.id == egg.0)
            .unwrap()
            .powers
            .len();
        assert_eq!(
            egg_puids, 1,
            "egg's powers vec holds only the kept Minion puid (shed puid removed from owner)"
        );
        // Decoy egg keeps its own single Vulnerable puid (untouched).
        let decoy_puids = state
            .creatures
            .iter()
            .find(|c| c.id == decoy.0)
            .unwrap()
            .powers
            .len();
        assert_eq!(decoy_puids, 1, "decoy egg's powers vec untouched");
    }

    /// `entomancer_personal_hive_amount` (act2_hive:886): the read must match
    /// PersonalHive by kind. A creature carrying ONLY a Strength power must
    /// report hive == 0. Kills `&&`→`||` (886:46) — under `||` the id-match
    /// alone would return the Strength power's amount.
    #[test]
    fn entomancer_hive_amount_zero_without_personal_hive() {
        // Bare creature (no on-spawn PersonalHive buff) carrying only
        // Strength — the kind filter must report hive == 0.
        let mut state = CombatState::default();
        let uid = spawn_monster(&mut state, MonsterKind::Entomancer);
        apply_power_to_creature(&mut state, uid, PowerKind::Strength, 5);
        assert_eq!(
            entomancer_personal_hive_amount(&state, uid),
            0,
            "no PersonalHive → 0 (kind filter, not id-only)"
        );
    }

    /// `curl_up_after_damage` (act2_hive:988): the CurlUp lookup
    /// `p.id == puid.0 && p.kind == CurlUp && p.amount > 0`. The owner's powers
    /// are, IN ORDER: Strength(3) decoy, a spent CurlUp(amount 0), then the
    /// live CurlUp(8). A powered player hit must latch ONLY the live CurlUp's
    /// counter. Kills `&&`→`||` (988:46 — would latch the id-matched Strength),
    /// `&&`→`||` (988:77 — same via the amount clause) and `>`→`>=` (988:89 —
    /// would latch the spent amount-0 CurlUp instead).
    #[test]
    fn curl_up_latches_live_curlup_past_decoy_and_spent_curlup() {
        use sts2_core::hooks::{CreatureRef, ValueProps};

        let mut state = CombatState::default();
        // Bare owner (no on-spawn CurlUp) so the power set is exactly ours.
        let owner = spawn_monster(&mut state, MonsterKind::LouseProgenitor);

        // Build three powers in a known order with distinct ids.
        let push_power = |state: &mut CombatState, owner: Cuid, kind: PowerKind, amount: i32| {
            let id = std::num::NonZeroU32::new(
                state.powers.iter().map(|p| p.id.get()).max().unwrap_or(0) + 1,
            )
            .expect("nz");
            state.powers.push(sts2_core::Power {
                id,
                kind,
                amount,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            if let Some(c) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
                c.powers.push(sts2_core::PowerUid(id));
            }
            id
        };
        push_power(&mut state, owner, PowerKind::Strength, 3);
        let spent_curlup = push_power(&mut state, owner, PowerKind::CurlUp, 0);
        let live_curlup = push_power(&mut state, owner, PowerKind::CurlUp, 8);

        // `unblocked_damage = 0`: C# CurlUp latches even on a fully-blocked
        // powered hit (its body ignores the DamageResult).
        let res = curl_up_after_damage(&mut state, owner, CreatureRef::Player, ValueProps::MOVE, 0);
        assert_eq!(res, EffectResult::Resolved, "powered player hit latches");

        let counter_of = |state: &CombatState, id: std::num::NonZeroU32| -> i32 {
            state
                .powers
                .iter()
                .find(|p| p.id == id)
                .map_or(-1, |p| p.counter)
        };
        assert_eq!(
            counter_of(&state, live_curlup),
            1,
            "the LIVE CurlUp(8) is latched to counter 1"
        );
        assert_eq!(
            counter_of(&state, spent_curlup),
            0,
            "the spent CurlUp(0) is NOT latched (amount > 0 gate)"
        );
        // Strength decoy's counter must stay 0 (it must not be mistaken for
        // the CurlUp power).
        let strength_counter = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Strength)
            .map_or(-1, |p| p.counter);
        assert_eq!(strength_counter, 0, "Strength decoy not latched");
    }

    /// `curl_up_after_damage` guard (act2_hive:974): an UNPOWERED player hit
    /// and a powered NON-player hit must both be ignored (NotImplemented).
    /// Kills the `delete !` (974:8) and `||`→`&&` (974:28) guard mutants.
    #[test]
    fn curl_up_ignores_unpowered_and_nonplayer_hits() {
        use sts2_core::hooks::{CreatureRef, ValueProps};
        let mut state = CombatState::default();
        let owner = state.spawn_enemy(MonsterKind::LouseProgenitor);
        apply_power_to_creature(&mut state, owner, PowerKind::CurlUp, 8);

        // Unpowered player hit → guard returns NotImplemented.
        assert_eq!(
            curl_up_after_damage(&mut state, owner, CreatureRef::Player, ValueProps::NONE, 1),
            EffectResult::NotImplemented,
            "unpowered hit is ignored (is_powered() guard)"
        );
        // Powered hit from a non-player source → ignored.
        let other = state.spawn_enemy(MonsterKind::SpinyToad);
        assert_eq!(
            curl_up_after_damage(
                &mut state,
                owner,
                CreatureRef::Enemy(other),
                ValueProps::MOVE,
                1
            ),
            EffectResult::NotImplemented,
            "powered non-player hit is ignored (dealer guard)"
        );
        // Neither should have latched the CurlUp counter.
        let latched = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .unwrap()
            .powers
            .iter()
            .find_map(|pu| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == pu.0 && p.kind == PowerKind::CurlUp)
                    .map(|p| p.counter)
            })
            .unwrap_or(-1);
        assert_eq!(latched, 0, "CurlUp counter not latched by ignored hits");
    }

    /// `bowlbug_rock_dizzy_effect` (act2_hive:1358): clears the off-balance
    /// counter on the OWNER's AI only. With two BowlbugRocks, DIZZY on one
    /// must not reset the other's counter. Kills `c.id == owner.0` → `!=`
    /// (1358:70).
    #[test]
    fn bowlbug_rock_dizzy_clears_only_owner_counter() {
        let mut state = CombatState::default();
        let a = state.spawn_enemy(MonsterKind::BowlbugRock);
        let b = state.spawn_enemy(MonsterKind::BowlbugRock);
        state.set_creature_ai(a, create_bowlbug_rock_ai());
        state.set_creature_ai(b, create_bowlbug_rock_ai());
        // Both off-balance.
        for uid in [a, b] {
            if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
                if let Some(ai) = c.ai.as_mut() {
                    ai.set_counter(1);
                }
            }
        }
        bowlbug_rock_dizzy_effect(&mut state, a);
        let counter_of = |state: &CombatState, uid: Cuid| -> i32 {
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .and_then(|c| c.ai.as_ref())
                .map_or(-1, MonsterAi::counter)
        };
        assert_eq!(counter_of(&state, a), 0, "owner counter cleared");
        assert_eq!(counter_of(&state, b), 1, "non-owner counter untouched");
    }

    /// `bowlbug_rock_is_off_balance` (act2_hive:1352): returns `counter != 0`.
    /// The mutant `replace ... with true` is killed by asserting the
    /// counter==0 case is FALSE.
    #[test]
    fn bowlbug_rock_off_balance_false_at_zero() {
        let state = CombatState::default();
        let uid = CreatureUid::new(1).expect("nz");
        assert!(
            !bowlbug_rock_is_off_balance(&state, uid, 0),
            "counter 0 is NOT off-balance"
        );
        assert!(
            bowlbug_rock_is_off_balance(&state, uid, 1),
            "counter 1 IS off-balance"
        );
    }

    /// `ovicopter_can_lay` (act2_hive:1875): counts living enemy-side
    /// creatures and lays only at `<= 3`. The liveness filter
    /// `c.id == uid.0 && c.current_hp > 0` must skip corpses. Kills the
    /// `replace -> true`, `&&`/`==`/`>` mutants at 1875.
    /// Mutation coverage (issue #65): `ovicopter_lay_eggs_effect` occupied-slot
    /// filter (act2_hive:707). A LIVE egg holds slot 4 and a DEAD egg holds
    /// slot 2. The filter `current_hp > 0 && species == ToughEgg` must count
    /// ONLY the living egg, so freshly-laid eggs reuse the dead egg's freed
    /// slot 2 (but NOT the live egg's slot 4). Kills the 707 `>`/`&&` mutants.
    #[test]
    fn ovicopter_lay_eggs_reuses_dead_egg_slot_and_orders_ascending() {
        let mut state = CombatState::default();
        let ovi = state.spawn_enemy(MonsterKind::Ovicopter);
        let live_egg = state.spawn_enemy(MonsterKind::ToughEgg);
        let dead_egg = state.spawn_enemy(MonsterKind::ToughEgg);
        // `set_enemy_slot` only records on a creature with an AI attached.
        state.set_creature_ai(live_egg, create_tough_egg_ai());
        state.set_creature_ai(dead_egg, create_tough_egg_ai());
        state.set_enemy_slot(live_egg, 4);
        state.set_enemy_slot(dead_egg, 2);
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == dead_egg.0) {
            c.current_hp = 0;
        }

        let pre_egg_ids: std::collections::HashSet<u32> =
            [live_egg.0.get(), dead_egg.0.get()].into_iter().collect();
        ovicopter_lay_eggs_effect(&mut state, ovi);

        // Slots taken by the THREE newly-laid living eggs: highest free first,
        // skipping the live egg's slot 4 but reusing the dead egg's slot 2.
        let new_egg_slots: Vec<u8> = state
            .enemy_uids
            .iter()
            .filter_map(|u| {
                let c = state.creatures.iter().find(|c| c.id == u.0)?;
                if c.species == Some(MonsterKind::ToughEgg)
                    && c.current_hp > 0
                    && !pre_egg_ids.contains(&u.0.get())
                {
                    state.enemy_slot(*u)
                } else {
                    None
                }
            })
            .collect();
        let mut sorted = new_egg_slots.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            vec![1, 2, 3],
            "new eggs fill slots 3,2,1 — slot 2 reused from the DEAD egg, slot 4 kept by the LIVE egg"
        );

        // Insert-position parity (732/737): each newly-laid egg (slot < 4)
        // is inserted BEFORE the pre-existing live egg (slot 4) in enemy_uids.
        let pos = |state: &CombatState, uid: Cuid| -> usize {
            state
                .enemy_uids
                .iter()
                .position(|u| u.0 == uid.0)
                .expect("in enemy_uids")
        };
        let live_egg_pos = pos(&state, live_egg);
        let new_egg_positions: Vec<usize> = state
            .enemy_uids
            .iter()
            .enumerate()
            .filter_map(|(i, u)| {
                let c = state.creatures.iter().find(|c| c.id == u.0)?;
                if c.species == Some(MonsterKind::ToughEgg)
                    && c.current_hp > 0
                    && !pre_egg_ids.contains(&u.0.get())
                {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(new_egg_positions.len(), 3, "three new eggs present");
        for p in &new_egg_positions {
            assert!(
                *p < live_egg_pos,
                "new egg (slot < 4) inserted before the live egg@4 (key > slot scan)"
            );
        }
        // Among themselves the new eggs are slot-descending in array order
        // (highest free slot 3 inserted first, then 2 ahead of it, then 1):
        // each new insert lands ahead of higher-slotted living eggs.
        let new_egg_slot_seq: Vec<u8> = new_egg_positions
            .iter()
            .map(|&i| state.enemy_slot(state.enemy_uids[i]).expect("slot"))
            .collect();
        assert_eq!(
            new_egg_slot_seq,
            vec![1, 2, 3],
            "new eggs are array-ordered slot-ascending among themselves (insert-before-greater-key)"
        );
    }

    /// Mutation coverage (issue #65): `ovicopter_lay_eggs_effect` insert
    /// position key (act2_hive:732). Two LIVING eggs bracket a gap — slot 2
    /// and slot 4 — so a freshly-laid slot-3 egg must insert BETWEEN them
    /// (after egg@2, before egg@4). Under the key-computation `==`→`!=`
    /// (732:40) the egg@2's key flips to 5, so the slot-3 egg inserts before
    /// egg@2 instead.
    #[test]
    fn ovicopter_lay_eggs_inserts_between_bracketing_eggs() {
        let mut state = CombatState::default();
        // Eggs precede the Ovicopter in enemy_uids (slot order), so the
        // egg-key comparison — not the Ovicopter — decides the insert point.
        let egg_lo = state.spawn_enemy(MonsterKind::ToughEgg); // slot 2
        let egg_hi = state.spawn_enemy(MonsterKind::ToughEgg); // slot 4
        let ovi = state.spawn_enemy(MonsterKind::Ovicopter);
        state.set_creature_ai(egg_lo, create_tough_egg_ai());
        state.set_creature_ai(egg_hi, create_tough_egg_ai());
        state.set_enemy_slot(egg_lo, 2);
        state.set_enemy_slot(egg_hi, 4);

        let pre_ids: std::collections::HashSet<u32> =
            [egg_lo.0.get(), egg_hi.0.get()].into_iter().collect();
        ovicopter_lay_eggs_effect(&mut state, ovi);

        // The new egg occupying slot 3 must sit AFTER egg@2 and BEFORE egg@4
        // in enemy_uids.
        let pos = |state: &CombatState, uid: Cuid| -> usize {
            state
                .enemy_uids
                .iter()
                .position(|u| u.0 == uid.0)
                .expect("present")
        };
        let slot3_egg = state
            .enemy_uids
            .iter()
            .find(|u| {
                state.creatures.iter().any(|c| {
                    c.id == u.0
                        && c.species == Some(MonsterKind::ToughEgg)
                        && !pre_ids.contains(&u.0.get())
                        && state.enemy_slot(**u) == Some(3)
                })
            })
            .copied()
            .expect("a new egg took slot 3");
        let p_lo = pos(&state, egg_lo);
        let p_hi = pos(&state, egg_hi);
        let p_3 = pos(&state, slot3_egg);
        assert!(
            p_lo < p_3 && p_3 < p_hi,
            "slot-3 egg inserts between egg@2 (idx {p_lo}) and egg@4 (idx {p_hi}), got idx {p_3}"
        );
    }

    #[test]
    fn ovicopter_can_lay_counts_only_living() {
        let mut state = CombatState::default();
        let ovi = state.spawn_enemy(MonsterKind::Ovicopter);
        // 3 more living enemies → side count 4 → cannot lay.
        let mut eggs = Vec::new();
        for _ in 0..3 {
            eggs.push(state.spawn_enemy(MonsterKind::ToughEgg));
        }
        assert!(
            !ovicopter_can_lay(&state, ovi),
            "4 living enemies (> 3) → cannot lay"
        );
        // Kill one egg → side count 3 → can lay.
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == eggs[0].0) {
            c.current_hp = 0;
        }
        assert!(
            ovicopter_can_lay(&state, ovi),
            "3 living enemies (<= 3) → can lay (corpse excluded)"
        );
    }

    /// Push a power onto `owner` with a fresh id, preserving insertion order
    /// in the creature's `powers` vec. Returns the power id.
    fn push_power_ordered(
        state: &mut CombatState,
        owner: Cuid,
        kind: PowerKind,
        amount: i32,
    ) -> std::num::NonZeroU32 {
        let id = std::num::NonZeroU32::new(
            state.powers.iter().map(|p| p.id.get()).max().unwrap_or(0) + 1,
        )
        .expect("nz");
        state.powers.push(sts2_core::Power {
            id,
            kind,
            amount,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == owner.0) {
            c.powers.push(sts2_core::PowerUid(id));
        }
        id
    }

    /// `slumbering_beetle_wake_effect` (act2_hive:1900): the Plating lookup
    /// `p.id == u.0 && p.kind == Plating`. The owner's powers are ordered
    /// [Strength decoy, Plating] (bare creature, no on-spawn Plating), so the
    /// decoy is iterated FIRST. WAKE must still remove ONLY Plating. Kills the
    /// `&&`→`||` (1900:43 — under `||` the decoy's id-match removes Strength
    /// instead), the `==`/`!=` mutants (1900/1905/1907) and the `replace with
    /// ()` (1891).
    #[test]
    fn slumbering_beetle_wake_removes_plating_past_decoy() {
        let mut state = CombatState::default();
        let owner = spawn_monster(&mut state, MonsterKind::SlumberingBeetle);
        push_power_ordered(&mut state, owner, PowerKind::Strength, 4);
        push_power_ordered(&mut state, owner, PowerKind::Plating, 6);
        slumbering_beetle_wake_effect(&mut state, owner);
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Plating),
            "WAKE removes the Plating power entirely"
        );
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Strength),
            4,
            "Strength decoy (iterated first) is NOT removed"
        );
        // The owner's powers vec must retain the Strength puid and drop ONLY
        // the Plating puid (kills the `c.powers.retain` `!=`→`==` at 1907 —
        // which would instead drop everything BUT Plating). The global powers
        // table must still hold the Strength power (kills the global-retain
        // `!=`→`==` at 1905).
        let owner_kinds: Vec<PowerKind> = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .unwrap()
            .powers
            .iter()
            .filter_map(|u| state.powers.iter().find(|p| p.id == u.0).map(|p| p.kind))
            .collect();
        assert_eq!(
            owner_kinds,
            vec![PowerKind::Strength],
            "only the Strength decoy remains on the owner (Plating dropped, Strength kept)"
        );
    }

    /// `slumbering_beetle_rollout_effect` (act2_hive:1932): same Plating lookup
    /// with a [Vulnerable decoy, Plating]-ordered bare owner. ROLLOUT removes
    /// ONLY Plating and grants Strength +2. Kills the `&&`→`||` (1932:43), the
    /// `==`/`!=` mutants at 1932 and the `replace with ()` (1923).
    #[test]
    fn slumbering_beetle_rollout_removes_plating_and_buffs() {
        let mut state = CombatState::default();
        let owner = spawn_monster(&mut state, MonsterKind::SlumberingBeetle);
        push_power_ordered(&mut state, owner, PowerKind::Vulnerable, 2);
        push_power_ordered(&mut state, owner, PowerKind::Plating, 6);
        slumbering_beetle_rollout_effect(&mut state, owner);
        assert!(
            !state.powers.iter().any(|p| p.kind == PowerKind::Plating),
            "ROLLOUT removes the Plating power entirely"
        );
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Strength),
            2,
            "ROLLOUT grants Strength +2"
        );
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Vulnerable),
            2,
            "Vulnerable decoy (iterated first) untouched"
        );
        // Owner's powers vec drops ONLY the Plating puid, keeping Vulnerable +
        // the freshly-granted Strength (kills the `c.powers.retain` `!=`→`==`
        // at 1939 — which would drop everything but Plating). The global table
        // keeps Vulnerable + Strength (kills the global-retain at 1937).
        let owner_kinds: std::collections::BTreeSet<PowerKind> = state
            .creatures
            .iter()
            .find(|c| c.id == owner.0)
            .unwrap()
            .powers
            .iter()
            .filter_map(|u| state.powers.iter().find(|p| p.id == u.0).map(|p| p.kind))
            .collect();
        assert_eq!(
            owner_kinds,
            [PowerKind::Vulnerable, PowerKind::Strength]
                .into_iter()
                .collect(),
            "owner keeps exactly Vulnerable + Strength (Plating dropped)"
        );
    }

    /// `knowledge_demon_curse_of_knowledge_effect` (act2_hive:2492): the
    /// `counter < 3` gate must NOT open a curse choice once the counter has
    /// reached 3. Kills the `<`→`<=` mutant at 2492:16.
    #[test]
    fn knowledge_demon_curse_no_choice_at_counter_three() {
        let mut state = CombatState::default();
        let owner = state.spawn_enemy(MonsterKind::KnowledgeDemon);
        let mut ai = create_knowledge_demon_ai();
        ai.set_owner(owner);
        ai.set_counter(3);
        state.set_creature_ai(owner, ai);
        state.pending_curse_choice = None;
        knowledge_demon_curse_of_knowledge_effect(&mut state, owner);
        assert_eq!(
            state.pending_curse_choice, None,
            "counter == 3 → no curse choice opened (counter < 3 is false)"
        );

        // Sanity: counter == 2 DOES open the choice (index 2).
        let mut state2 = CombatState::default();
        let owner2 = state2.spawn_enemy(MonsterKind::KnowledgeDemon);
        let mut ai2 = create_knowledge_demon_ai();
        ai2.set_owner(owner2);
        ai2.set_counter(2);
        state2.set_creature_ai(owner2, ai2);
        knowledge_demon_curse_of_knowledge_effect(&mut state2, owner2);
        assert_eq!(
            state2.pending_curse_choice,
            Some(2),
            "counter 2 < 3 opens curse choice at index 2"
        );
    }

    /// `louse_progenitor_web_effect` / `_curl_and_grow_effect`
    /// (act2_hive:651/660): the `replace ... with ()` mutants are killed by
    /// asserting WEB applies Frail(2) to the player and CURL_AND_GROW applies
    /// Strength(5) to self.
    #[test]
    fn louse_progenitor_web_and_curl_effects_apply_powers() {
        let mut state = CombatState::default();
        let owner = state.spawn_enemy(MonsterKind::LouseProgenitor);
        louse_progenitor_web_effect(&mut state, owner);
        assert_eq!(
            player_power_amount(&state, PowerKind::Frail),
            2,
            "WEB applies Frail(2) to the player"
        );
        louse_progenitor_curl_and_grow_effect(&mut state, owner);
        assert_eq!(
            creature_power_amount(&state, owner, PowerKind::Strength),
            5,
            "CURL_AND_GROW applies Strength(5) to self"
        );
    }
}
