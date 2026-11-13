//! Per-potion factories — bead `sts2-rs-13q` (P7-6).
//!
//! Ports `sts2-rl-agent@01c094d:sts2_env/potions/all.py` (63 potions).
//! Each factory returns a [`PotionSpec`] (static template). On-drink
//! behaviour is exposed through [`crate::potions::dispatch`] — handler
//! bodies are intentionally marker-only ([`EffectResult::Resolved`])
//! per the convention established by the relic pool beads. The full
//! per-potion gameplay bodies land in P7-8 (`sts2-rs-h9u`).

use sts2_core::{
    CharacterId, CombatState, DotNetRng, PotionKind, PotionRarity, PotionTargetType,
    PotionUsageType,
};

use crate::potions::{PotionEffect, PotionSpec};

// Local aliases mirroring `all.py` to keep the table compact.
const C: PotionRarity = PotionRarity::Common;
const U: PotionRarity = PotionRarity::Uncommon;
const R: PotionRarity = PotionRarity::Rare;
const E: PotionRarity = PotionRarity::Event;
const T: PotionRarity = PotionRarity::Token;

const CO: PotionUsageType = PotionUsageType::CombatOnly;
const AT: PotionUsageType = PotionUsageType::AnyTime;
const AU: PotionUsageType = PotionUsageType::Automatic;

const S: PotionTargetType = PotionTargetType::Self_;
const AE: PotionTargetType = PotionTargetType::AnyEnemy;
const ALE: PotionTargetType = PotionTargetType::AllEnemies;
const AP: PotionTargetType = PotionTargetType::AnyPlayer;

const fn spec(
    id: PotionKind,
    name: &'static str,
    rarity: PotionRarity,
    usage_type: PotionUsageType,
    target_type: PotionTargetType,
    character_lock: Option<CharacterId>,
    is_event_only: bool,
) -> PotionSpec {
    PotionSpec {
        id,
        name,
        rarity,
        usage_type,
        target_type,
        character_lock,
        is_event_only,
        effect: PotionEffect::Noop,
    }
}

/// Returns the [`PotionSpec`] for `kind`. Total over the 63 ported
/// kinds. The match exhaustiveness is enforced by `PotionKind::ALL`
/// in the unit tests below.
#[must_use]
pub const fn spec_for(kind: PotionKind) -> PotionSpec {
    match kind {
        // ── Common (20) ─────────────────────────────────────────────
        PotionKind::AttackPotion => spec(kind, "ATTACK_POTION", C, CO, S, None, false),
        PotionKind::BlockPotion => spec(kind, "BLOCK_POTION", C, CO, AP, None, false),
        PotionKind::BloodPotion => spec(
            kind,
            "BLOOD_POTION",
            C,
            AT,
            AP,
            Some(CharacterId::Ironclad),
            false,
        ),
        PotionKind::ColorlessPotion => spec(kind, "COLORLESS_POTION", C, CO, S, None, false),
        PotionKind::DexterityPotion => spec(kind, "DEXTERITY_POTION", C, CO, AP, None, false),
        PotionKind::EnergyPotion => spec(kind, "ENERGY_POTION", C, CO, AP, None, false),
        PotionKind::ExplosiveAmpoule => spec(kind, "EXPLOSIVE_AMPOULE", C, CO, ALE, None, false),
        PotionKind::FirePotion => spec(kind, "FIRE_POTION", C, CO, AE, None, false),
        PotionKind::FlexPotion => spec(kind, "FLEX_POTION", C, CO, AP, None, false),
        PotionKind::FocusPotion => spec(
            kind,
            "FOCUS_POTION",
            C,
            CO,
            S,
            Some(CharacterId::Defect),
            false,
        ),
        PotionKind::PoisonPotion => spec(
            kind,
            "POISON_POTION",
            C,
            CO,
            AE,
            Some(CharacterId::Silent),
            false,
        ),
        PotionKind::PotionOfDoom => spec(
            kind,
            "POTION_OF_DOOM",
            C,
            CO,
            AE,
            Some(CharacterId::Necrobinder),
            false,
        ),
        PotionKind::PowerPotion => spec(kind, "POWER_POTION", C, CO, S, None, false),
        PotionKind::SkillPotion => spec(kind, "SKILL_POTION", C, CO, S, None, false),
        PotionKind::SpeedPotion => spec(kind, "SPEED_POTION", C, CO, AP, None, false),
        PotionKind::StarPotion => spec(
            kind,
            "STAR_POTION",
            C,
            CO,
            S,
            Some(CharacterId::Regent),
            false,
        ),
        PotionKind::StrengthPotion => spec(kind, "STRENGTH_POTION", C, CO, AP, None, false),
        PotionKind::SwiftPotion => spec(kind, "SWIFT_POTION", C, CO, AP, None, false),
        PotionKind::VulnerablePotion => spec(kind, "VULNERABLE_POTION", C, CO, AE, None, false),
        PotionKind::WeakPotion => spec(kind, "WEAK_POTION", C, CO, AE, None, false),
        // ── Uncommon (20) ──────────────────────────────────────────
        PotionKind::Ashwater => spec(
            kind,
            "ASHWATER",
            U,
            CO,
            S,
            Some(CharacterId::Ironclad),
            false,
        ),
        PotionKind::BlessingOfTheForge => {
            spec(kind, "BLESSING_OF_THE_FORGE", U, CO, S, None, false)
        }
        PotionKind::BoneBrew => spec(
            kind,
            "BONE_BREW",
            U,
            CO,
            S,
            Some(CharacterId::Necrobinder),
            false,
        ),
        PotionKind::Clarity => spec(kind, "CLARITY", U, CO, AP, None, false),
        PotionKind::CunningPotion => spec(
            kind,
            "CUNNING_POTION",
            U,
            CO,
            S,
            Some(CharacterId::Silent),
            false,
        ),
        PotionKind::CureAll => spec(kind, "CURE_ALL", U, CO, AP, None, false),
        PotionKind::Duplicator => spec(kind, "DUPLICATOR", U, CO, S, None, false),
        PotionKind::Fortifier => spec(kind, "FORTIFIER", U, CO, AP, None, false),
        PotionKind::FyshOil => spec(kind, "FYSH_OIL", U, CO, AP, None, false),
        PotionKind::GamblersBrew => spec(kind, "GAMBLERS_BREW", U, CO, S, None, false),
        PotionKind::HeartOfIron => spec(kind, "HEART_OF_IRON", U, CO, AP, None, false),
        PotionKind::KingsCourage => spec(
            kind,
            "KINGS_COURAGE",
            U,
            CO,
            AP,
            Some(CharacterId::Regent),
            false,
        ),
        PotionKind::LiquidBronze => spec(kind, "LIQUID_BRONZE", U, CO, AP, None, false),
        PotionKind::PotionOfBinding => spec(kind, "POTION_OF_BINDING", U, CO, ALE, None, false),
        PotionKind::PotionOfCapacity => spec(
            kind,
            "POTION_OF_CAPACITY",
            U,
            CO,
            S,
            Some(CharacterId::Defect),
            false,
        ),
        PotionKind::PowderedDemise => spec(kind, "POWDERED_DEMISE", U, CO, AE, None, false),
        PotionKind::RadiantTincture => spec(kind, "RADIANT_TINCTURE", U, CO, AP, None, false),
        PotionKind::RegenPotion => spec(kind, "REGEN_POTION", U, CO, AP, None, true),
        PotionKind::StableSerum => spec(kind, "STABLE_SERUM", U, CO, AP, None, false),
        PotionKind::TouchOfInsanity => spec(kind, "TOUCH_OF_INSANITY", U, CO, S, None, false),
        // ── Rare (20) ──────────────────────────────────────────────
        PotionKind::BeetleJuice => spec(kind, "BEETLE_JUICE", R, CO, AE, None, false),
        PotionKind::BottledPotential => spec(kind, "BOTTLED_POTENTIAL", R, CO, AP, None, false),
        PotionKind::CosmicConcoction => spec(
            kind,
            "COSMIC_CONCOCTION",
            R,
            CO,
            S,
            Some(CharacterId::Regent),
            false,
        ),
        PotionKind::DistilledChaos => spec(kind, "DISTILLED_CHAOS", R, CO, S, None, false),
        PotionKind::DropletOfPrecognition => {
            spec(kind, "DROPLET_OF_PRECOGNITION", R, CO, S, None, false)
        }
        PotionKind::EntropicBrew => spec(kind, "ENTROPIC_BREW", R, AT, S, None, false),
        PotionKind::EssenceOfDarkness => spec(
            kind,
            "ESSENCE_OF_DARKNESS",
            R,
            CO,
            S,
            Some(CharacterId::Defect),
            false,
        ),
        PotionKind::FairyInABottle => spec(kind, "FAIRY_IN_A_BOTTLE", R, AU, S, None, true),
        PotionKind::FruitJuice => spec(kind, "FRUIT_JUICE", R, AT, AP, None, true),
        PotionKind::GhostInAJar => spec(
            kind,
            "GHOST_IN_A_JAR",
            R,
            CO,
            AP,
            Some(CharacterId::Silent),
            false,
        ),
        PotionKind::GigantificationPotion => {
            spec(kind, "GIGANTIFICATION_POTION", R, CO, AP, None, false)
        }
        PotionKind::LiquidMemories => spec(kind, "LIQUID_MEMORIES", R, CO, S, None, false),
        PotionKind::LuckyTonic => spec(kind, "LUCKY_TONIC", R, CO, AP, None, false),
        PotionKind::MazalethsGift => spec(kind, "MAZALETHS_GIFT", R, CO, AP, None, false),
        PotionKind::OrobicAcid => spec(kind, "OROBIC_ACID", R, CO, S, None, false),
        PotionKind::PotOfGhouls => spec(
            kind,
            "POT_OF_GHOULS",
            R,
            CO,
            S,
            Some(CharacterId::Necrobinder),
            false,
        ),
        PotionKind::ShacklingPotion => spec(kind, "SHACKLING_POTION", R, CO, ALE, None, false),
        PotionKind::ShipInABottle => spec(kind, "SHIP_IN_A_BOTTLE", R, CO, AP, None, false),
        PotionKind::SneckoOil => spec(kind, "SNECKO_OIL", R, CO, AP, None, false),
        PotionKind::SoldiersStew => spec(
            kind,
            "SOLDIERS_STEW",
            R,
            CO,
            AP,
            Some(CharacterId::Ironclad),
            false,
        ),
        // ── Event / Token (3) ──────────────────────────────────────
        PotionKind::FoulPotion => spec(kind, "FOUL_POTION", E, AT, S, None, true),
        PotionKind::GlowwaterPotion => spec(kind, "GLOWWATER_POTION", E, CO, S, None, true),
        PotionKind::PotionShapedRock => spec(kind, "POTION_SHAPED_ROCK", T, CO, AE, None, true),
        // `PotionKind` is `#[non_exhaustive]` to allow downstream pin
        // bumps to add variants without breaking SemVer. Every variant
        // shipped at pin `01c094d` is handled above; a future variant
        // landing here will need its own arm before the unit tests pass.
        #[allow(unreachable_patterns)]
        _ => panic!("PotionKind variant not handled by spec_for; add an arm in potions/all.rs"),
    }
}

// ---------------------------------------------------------------------------
// Out-of-combat random potion generation (C# PotionFactory)
// ---------------------------------------------------------------------------

/// C# `SharedPotionPool.GenerateAllPotions` order
/// (`MegaCrit.Sts2.Core.Models.PotionPools/SharedPotionPool.cs`, 45 potions).
///
/// This is the *colorless* pool every character draws from, in the exact
/// array order the game declares — `Rng.NextItem` indexes into the
/// rarity-filtered subsequence preserving this order, so the order is
/// load-bearing for potion *identity*, not just the draw count.
const SHARED_POOL_ORDER: [PotionKind; 45] = [
    PotionKind::AttackPotion,
    PotionKind::BeetleJuice,
    PotionKind::BlessingOfTheForge,
    PotionKind::BlockPotion,
    PotionKind::BottledPotential,
    PotionKind::Clarity,
    PotionKind::ColorlessPotion,
    PotionKind::CureAll,
    PotionKind::DexterityPotion,
    PotionKind::DistilledChaos,
    PotionKind::DropletOfPrecognition,
    PotionKind::Duplicator,
    PotionKind::EnergyPotion,
    PotionKind::EntropicBrew,
    PotionKind::ExplosiveAmpoule,
    PotionKind::FairyInABottle,
    PotionKind::FirePotion,
    PotionKind::FlexPotion,
    PotionKind::Fortifier,
    PotionKind::FruitJuice,
    PotionKind::FyshOil,
    PotionKind::GamblersBrew,
    PotionKind::GigantificationPotion,
    PotionKind::HeartOfIron,
    PotionKind::LiquidBronze,
    PotionKind::LiquidMemories,
    PotionKind::LuckyTonic,
    PotionKind::MazalethsGift,
    PotionKind::OrobicAcid,
    PotionKind::PotionOfBinding,
    PotionKind::PowderedDemise,
    PotionKind::PowerPotion,
    PotionKind::RadiantTincture,
    PotionKind::RegenPotion,
    PotionKind::ShacklingPotion,
    PotionKind::ShipInABottle,
    PotionKind::SkillPotion,
    PotionKind::SneckoOil,
    PotionKind::SpeedPotion,
    PotionKind::StableSerum,
    PotionKind::StrengthPotion,
    PotionKind::SwiftPotion,
    PotionKind::TouchOfInsanity,
    PotionKind::VulnerablePotion,
    PotionKind::WeakPotion,
];

/// Per-character potion pool in C# `<Character>4Epoch.Potions` declaration
/// order (`MegaCrit.Sts2.Core.Timeline.Epochs/`). C# `GetPotionOptions`
/// concatenates the character pool *before* the shared pool, so these come
/// first in the option list.
const fn character_pool_order(character: CharacterId) -> &'static [PotionKind] {
    match character {
        CharacterId::Ironclad => &[
            PotionKind::BloodPotion,
            PotionKind::SoldiersStew,
            PotionKind::Ashwater,
        ],
        CharacterId::Silent => &[
            PotionKind::PoisonPotion,
            PotionKind::GhostInAJar,
            PotionKind::CunningPotion,
        ],
        CharacterId::Defect => &[
            PotionKind::FocusPotion,
            PotionKind::EssenceOfDarkness,
            PotionKind::PotionOfCapacity,
        ],
        CharacterId::Necrobinder => &[
            PotionKind::PotionOfDoom,
            PotionKind::PotOfGhouls,
            PotionKind::BoneBrew,
        ],
        CharacterId::Regent => &[
            PotionKind::StarPotion,
            PotionKind::CosmicConcoction,
            PotionKind::KingsCourage,
        ],
    }
}

/// Roll `count` random out-of-combat potions, mirroring C#
/// `PotionFactory.CreateRandomPotionsOutOfCombat`
/// (`MegaCrit.Sts2.Core.Factories/PotionFactory.cs:27-53`).
///
/// The option pool is the character pool concatenated before the shared
/// pool ([`character_pool_order`] ++ [`SHARED_POOL_ORDER`]) — exactly the
/// `Character.PotionPool ++ SharedPotionPool` set the game samples, with
/// **all unlock epochs assumed revealed** (the steady state for any account
/// that has played enough to be in the scraped corpus). Each potion is
/// rolled by:
/// 1. `rng.NextFloat()` → rarity (`<= 0.1` Rare, `<= 0.35` Uncommon, else
///    Common);
/// 2. `rng.NextItem(pool.where(rarity))` → uniform pick over the
///    rarity-filtered subsequence, preserving pool order
///    (`NextInt(0, filtered.len())`);
/// 3. remove the picked potion from the pool so the next draw is distinct.
///
/// The caller passes the `combat_potion_generation` (`CombatPotionGeneration`)
/// stream. This consumes exactly `2 * count` draws regardless of pool order.
#[must_use]
pub fn create_random_potions_out_of_combat(
    rng: &mut DotNetRng,
    character: CharacterId,
    count: usize,
) -> Vec<PotionKind> {
    let pool = full_potion_options(character);
    create_random_potions_from_pool(rng, pool, count)
}

/// In-combat sibling of [`create_random_potions_out_of_combat`], mirroring
/// C# `PotionFactory.CreateRandomPotionInCombat`
/// (`MegaCrit.Sts2.Core.Factories/PotionFactory.cs:32-36`). Identical to the
/// out-of-combat roll except the option pool is additionally filtered by
/// `where p.CanBeGeneratedInCombat` — which in v0.103.2 drops exactly
/// `FairyInABottle`, `FruitJuice`, and `RegenPotion` (the three
/// `CanBeGeneratedInCombat => false` potions, all covered by
/// [`PotionKind::is_event_only`]). The token/event-only potions are already
/// absent from [`SHARED_POOL_ORDER`]. ALCHEMIZE
/// (`Alchemize.cs::OnPlay -> CreateRandomPotionInCombat`) is the sole caller,
/// consuming exactly `2 * count` draws off the `CombatPotionGeneration`
/// stream. Bead `sts2-rs-jejon`.
#[must_use]
pub fn create_random_potions_in_combat(
    rng: &mut DotNetRng,
    character: CharacterId,
    count: usize,
) -> Vec<PotionKind> {
    let pool: Vec<PotionKind> = full_potion_options(character)
        .into_iter()
        .filter(|k| !k.is_event_only())
        .collect();
    create_random_potions_from_pool(rng, pool, count)
}

/// C# `PotionFactory.GetPotionOptions`: the character potion pool
/// concatenated *before* the shared pool, all unlock epochs assumed
/// revealed.
fn full_potion_options(character: CharacterId) -> Vec<PotionKind> {
    let mut pool: Vec<PotionKind> = character_pool_order(character).to_vec();
    pool.extend_from_slice(&SHARED_POOL_ORDER);
    pool
}

/// `CrystalSpherePotion.ToReward`: one `NextItem` over `GetPotionOptions`
/// filtered to the item's rarity (character pool before the shared pool, pool
/// order kept, nothing removed) off the event's RNG. `None`, drawing nothing,
/// when no potion of that rarity exists. Lane F's F8 (`crystal-sphere`).
#[must_use]
pub fn roll_potion_of_rarity(
    rng: &mut DotNetRng,
    character: CharacterId,
    rarity: PotionRarity,
) -> Option<PotionKind> {
    let items: Vec<PotionKind> = full_potion_options(character)
        .into_iter()
        .filter(|k| spec_for(*k).rarity == rarity)
        .collect();
    if items.is_empty() {
        return None;
    }
    let idx = rng.next_int_exclusive(0, items.len() as i32) as usize;
    Some(items[idx])
}

/// One `PlayerRng.Rewards` `NextItem` over the FULL character++shared
/// pool — the shape five events share (TheLegendsWereTrue
/// `SlowlyFindAnExit`, Wellspring `Bottle`, BattlewornDummy `Setting1`,
/// EndlessConveyor `SuspiciousCondiment`, PotionCourier `Ransack`):
///
/// [decompiled C# omitted — see the cited path]
///
/// ONE int read (no rarity float — the reward arrives pre-populated, so
/// `PotionReward.Populate` never rolls). Queue the result via
/// `Reward::Potion(PotionReward::with_kind(..))`.
#[must_use]
pub fn roll_event_potion_full_pool(
    rng: &mut DotNetRng,
    character: CharacterId,
) -> Option<PotionKind> {
    let pool = full_potion_options(character);
    if pool.is_empty() {
        return None;
    }
    let idx = rng.next_int_exclusive(0, pool.len() as i32) as usize;
    Some(pool[idx])
}

/// [`roll_event_potion_full_pool`] sibling for PotionCourier `Ransack`:
/// the concat pool is rarity-filtered to Uncommon BEFORE the single
/// `NextItem` (`where p.Rarity == PotionRarity.Uncommon`).
#[must_use]
pub fn roll_event_potion_full_pool_uncommon(
    rng: &mut DotNetRng,
    character: CharacterId,
) -> Option<PotionKind> {
    let pool: Vec<PotionKind> = full_potion_options(character)
        .into_iter()
        .filter(|k| spec_for(*k).rarity == PotionRarity::Uncommon)
        .collect();
    if pool.is_empty() {
        return None;
    }
    let idx = rng.next_int_exclusive(0, pool.len() as i32) as usize;
    Some(pool[idx])
}

/// Shared body for [`create_random_potions_out_of_combat`] /
/// [`create_random_potions_in_combat`], mirroring C#
/// `PotionFactory.CreateRandomPotion`
/// (`MegaCrit.Sts2.Core.Factories/PotionFactory.cs:38-53`). Each draw:
/// 1. `rng.NextFloat()` -> rarity (`<= 0.1` Rare, `<= 0.35` Uncommon, else
///    Common);
/// 2. `rng.NextItem(pool.where(rarity))` -> uniform pick over the
///    rarity-filtered subsequence, preserving pool order
///    (`NextInt(0, filtered.len())`);
/// 3. remove the picked potion so the next draw is distinct.
///
/// Consumes exactly `2 * count` draws regardless of pool order.
fn create_random_potions_from_pool(
    rng: &mut DotNetRng,
    mut pool: Vec<PotionKind>,
    count: usize,
) -> Vec<PotionKind> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let roll = rng.next_float(1.0);
        let rarity = if roll <= 0.1 {
            PotionRarity::Rare
        } else if roll <= 0.35 {
            PotionRarity::Uncommon
        } else {
            PotionRarity::Common
        };
        let filtered: Vec<usize> = pool
            .iter()
            .enumerate()
            .filter(|(_, k)| spec_for(**k).rarity == rarity)
            .map(|(i, _)| i)
            .collect();
        // C# `NextItem` on an empty sequence returns default(T) without
        // consuming RNG; the C/U/R pool is never empty for these
        // rarities, so this branch is unreachable in practice.
        if filtered.is_empty() {
            continue;
        }
        let pick = rng.next_int_exclusive(0, filtered.len() as i32) as usize;
        let pool_idx = filtered[pick];
        out.push(pool[pool_idx]);
        pool.remove(pool_idx);
    }
    out
}

/// ALCHEMIZE: procure ONE random in-combat potion onto the player's belt,
/// rolled off the `CombatPotionGeneration` stream. Mirrors C#
/// `Alchemize.OnPlay`:
/// `PotionCmd.TryToProcure(PotionFactory.CreateRandomPotionInCombat(owner,
/// RunState.Rng.CombatPotionGeneration), owner)`
/// (`Alchemize.cs:25`). No-op when the belt is already full
/// (`PotionCmd.TryToProcure` returns `success: false`); C# still rolls the
/// potion first (consuming 2 draws) and only then discovers the belt is full,
/// so the roll is performed unconditionally to keep the stream aligned. The
/// belt cap is the player's soft `effective_potion_slots()` (C#
/// `Player.MaxPotionCount`, default 3), not the hard `MAX_POTIONS`. Bead
/// `sts2-rs-jejon`.
pub fn alchemize_procure_random_potion(state: &mut CombatState) {
    let character = state.player.character;
    let rolled = character.and_then(|c| {
        create_random_potions_in_combat(&mut state.run_rng.combat_potion_generation, c, 1)
            .into_iter()
            .next()
    });
    if let Some(kind) = rolled {
        if state.player.potions.len() < state.player.effective_potion_slots() {
            state.player.potions.push(kind);
            // C# `TryToProcure` → `Hook.AfterPotionProcured` (sts2-agent
            // fork, belt-buckle).
            sts2_core::relic_effects::belt_buckle::after_potion_procured(state);
        }
    }
}

/// ENTROPIC_BREW: fill every open potion slot with random *out-of-combat*
/// potions rolled off the `CombatPotionGeneration` stream. Mirrors C#
/// `EntropicBrew.OnUse`:
/// `while (HasOpenPotionSlots) TryToProcure(
///   CreateRandomPotionOutOfCombat(owner, RunState.Rng.CombatPotionGeneration))`
/// (`EntropicBrew.cs:21-27`). EntropicBrew uses the OUT-of-combat pool (no
/// `CanBeGeneratedInCombat` filter), so `FairyInABottle` / `FruitJuice` /
/// `RegenPotion` remain eligible — distinct from ALCHEMIZE. Each fill rolls
/// one potion (2 draws) until the belt reaches the player's soft
/// `effective_potion_slots()` cap (C# `HasOpenPotionSlots`). Bead
/// `sts2-rs-jejon`.
pub fn entropic_brew_fill_belt(state: &mut CombatState) {
    let Some(character) = state.player.character else {
        return;
    };
    while state.player.potions.len() < state.player.effective_potion_slots() {
        let Some(kind) = create_random_potions_out_of_combat(
            &mut state.run_rng.combat_potion_generation,
            character,
            1,
        )
        .into_iter()
        .next() else {
            break;
        };
        state.player.potions.push(kind);
        // C# `TryToProcure` → `Hook.AfterPotionProcured` (sts2-agent fork,
        // belt-buckle).
        sts2_core::relic_effects::belt_buckle::after_potion_procured(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `PotionKind` has a spec whose `id` round-trips.
    #[test]
    fn every_kind_has_spec_with_matching_id() {
        for &k in &PotionKind::ALL {
            let s = spec_for(k);
            assert_eq!(s.id, k, "spec_for({k:?}).id mismatch");
            assert_eq!(s.name, k.python_name(), "name mismatch for {k:?}");
        }
    }

    /// Character-lock parity check: `PotionKind::character_lock()`
    /// agrees with `PotionSpec::character_lock` for every potion.
    #[test]
    fn character_lock_parity_with_kind_method() {
        for &k in &PotionKind::ALL {
            assert_eq!(
                spec_for(k).character_lock,
                k.character_lock(),
                "character_lock mismatch for {k:?}"
            );
        }
    }

    /// `is_event_only` parity between `PotionKind` and `PotionSpec`.
    #[test]
    fn is_event_only_parity() {
        for &k in &PotionKind::ALL {
            assert_eq!(
                spec_for(k).is_event_only,
                k.is_event_only(),
                "is_event_only mismatch for {k:?}"
            );
        }
    }

    /// `PotionUsage.Automatic` lockstep: the spec table's `Automatic`
    /// set must equal `PotionKind::is_automatic()` — core's
    /// `legal_actions` / DrinkPotion gates read the predicate, not the
    /// spec, so a new Automatic potion tagged only here would leak back
    /// into the manual-drink surface (A8 audit §7a). FairyInABottle is
    /// the only Automatic potion in the v0.107.1 decomp.
    #[test]
    fn automatic_spec_agrees_with_core_predicate() {
        for &k in &PotionKind::ALL {
            assert_eq!(
                spec_for(k).usage_type == PotionUsageType::Automatic,
                k.is_automatic(),
                "Automatic usage mismatch for {k:?}: keep the spec table \
                 and PotionKind::is_automatic in lockstep"
            );
        }
    }

    /// Counts per rarity match the Python source.
    #[test]
    fn rarity_counts_match_python_source() {
        let mut common = 0;
        let mut uncommon = 0;
        let mut rare = 0;
        let mut event = 0;
        let mut token = 0;
        for &k in &PotionKind::ALL {
            match spec_for(k).rarity {
                PotionRarity::Common => common += 1,
                PotionRarity::Uncommon => uncommon += 1,
                PotionRarity::Rare => rare += 1,
                PotionRarity::Event => event += 1,
                PotionRarity::Token => token += 1,
                PotionRarity::None => panic!("None rarity slipped in for {k:?}"),
            }
        }
        assert_eq!(common, 20, "expected 20 Common potions");
        assert_eq!(uncommon, 20, "expected 20 Uncommon potions");
        assert_eq!(rare, 20, "expected 20 Rare potions");
        assert_eq!(event, 2, "expected 2 Event potions");
        assert_eq!(token, 1, "expected 1 Token potion");
    }

    /// The full-pool event roll is ONE NextItem over the 48-entry
    /// Ironclad++shared concat — index 45 (3 char + shared slot 42) is
    /// `TouchOfInsanity`, the live-C# ground truth from run-parity RP0
    /// (TheLegendsWereTrue `SlowlyFindAnExit`).
    #[test]
    fn event_full_pool_roll_indexes_concat_order() {
        let pool = full_potion_options(CharacterId::Ironclad);
        assert_eq!(pool.len(), 48);
        assert_eq!(pool[0], PotionKind::BloodPotion);
        assert_eq!(pool[45], PotionKind::TouchOfInsanity);
    }

    /// The PotionCourier sibling filters the concat pool to Uncommon
    /// BEFORE the single NextItem — every candidate must be Uncommon.
    #[test]
    fn event_full_pool_uncommon_roll_only_offers_uncommons() {
        let mut rng = DotNetRng::new(7);
        for _ in 0..20 {
            let kind = roll_event_potion_full_pool_uncommon(&mut rng, CharacterId::Ironclad)
                .expect("non-empty uncommon pool");
            assert_eq!(spec_for(kind).rarity, PotionRarity::Uncommon);
        }
    }

    /// The shared pool order matches the C# `SharedPotionPool` set: exactly
    /// the 45 non-character-locked Common/Uncommon/Rare potions, with no
    /// Event/Token tiers and no character-locked potions leaking in.
    #[test]
    fn shared_pool_matches_csharp_45_set() {
        assert_eq!(SHARED_POOL_ORDER.len(), 45);
        let mut seen = std::collections::HashSet::new();
        for &k in &SHARED_POOL_ORDER {
            let s = spec_for(k);
            assert_eq!(s.character_lock, None, "{k:?} is character-locked");
            assert!(
                matches!(
                    s.rarity,
                    PotionRarity::Common | PotionRarity::Uncommon | PotionRarity::Rare
                ),
                "{k:?} has non-generatable rarity {:?}",
                s.rarity
            );
            assert!(seen.insert(k), "{k:?} duplicated in shared pool");
        }
        // The shared pool must contain every None-locked C/U/R potion.
        let expected = PotionKind::ALL
            .iter()
            .filter(|&&k| {
                let s = spec_for(k);
                s.character_lock.is_none()
                    && matches!(
                        s.rarity,
                        PotionRarity::Common | PotionRarity::Uncommon | PotionRarity::Rare
                    )
            })
            .count();
        assert_eq!(expected, 45, "expected 45 shared-pool potions");
    }

    /// Each playable character pool is the 3 character-locked C/U/R potions.
    #[test]
    fn character_pools_are_three_locked_potions() {
        for c in [
            CharacterId::Ironclad,
            CharacterId::Silent,
            CharacterId::Defect,
            CharacterId::Necrobinder,
            CharacterId::Regent,
        ] {
            let pool = character_pool_order(c);
            assert_eq!(pool.len(), 3, "{c:?} pool size");
            for &k in pool {
                assert_eq!(
                    spec_for(k).character_lock,
                    Some(c),
                    "{k:?} not locked to {c:?}"
                );
            }
        }
    }

    /// `create_random_potions_out_of_combat` returns `count` distinct
    /// potions, consumes exactly `2 * count` RNG draws (one float + one int
    /// per potion), and is deterministic for the same stream.
    #[test]
    fn out_of_combat_roll_count_distinct_and_draw_count() {
        let mut rng = DotNetRng::with_name(0x51A2, "combat_potion_generation");
        let before = rng.counter();
        let potions = create_random_potions_out_of_combat(&mut rng, CharacterId::Ironclad, 2);
        assert_eq!(potions.len(), 2);
        assert_ne!(potions[0], potions[1], "potions are distinct");
        assert_eq!(rng.counter() - before, 4, "exactly 2 draws per potion");

        // Determinism: replaying the same stream yields the same potions.
        let mut rng2 = DotNetRng::with_name(0x51A2, "combat_potion_generation");
        let potions2 = create_random_potions_out_of_combat(&mut rng2, CharacterId::Ironclad, 2);
        assert_eq!(potions, potions2);
    }

    /// `create_random_potions_in_combat` mirrors the out-of-combat roll
    /// (`2 * count` draws, distinct, deterministic) but additionally drops
    /// the three C# `CanBeGeneratedInCombat => false` potions. Bead
    /// `sts2-rs-jejon`.
    #[test]
    fn in_combat_roll_count_distinct_and_draw_count() {
        let mut rng = DotNetRng::with_name(0x51A2, "combat_potion_generation");
        let before = rng.counter();
        let potions = create_random_potions_in_combat(&mut rng, CharacterId::Ironclad, 2);
        assert_eq!(potions.len(), 2);
        assert_ne!(potions[0], potions[1], "potions are distinct");
        assert_eq!(rng.counter() - before, 4, "exactly 2 draws per potion");

        let mut rng2 = DotNetRng::with_name(0x51A2, "combat_potion_generation");
        let potions2 = create_random_potions_in_combat(&mut rng2, CharacterId::Ironclad, 2);
        assert_eq!(potions, potions2, "deterministic for the same stream");
    }

    /// The in-combat pool is exactly the out-of-combat pool minus the three
    /// C# `CanBeGeneratedInCombat => false` potions (FairyInABottle,
    /// FruitJuice, RegenPotion). Verified by drawing the entire pool: no
    /// in-combat draw can ever surface those three. Bead `sts2-rs-jejon`.
    #[test]
    fn in_combat_pool_excludes_non_combat_generatable() {
        let banned = [
            PotionKind::FairyInABottle,
            PotionKind::FruitJuice,
            PotionKind::RegenPotion,
        ];
        // Drain a large number of seeds; the banned potions must never appear.
        for seed in 0..256_i32 {
            let mut rng = DotNetRng::with_name(seed, "combat_potion_generation");
            // 48 = the full character+shared in-combat pool size; ask for more
            // than the pool holds so the loop exercises every rarity bucket.
            let rolled = create_random_potions_in_combat(&mut rng, CharacterId::Ironclad, 48);
            for k in &rolled {
                assert!(!banned.contains(k), "{k:?} leaked into in-combat pool");
                assert!(!k.is_event_only(), "{k:?} is_event_only leaked");
            }
        }
        // And those three ARE reachable out-of-combat (sanity: the filter is
        // the only difference).
        let mut seen_banned = false;
        for seed in 0..512_i32 {
            let mut rng = DotNetRng::with_name(seed, "combat_potion_generation");
            let rolled = create_random_potions_out_of_combat(&mut rng, CharacterId::Ironclad, 48);
            if rolled.iter().any(|k| banned.contains(k)) {
                seen_banned = true;
                break;
            }
        }
        assert!(
            seen_banned,
            "out-of-combat pool should still surface FairyInABottle/FruitJuice/RegenPotion"
        );
    }

    /// Helper: one `PotionKind` per generatable rarity, so a single draw's
    /// output rarity uniquely reveals which rarity branch the float roll
    /// selected.
    #[cfg(test)]
    fn one_per_rarity_pool() -> Vec<PotionKind> {
        let pick = |r: PotionRarity| {
            *PotionKind::ALL
                .iter()
                .find(|&&k| spec_for(k).rarity == r)
                .expect("rarity present")
        };
        vec![
            pick(PotionRarity::Common),
            pick(PotionRarity::Uncommon),
            pick(PotionRarity::Rare),
        ]
    }

    /// The rarity ladder maps the float roll to the right bucket at each
    /// boundary. Seeds re-derived under v0.107.1 MegaRandom (the old
    /// System.Random seeds 14/18/0 no longer land in those buckets) so the
    /// rolled rarity sits cleanly inside one bucket and would cross a boundary
    /// under a `<=`/`==` operator mutation:
    /// - seed 9 (`<= 0.1` → Rare; `>` mutant slips to Uncommon)
    /// - seed 2 (`<= 0.35` → Uncommon; `>` mutant slips to Common)
    /// - seed 0 (`else` → Common; the `== rarity` filter mutation to `!=`
    ///   would discard the Common potion and surface a non-Common)
    #[test]
    fn rarity_branch_boundaries_select_expected_rarity() {
        // Rare branch.
        let mut rng = DotNetRng::new(9);
        let out = create_random_potions_from_pool(&mut rng, one_per_rarity_pool(), 1);
        assert_eq!(out.len(), 1);
        assert_eq!(
            spec_for(out[0]).rarity,
            PotionRarity::Rare,
            "seed 9 roll (<= 0.1) selects Rare"
        );

        // Uncommon branch.
        let mut rng = DotNetRng::new(2);
        let out = create_random_potions_from_pool(&mut rng, one_per_rarity_pool(), 1);
        assert_eq!(out.len(), 1);
        assert_eq!(
            spec_for(out[0]).rarity,
            PotionRarity::Uncommon,
            "seed 2 roll (<= 0.35) selects Uncommon"
        );

        // Common branch + `== rarity` filter (seed 0 still lands Common).
        let mut rng = DotNetRng::new(0);
        let out = create_random_potions_from_pool(&mut rng, one_per_rarity_pool(), 1);
        assert_eq!(out.len(), 1);
        assert_eq!(
            spec_for(out[0]).rarity,
            PotionRarity::Common,
            "seed 0 roll (else) + `== rarity` filter selects Common"
        );
    }
}
