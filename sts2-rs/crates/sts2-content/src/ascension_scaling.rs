//! Ascension attack-damage scaling — data table + post-build patch.
//!
//! C# scales individual monster attack damages at `AscensionLevel.DeadlyEnemies`
//! (>= ascension 9) via `AscensionHelper.GetValueIfAscension(DeadlyEnemies,
//! deadly, base)`. Rather than thread `ascension` into all ~123 monster AI
//! factories, we keep the factories ascension-blind (they encode the A0/base
//! damage) and patch the built `MonsterAi` in [`crate::monster_dispatch::
//! build_ai_with_ascension`]: at ascension >= 9 each scaled move's attack
//! intent damage is bumped to its DeadlyEnemies value. Ascension is fixed for
//! the whole combat, so patching once at spawn is equivalent to C# resolving
//! the `GetValueIfAscension` property on every telegraph.
//!
//! The table is keyed by `(MonsterKind, MoveState id)` and carries BOTH the
//! non-ascension `base` and the `deadly` value. `base` is redundant for the
//! patch itself but lets [`tests::ascension_attack_table_matches_real_moves`]
//! assert that every row resolves to a real move whose A0 damage equals
//! `base` — a compile-and-test guard that the table stays in sync with the
//! factories and that each row was transcribed from the C# correctly.
//!
//! Values transcribed from `../sts2-wiki/decompiled/v0.103.2/` per-monster
//! `<Class>.cs` `GetValueIfAscension(DeadlyEnemies, …)` attack properties.

use sts2_core::monster_ai::{MonsterAi, MonsterState};
use sts2_core::monsters::{Intent, MonsterKind};
use sts2_core::AscensionLevel;

/// `(MonsterKind, MoveState id, base damage, DeadlyEnemies damage)`.
/// Only attacks whose damage actually scales (deadly != base) are listed.
#[rustfmt::skip]
pub(crate) const ASCENSION_ATTACK_SCALING: &[(MonsterKind, &str, i32, i32)] = &[
    // Aeonglass (v0.107.1): EbbDamage GetValueIfAscension(DeadlyEnemies, 32, 26)
    // ⇒ A0 26 / A9 32; EyeLasersDamage (..12, 11) ⇒ A0 11 / A9 12.
    (MonsterKind::Aeonglass, "EBB_MOVE", 26, 32),
    (MonsterKind::Aeonglass, "EYE_LASERS_MOVE", 11, 12),
    // `…wb8lk.11`: v0.107.1 KillshotDamage GetValueIfAscension(DeadlyEnemies,
    // 11, 10) ⇒ A0 10 / A9 11 (was A0 11 / A9 12).
    (MonsterKind::AssassinRubyRaider, "KILLSHOT", 10, 11),
    (MonsterKind::AxeRubyRaider, "BIG_SWING", 12, 13),
    (MonsterKind::AxeRubyRaider, "SWING_1", 5, 6),
    (MonsterKind::AxeRubyRaider, "SWING_2", 5, 6),
    // `…wb8lk.11`: v0.107.1 Axebot rework. HammerUppercutDamage
    // GetValueIfAscension(DeadlyEnemies, 14, 12) ⇒ A0 12 / A9 14;
    // OneTwoDamage (..10, 9) ⇒ A0 9 / A9 10.
    (MonsterKind::Axebot, "HAMMER_UPPERCUT", 12, 14),
    (MonsterKind::Axebot, "ONE_TWO", 9, 10),
    (MonsterKind::BowlbugEgg, "BITE", 7, 8),
    (MonsterKind::BowlbugRock, "HEADBUTT", 15, 16),
    (MonsterKind::BowlbugSilk, "THRASH", 4, 5),
    (MonsterKind::BruteRubyRaider, "BEAT", 7, 8),
    (MonsterKind::BygoneEffigy, "SLASHES", 13, 15),
    (MonsterKind::Byrdonis, "PECK", 3, 4),
    (MonsterKind::Byrdonis, "SWOOP", 17, 19),
    (MonsterKind::CalcifiedCultist, "DARK_STRIKE", 9, 11),
    (MonsterKind::CeremonialBeast, "CRUSH", 17, 19),
    (MonsterKind::CeremonialBeast, "PLOW", 18, 20),
    (MonsterKind::CeremonialBeast, "STOMP", 15, 17),
    (MonsterKind::Chomper, "CLAMP", 8, 9),
    (MonsterKind::CorpseSlug, "GLOMP", 8, 9),
    (MonsterKind::CrossbowRubyRaider, "FIRE", 14, 16),
    (MonsterKind::Crusher, "BUG_STING", 6, 7),
    (MonsterKind::Crusher, "GUARDED_STRIKE", 12, 14),
    (MonsterKind::Crusher, "THRASH", 12, 14),
    (MonsterKind::CubexConstruct, "EXPEL_BLAST", 5, 6),
    (MonsterKind::CubexConstruct, "REPEATER", 7, 8),
    (MonsterKind::CubexConstruct, "REPEATER_2", 7, 8),
    (MonsterKind::DampCultist, "DARK_STRIKE", 1, 3),
    (MonsterKind::DecimillipedeSegment, "BULK", 6, 7),
    (MonsterKind::DecimillipedeSegment, "CONSTRICT", 8, 9),
    (MonsterKind::DecimillipedeSegment, "WRITHE", 5, 6),
    (MonsterKind::DecimillipedeSegmentBack, "BULK", 6, 7),
    (MonsterKind::DecimillipedeSegmentBack, "CONSTRICT", 8, 9),
    (MonsterKind::DecimillipedeSegmentBack, "WRITHE", 5, 6),
    (MonsterKind::DecimillipedeSegmentFront, "BULK", 6, 7),
    (MonsterKind::DecimillipedeSegmentFront, "CONSTRICT", 8, 9),
    (MonsterKind::DecimillipedeSegmentFront, "WRITHE", 5, 6),
    (MonsterKind::DecimillipedeSegmentMiddle, "BULK", 6, 7),
    (MonsterKind::DecimillipedeSegmentMiddle, "CONSTRICT", 8, 9),
    (MonsterKind::DecimillipedeSegmentMiddle, "WRITHE", 5, 6),
    (MonsterKind::DenseVegetationWriggler, "NASTY_BITE", 6, 7),
    (MonsterKind::DevotedSculptor, "SAVAGE", 12, 15),
    (MonsterKind::Entomancer, "SPEAR", 18, 20),
    (MonsterKind::Exoskeleton, "MANDIBLE", 8, 9),
    (MonsterKind::Fabricator, "DISINTEGRATE", 11, 13),
    (MonsterKind::Fabricator, "FABRICATING_STRIKE", 18, 21),
    (MonsterKind::FakeMerchantMonster, "SWIPE", 13, 15),
    (MonsterKind::FakeMerchantMonster, "THROW_RELIC", 9, 10),
    (MonsterKind::FlailKnight, "FLAIL", 9, 10),
    (MonsterKind::FlailKnight, "RAM", 15, 17),
    (MonsterKind::Flyconid, "FRAIL_SPORES", 8, 9),
    (MonsterKind::Flyconid, "SMASH", 11, 12),
    (MonsterKind::Fogmog, "HEADBUTT", 14, 16),
    (MonsterKind::Fogmog, "SWIPE", 8, 9),
    (MonsterKind::Fogmog, "SWIPE_RANDOM", 8, 9),
    (MonsterKind::FossilStalker, "LASH", 3, 4),
    (MonsterKind::FossilStalker, "LATCH", 12, 14),
    (MonsterKind::FossilStalker, "TACKLE", 9, 11),
    (MonsterKind::FrogKnight, "BEETLE_CHARGE", 35, 40),
    (MonsterKind::FrogKnight, "STRIKE_DOWN_EVIL", 21, 23),
    (MonsterKind::FrogKnight, "TONGUE_LASH", 13, 14),
    (MonsterKind::FuzzyWurmCrawler, "ACID_GOOP", 4, 6),
    (MonsterKind::FuzzyWurmCrawler, "FIRST_ACID_GOOP", 4, 6),
    (MonsterKind::GasBomb, "EXPLODE", 8, 9),
    (MonsterKind::GlobeHead, "GALVANIC_BURST", 16, 17),
    (MonsterKind::GlobeHead, "SHOCKING_SLAP", 13, 14),
    (MonsterKind::GlobeHead, "THUNDER_STRIKE", 6, 7),
    // GremlinMerc's attacks scale at ToughEnemies (A8), not here:
    // `TOUGH_ATTACK_SCALING`.
    // `…wb8lk.11`: v0.107.1 HauntedShip attacks scale at A9. SwipeDamage
    // GetValueIfAscension(DeadlyEnemies, 14, 13) ⇒ A0 13 / A9 14; StompDamage
    // (..5, 4) ⇒ A0 4 / A9 5 (per hit, × StompRepeat 3). Decomp
    // `v0.107.1/…/Monsters/HauntedShip.cs:31,33`.
    (MonsterKind::HauntedShip, "SWIPE_MOVE", 13, 14),
    (MonsterKind::HauntedShip, "STOMP_MOVE", 4, 5),
    (MonsterKind::HunterKiller, "BITE_MOVE", 17, 19),
    (MonsterKind::HunterKiller, "PUNCTURE_MOVE", 7, 8),
    // `…wb8lk.11`: v0.107.1 InfestedPrism stat cuts. JabDamage
    // GetValueIfAscension(DeadlyEnemies, 17, 15) ⇒ A0 15 / A9 17;
    // RadiateDamage (..13, 11) ⇒ A0 11 / A9 13; WhirlwindDamage (..6, 5)
    // ⇒ A0 5 / A9 6; PulsateDamage (..10, 8) ⇒ A0 8 / A9 10 (PULSATE
    // gained an attack in v0.107.1, replacing the pure Strength buff).
    (MonsterKind::InfestedPrism, "JAB", 15, 17),
    (MonsterKind::InfestedPrism, "PULSATE", 8, 10),
    (MonsterKind::InfestedPrism, "RADIATE", 11, 13),
    (MonsterKind::InfestedPrism, "WHIRLWIND", 5, 6),
    (MonsterKind::Inklet, "JAB", 3, 4),
    (MonsterKind::Inklet, "PIERCING_GAZE", 10, 11),
    (MonsterKind::Inklet, "WHIRLWIND", 2, 3),
    (MonsterKind::KinPriest, "ORB_OF_FRAILTY_MOVE", 8, 9),
    (MonsterKind::KinPriest, "ORB_OF_WEAKNESS_MOVE", 8, 9),
    (MonsterKind::KnowledgeDemon, "KNOWLEDGE_OVERWHELMING", 8, 9),
    (MonsterKind::KnowledgeDemon, "PONDER", 11, 13),
    (MonsterKind::KnowledgeDemon, "SLAP", 17, 18),
    (MonsterKind::LagavulinMatriarch, "DISEMBOWEL", 9, 10),
    (MonsterKind::LagavulinMatriarch, "SLASH", 19, 21),
    (MonsterKind::LagavulinMatriarch, "SLASH2", 12, 14),
    (MonsterKind::LeafSlimeM, "CLUMP_SHOT", 8, 9),
    (MonsterKind::LeafSlimeS, "BUTT", 3, 4),
    (MonsterKind::LivingFog, "ADVANCED_GAS", 8, 9),
    (MonsterKind::LivingFog, "BLOAT", 5, 6),
    (MonsterKind::LivingFog, "SUPER_GAS_BLAST", 8, 9),
    (MonsterKind::LivingShield, "SMASH", 16, 18),
    (MonsterKind::LouseProgenitor, "POUNCE_MOVE", 14, 16),
    (MonsterKind::LouseProgenitor, "WEB_CANNON_MOVE", 9, 10),
    (MonsterKind::MagiKnight, "BOMB", 35, 40),
    (MonsterKind::MagiKnight, "FIRST_POWER_SHIELD", 6, 7),
    (MonsterKind::MagiKnight, "RAM", 10, 11),
    (MonsterKind::Mawler, "CLAW", 4, 5),
    (MonsterKind::Mawler, "RIP_AND_TEAR", 14, 16),
    (MonsterKind::MechaKnight, "CHARGE", 25, 30),
    (MonsterKind::MechaKnight, "HEAVY_CLEAVE", 35, 40),
    (MonsterKind::MysteriousKnight, "FLAIL", 9, 10),
    (MonsterKind::MysteriousKnight, "RAM", 15, 17),
    (MonsterKind::Myte, "BITE_MOVE", 13, 15),
    (MonsterKind::Myte, "SUCK_MOVE", 4, 6),
    (MonsterKind::Nibbit, "BUTT", 12, 13),
    (MonsterKind::Nibbit, "SLICE", 6, 7),
    (MonsterKind::Ovicopter, "SMASH_MOVE", 16, 17),
    (MonsterKind::Ovicopter, "TENDERIZER_MOVE", 7, 8),
    (MonsterKind::OwlMagistrate, "MAGISTRATE_SCRUTINY", 16, 17),
    (MonsterKind::OwlMagistrate, "VERDICT", 33, 36),
    (MonsterKind::Parafright, "SLAM", 16, 17),
    (MonsterKind::PhrogParasite, "LASH", 4, 5),
    (MonsterKind::PunchConstruct, "FAST_PUNCH", 5, 6),
    (MonsterKind::PunchConstruct, "STRONG_PUNCH", 14, 16),
    (MonsterKind::Queen, "EXECUTION_MOVE", 15, 18),
    (MonsterKind::Queen, "OFF_WITH_YOUR_HEAD_MOVE", 3, 4),
    (MonsterKind::Rocket, "LASER_MOVE", 31, 35),
    (MonsterKind::Rocket, "PRECISION_BEAM_MOVE", 18, 20),
    (MonsterKind::Rocket, "TARGETING_RETICLE_MOVE", 3, 4),
    (MonsterKind::ScrollOfBiting, "CHEW", 5, 6),
    (MonsterKind::ScrollOfBiting, "CHOMP", 14, 16),
    (MonsterKind::Seapunk, "SEA_KICK", 11, 13),
    (MonsterKind::SewerClam, "JET", 10, 11),
    (MonsterKind::ShrinkerBeetle, "CHOMP", 7, 8),
    (MonsterKind::ShrinkerBeetle, "STOMP", 13, 14),
    // `…wb8lk.11`: v0.107.1 SkulkingColony removed SMASH_MOVE and added a
    // second Zoom (ZOOM_MOVE_2, same ZoomDamage). InertiaDamage/Zoom/
    // PiercingStabs values are unchanged from v0.103.2.
    (MonsterKind::SkulkingColony, "INERTIA_MOVE", 9, 11),
    (MonsterKind::SkulkingColony, "PIERCING_STABS_MOVE", 7, 8),
    (MonsterKind::SkulkingColony, "ZOOM_MOVE", 14, 16),
    (MonsterKind::SkulkingColony, "ZOOM_MOVE_2", 14, 16),
    (MonsterKind::SlimedBerserker, "FURIOUS_PUMMELING_MOVE", 4, 5),
    (MonsterKind::SlimedBerserker, "SMOTHER_MOVE", 30, 33),
    (MonsterKind::SlitheringStrangler, "LASH", 12, 13),
    (MonsterKind::SlitheringStrangler, "TWACK", 7, 8),
    (MonsterKind::SludgeSpinner, "OIL_SPRAY", 8, 9),
    (MonsterKind::SludgeSpinner, "RAGE", 6, 7),
    (MonsterKind::SludgeSpinner, "SLAM", 11, 12),
    (MonsterKind::SlumberingBeetle, "ROLL_OUT_MOVE", 16, 18),
    // `wb-010871941d034711` sweep: SneakyGremlin was absent from this table.
    // C# `SneakyGremlin.cs:25` `TackleDamage =>
    // GetValueIfAscension(DeadlyEnemies, 10, 9)`.
    (MonsterKind::SneakyGremlin, "TACKLE", 9, 10),
    (MonsterKind::SnappingJaxfruit, "ENERGY_ORB_MOVE", 3, 4),
    (MonsterKind::SoulFysh, "DE_GAS", 16, 17),
    (MonsterKind::SoulFysh, "GAZE", 7, 8),
    // `…wb8lk.11`: v0.107.1 ScreamDamage GetValueIfAscension(DeadlyEnemies,
    // 15, 13) ⇒ A0 13 / A9 15 (was A0 11 / A9 12).
    (MonsterKind::SoulFysh, "SCREAM", 13, 15),
    (MonsterKind::SoulNexus, "DRAIN_LIFE_MOVE", 18, 19),
    (MonsterKind::SoulNexus, "MAELSTROM_MOVE", 6, 7),
    (MonsterKind::SoulNexus, "SOUL_BURN_MOVE", 29, 31),
    (MonsterKind::SpectralKnight, "SOUL_FLAME", 3, 4),
    (MonsterKind::SpectralKnight, "SOUL_SLASH", 15, 17),
    (MonsterKind::SpinyToad, "EXPLOSION", 23, 25),
    (MonsterKind::SpinyToad, "LASH", 17, 19),
    (MonsterKind::Stabbot, "STAB", 11, 12),
    (MonsterKind::TerrorEel, "CRASH_MOVE", 16, 18),
    (MonsterKind::TerrorEel, "ThrashMove", 3, 4),
    (MonsterKind::TestSubject, "BITE", 20, 22),
    (MonsterKind::TestSubject, "SKULL_BASH", 14, 16),
    // `wb-010871941d034711` sweep: the two phase-3 MultiAttacks were missing.
    // C# `TestSubject.cs:94,98`: MultiClawDamage / Phase3LacerateDamage =>
    // GetValueIfAscension(DeadlyEnemies, 11, 10). MULTI_CLAW's hit COUNT
    // grows via `ExtraMultiClawCount`, not ascension, so only damage scales.
    (MonsterKind::TestSubject, "MULTI_CLAW", 10, 11),
    (MonsterKind::TestSubject, "PHASE3_LACERATE", 10, 11),
    (MonsterKind::TheForgotten, "DREAD", 13, 15),
    (MonsterKind::TheInsatiable, "LUNGING_BITE", 28, 31),
    // `wb-010871941d034711` sweep: both THRASH move states share C#
    // `TheInsatiable.cs:62` `ThrashDamage =>
    // GetValueIfAscension(DeadlyEnemies, 9, 8)` (2 hits each) and were
    // missing while LUNGING_BITE was present.
    (MonsterKind::TheInsatiable, "THRASH_1", 8, 9),
    (MonsterKind::TheInsatiable, "THRASH_2", 8, 9),
    (MonsterKind::TheLost, "EYE_LASERS", 4, 5),
    (MonsterKind::TheObscura, "HARDENING_STRIKE_MOVE", 6, 7),
    (MonsterKind::TheObscura, "PIERCING_GAZE_MOVE", 10, 11),
    (MonsterKind::ThievingHopper, "HAT_TRICK_MOVE", 21, 23),
    (MonsterKind::ThievingHopper, "NAB_MOVE", 14, 16),
    (MonsterKind::ThievingHopper, "THIEVERY_MOVE", 17, 19),
    (MonsterKind::TorchHeadAmalgam, "TACKLE_1", 18, 19),
    (MonsterKind::TorchHeadAmalgam, "TACKLE_2", 18, 19),
    (MonsterKind::TorchHeadAmalgam, "TACKLE_3", 14, 15),
    (MonsterKind::TorchHeadAmalgam, "TACKLE_4", 14, 15),
    // `wb-010871941d034711` (seed DZV0PD70HW, IC A9, Underdocks): Toadpole
    // had NO rows here, so A9 telegraphed/dealt the A0 values. C#
    // `Toadpole.cs:49,53`: SpikeSpitDamage =>
    // GetValueIfAscension(DeadlyEnemies, 4, 3) (x3 hits), WhirlDamage =>
    // (.., 8, 7).
    (MonsterKind::Toadpole, "SPIKE_SPIT_MOVE", 3, 4),
    (MonsterKind::Toadpole, "WHIRL_MOVE", 7, 8),
    (MonsterKind::ToughEgg, "NIBBLE_MOVE", 4, 5),
    // NOTE: TrackerRubyRaider HOUNDS scales its MultiAttack *hits* (8→9 at
    // DeadlyEnemies), not per-hit damage (C# `TrackerRubyRaider.cs:23`
    // HoundsDamage is 1 at both levels), so its row lives in
    // ASCENSION_HITS_SCALING below, not here.
    (MonsterKind::Tunneler, "BELOW_MOVE_1", 23, 26),
    (MonsterKind::Tunneler, "BITE_MOVE", 13, 15),
    (MonsterKind::TurretOperator, "UNLOAD_MOVE_1", 3, 4),
    (MonsterKind::TurretOperator, "UNLOAD_MOVE_2", 3, 4),
    (MonsterKind::TwigSlimeM, "CLUMP_SHOT", 11, 12),
    (MonsterKind::TwigSlimeS, "BUTT", 4, 5),
    (MonsterKind::TwoTailedRat, "DISEASE_BITE", 6, 7),
    (MonsterKind::TwoTailedRat, "SCRATCH", 8, 9),
    // `…wb8lk.11`: v0.107.1 DismemberDamage 27→26 at A0 (decomp
    // `Vantom.cs:72 GetValueIfAscension(DeadlyEnemies, 30, 26)`).
    (MonsterKind::Vantom, "DISMEMBER", 26, 30),
    (MonsterKind::Vantom, "INKY_LANCE", 6, 7),
    (MonsterKind::Vantom, "INK_BLOT", 7, 8),
    (MonsterKind::VineShambler, "CHOMP", 16, 18),
    (MonsterKind::VineShambler, "GRASPING_VINES", 8, 9),
    (MonsterKind::VineShambler, "SWIPE", 6, 7),
    (MonsterKind::WaterfallGiant, "PRESSURE_GUN", 20, 23),
    (MonsterKind::WaterfallGiant, "PRESSURE_UP", 13, 14),
    (MonsterKind::WaterfallGiant, "RAM", 10, 11),
    (MonsterKind::WaterfallGiant, "STOMP", 15, 16),
    (MonsterKind::Wriggler, "NASTY_BITE", 6, 7),
    (MonsterKind::Zapbot, "ZAP", 14, 15),
];

/// Attacks C# scales at `AscensionLevel.ToughEnemies` (>= ascension 8)
/// rather than `DeadlyEnemies`: `(kind, move_id, base, tough)`.
#[rustfmt::skip]
pub(crate) const TOUGH_ATTACK_SCALING: &[(MonsterKind, &str, i32, i32)] = &[
    // C# `GremlinMerc.cs:36,40,44` `GimmeDamage => (ToughEnemies, 8, 7)`,
    // `DoubleSmashDamage => (.., 7, 6)`, `HeheDamage => (.., 9, 8)`.
    (MonsterKind::GremlinMerc, "DOUBLE_SMASH", 6, 7),
    (MonsterKind::GremlinMerc, "GIMME", 7, 8),
    (MonsterKind::GremlinMerc, "HEHE", 8, 9),
];

/// Move blocks C# scales at `AscensionLevel.ToughEnemies` (A8):
/// `(kind, move_id, base, tough)`. The patch rewrites the move's
/// `Intent::Defend` (the block a primary defend grants, and the telegraph),
/// and the effect_fns that grant a secondary defend read the same row
/// through [`tough_block`].
#[rustfmt::skip]
pub(crate) const TOUGH_BLOCK_SCALING: &[(MonsterKind, &str, i32, i32)] = &[
    // C# `Tunneler.cs:52` `BlockGain => (ToughEnemies, 37, 32)`.
    (MonsterKind::Tunneler, "BURROW_MOVE", 32, 37),
    // C# `LagavulinMatriarch.cs:57` `Slash2Block => (.., 14, 12)`.
    (MonsterKind::LagavulinMatriarch, "SLASH2", 12, 14),
    // C# `InfestedPrism.cs:44` `PulsateBlock => (.., 22, 20)`.
    (MonsterKind::InfestedPrism, "PULSATE", 20, 22),
    // C# `LouseProgenitor.cs:65` `CurlBlock => (.., 18, 14)` — a PRIMARY
    // defend, applied by `perform_intent` off the patched intent.
    (MonsterKind::LouseProgenitor, "CURL_AND_GROW_MOVE", 14, 18),
    // C# `MagiKnight.cs:49` `PowerShieldBlock => (.., 9, 5)`, read by both
    // `PowerShieldMove` and `PrepMove` (a primary defend).
    (MonsterKind::MagiKnight, "FIRST_POWER_SHIELD", 5, 9),
    (MonsterKind::MagiKnight, "PREP", 5, 9),
    // C# `Nibbit.cs:32` `SliceBlock => (.., 6, 5)`; the effect_fn reads
    // `ToughEnemies` itself, this keeps the telegraph in step.
    (MonsterKind::Nibbit, "SLICE", 5, 6),
];

/// The `ascension`-resolved block for `(kind, move_id)` from
/// [`TOUGH_BLOCK_SCALING`]: `base` below `ToughEnemies` or with no row.
#[must_use]
pub fn tough_block(ascension: u8, kind: MonsterKind, move_id: &str, base: i32) -> i32 {
    if ascension < AscensionLevel::ToughEnemies as u8 {
        return base;
    }
    TOUGH_BLOCK_SCALING
        .iter()
        .find(|e| e.0 == kind && e.1 == move_id)
        .map_or(base, |e| e.3)
}

/// The AI of a monster a fight adds (a summon, a respawn), patched for the
/// fight's ascension as [`crate::monster_dispatch::build_ai_with_ascension`]
/// patches a roster's. Every mid-fight spawn site builds its AI through this
/// before reading the first telegraph.
#[must_use]
pub fn summoned_ai(
    state: &sts2_core::CombatState,
    kind: MonsterKind,
    mut ai: MonsterAi,
) -> MonsterAi {
    apply_ascension_attack_scaling(&mut ai, kind, state.ascension);
    ai
}

/// Monsters whose `DeadlyEnemies` (A9) scaling bumps a move's BLOCK.
///
/// C# reads a `…Block` property in `CreatureCmd.GainBlock`, and the move's
/// `DefendIntent` telegraphs the same property, so one row drives both: the
/// patch rewrites `Intent::Defend { block }`, and the effect_fns that grant
/// a SECONDARY defend (which `perform_intent` drops) read the same pair via
/// [`deadly_block`] so telegraph and application cannot diverge.
///
/// `(kind, move_id, base_block, deadly_block)`.
pub(crate) const ASCENSION_BLOCK_SCALING: &[(MonsterKind, &str, i32, i32)] = &[
    // C# `AxeRubyRaider.cs:26` `SwingBlock => (.., 6, 5)`; both SWING move
    // states share `SwingMove`.
    (MonsterKind::AxeRubyRaider, "SWING_1", 5, 6),
    (MonsterKind::AxeRubyRaider, "SWING_2", 5, 6),
    // C# `Axebot.cs:36` `BootUpBlock => (.., 15, 10)` — a PRIMARY defend,
    // applied by `perform_intent` straight off the patched intent.
    (MonsterKind::Axebot, "BOOT_UP", 10, 15),
    // C# `BowlbugEgg.cs:25` `ProtectBlock => (.., 8, 7)`.
    (MonsterKind::BowlbugEgg, "BITE", 7, 8),
    // C# `InfestedPrism.cs:48` `RadiateBlock => (.., 13, 11)`.
    (MonsterKind::InfestedPrism, "RADIATE", 11, 13),
    // C# `TheObscura.cs:44` `HardeningStrikeBlock => (.., 7, 6)`.
    (MonsterKind::TheObscura, "HARDENING_STRIKE_MOVE", 6, 7),
];

/// The `ascension`-resolved block for `(kind, move_id)` — the value C#'s
/// `…Block` property yields. Returns `base` below `DeadlyEnemies` and for
/// any move with no row. Used by the secondary-defend effect_fns, whose
/// grant is invisible to the [`ASCENSION_BLOCK_SCALING`] intent patch.
#[must_use]
pub fn deadly_block(ascension: u8, kind: MonsterKind, move_id: &str, base: i32) -> i32 {
    if ascension < AscensionLevel::DeadlyEnemies as u8 {
        return base;
    }
    ASCENSION_BLOCK_SCALING
        .iter()
        .find(|e| e.0 == kind && e.1 == move_id)
        .map_or(base, |e| e.3)
}

/// Monsters whose `DeadlyEnemies` (A9) scaling bumps a MultiAttack's HIT
/// COUNT rather than its per-hit damage. The intent's `hits` drives BOTH
/// the telegraph and the number of `apply_damage_to_player` calls in
/// `perform_intent`, so patching it scales the dealt damage too — the
/// hits-side analogue of [`ASCENSION_ATTACK_SCALING`].
///
/// `(kind, move_id, base_hits, deadly_hits)`.
const ASCENSION_HITS_SCALING: &[(MonsterKind, &str, u8, u8)] = &[
    // C# `Exoskeleton.cs:36`: `SkitterRepeats =>
    // GetValueIfAscension(DeadlyEnemies, 4, 3)` — consumed by both the
    // telegraph (`:55` `MultiAttackIntent(SkitterDamage, SkitterRepeats)`)
    // and the effect (`:79` `.WithHitCount(SkitterRepeats)`). Per-hit
    // `SkitterDamage` (`:34`) is a flat 1 with no ascension pair, so the
    // damage table correctly has no SKITTER row; pre-fix the A9 SKITTER
    // dealt 1×3 instead of 1×4 (25% low) and telegraphed the same.
    // C# `Entomancer.cs:29` `BeesRepeat =>
    // GetValueIfAscension(DeadlyEnemies, 8, 7)`, read by both the telegraph
    // (`:45` `MultiAttackIntent(BeesDamage, BeesRepeat)`) and the effect
    // (`:73` `.WithHitCount(BeesRepeat)`). Per-hit `BeesDamage` is a flat 3.
    (MonsterKind::Entomancer, "BEES", 7, 8),
    (MonsterKind::Exoskeleton, "SKITTER", 3, 4),
    // C# `TrackerRubyRaider.cs:23,25`: `HoundsDamage =>
    // GetValueIfAscension(DeadlyEnemies, 1, 1)` (flat — no damage row) and
    // `HoundsRepeat => (.., 9, 8)`, consumed by both the telegraph (`:33`
    // `MultiAttackIntent(HoundsDamage, HoundsRepeat)`) and the effect
    // (`:49` `.WithHitCount(HoundsRepeat)`).
    (MonsterKind::TrackerRubyRaider, "HOUNDS", 8, 9),
];

/// The `ToughEnemies` (A8) half of [`apply_ascension_attack_scaling`]:
/// [`TOUGH_ATTACK_SCALING`] and [`TOUGH_BLOCK_SCALING`].
fn apply_tough_scaling(ai: &mut MonsterAi, kind: MonsterKind) {
    if !TOUGH_ATTACK_SCALING.iter().any(|e| e.0 == kind)
        && !TOUGH_BLOCK_SCALING.iter().any(|e| e.0 == kind)
    {
        return;
    }
    for state in std::sync::Arc::make_mut(&mut ai.states).values_mut() {
        let MonsterState::Move(m) = state else {
            continue;
        };
        if let Some(e) = TOUGH_ATTACK_SCALING
            .iter()
            .find(|e| e.0 == kind && e.1 == m.id)
        {
            for intent in &mut m.intents {
                match intent {
                    Intent::Attack { damage }
                    | Intent::DeathBlow { damage }
                    | Intent::MultiAttack { damage, .. } => *damage = e.3,
                    _ => {}
                }
            }
        }
        if let Some(e) = TOUGH_BLOCK_SCALING
            .iter()
            .find(|e| e.0 == kind && e.1 == m.id)
        {
            for intent in &mut m.intents {
                if let Intent::Defend { block } = intent {
                    *block = e.3;
                }
            }
        }
    }
}

/// Patch `ai` in place for the fight's ascension: the `ToughEnemies` rows at
/// `ascension >= 8`, then every scaled attack move's `DeadlyEnemies` damage
/// (and blocks, hit counts) at `ascension >= 9`. Below both, the
/// factory-built A0 values stand.
pub fn apply_ascension_attack_scaling(ai: &mut MonsterAi, kind: MonsterKind, ascension: u8) {
    if ascension >= AscensionLevel::ToughEnemies as u8 {
        apply_tough_scaling(ai, kind);
    }
    if ascension < AscensionLevel::DeadlyEnemies as u8 {
        return;
    }
    for state in std::sync::Arc::make_mut(&mut ai.states).values_mut() {
        let MonsterState::Move(m) = state else {
            continue;
        };
        if let Some(entry) = ASCENSION_ATTACK_SCALING
            .iter()
            .find(|e| e.0 == kind && e.1 == m.id)
        {
            let deadly = entry.3;
            for intent in &mut m.intents {
                match intent {
                    Intent::Attack { damage } | Intent::DeathBlow { damage } => *damage = deadly,
                    Intent::MultiAttack { damage, .. } => *damage = deadly,
                    _ => {}
                }
            }
        }
        if let Some(entry) = ASCENSION_BLOCK_SCALING
            .iter()
            .find(|e| e.0 == kind && e.1 == m.id)
        {
            let deadly = entry.3;
            for intent in &mut m.intents {
                if let Intent::Defend { block } = intent {
                    *block = deadly;
                }
            }
        }
        if let Some(entry) = ASCENSION_HITS_SCALING
            .iter()
            .find(|e| e.0 == kind && e.1 == m.id)
        {
            let deadly_hits = entry.3;
            for intent in &mut m.intents {
                if let Intent::MultiAttack { hits, .. } = intent {
                    *hits = deadly_hits;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sts2_core::powers::PowerKind;

    /// DeadlyEnemies (A9) power amounts applied by a move's `effect_fn`,
    /// as `(kind, move_id, power, base, deadly)`. The engine reads
    /// `state.ascension` inside each effect_fn — this is the transcribed
    /// expectation, exercised by [`ascension_buff_scaling_matches_csharp`]
    /// and counted by
    /// [`ascension_attack_table_covers_every_scaling_decomp_property`].
    const ASCENSION_BUFF_SCALING: &[(MonsterKind, &str, PowerKind, i32, i32)] = &[
        (
            MonsterKind::BowlbugNectar,
            "BUFF",
            PowerKind::Strength,
            15,
            16,
        ),
        (
            MonsterKind::CeremonialBeast,
            "CRUSH",
            PowerKind::Strength,
            3,
            4,
        ),
        (
            MonsterKind::CeremonialBeast,
            "STAMP",
            PowerKind::Plow,
            150,
            160,
        ),
        (MonsterKind::Crusher, "ADAPT", PowerKind::Strength, 2, 3),
        (
            MonsterKind::DampCultist,
            "INCANTATION",
            PowerKind::Ritual,
            5,
            6,
        ),
        // Aeonglass IncreasingIntensity grants base Strength
        // GetValueIfAscension(DeadlyEnemies, 4, 3) on the first move
        // (AdditionalStrength == 0); the effect adds `base + count`.
        (
            MonsterKind::Aeonglass,
            "INCREASING_INTENSITY_MOVE",
            PowerKind::Strength,
            3,
            4,
        ),
        // `…wb8lk.11`: v0.107.1 PULSATE re-applies VitalSpark (the Tainted
        // applier), not Strength. VitalSparkAmount A0 2 / A9 3.
        (
            MonsterKind::InfestedPrism,
            "PULSATE",
            PowerKind::VitalSpark,
            2,
            3,
        ),
        (
            MonsterKind::KinFollower,
            "POWER_DANCE",
            PowerKind::Strength,
            2,
            3,
        ),
        (
            MonsterKind::KinPriest,
            "RITUAL_MOVE",
            PowerKind::Strength,
            2,
            3,
        ),
        (
            MonsterKind::KnowledgeDemon,
            "PONDER",
            PowerKind::Strength,
            2,
            3,
        ),
        // NOTE: MagiKnight FIRST_POWER_SHIELD's Plating(5→9) is NOT ported in
        // the Rust effect_fn (applies 0), a pre-existing gap unrelated to
        // ascension; its attack damage (6→7) is covered by the attack table.
        (MonsterKind::Myte, "SUCK_MOVE", PowerKind::Strength, 2, 3),
        (MonsterKind::Nibbit, "HISS", PowerKind::Strength, 2, 3),
        (
            MonsterKind::Ovicopter,
            "NUTRITIONAL_PASTE_MOVE",
            PowerKind::Strength,
            3,
            4,
        ),
        (
            MonsterKind::PhantasmalGardener,
            "ENLARGE",
            PowerKind::Strength,
            2,
            3,
        ),
        (
            MonsterKind::Rocket,
            "CHARGE_UP_MOVE",
            PowerKind::Strength,
            2,
            3,
        ),
        (
            MonsterKind::Seapunk,
            "BUBBLE_BURP",
            PowerKind::Strength,
            1,
            2,
        ),
        // v0.107.1 `SkulkingColony.cs:49` is (4, 2) — A9 gives 4, not
        // the 3 this case asserted while the effect_fn read (3, 2).
        (
            MonsterKind::SkulkingColony,
            "INERTIA_MOVE",
            PowerKind::Strength,
            2,
            4,
        ),
        (
            MonsterKind::TheInsatiable,
            "SALIVATE",
            PowerKind::Strength,
            2,
            3,
        ),
        (
            MonsterKind::WaterfallGiant,
            "PRESSURIZE",
            PowerKind::SteamEruption,
            15,
            20,
        ),
        // C# `TestSubject.cs:104` `BurningGrowlStrengthGain => (.., 3, 2)`.
        (
            MonsterKind::TestSubject,
            "BURNING_GROWL",
            PowerKind::Strength,
            2,
            3,
        ),
    ];

    use super::*;
    use crate::monster_dispatch::{
        build_ai, build_ai_with_ascension, MonsterSpawnArgs, SpawnPosition,
    };

    fn move_attack_damage(ai: &MonsterAi, move_id: &str) -> Option<i32> {
        ai.states.values().find_map(|s| match s {
            MonsterState::Move(m) if m.id == move_id => m.intents.iter().find_map(|i| match i {
                Intent::Attack { damage } | Intent::DeathBlow { damage } => Some(*damage),
                Intent::MultiAttack { damage, .. } => Some(*damage),
                _ => None,
            }),
            _ => None,
        })
    }

    fn move_block(ai: &MonsterAi, move_id: &str) -> Option<i32> {
        ai.states.values().find_map(|s| match s {
            MonsterState::Move(m) if m.id == move_id => m.intents.iter().find_map(|i| match i {
                Intent::Defend { block } => Some(*block),
                _ => None,
            }),
            _ => None,
        })
    }

    /// The `ToughEnemies` (A8) rows: each resolves to a real move with its
    /// base value at A7 and its A8 value at A8 and A10 — Gremlin Merc's
    /// attacks (moved from the A9 table: C# `GremlinMerc.cs:36-44` scales
    /// them at ToughEnemies) and the move blocks.
    #[test]
    fn tough_enemies_rows_scale_at_a8() {
        let ai_at = |kind, asc| {
            build_ai_with_ascension(
                kind,
                MonsterSpawnArgs::default(),
                SpawnPosition::default(),
                asc,
            )
        };
        for &(kind, move_id, base, tough) in TOUGH_ATTACK_SCALING {
            assert_eq!(
                move_attack_damage(&ai_at(kind, 7), move_id),
                Some(base),
                "{kind:?} {move_id}"
            );
            for asc in [8, 10] {
                assert_eq!(
                    move_attack_damage(&ai_at(kind, asc), move_id),
                    Some(tough),
                    "{kind:?} {move_id} A{asc}"
                );
            }
        }
        for &(kind, move_id, base, tough) in TOUGH_BLOCK_SCALING {
            assert_eq!(
                move_block(&ai_at(kind, 7), move_id),
                Some(base),
                "{kind:?} {move_id}"
            );
            assert_eq!(
                move_block(&ai_at(kind, 8), move_id),
                Some(tough),
                "{kind:?} {move_id}"
            );
            assert_eq!(tough_block(7, kind, move_id, base), base);
            assert_eq!(tough_block(8, kind, move_id, base), tough);
        }
    }

    /// A secondary defend's block is granted by the move's effect_fn, which
    /// reads [`tough_block`]: Tunneler's BURROW 37 at A8 (A6d4: 37 on
    /// Headless at A10), Lagavulin Matriarch's SLASH2 14, Infested Prism's
    /// PULSATE 22, Magi Knight's FIRST_POWER_SHIELD 9.
    #[test]
    fn tough_enemies_secondary_defends_grant_the_a8_block() {
        for (kind, move_id, base, tough) in [
            (MonsterKind::Tunneler, "BURROW_MOVE", 32, 37),
            (MonsterKind::LagavulinMatriarch, "SLASH2", 12, 14),
            (MonsterKind::InfestedPrism, "PULSATE", 20, 22),
            (MonsterKind::MagiKnight, "FIRST_POWER_SHIELD", 5, 9),
        ] {
            for (asc, want) in [(7u8, base), (8, tough)] {
                let ai = build_ai_with_ascension(
                    kind,
                    MonsterSpawnArgs::default(),
                    SpawnPosition::default(),
                    asc,
                );
                let effect = ai
                    .states
                    .values()
                    .find_map(|s| match s {
                        MonsterState::Move(m) if m.id == move_id => Some(m.effect),
                        _ => None,
                    })
                    .expect("move");
                let mut state = sts2_core::CombatState {
                    ascension: asc,
                    ..sts2_core::CombatState::default()
                };
                let uid = state.spawn_enemy(kind);
                let before = state
                    .creatures
                    .iter()
                    .find(|c| c.id == uid.0)
                    .unwrap()
                    .block;
                effect(&mut state, uid);
                let after = state
                    .creatures
                    .iter()
                    .find(|c| c.id == uid.0)
                    .unwrap()
                    .block;
                assert_eq!(after - before, want, "{kind:?} {move_id} A{asc}");
            }
        }
    }

    /// A monster a fight adds mid-combat gets the fight's ascension patch,
    /// as a roster's does (A6c5: Gas Bomb's EXPLODE 9, Sneaky Gremlin's
    /// TACKLE 10, Wriggler's NASTY_BITE 7 at A10), including the telegraph
    /// read at spawn.
    #[test]
    fn mid_fight_summons_get_the_ascension_patch() {
        fn spawned(state: &sts2_core::CombatState, kind: MonsterKind) -> &MonsterAi {
            state
                .creatures
                .iter()
                .rev()
                .find(|c| c.species == Some(kind))
                .and_then(|c| c.ai.as_ref())
                .expect("summon with an AI")
        }
        let a10 = || {
            let mut state = sts2_core::CombatState {
                ascension: 10,
                ..sts2_core::CombatState::default()
            };
            state.player.current_hp = 80;
            state.player.max_hp = 80;
            state.init_player_creature();
            state
        };

        // Living Fog's BLOAT summons a Gas Bomb.
        let mut state = a10();
        let fog = state.spawn_enemy(MonsterKind::LivingFog);
        let fog_ai = build_ai_with_ascension(
            MonsterKind::LivingFog,
            MonsterSpawnArgs::default(),
            SpawnPosition::default(),
            10,
        );
        let bloat = fog_ai
            .states
            .values()
            .find_map(|s| match s {
                MonsterState::Move(m) if m.id == "BLOAT" => Some(m.effect),
                _ => None,
            })
            .expect("BLOAT");
        bloat(&mut state, fog);
        assert_eq!(
            move_attack_damage(spawned(&state, MonsterKind::GasBomb), "EXPLODE"),
            Some(9)
        );

        // A Gremlin Merc's Surprise summons Sneaky and Fat Gremlins.
        let mut state = a10();
        let merc = state.spawn_enemy(MonsterKind::GremlinMerc);
        crate::monsters::act1_underdocks::surprise_after_death_spawn(&mut state, merc);
        let sneaky = spawned(&state, MonsterKind::SneakyGremlin);
        assert_eq!(move_attack_damage(sneaky, "TACKLE"), Some(10));

        // A Phrog Parasite's Infested death spawns Wrigglers.
        let mut state = a10();
        let phrog = state.spawn_enemy(MonsterKind::PhrogParasite);
        crate::monsters::act1_overgrowth::infested_after_death_spawn(&mut state, phrog);
        assert_eq!(
            move_attack_damage(spawned(&state, MonsterKind::Wriggler), "NASTY_BITE"),
            Some(7)
        );
    }

    /// Every table row must resolve to a real attack move whose A0 damage
    /// equals `base`, and applying DeadlyEnemies scaling must bump it to
    /// `deadly`. This validates the C#-transcribed table against the actual
    /// factories — a bad move-id, wrong base, or non-attack move fails here.
    #[test]
    fn ascension_attack_table_matches_real_moves() {
        let mut fails = Vec::new();
        for &(kind, move_id, base, deadly) in ASCENSION_ATTACK_SCALING {
            if deadly <= base {
                fails.push(format!(
                    "{kind:?} {move_id}: deadly {deadly} <= base {base}"
                ));
                continue;
            }
            let ai = build_ai(kind, MonsterSpawnArgs::None, SpawnPosition::default());
            match move_attack_damage(&ai, move_id) {
                None => fails.push(format!("{kind:?} {move_id}: no attack move with that id")),
                Some(dmg) if dmg != base => {
                    fails.push(format!(
                        "{kind:?} {move_id}: A0 damage {dmg} != table base {base}"
                    ));
                }
                Some(_) => {
                    let mut ai9 = build_ai(kind, MonsterSpawnArgs::None, SpawnPosition::default());
                    apply_ascension_attack_scaling(&mut ai9, kind, 9);
                    let dmg9 = move_attack_damage(&ai9, move_id).unwrap();
                    if dmg9 != deadly {
                        fails.push(format!(
                            "{kind:?} {move_id}: A9 damage {dmg9} != table deadly {deadly}"
                        ));
                    }
                }
            }
        }
        assert!(
            fails.is_empty(),
            "{} bad rows:\n{}",
            fails.len(),
            fails.join("\n")
        );
    }

    /// Regression for the missing-`PECK`-row bug: at A9 `DeadlyEnemies`,
    /// Byrdonis `PECK` (a `MultiAttack`) must scale per-hit damage 3→4 while
    /// keeping hits=3 (C# `Byrdonis.cs:23` `PeckDamage` scales 4/3; `:25`
    /// `PeckRepeat` stays 3/3), so total damage goes 9→12. `SWOOP` (the
    /// already-present row) must still scale 17→19. The whole-table self-test
    /// can only validate rows that EXIST, so a dropped row like `PECK` was
    /// invisible there — this exercises the built AI end-to-end via
    /// `build_ai_with_ascension`, the path combat actually uses.
    #[test]
    fn byrdonis_peck_and_swoop_scale_at_a9() {
        fn move_intent<'a>(ai: &'a MonsterAi, id: &str) -> &'a Intent {
            let Some(MonsterState::Move(m)) = ai.states.get(id) else {
                panic!("no move {id}");
            };
            &m.intents[0]
        }

        let a0 = build_ai_with_ascension(
            MonsterKind::Byrdonis,
            MonsterSpawnArgs::None,
            SpawnPosition::default(),
            0,
        );
        assert!(matches!(
            move_intent(&a0, "PECK"),
            Intent::MultiAttack { damage: 3, hits: 3 }
        ));
        assert!(matches!(
            move_intent(&a0, "SWOOP"),
            Intent::Attack { damage: 17 }
        ));

        let a9 = build_ai_with_ascension(
            MonsterKind::Byrdonis,
            MonsterSpawnArgs::None,
            SpawnPosition::default(),
            9,
        );
        assert!(
            matches!(
                move_intent(&a9, "PECK"),
                Intent::MultiAttack { damage: 4, hits: 3 }
            ),
            "PECK must scale per-hit 3->4 and keep hits=3 at A9"
        );
        assert!(matches!(
            move_intent(&a9, "SWOOP"),
            Intent::Attack { damage: 19 }
        ));
    }

    /// C# `Exoskeleton.cs:36` `SkitterRepeats =>
    /// GetValueIfAscension(DeadlyEnemies, 4, 3)`: the A9 scaling bumps the
    /// SKITTER hit COUNT (1×3 → 1×4), not the flat per-hit 1 — the first
    /// member of the [`ASCENSION_HITS_SCALING`] lane. MANDIBLE keeps its
    /// damage-lane 8→9 row (`Exoskeleton.cs:38`). End-to-end via
    /// `build_ai_with_ascension`, the path combat actually uses.
    #[test]
    fn exoskeleton_skitter_hits_scale_at_a9() {
        fn move_intent<'a>(ai: &'a MonsterAi, id: &str) -> &'a Intent {
            let Some(MonsterState::Move(m)) = ai.states.get(id) else {
                panic!("no move {id}");
            };
            &m.intents[0]
        }

        let a0 = build_ai_with_ascension(
            MonsterKind::Exoskeleton,
            MonsterSpawnArgs::None,
            SpawnPosition::default(),
            0,
        );
        assert!(matches!(
            move_intent(&a0, "SKITTER"),
            Intent::MultiAttack { damage: 1, hits: 3 }
        ));
        assert!(matches!(
            move_intent(&a0, "MANDIBLE"),
            Intent::Attack { damage: 8 }
        ));

        let a9 = build_ai_with_ascension(
            MonsterKind::Exoskeleton,
            MonsterSpawnArgs::None,
            SpawnPosition::default(),
            9,
        );
        assert!(
            matches!(
                move_intent(&a9, "SKITTER"),
                Intent::MultiAttack { damage: 1, hits: 4 }
            ),
            "SKITTER must scale hits 3->4 (per-hit stays 1) at A9"
        );
        assert!(matches!(
            move_intent(&a9, "MANDIBLE"),
            Intent::Attack { damage: 9 }
        ));

        // A8 (ToughEnemies) is below the DeadlyEnemies threshold — no bump.
        let a8 = build_ai_with_ascension(
            MonsterKind::Exoskeleton,
            MonsterSpawnArgs::None,
            SpawnPosition::default(),
            8,
        );
        assert!(matches!(
            move_intent(&a8, "SKITTER"),
            Intent::MultiAttack { damage: 1, hits: 3 }
        ));
    }

    /// Every hits-scaling row must resolve to a real MultiAttack move whose
    /// A0 hit count equals `base_hits`, bumped to `deadly_hits` at A9 —
    /// the hits-lane analogue of `ascension_attack_table_matches_real_moves`.
    #[test]
    fn ascension_hits_table_matches_real_moves() {
        fn move_hits(ai: &MonsterAi, move_id: &str) -> Option<u8> {
            ai.states.values().find_map(|s| match s {
                MonsterState::Move(m) if m.id == move_id => {
                    m.intents.iter().find_map(|i| match i {
                        Intent::MultiAttack { hits, .. } => Some(*hits),
                        _ => None,
                    })
                }
                _ => None,
            })
        }
        let mut fails = Vec::new();
        for &(kind, move_id, base, deadly) in ASCENSION_HITS_SCALING {
            if deadly <= base {
                fails.push(format!(
                    "{kind:?} {move_id}: deadly {deadly} <= base {base}"
                ));
                continue;
            }
            let ai = build_ai(kind, MonsterSpawnArgs::None, SpawnPosition::default());
            match move_hits(&ai, move_id) {
                None => fails.push(format!(
                    "{kind:?} {move_id}: no MultiAttack move with that id"
                )),
                Some(h) if h != base => {
                    fails.push(format!(
                        "{kind:?} {move_id}: A0 hits {h} != table base {base}"
                    ));
                }
                Some(_) => {
                    let mut ai9 = build_ai(kind, MonsterSpawnArgs::None, SpawnPosition::default());
                    apply_ascension_attack_scaling(&mut ai9, kind, 9);
                    let h9 = move_hits(&ai9, move_id).unwrap();
                    if h9 != deadly {
                        fails.push(format!(
                            "{kind:?} {move_id}: A9 hits {h9} != table deadly {deadly}"
                        ));
                    }
                }
            }
        }
        assert!(
            fails.is_empty(),
            "{} bad rows:\n{}",
            fails.len(),
            fails.join("\n")
        );
    }

    /// Workbench report `wb-010871941d034711` (seed DZV0PD70HW, Ironclad
    /// A9, Underdocks `ToadpolesWeak`): the real game telegraphed WHIRL 8
    /// and SPIKE_SPIT 4x3 while we showed 7 and 3x3 — Toadpole had NO rows
    /// in [`ASCENSION_ATTACK_SCALING`]. C# `Toadpole.cs:49,53`:
    /// `SpikeSpitDamage => GetValueIfAscension(DeadlyEnemies, 4, 3)`,
    /// `WhirlDamage => (.., 8, 7)`.
    #[test]
    fn toadpole_attacks_scale_at_a9() {
        fn move_intent<'a>(ai: &'a MonsterAi, id: &str) -> &'a Intent {
            let Some(MonsterState::Move(m)) = ai.states.get(id) else {
                panic!("no move {id}");
            };
            &m.intents[0]
        }

        let a0 = build_ai_with_ascension(
            MonsterKind::Toadpole,
            MonsterSpawnArgs::None,
            SpawnPosition::default(),
            0,
        );
        assert!(matches!(
            move_intent(&a0, "SPIKE_SPIT_MOVE"),
            Intent::MultiAttack { damage: 3, hits: 3 }
        ));
        assert!(matches!(
            move_intent(&a0, "WHIRL_MOVE"),
            Intent::Attack { damage: 7 }
        ));

        let a9 = build_ai_with_ascension(
            MonsterKind::Toadpole,
            MonsterSpawnArgs::None,
            SpawnPosition::default(),
            9,
        );
        assert!(
            matches!(
                move_intent(&a9, "SPIKE_SPIT_MOVE"),
                Intent::MultiAttack { damage: 4, hits: 3 }
            ),
            "SPIKE_SPIT must be 4x3 at A9 (DeadlyEnemies), not 3x3"
        );
        assert!(
            matches!(move_intent(&a9, "WHIRL_MOVE"), Intent::Attack { damage: 8 }),
            "WHIRL must be 8 at A9 (DeadlyEnemies), not 7"
        );
    }

    /// Every [`ASCENSION_BLOCK_SCALING`] row must resolve to a real move
    /// whose A0 `Intent::Defend` block equals `base`, bumped to `deadly` at
    /// A9 — the block-lane analogue of
    /// `ascension_attack_table_matches_real_moves`.
    #[test]
    fn ascension_block_table_matches_real_moves() {
        fn move_block(ai: &MonsterAi, move_id: &str) -> Option<i32> {
            ai.states.values().find_map(|s| match s {
                MonsterState::Move(m) if m.id == move_id => {
                    m.intents.iter().find_map(|i| match i {
                        Intent::Defend { block } => Some(*block),
                        _ => None,
                    })
                }
                _ => None,
            })
        }
        let mut fails = Vec::new();
        for &(kind, move_id, base, deadly) in ASCENSION_BLOCK_SCALING {
            if deadly <= base {
                fails.push(format!(
                    "{kind:?} {move_id}: deadly {deadly} <= base {base}"
                ));
                continue;
            }
            let ai = build_ai(kind, MonsterSpawnArgs::None, SpawnPosition::default());
            match move_block(&ai, move_id) {
                None => fails.push(format!(
                    "{kind:?} {move_id}: no Defend intent on a move with that id"
                )),
                Some(b) if b != base => fails.push(format!(
                    "{kind:?} {move_id}: A0 block {b} != table base {base}"
                )),
                Some(_) => {
                    let mut ai9 = build_ai(kind, MonsterSpawnArgs::None, SpawnPosition::default());
                    apply_ascension_attack_scaling(&mut ai9, kind, 9);
                    let b9 = move_block(&ai9, move_id).unwrap();
                    if b9 != deadly {
                        fails.push(format!(
                            "{kind:?} {move_id}: A9 block {b9} != table deadly {deadly}"
                        ));
                    }
                }
            }
            // `deadly_block` — the seam the secondary-defend effect_fns use —
            // must agree with the table on both sides of the threshold.
            for (asc, want) in [(8u8, base), (9u8, deadly)] {
                let got = deadly_block(asc, kind, move_id, base);
                if got != want {
                    fails.push(format!(
                        "{kind:?} {move_id}: deadly_block(A{asc}) = {got} != {want}"
                    ));
                }
            }
        }
        assert!(
            fails.is_empty(),
            "{} bad block rows:\n{}",
            fails.len(),
            fails.join("\n")
        );
    }

    /// The four monsters whose Defend is a SECONDARY intent grant their
    /// block from an `effect_fn` (`perform_intent` only applies a move's
    /// PRIMARY intent), so the intent patch alone would leave the telegraph
    /// and the granted block disagreeing at A9. Drive the effect_fn itself.
    #[test]
    fn secondary_defend_block_grants_scale_at_a9() {
        use sts2_core::{CombatState, MonsterKind};

        let cases: &[(MonsterKind, &str, i32, i32)] = &[
            (MonsterKind::AxeRubyRaider, "SWING_1", 5, 6),
            (MonsterKind::AxeRubyRaider, "SWING_2", 5, 6),
            (MonsterKind::BowlbugEgg, "BITE", 7, 8),
            (MonsterKind::InfestedPrism, "RADIATE", 11, 13),
            (MonsterKind::TheObscura, "HARDENING_STRIKE_MOVE", 6, 7),
        ];
        let mut fails = Vec::new();
        for &(kind, move_id, base, deadly) in cases {
            for (asc, want) in [(0u8, base), (9u8, deadly)] {
                let mut state = CombatState {
                    ascension: asc,
                    ..CombatState::default()
                };
                let uid = state.spawn_enemy(kind);
                let ai = build_ai(kind, MonsterSpawnArgs::None, SpawnPosition::default());
                let Some(MonsterState::Move(m)) = ai.states.get(move_id) else {
                    fails.push(format!("{kind:?} {move_id}: no such move"));
                    continue;
                };
                (m.effect)(&mut state, uid);
                let got = state
                    .creatures
                    .iter()
                    .find(|c| c.id == uid.0)
                    .map_or(-1, |c| c.block);
                if got != want {
                    fails.push(format!(
                        "{kind:?} {move_id} A{asc}: granted {got} block != {want}"
                    ));
                }
            }
        }
        assert!(
            fails.is_empty(),
            "{} secondary-block mismatches:\n{}",
            fails.len(),
            fails.join("\n")
        );
    }

    /// C# `Entomancer.cs:29` `BeesRepeat => (.., 8, 7)` — the A9 scaling
    /// bumps the BEES hit COUNT (3x7 → 3x8); per-hit `BeesDamage` is flat 3.
    #[test]
    fn entomancer_bees_hits_scale_at_a9() {
        fn move_intent<'a>(ai: &'a MonsterAi, id: &str) -> &'a Intent {
            let Some(MonsterState::Move(m)) = ai.states.get(id) else {
                panic!("no move {id}");
            };
            &m.intents[0]
        }
        let a0 = build_ai_with_ascension(
            MonsterKind::Entomancer,
            MonsterSpawnArgs::None,
            SpawnPosition::default(),
            0,
        );
        assert!(matches!(
            move_intent(&a0, "BEES"),
            Intent::MultiAttack { damage: 3, hits: 7 }
        ));
        let a9 = build_ai_with_ascension(
            MonsterKind::Entomancer,
            MonsterSpawnArgs::None,
            SpawnPosition::default(),
            9,
        );
        assert!(
            matches!(
                move_intent(&a9, "BEES"),
                Intent::MultiAttack { damage: 3, hits: 8 }
            ),
            "BEES must be 3x8 at A9 (per-hit stays 3)"
        );
    }

    /// C# `AfterAddedToRoom` amounts that scale: CorpseSlug's Ravenous
    /// (`CorpseSlug.cs:51`) and TestSubject's Enrage (`TestSubject.cs:88`).
    /// `on_spawn_self_buffs` carries the A0 amount; `spawn_enemy` must swap
    /// in the DeadlyEnemies value at A9.
    #[test]
    fn on_spawn_self_buffs_scale_at_a9() {
        use sts2_core::powers::PowerKind;
        use sts2_core::{CombatState, MonsterKind};

        fn spawned_amount(kind: MonsterKind, asc: u8, power: PowerKind) -> i32 {
            let mut state = CombatState {
                ascension: asc,
                ..CombatState::default()
            };
            let uid = state.spawn_enemy(kind);
            let creature = state.creatures.iter().find(|c| c.id == uid.0).unwrap();
            let owned: Vec<_> = creature.powers.to_vec();
            state
                .powers
                .iter()
                .filter(|p| p.kind == power && owned.iter().any(|u| u.0 == p.id))
                .map(|p| p.amount)
                .sum()
        }

        for (kind, power, base, deadly) in [
            (MonsterKind::CorpseSlug, PowerKind::Ravenous, 4, 5),
            (MonsterKind::TestSubject, PowerKind::Enrage, 2, 3),
        ] {
            assert_eq!(
                spawned_amount(kind, 8, power),
                base,
                "{kind:?} {power:?} at A8 (below DeadlyEnemies)"
            );
            assert_eq!(
                spawned_amount(kind, 9, power),
                deadly,
                "{kind:?} {power:?} at A9 (DeadlyEnemies)"
            );
        }
    }

    /// Properties whose A9 value is honoured somewhere other than the four
    /// tables, listed so the completeness guard below stays exhaustive
    /// instead of silently ignoring a lane. `(class, property, where)`.
    const ACCOUNTED_ELSEWHERE: &[(&str, &str, &str)] = &[
        (
            "Axebot",
            "BootUpStrGain",
            "axebot_boot_up_effect reads it directly; the amount is multiplied \
             by (2 - StockAmount), which the flat buff-case harness cannot express",
        ),
        (
            "CorpseSlug",
            "RavenousStr",
            "AfterAddedToRoom power — MonsterKind::on_spawn_self_buff_deadly",
        ),
        (
            "TestSubject",
            "EnrageAmount",
            "AfterAddedToRoom power — MonsterKind::on_spawn_self_buff_deadly",
        ),
        (
            "TestSubject",
            "BurningGrowlBurnCount",
            "a Burn STATUS-CARD count, not a power — \
             test_subject::BURNING_GROWL_BURN_COUNT_DEADLY",
        ),
        (
            "Aeonglass",
            "WitherAmount",
            "a Wither STATUS-CARD count, not a power — read inline by \
             aeonglass_increasing_intensity_effect",
        ),
    ];

    /// Completeness guard against the v0.107.1 decomp, across every lane.
    ///
    /// Each `…  => AscensionHelper.GetValueIfAscension(AscensionLevel.
    /// DeadlyEnemies, deadly, base)` member with `deadly != base` must be
    /// represented on our side: `…Damage` by an [`ASCENSION_ATTACK_SCALING`]
    /// row, `…Block` by [`ASCENSION_BLOCK_SCALING`], `…Repeat(s)` by
    /// [`ASCENSION_HITS_SCALING`], anything else by an
    /// [`ASCENSION_BUFF_SCALING`] case or an [`ACCOUNTED_ELSEWHERE`] entry.
    ///
    /// The sibling `ascension_attack_table_matches_real_moves` only
    /// validates rows that ARE present, so a monster with NO rows passed it.
    /// `wb-010871941d034711` was exactly that: Toadpole dealt its A0 3x3 / 7
    /// at A9. This guard also caught SneakyGremlin, TestSubject and
    /// TheInsatiable in the damage lane, the whole block lane, Entomancer's
    /// BEES hit count and two spawn-time powers.
    ///
    /// Move ids differ between C# (`TACKLE_MOVE`) and our factories
    /// (`TACKLE`), so this counts members per monster rather than matching
    /// ids; a row pointing at the wrong move still trips
    /// `ascension_attack_table_matches_real_moves`. Skips (with a printed
    /// note) when the decomp sibling is not checked out.
    #[test]
    fn ascension_attack_table_covers_every_scaling_decomp_property() {
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.Models.Monsters");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            eprintln!("skipping: decomp dir not present at {}", dir.display());
            return;
        };

        // lane name -> class name -> how many values we carry for it.
        let mut have: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
        for &(kind, ..) in ASCENSION_ATTACK_SCALING {
            *have
                .entry("damage")
                .or_default()
                .entry(kind.python_name())
                .or_default() += 1;
        }
        for &(kind, ..) in ASCENSION_BLOCK_SCALING {
            *have
                .entry("block")
                .or_default()
                .entry(kind.python_name())
                .or_default() += 1;
        }
        for &(kind, ..) in ASCENSION_HITS_SCALING {
            *have
                .entry("hits")
                .or_default()
                .entry(kind.python_name())
                .or_default() += 1;
        }
        for &(kind, ..) in ASCENSION_BUFF_SCALING {
            *have
                .entry("other")
                .or_default()
                .entry(kind.python_name())
                .or_default() += 1;
        }
        for &(class, ..) in ACCOUNTED_ELSEWHERE {
            *have.entry("other").or_default().entry(class).or_default() += 1;
        }

        // Same, from the decomp.
        let mut want: BTreeMap<&str, BTreeMap<String, usize>> = BTreeMap::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "cs") {
                continue;
            }
            let Some(class) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // Only classes we have actually ported.
            if !MonsterKind::ALL_KNOWN
                .iter()
                .any(|k| k.python_name() == class)
            {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            // C# sometimes assigns the scaled value to a local inside the
            // property body (`TheForgotten.DreadDamage`), so remember the
            // enclosing member and fall back to it for a lowercase token.
            let mut member = "";
            for line in src.lines() {
                if let Some(rest) = line.trim().strip_prefix("private int ") {
                    member = rest
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                }
                let Some((head, args)) = line.split_once(
                    "AscensionHelper.GetValueIfAscension(AscensionLevel.DeadlyEnemies, ",
                ) else {
                    continue;
                };
                let Some(args) = args.split(')').next() else {
                    continue;
                };
                let Some((deadly, base)) = args.split_once(", ") else {
                    continue;
                };
                if deadly.trim() == base.trim() {
                    continue;
                }
                // Member name: the last identifier before `=>` / `=`.
                let token = head
                    .trim_end()
                    .trim_end_matches(['=', '>', ' '])
                    .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("");
                // A lowercase token is a local (`int valueIfAscension = …`);
                // the lane is decided by the property that wraps it.
                let name = if token.starts_with(|c: char| c.is_lowercase()) {
                    member
                } else {
                    token
                };
                let lane = if name.ends_with("Damage") {
                    "damage"
                } else if name.ends_with("Block") {
                    "block"
                } else if name.ends_with("Repeat") || name.ends_with("Repeats") {
                    "hits"
                } else {
                    "other"
                };
                *want
                    .entry(lane)
                    .or_default()
                    .entry(class.to_string())
                    .or_default() += 1;
            }
        }

        let mut fails = Vec::new();
        for (lane, classes) in &want {
            for (class, n) in classes {
                let got = have
                    .get(lane)
                    .and_then(|m| m.get(class.as_str()))
                    .copied()
                    .unwrap_or(0);
                if got < *n {
                    fails.push(format!(
                        "{class}: {n} scaling `{lane}` member(s) in the decomp, \
                         but we carry only {got}"
                    ));
                }
            }
        }
        assert!(
            fails.is_empty(),
            "{} monster/lane pair(s) under-covered by the ascension tables:\n{}",
            fails.len(),
            fails.join("\n")
        );
    }

    /// Below the DeadlyEnemies threshold the patch is a no-op.
    #[test]
    fn ascension_attack_scaling_noop_below_a9() {
        if let Some(&(kind, move_id, base, _)) = ASCENSION_ATTACK_SCALING.first() {
            let mut ai = build_ai(kind, MonsterSpawnArgs::None, SpawnPosition::default());
            apply_ascension_attack_scaling(&mut ai, kind, 8);
            assert_eq!(move_attack_damage(&ai, move_id), Some(base));
        }
    }
    /// DeadlyEnemies (>= A9) buff scaling: triggering each scaled move's
    /// `effect_fn` must apply `base` Strength/Ritual/etc at A0 and `deadly`
    /// at A9. Location-agnostic — asserts the delta in the total amount of
    /// the target `PowerKind` across the whole combat, so it works whether
    /// the buff lands on self or the player. Transcribed from the C#
    /// `GetValueIfAscension(DeadlyEnemies, …)` power amounts.
    #[test]
    fn ascension_buff_scaling_matches_csharp() {
        use sts2_core::powers::PowerKind;
        use sts2_core::CombatState;

        fn total_power(state: &CombatState, kind: PowerKind) -> i32 {
            state
                .powers
                .iter()
                .filter(|p| p.kind == kind)
                .map(|p| p.amount)
                .sum()
        }

        let cases = ASCENSION_BUFF_SCALING;
        let mut fails = Vec::new();
        for &(kind, move_id, power, base, deadly) in cases {
            for (asc, expected) in [(0u8, base), (9u8, deadly)] {
                let mut state = CombatState {
                    ascension: asc,
                    ..CombatState::default()
                };
                let uid = state.spawn_enemy(kind);
                let ai = build_ai(kind, MonsterSpawnArgs::None, SpawnPosition::default());
                let Some(MonsterState::Move(m)) = ai.states.get(move_id) else {
                    fails.push(format!("{kind:?} {move_id}: no such move"));
                    continue;
                };
                let effect = m.effect;
                let before = total_power(&state, power);
                effect(&mut state, uid);
                let got = total_power(&state, power) - before;
                if got != expected {
                    fails.push(format!(
                        "{kind:?} {move_id} {power:?} A{asc}: applied {got} != {expected}"
                    ));
                }
            }
        }
        assert!(
            fails.is_empty(),
            "{} buff mismatches:\n{}",
            fails.len(),
            fails.join("\n")
        );
    }
}
