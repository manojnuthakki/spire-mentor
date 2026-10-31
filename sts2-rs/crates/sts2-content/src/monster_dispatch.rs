//! Monster spawn dispatch — Phase 1 of the Act-1 wiring umbrella
//! (`docs/plans/2026-05-14-expose-all-sts2-enemies-all-ironclad-cards-through-sts2-cli-.md`,
//! decision D1+D2; bead `sts2-rs-tgr4`).
//!
//! Factors the implicit `MonsterKind → factory fn` dispatch — previously
//! buried inside `sts2-cli`'s `encounter::monster_ai` — into a single
//! data-driven [`build_ai`] function so Phase 2 (and later Acts 2/3/4)
//! land as table extensions rather than a 4-callsite refactor.
//!
//! The function is total over the 12 [`MonsterKind`] variants that
//! exist on the Phase-1 commit; future kinds added by Phase 2 wiring
//! beads (`sts2-rs-h37x` / `mv7c` / `ckj8`) extend the match and the
//! [`MonsterSpawnArgs`] sum-type in step. The catch-all arm panics with
//! a directive pointing at this file; that arm is unreachable today.
//!
//! Per-kind structured spawn arguments (Nibbit's group-position fix-up,
//! Inklet/KinFollower slot args) flow through
//! [`MonsterSpawnArgs`]; generic positional context (alone / front /
//! back / row index) flows through [`SpawnPosition`]. The two surfaces
//! overlap intentionally — Nibbit's `is_alone` + `is_front` are
//! captured in both `MonsterSpawnArgs::Nibbit { … }` (the explicit,
//! caller-supplied form, used by the Phase-2 CLI parser for
//! `Inklet:Left`-style syntax) AND in `SpawnPosition` (the implicit,
//! `Scenario::from_monsters` group-fixup form). When the spawn args
//! carry an explicit kind-specific variant, it wins; otherwise the
//! position is used as a fallback so the existing
//! `Scenario::from_monsters` callers keep working untouched.

use sts2_core::monster_ai::MonsterAi;
use sts2_core::MonsterKind;

use crate::monsters::act1_overgrowth::{
    create_assassin_ruby_raider_ai, create_axe_ruby_raider_ai, create_brute_ruby_raider_ai,
    create_bygone_effigy_ai, create_byrdonis_ai, create_ceremonial_beast_ai,
    create_crossbow_ruby_raider_ai, create_cubex_construct_ai, create_eye_with_teeth_ai,
    create_flyconid_ai, create_fogmog_ai, create_inklet_ai, create_kin_follower_ai,
    create_kin_priest_ai, create_mawler_ai, create_parafright_ai, create_phrog_parasite_ai,
    create_slithering_strangler_ai, create_snapping_jaxfruit_ai, create_tracker_ruby_raider_ai,
    create_vantom_ai, create_vine_shambler_ai, InkletSlot, KinFollowerSlot,
};
use crate::monsters::act1_underdocks::{
    create_calcified_cultist_ai, create_corpse_slug_ai, create_damp_cultist_ai,
    create_fat_gremlin_ai, create_fossil_stalker_ai, create_gas_bomb_ai, create_gremlin_merc_ai,
    create_haunted_ship_ai, create_lagavulin_matriarch_ai, create_living_fog_ai,
    create_phantasmal_gardener_ai, create_punch_construct_ai, create_seapunk_ai,
    create_sewer_clam_ai, create_skulking_colony_ai, create_sludge_spinner_ai,
    create_sneaky_gremlin_ai, create_soul_fysh_ai, create_terror_eel_ai, create_toadpole_ai,
    create_two_tailed_rat_ai, create_waterfall_giant_ai, PhantasmalGardenerSlot, ToadpoleSlot,
    TwoTailedRatSlot,
};
use crate::monsters::act1_weak::{
    create_fuzzy_wurm_crawler_ai, create_leaf_slime_m_ai, create_leaf_slime_s_ai, create_nibbit_ai,
    create_shrinker_beetle_ai, create_twig_slime_m_ai, create_twig_slime_s_ai,
};
use crate::monsters::act2_hive::{
    create_bowlbug_egg_ai, create_bowlbug_nectar_ai, create_bowlbug_rock_ai,
    create_bowlbug_silk_ai, create_chomper_ai, create_crusher_ai, create_decimillipede_segment_ai,
    create_decimillipede_segment_back_ai, create_decimillipede_segment_front_ai,
    create_decimillipede_segment_middle_ai, create_entomancer_ai, create_exoskeleton_ai,
    create_hunter_killer_ai, create_infested_prism_ai, create_knowledge_demon_ai,
    create_louse_progenitor_ai, create_myte_ai, create_ovicopter_ai, create_rocket_ai,
    create_slumbering_beetle_ai, create_spiny_toad_ai, create_the_insatiable_ai,
    create_the_obscura_ai, create_thieving_hopper_ai, create_tough_egg_ai, create_tunneler_ai,
    create_wriggler_ai, ExoskeletonSlot, InfestedPrismSlot, MyteSlot,
};
use crate::monsters::act3_glory::{
    create_aeonglass_ai, create_axebot_ai, create_devoted_sculptor_ai, create_fabricator_ai,
    create_flail_knight_ai, create_frog_knight_ai, create_globe_head_ai, create_guardbot_ai,
    create_living_shield_ai, create_magi_knight_ai, create_mecha_knight_ai,
    create_mysterious_knight_ai, create_noisebot_ai, create_osty_ai, create_owl_magistrate_ai,
    create_queen_ai, create_scroll_of_biting_ai, create_slimed_berserker_ai, create_soul_nexus_ai,
    create_spectral_knight_ai, create_stabbot_ai, create_test_subject_ai, create_the_forgotten_ai,
    create_the_lost_ai, create_turret_operator_ai, create_zapbot_ai,
};
use crate::monsters::shared::{
    create_architect_ai, create_battle_friend_v1_ai, create_battle_friend_v2_ai,
    create_battle_friend_v3_ai, create_big_dummy_ai, create_byrdpip_ai,
    create_dense_vegetation_wriggler_ai, create_fake_merchant_monster_ai,
    create_multi_attack_move_monster_ai, create_one_hp_monster_ai, create_paels_legion_ai,
    create_single_attack_move_monster_ai, create_ten_hp_monster_ai, create_the_adversary_mk_one_ai,
    create_the_adversary_mk_three_ai, create_the_adversary_mk_two_ai, create_torch_head_amalgam_ai,
};

/// Per-kind structured spawn arguments. `None` is the default for
/// monsters whose factory takes no per-spawn config. Phase 2.2
/// (`sts2-rs-mv7c`) added `Inklet(InkletSlot)` and
/// `KinFollower(KinFollowerSlot)` for the two slot-arg act1 monsters;
/// future Acts 2/3/4 extend further (`KnowledgeDemon`, Decimillipede, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[derive(Default)]
pub enum MonsterSpawnArgs {
    /// Default: factory takes no per-spawn config.
    #[default]
    None,
    /// Nibbit's `is_alone` + `is_front` group-position fix-up. The
    /// Phase-5 monster DSL routes one of three move chains
    /// (BUTT / SLICE / HISS) depending on these two booleans.
    Nibbit { is_alone: bool, is_front: bool },
    /// Inklet's starting move depends on the slot in the encounter
    /// (First → SPLATTER, Second → SUBMERGE, Other → RAND).
    Inklet { slot: InkletSlot },
    /// `KinFollower`'s starting move depends on the slot
    /// (First/Second start on BASH; Third starts on BITE).
    KinFollower { slot: KinFollowerSlot },
    // ── Acts 2/3 (sts2-rs-6ynt.10) ──
    /// Exoskeleton's starting move depends on its slot in the
    /// encounter (First → SKITTER, Second → MANDIBLE, Third → ENRAGE,
    /// Fourth → RAND).
    Exoskeleton { slot: ExoskeletonSlot },
    /// Chomper's `scream_first` flag selects whether the first move
    /// is SCREAM (true) or BITE (false).
    Chomper { scream_first: bool },
    /// `DecimillipedeSegment`'s `starter_idx` cycles starting moves
    /// across segments in the elite encounter.
    DecimillipedeSegment { starter_idx: u8 },
    /// `ScrollOfBiting`'s `starter_idx` selects the C# initial state
    /// `StarterMoveIdx % 3` (0=CHOMP, 1=CHEW, 2=MORE_TEETH). The
    /// `ScrollsOfBitingNormal/Weak.cs::GenerateMonsters` rotation
    /// assigns each scroll a distinct opener. Bead `sts2-rs-ybtmu`.
    ScrollOfBiting { starter_idx: u8 },
    /// `InfestedPrism`'s starting move depends on the slot (First →
    /// LASER_BEAM, Second → INFESTED_LASER, Other → RAND).
    InfestedPrism { slot: InfestedPrismSlot },
    /// `Myte`'s starting move depends on its slot in the 2-Myte
    /// encounter (First → TOXIC_MOVE, Second → SUCK_MOVE). Bead
    /// `sts2-rs-ybtmu`.
    Myte { slot: MyteSlot },
    /// Axebot's `start_with_boot_up` flag selects whether the first
    /// move is BOOT_UP (true) or its normal opener (false).
    Axebot { start_with_boot_up: bool },
    /// `DenseVegetationWriggler`'s 4-slot starter-move table. Slots
    /// 1/3 start on BITE, slots 2/4 on WRIGGLE (Python
    /// `shared.py:200`). Bead `sts2-rs-v9zv`.
    DenseVegetationWriggler {
        slot: crate::monsters::shared::DenseVegetationWrigglerSlot,
    },
    /// A `Wriggler` with C#'s `StartStunned` set explicitly: `true` opens on
    /// SPAWNED_MOVE (an Infested spawn), `false` on its slot's move
    /// (`DenseVegetationEventEncounter`). sts2-agent fork.
    Wriggler {
        slot: crate::monsters::shared::DenseVegetationWrigglerSlot,
        start_stunned: bool,
    },
    /// `PunchConstruct`'s `starts_with_fast_punch` flag selects the opener
    /// (READY/Defend if false; FAST_PUNCH if true). v0.107.1 renamed the
    /// C# `StartsWithStrongPunch` → `StartsWithFastPunch` (decomp
    /// `v0.107.1/…/PunchConstruct.cs:45`); `PunchOffEventEncounter` sets it
    /// true for the first construct. Bead `…wb8lk.11`.
    PunchConstruct { starts_with_fast_punch: bool },
    // ── Act 1 alternate (Underdocks) — GH #544 ──
    /// `CorpseSlug`'s `StarterMoveIdx`, which `% 3` selects the opener
    /// (0=WHIP_SLAP, 1=GLOMP, 2=GOOP). The Corpse Slug encounters roll ONE
    /// `NextInt(3)` and walk consecutive indices from it
    /// (`CorpseSlug.EnsureCorpseSlugsStartWithDifferentMoves`), so the slugs
    /// in one fight always open on different moves.
    CorpseSlug { starter_idx: u8 },
    /// `TwoTailedRat`'s `StarterMoveIndex` slot. `TwoTailedRatsNormal` rolls
    /// one `NextInt(3)` and hands the three rats `n`, `n+1`, `n+2` (mod 3);
    /// mid-combat summons carry `-1` (`Other` → RAND).
    TwoTailedRat { slot: TwoTailedRatSlot },
}

/// Generic positional context for a monster in its encounter group.
/// Independent of kind; populated by `Scenario::from_monsters` from
/// the monster's index in the `Vec<MonsterSpec>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpawnPosition {
    /// `monsters.len() == 1`.
    pub alone: bool,
    /// `index == 0`.
    pub front: bool,
    /// `index == monsters.len() - 1` (and not also front in a
    /// 1-monster group).
    pub back: bool,
    /// Zero-indexed position in the encounter `Vec<MonsterSpec>`.
    pub index: u8,
}

/// Build the [`MonsterAi`] for `kind`, given structured per-spawn args
/// and positional context. Total over the 12 current
/// [`MonsterKind`] variants — Phase 2 wiring beads add the remaining
/// 22 Act-1 monsters to the match below.
///
/// # Panics
///
/// The catch-all arm panics on any `MonsterKind` variant not yet wired
/// here. `#[non_exhaustive]` on `MonsterKind` forces the catch-all;
/// the match above is exhaustive over today's roster, so the panic is
/// unreachable on the Phase-1 commit.
#[must_use]
pub fn build_ai(kind: MonsterKind, args: MonsterSpawnArgs, position: SpawnPosition) -> MonsterAi {
    // Ascension-0 convenience for the test/fixture callers that don't
    // model ascension. Real combat-entry paths (run_bridge, PyO3,
    // scenario, corpus-gen) call [`build_ai_with_ascension`] so monster
    // attack damage scales per C# `GetValueIfAscension(DeadlyEnemies, …)`.
    build_ai_with_ascension(kind, args, position, 0)
}

/// Ascension-aware [`build_ai`]. `ascension` feeds the C#
/// `GetValueIfAscension(DeadlyEnemies, …)` attack-damage branch (active
/// at ascension >= 9). Only factories already wired for ascension consume
/// it (FuzzyWurmCrawler today); the rest ignore it pending the
/// per-monster fan-out.
#[must_use]
pub fn build_ai_with_ascension(
    kind: MonsterKind,
    args: MonsterSpawnArgs,
    position: SpawnPosition,
    ascension: u8,
) -> MonsterAi {
    let mut ai = match kind {
        MonsterKind::Nibbit => {
            let (is_alone, is_front) = match args {
                MonsterSpawnArgs::Nibbit { is_alone, is_front } => (is_alone, is_front),
                _ => (position.alone, position.front),
            };
            create_nibbit_ai(is_alone, is_front)
        }
        MonsterKind::ShrinkerBeetle => create_shrinker_beetle_ai(),
        MonsterKind::FuzzyWurmCrawler => create_fuzzy_wurm_crawler_ai(),
        MonsterKind::LeafSlimeS => create_leaf_slime_s_ai(),
        MonsterKind::TwigSlimeS => create_twig_slime_s_ai(),
        MonsterKind::LeafSlimeM => create_leaf_slime_m_ai(),
        MonsterKind::TwigSlimeM => create_twig_slime_m_ai(),
        // ── Act 1 — beads sts2-rs-h37x / mv7c / ckj8 ──
        MonsterKind::CubexConstruct => create_cubex_construct_ai(),
        MonsterKind::Flyconid => create_flyconid_ai(),
        MonsterKind::EyeWithTeeth => create_eye_with_teeth_ai(),
        MonsterKind::Fogmog => create_fogmog_ai(),
        MonsterKind::Inklet => {
            let slot = match args {
                MonsterSpawnArgs::Inklet { slot } => slot,
                _ => match position.index {
                    0 => InkletSlot::First,
                    1 => InkletSlot::Second,
                    _ => InkletSlot::Other,
                },
            };
            create_inklet_ai(slot)
        }
        MonsterKind::Mawler => create_mawler_ai(),
        MonsterKind::VineShambler => create_vine_shambler_ai(),
        MonsterKind::SlitheringStrangler => create_slithering_strangler_ai(),
        MonsterKind::SnappingJaxfruit => create_snapping_jaxfruit_ai(),
        MonsterKind::AssassinRubyRaider => create_assassin_ruby_raider_ai(),
        MonsterKind::AxeRubyRaider => create_axe_ruby_raider_ai(),
        MonsterKind::BruteRubyRaider => create_brute_ruby_raider_ai(),
        MonsterKind::CrossbowRubyRaider => create_crossbow_ruby_raider_ai(),
        MonsterKind::TrackerRubyRaider => create_tracker_ruby_raider_ai(),
        MonsterKind::BygoneEffigy => create_bygone_effigy_ai(),
        MonsterKind::Byrdonis => create_byrdonis_ai(),
        MonsterKind::PhrogParasite => create_phrog_parasite_ai(),
        MonsterKind::Parafright => create_parafright_ai(),
        MonsterKind::Vantom => create_vantom_ai(),
        MonsterKind::CeremonialBeast => create_ceremonial_beast_ai(),
        MonsterKind::KinPriest => create_kin_priest_ai(),
        MonsterKind::KinFollower => {
            let slot = match args {
                MonsterSpawnArgs::KinFollower { slot } => slot,
                _ => match position.index {
                    0 => KinFollowerSlot::First,
                    1 => KinFollowerSlot::Second,
                    _ => KinFollowerSlot::Third,
                },
            };
            create_kin_follower_ai(slot)
        }
        // ── Act 2 minions (incremental — full act2 port lives in the
        // `78ks`/`ux2-*` umbrella). Wriggler is wired here because it
        // spawns from `PhrogParasite`'s INFEST move and otherwise
        // breaks the hint adapter on mid-combat states.
        // The slot decides where SPAWNED_MOVE goes after the spawn-turn stun
        // (slots 2/4 to WRIGGLE, 1/3 to NASTY_BITE — `Wriggler.cs`), so a
        // slot-blind rebuild sends every wriggler to NASTY_BITE and the
        // telegraph is wrong for half the roster from the first turn they act.
        // The live spawn path (`act1_overgrowth`'s InfestedPower) builds
        // `create_infested_wriggler_ai` directly; this arm is what a
        // RECONSTRUCTION goes through, which is why it needs the payload.
        MonsterKind::Wriggler => match args {
            MonsterSpawnArgs::DenseVegetationWriggler { slot } => {
                crate::monsters::shared::create_infested_wriggler_ai(slot)
            }
            MonsterSpawnArgs::Wriggler {
                slot,
                start_stunned: true,
            } => crate::monsters::shared::create_infested_wriggler_ai(slot),
            MonsterSpawnArgs::Wriggler {
                slot,
                start_stunned: false,
            } => create_dense_vegetation_wriggler_ai(slot),
            _ => create_wriggler_ai(),
        },
        // ── Act 2 (sts2-rs-6ynt.10) ── factories live in
        // `sts2-content::monsters::act2_hive`. Slot-arg variants pull the
        // explicit `MonsterSpawnArgs::*` payload when present and fall
        // back to `position.index` otherwise (same pattern as Inklet).
        MonsterKind::ThievingHopper => create_thieving_hopper_ai(),
        MonsterKind::Tunneler => create_tunneler_ai(),
        MonsterKind::BowlbugEgg => create_bowlbug_egg_ai(),
        MonsterKind::BowlbugNectar => create_bowlbug_nectar_ai(),
        MonsterKind::BowlbugRock => create_bowlbug_rock_ai(),
        MonsterKind::BowlbugSilk => create_bowlbug_silk_ai(),
        MonsterKind::Exoskeleton => {
            let slot = match args {
                MonsterSpawnArgs::Exoskeleton { slot } => slot,
                _ => match position.index {
                    0 => ExoskeletonSlot::First,
                    1 => ExoskeletonSlot::Second,
                    2 => ExoskeletonSlot::Third,
                    _ => ExoskeletonSlot::Fourth,
                },
            };
            create_exoskeleton_ai(slot)
        }
        MonsterKind::Chomper => {
            let scream_first = match args {
                MonsterSpawnArgs::Chomper { scream_first } => scream_first,
                _ => false,
            };
            create_chomper_ai(scream_first)
        }
        MonsterKind::HunterKiller => create_hunter_killer_ai(),
        MonsterKind::LouseProgenitor => create_louse_progenitor_ai(),
        MonsterKind::Myte => {
            let slot = match args {
                MonsterSpawnArgs::Myte { slot } => slot,
                _ => match position.index {
                    0 => MyteSlot::First,
                    _ => MyteSlot::Second,
                },
            };
            create_myte_ai(slot)
        }
        MonsterKind::ToughEgg => create_tough_egg_ai(),
        MonsterKind::Ovicopter => create_ovicopter_ai(),
        MonsterKind::SlumberingBeetle => create_slumbering_beetle_ai(),
        MonsterKind::SpinyToad => create_spiny_toad_ai(),
        MonsterKind::TheObscura => create_the_obscura_ai(),
        MonsterKind::DecimillipedeSegment => {
            let starter_idx = match args {
                MonsterSpawnArgs::DecimillipedeSegment { starter_idx } => starter_idx,
                _ => position.index,
            };
            create_decimillipede_segment_ai(starter_idx)
        }
        // ── 78ks-slotargs (bead `sts2-rs-v9zv`) ──
        // Decimillipede Front/Middle/Back are visual-only Python
        // wrappers around the shared segment AI (act2.py:501-522).
        // They reuse `MonsterSpawnArgs::DecimillipedeSegment { starter_idx }`
        // since the wrappers accept the same starter_idx parameter
        // (act2.rs:541-555). DenseVegetationWriggler carries the
        // 4-slot starter-move discriminant.
        MonsterKind::DecimillipedeSegmentFront => {
            let starter_idx = match args {
                MonsterSpawnArgs::DecimillipedeSegment { starter_idx } => starter_idx,
                _ => position.index,
            };
            create_decimillipede_segment_front_ai(starter_idx)
        }
        MonsterKind::DecimillipedeSegmentMiddle => {
            let starter_idx = match args {
                MonsterSpawnArgs::DecimillipedeSegment { starter_idx } => starter_idx,
                _ => position.index,
            };
            create_decimillipede_segment_middle_ai(starter_idx)
        }
        MonsterKind::DecimillipedeSegmentBack => {
            let starter_idx = match args {
                MonsterSpawnArgs::DecimillipedeSegment { starter_idx } => starter_idx,
                _ => position.index,
            };
            create_decimillipede_segment_back_ai(starter_idx)
        }
        MonsterKind::DenseVegetationWriggler => {
            use crate::monsters::shared::DenseVegetationWrigglerSlot;
            let slot = match args {
                MonsterSpawnArgs::DenseVegetationWriggler { slot } => slot,
                _ => match position.index {
                    0 => DenseVegetationWrigglerSlot::First,
                    1 => DenseVegetationWrigglerSlot::Second,
                    2 => DenseVegetationWrigglerSlot::Third,
                    _ => DenseVegetationWrigglerSlot::Fourth,
                },
            };
            create_dense_vegetation_wriggler_ai(slot)
        }
        MonsterKind::Entomancer => create_entomancer_ai(),
        MonsterKind::InfestedPrism => {
            let slot = match args {
                MonsterSpawnArgs::InfestedPrism { slot } => slot,
                _ => match position.index {
                    0 => InfestedPrismSlot::First,
                    1 => InfestedPrismSlot::Second,
                    _ => InfestedPrismSlot::Other,
                },
            };
            create_infested_prism_ai(slot)
        }
        MonsterKind::TheInsatiable => create_the_insatiable_ai(),
        MonsterKind::KnowledgeDemon => create_knowledge_demon_ai(),
        MonsterKind::Crusher => create_crusher_ai(),
        MonsterKind::Rocket => create_rocket_ai(),
        // ── Act 3 (sts2-rs-6ynt.10) ──
        MonsterKind::DevotedSculptor => create_devoted_sculptor_ai(),
        MonsterKind::ScrollOfBiting => {
            let starter_idx = match args {
                MonsterSpawnArgs::ScrollOfBiting { starter_idx } => starter_idx,
                _ => position.index,
            };
            create_scroll_of_biting_ai(starter_idx)
        }
        MonsterKind::TurretOperator => create_turret_operator_ai(),
        MonsterKind::Axebot => {
            let start_with_boot_up = match args {
                MonsterSpawnArgs::Axebot { start_with_boot_up } => start_with_boot_up,
                _ => false,
            };
            create_axebot_ai(start_with_boot_up)
        }
        MonsterKind::Zapbot => create_zapbot_ai(),
        MonsterKind::Stabbot => create_stabbot_ai(),
        MonsterKind::Guardbot => create_guardbot_ai(),
        MonsterKind::Noisebot => create_noisebot_ai(),
        MonsterKind::Fabricator => create_fabricator_ai(),
        MonsterKind::FrogKnight => create_frog_knight_ai(),
        MonsterKind::GlobeHead => create_globe_head_ai(),
        MonsterKind::OwlMagistrate => create_owl_magistrate_ai(),
        MonsterKind::SlimedBerserker => create_slimed_berserker_ai(),
        MonsterKind::TheLost => create_the_lost_ai(),
        MonsterKind::TheForgotten => create_the_forgotten_ai(),
        MonsterKind::FlailKnight => create_flail_knight_ai(),
        MonsterKind::MysteriousKnight => create_mysterious_knight_ai(),
        MonsterKind::LivingShield => create_living_shield_ai(),
        MonsterKind::MagiKnight => create_magi_knight_ai(),
        MonsterKind::SpectralKnight => create_spectral_knight_ai(),
        MonsterKind::MechaKnight => create_mecha_knight_ai(),
        MonsterKind::Osty => create_osty_ai(),
        MonsterKind::SoulNexus => create_soul_nexus_ai(),
        MonsterKind::Aeonglass => create_aeonglass_ai(),
        MonsterKind::Queen => create_queen_ai(),
        MonsterKind::TestSubject => create_test_subject_ai(),
        // ── Shared (sts2-rs-qz5p / parent sts2-rs-78ks) ──
        MonsterKind::Architect => create_architect_ai(),
        MonsterKind::Byrdpip => create_byrdpip_ai(),
        MonsterKind::PaelsLegion => create_paels_legion_ai(),
        MonsterKind::FakeMerchantMonster => create_fake_merchant_monster_ai(),
        MonsterKind::TorchHeadAmalgam => create_torch_head_amalgam_ai(),
        MonsterKind::TheAdversaryMkOne => create_the_adversary_mk_one_ai(),
        MonsterKind::TheAdversaryMkTwo => create_the_adversary_mk_two_ai(),
        MonsterKind::TheAdversaryMkThree => create_the_adversary_mk_three_ai(),
        MonsterKind::BattleFriendV1 => create_battle_friend_v1_ai(),
        MonsterKind::BattleFriendV2 => create_battle_friend_v2_ai(),
        MonsterKind::BattleFriendV3 => create_battle_friend_v3_ai(),
        MonsterKind::BigDummy => create_big_dummy_ai(),
        MonsterKind::OneHpMonster => create_one_hp_monster_ai(),
        MonsterKind::TenHpMonster => create_ten_hp_monster_ai(),
        MonsterKind::SingleAttackMoveMonster => create_single_attack_move_monster_ai(),
        MonsterKind::MultiAttackMoveMonster => create_multi_attack_move_monster_ai(),
        // ── act4 (sts2-rs-551m) ──
        MonsterKind::PunchConstruct => {
            let starts_with_fast_punch = match args {
                MonsterSpawnArgs::PunchConstruct {
                    starts_with_fast_punch,
                } => starts_with_fast_punch,
                _ => false,
            };
            create_punch_construct_ai(starts_with_fast_punch)
        }
        // ── act4 bulk wiring (sts2-rs-y14x) — slot-arg factories
        //    default to the first slot since `MonsterSpawnArgs` was
        //    not extended for these (per bead scope). ──
        // C# `CorpseSlug.StarterMoveIdx` — set by the encounter's one-read
        // rotation. Defaults to 0 for spawn paths with no args (scenario
        // fixtures, mid-combat adds), matching a bare `ModelDb.Monster<>`.
        MonsterKind::CorpseSlug => create_corpse_slug_ai(match args {
            MonsterSpawnArgs::CorpseSlug { starter_idx } => starter_idx,
            _ => 0,
        }),
        MonsterKind::Seapunk => create_seapunk_ai(),
        MonsterKind::SludgeSpinner => create_sludge_spinner_ai(),
        // C# `ToadpolesWeak` spawns two Toadpoles: index 0 `IsFront = true`
        // (starts SPIKEN), index 1 `IsFront = false` (starts WHIRL). Map the
        // spawn slot onto the IsFront conditional-branch init (same per-slot
        // dispatch as TwoTailedRat / Inklet). Bead `sts2-rs-pn8mr`.
        MonsterKind::Toadpole => create_toadpole_ai(match position.index {
            0 => ToadpoleSlot::First,
            1 => ToadpoleSlot::Second,
            _ => ToadpoleSlot::Other,
        }),
        MonsterKind::CalcifiedCultist => create_calcified_cultist_ai(),
        MonsterKind::DampCultist => create_damp_cultist_ai(),
        MonsterKind::FossilStalker => create_fossil_stalker_ai(),
        MonsterKind::SneakyGremlin => create_sneaky_gremlin_ai(),
        MonsterKind::FatGremlin => create_fat_gremlin_ai(),
        MonsterKind::GremlinMerc => create_gremlin_merc_ai(),
        MonsterKind::HauntedShip => create_haunted_ship_ai(),
        MonsterKind::GasBomb => create_gas_bomb_ai(),
        MonsterKind::LivingFog => create_living_fog_ai(),
        MonsterKind::SewerClam => create_sewer_clam_ai(),
        // C# `TwoTailedRatsNormal` gives its three rats one of each
        // concrete starter (SCRATCH / DISEASE_BITE / SCREECH); map the
        // member's slot index onto C#'s `StarterMoveIndex` rotation so the
        // openers stay distinct (same per-slot dispatch as KinFollower /
        // Inklet). Index > 2 (no such initial slot exists) falls back to a
        // random opener. Bead `sts2-rs-dvude`.
        // Explicit args carry the encounter's rolled rotation offset (GH
        // #544); the positional fallback keeps the pre-existing fixed
        // 0/1/2 assignment for spawn paths that pass none.
        MonsterKind::TwoTailedRat => create_two_tailed_rat_ai(match args {
            MonsterSpawnArgs::TwoTailedRat { slot } => slot,
            _ => match position.index {
                0 => TwoTailedRatSlot::First,
                1 => TwoTailedRatSlot::Second,
                2 => TwoTailedRatSlot::Third,
                _ => TwoTailedRatSlot::Other,
            },
        }),
        // C# `PhantasmalGardenersElite` seeds 4 gardeners at slots
        // first/second/third/fourth, each entering the BITE→LASH→FLAIL→
        // ENLARGE ring at a distinct phase. Map `position.index` onto C#'s
        // SlotName→entry mapping (first→FLAIL, …). Bead `sts2-rs-dvude`.
        MonsterKind::PhantasmalGardener => create_phantasmal_gardener_ai(match position.index {
            0 => PhantasmalGardenerSlot::First,
            1 => PhantasmalGardenerSlot::Second,
            2 => PhantasmalGardenerSlot::Third,
            _ => PhantasmalGardenerSlot::Fourth,
        }),
        MonsterKind::SkulkingColony => create_skulking_colony_ai(),
        MonsterKind::TerrorEel => create_terror_eel_ai(),
        MonsterKind::WaterfallGiant => create_waterfall_giant_ai(),
        MonsterKind::SoulFysh => create_soul_fysh_ai(),
        MonsterKind::LagavulinMatriarch => create_lagavulin_matriarch_ai(),
        // `MonsterKind` is `#[non_exhaustive]`; this arm is unreachable
        // on the current commit but required for the match to compile.
        // Future Acts 2/3/4 wiring beads extend the arms above.
        #[allow(unreachable_patterns)]
        other => panic!(
            "build_ai: MonsterKind::{other:?} is not yet wired — add a row in \
             crates/sts2-content/src/monster_dispatch.rs"
        ),
    };
    // Post-build: bump scaled attack damages to their DeadlyEnemies
    // values at ascension >= 9 (data table in `ascension_scaling`).
    crate::ascension_scaling::apply_ascension_attack_scaling(&mut ai, kind, ascension);
    ai
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every wired `MonsterKind` must produce a `MonsterAi` whose current
    /// move telegraphs exactly what C# telegraphs for it. Guards against a
    /// Phase-2 wiring bead adding an enum row without a matching dispatch
    /// arm — an unwired kind falls back to an intent-less AI, which is why
    /// "has ≥1 intent" is the signal for all but the few creatures C# builds
    /// with no intents at all (see `SILENT_KINDS`).
    #[test]
    fn build_ai_yields_nonempty_move_set_for_every_current_kind() {
        // Plain kinds with `MonsterSpawnArgs::None`. Covers every
        // current `MonsterKind` variant except the three with
        // structured spawn args (Nibbit / Inklet / KinFollower)
        // — those have dedicated cases below.
        let plain_kinds = [
            MonsterKind::ShrinkerBeetle,
            MonsterKind::FuzzyWurmCrawler,
            MonsterKind::LeafSlimeS,
            MonsterKind::TwigSlimeS,
            MonsterKind::LeafSlimeM,
            MonsterKind::TwigSlimeM,
            // Act 1 — beads sts2-rs-h37x / mv7c / ckj8
            MonsterKind::CubexConstruct,
            MonsterKind::Flyconid,
            MonsterKind::EyeWithTeeth,
            MonsterKind::Fogmog,
            MonsterKind::Mawler,
            MonsterKind::VineShambler,
            MonsterKind::SlitheringStrangler,
            MonsterKind::SnappingJaxfruit,
            MonsterKind::AssassinRubyRaider,
            MonsterKind::AxeRubyRaider,
            MonsterKind::BruteRubyRaider,
            MonsterKind::CrossbowRubyRaider,
            MonsterKind::TrackerRubyRaider,
            MonsterKind::BygoneEffigy,
            MonsterKind::Byrdonis,
            MonsterKind::PhrogParasite,
            MonsterKind::Parafright,
            MonsterKind::Vantom,
            MonsterKind::CeremonialBeast,
            MonsterKind::KinPriest,
            MonsterKind::Wriggler,
            // ── Shared (sts2-rs-qz5p) ── no-arg factories.
            MonsterKind::Architect,
            MonsterKind::FakeMerchantMonster,
            MonsterKind::TorchHeadAmalgam,
            MonsterKind::TheAdversaryMkOne,
            MonsterKind::TheAdversaryMkTwo,
            MonsterKind::TheAdversaryMkThree,
            MonsterKind::BigDummy,
            MonsterKind::OneHpMonster,
            MonsterKind::TenHpMonster,
            MonsterKind::SingleAttackMoveMonster,
            MonsterKind::MultiAttackMoveMonster,
        ];
        for kind in plain_kinds {
            let ai = build_ai(kind, MonsterSpawnArgs::None, SpawnPosition::default());
            assert!(
                !ai.current_move().intents.is_empty(),
                "build_ai({kind:?}) produced empty intents"
            );
        }

        // The other direction, and just as load-bearing: these five are the
        // only wired kinds whose C# move is built with no
        // `params AbstractIntent[]` (`new MoveState("NOTHING_MOVE", _ =>
        // Task.CompletedTask)` — `Byrdpip.cs:28`, `PaelsLegion.cs:35`,
        // `BattleFriendV{1,2,3}.cs:28`). Asserting they stay silent stops the
        // fabricated-`Intent::Unknown` port artifact from creeping back;
        // `Architect` / `BigDummy` / `OneHpMonster` / `TenHpMonster` look
        // like them but pass `new HiddenIntent()`, so they belong above.
        for kind in [
            MonsterKind::Byrdpip,
            MonsterKind::PaelsLegion,
            MonsterKind::BattleFriendV1,
            MonsterKind::BattleFriendV2,
            MonsterKind::BattleFriendV3,
        ] {
            let ai = build_ai(kind, MonsterSpawnArgs::None, SpawnPosition::default());
            assert!(
                ai.current_move().intents.is_empty(),
                "build_ai({kind:?}) telegraphs {:?}; C# gives NOTHING_MOVE no intents",
                ai.current_move().intents
            );
        }

        // Nibbit: explicit-args win over position; position is used
        // when args are None.
        let nibbit_explicit = build_ai(
            MonsterKind::Nibbit,
            MonsterSpawnArgs::Nibbit {
                is_alone: true,
                is_front: false,
            },
            SpawnPosition::default(),
        );
        assert!(!nibbit_explicit.current_move().intents.is_empty());
        let nibbit_from_position = build_ai(
            MonsterKind::Nibbit,
            MonsterSpawnArgs::None,
            SpawnPosition {
                alone: true,
                front: true,
                back: false,
                index: 0,
            },
        );
        assert!(!nibbit_from_position.current_move().intents.is_empty());

        // Inklet + KinFollower: both slot variants + position fallback.
        for slot in [InkletSlot::First, InkletSlot::Second, InkletSlot::Other] {
            let ai = build_ai(
                MonsterKind::Inklet,
                MonsterSpawnArgs::Inklet { slot },
                SpawnPosition::default(),
            );
            assert!(!ai.current_move().intents.is_empty());
        }
        for slot in [
            KinFollowerSlot::First,
            KinFollowerSlot::Second,
            KinFollowerSlot::Third,
        ] {
            let ai = build_ai(
                MonsterKind::KinFollower,
                MonsterSpawnArgs::KinFollower { slot },
                SpawnPosition::default(),
            );
            assert!(!ai.current_move().intents.is_empty());
        }
        // Position fallback for slot-arg kinds — exercises the
        // `position.index` → slot mapping in the `MonsterSpawnArgs::None`
        // arms.
        for idx in 0..3 {
            let inklet = build_ai(
                MonsterKind::Inklet,
                MonsterSpawnArgs::None,
                SpawnPosition {
                    alone: false,
                    front: idx == 0,
                    back: idx == 2,
                    index: idx,
                },
            );
            assert!(!inklet.current_move().intents.is_empty());
            let kf = build_ai(
                MonsterKind::KinFollower,
                MonsterSpawnArgs::None,
                SpawnPosition {
                    alone: false,
                    front: idx == 0,
                    back: idx == 2,
                    index: idx,
                },
            );
            assert!(!kf.current_move().intents.is_empty());
        }
    }

    /// Bead `sts2-rs-dvude`: the three `two_tailed_rats_normal` rats get
    /// one of each concrete starter via `position.index` — SCRATCH /
    /// DISEASE_BITE / SCREECH — exactly as C# `TwoTailedRatsNormal`
    /// rotates `StarterMoveIndex` across its members. No two rats open on
    /// the same move.
    #[test]
    fn two_tailed_rat_position_index_selects_distinct_starters() {
        let starter = |idx: u8| {
            build_ai(
                MonsterKind::TwoTailedRat,
                MonsterSpawnArgs::None,
                SpawnPosition {
                    alone: false,
                    front: idx == 0,
                    back: idx == 2,
                    index: idx,
                },
            )
            .current_move()
            .id
        };
        assert_eq!(starter(0), "SCRATCH");
        assert_eq!(starter(1), "DISEASE_BITE");
        assert_eq!(starter(2), "SCREECH");
    }

    /// Inklet's slot enum routes to a distinct starting move id per
    /// `InkletSlot` variant — pins the args-take-precedence contract
    /// for slot-arg monsters introduced in Phase 2.2 (`sts2-rs-mv7c`).
    #[test]
    fn inklet_slot_argument_selects_starting_move() {
        let first = build_ai(
            MonsterKind::Inklet,
            MonsterSpawnArgs::Inklet {
                slot: InkletSlot::First,
            },
            SpawnPosition::default(),
        );
        let second = build_ai(
            MonsterKind::Inklet,
            MonsterSpawnArgs::Inklet {
                slot: InkletSlot::Second,
            },
            SpawnPosition::default(),
        );
        let other = build_ai(
            MonsterKind::Inklet,
            MonsterSpawnArgs::Inklet {
                slot: InkletSlot::Other,
            },
            SpawnPosition::default(),
        );
        // C# `_middleInklet ? WHIRLWIND : JAB`: only the middle Inklet
        // (index 1 → InkletSlot::Second) opens with WHIRLWIND; all other
        // slots open with the forced JAB (no turn-0 monster_ai roll).
        assert_eq!(first.current_move().id, "JAB");
        assert_eq!(second.current_move().id, "WHIRLWIND");
        assert_eq!(other.current_move().id, "JAB");
    }

    /// `KinFollower::Third` starts on BITE; First/Second start on BASH.
    /// Phase 2.2 slot-arg contract.
    #[test]
    fn kin_follower_slot_argument_selects_starting_move() {
        let first = build_ai(
            MonsterKind::KinFollower,
            MonsterSpawnArgs::KinFollower {
                slot: KinFollowerSlot::First,
            },
            SpawnPosition::default(),
        );
        let third = build_ai(
            MonsterKind::KinFollower,
            MonsterSpawnArgs::KinFollower {
                slot: KinFollowerSlot::Third,
            },
            SpawnPosition::default(),
        );
        assert_eq!(first.current_move().id, "QUICK_SLASH");
        assert_eq!(third.current_move().id, "POWER_DANCE");
    }

    /// Nibbit's `is_alone` + `is_front` route through three distinct
    /// move-chain initials per the Phase-5 DSL factory contract:
    /// alone → BUTT; in-front (in a group) → SLICE; otherwise HISS.
    /// Pins both arms of the args-vs-position fallback.
    #[test]
    fn nibbit_args_select_move_chain_and_position_is_fallback() {
        // Explicit args.
        let alone = build_ai(
            MonsterKind::Nibbit,
            MonsterSpawnArgs::Nibbit {
                is_alone: true,
                is_front: false,
            },
            SpawnPosition::default(),
        );
        assert_eq!(alone.current_move().id, "BUTT");

        let front = build_ai(
            MonsterKind::Nibbit,
            MonsterSpawnArgs::Nibbit {
                is_alone: false,
                is_front: true,
            },
            SpawnPosition::default(),
        );
        assert_eq!(front.current_move().id, "SLICE");

        let back = build_ai(
            MonsterKind::Nibbit,
            MonsterSpawnArgs::Nibbit {
                is_alone: false,
                is_front: false,
            },
            SpawnPosition::default(),
        );
        assert_eq!(back.current_move().id, "HISS");

        // Position-based fallback when args are `None`.
        let fallback_alone = build_ai(
            MonsterKind::Nibbit,
            MonsterSpawnArgs::None,
            SpawnPosition {
                alone: true,
                front: true,
                back: false,
                index: 0,
            },
        );
        assert_eq!(fallback_alone.current_move().id, "BUTT");

        let fallback_front = build_ai(
            MonsterKind::Nibbit,
            MonsterSpawnArgs::None,
            SpawnPosition {
                alone: false,
                front: true,
                back: false,
                index: 0,
            },
        );
        assert_eq!(fallback_front.current_move().id, "SLICE");
    }

    /// Bead `sts2-rs-v9zv`: Decimillipede Front/Middle/Back are visual
    /// wrappers around the shared segment AI; explicit
    /// `MonsterSpawnArgs::DecimillipedeSegment { starter_idx }` selects
    /// the initial move (0=WRITHE, 1=BULK, 2=CONSTRICT).
    #[test]
    fn decimillipede_segment_wrappers_honour_starter_idx() {
        for (idx, expected_move) in [(0, "WRITHE"), (1, "BULK"), (2, "CONSTRICT")] {
            for kind in [
                MonsterKind::DecimillipedeSegmentFront,
                MonsterKind::DecimillipedeSegmentMiddle,
                MonsterKind::DecimillipedeSegmentBack,
            ] {
                let ai = build_ai(
                    kind,
                    MonsterSpawnArgs::DecimillipedeSegment { starter_idx: idx },
                    SpawnPosition::default(),
                );
                assert_eq!(
                    ai.current_move().id,
                    expected_move,
                    "kind={kind:?} starter_idx={idx} expected {expected_move}",
                );
            }
        }
    }

    /// Bead `sts2-rs-v9zv`: `DenseVegetationWriggler`'s 4-slot table
    /// (shared.py:200): slots 1/3 → BITE, slots 2/4 → WRIGGLE.
    #[test]
    fn dense_vegetation_wriggler_slot_selects_starter_move() {
        use crate::monsters::shared::DenseVegetationWrigglerSlot;
        for (slot, expected) in [
            (DenseVegetationWrigglerSlot::First, "NASTY_BITE"),
            (DenseVegetationWrigglerSlot::Second, "WRIGGLE"),
            (DenseVegetationWrigglerSlot::Third, "NASTY_BITE"),
            (DenseVegetationWrigglerSlot::Fourth, "WRIGGLE"),
        ] {
            let ai = build_ai(
                MonsterKind::DenseVegetationWriggler,
                MonsterSpawnArgs::DenseVegetationWriggler { slot },
                SpawnPosition::default(),
            );
            assert_eq!(
                ai.current_move().id,
                expected,
                "slot={slot:?} expected {expected}"
            );
        }
    }

    /// Bead `sts2-rs-d7j`: `CombatState::summon_monster` is the
    /// engine-side primitive for cards / relics that add a hostile
    /// creature mid-combat. Mirrors Python's `combat.add_enemy(creature,
    /// ai)`. This integration test wires it through the content-side
    /// `build_ai` dispatch — the canonical path callers take — and
    /// verifies the resulting state shape: a fresh creature in the
    /// arena, a registered enemy uid, and a non-empty AI move set on
    /// the spawn (i.e. the summoned monster is ready to act).
    #[test]
    fn summon_monster_appends_to_creatures_and_enemy_uids() {
        use sts2_core::CombatState;

        let mut state = CombatState::default();
        // Player scalars must be self-consistent for `assert_invariants`
        // (max_hp > 0, current_hp ≤ max_hp). The summon path itself is
        // independent of player HP; this just satisfies the invariant
        // checker the test calls below.
        state.player.max_hp = 80;
        state.player.current_hp = 80;
        // Pre-seed one enemy so we can verify slot-1 append (vs slot-0
        // overwrite). Use the same dispatch path the call site exercises.
        let pre_ai = build_ai(
            MonsterKind::Nibbit,
            MonsterSpawnArgs::Nibbit {
                is_alone: true,
                is_front: true,
            },
            SpawnPosition::default(),
        );
        let pre_uid = state.summon_monster(MonsterKind::Nibbit, 12, pre_ai);

        // Mid-combat summon — content-side dispatch, then push through
        // the core primitive.
        let ai = build_ai(
            MonsterKind::ShrinkerBeetle,
            MonsterSpawnArgs::None,
            SpawnPosition::default(),
        );
        let summoned = state.summon_monster(MonsterKind::ShrinkerBeetle, 30, ai);

        // Distinct uids; both registered.
        assert_ne!(pre_uid, summoned, "summon allocates a fresh uid");
        assert_eq!(state.creatures.len(), 2);
        assert_eq!(state.enemy_uids.len(), 2);
        assert_eq!(state.enemy_uids[1], summoned, "summoned uid is appended");

        // Creature shape: full HP, species, AI attached.
        let c = state
            .creatures
            .iter()
            .find(|c| c.id == summoned.0)
            .expect("summoned creature present in arena");
        assert_eq!(c.current_hp, 30);
        assert_eq!(c.max_hp, 30);
        assert_eq!(c.block, 0);
        assert_eq!(c.species, Some(MonsterKind::ShrinkerBeetle));
        let ai = c.ai.as_ref().expect("summoned creature has AI");
        assert!(
            !ai.current_move().intents.is_empty(),
            "summoned creature's starting move has at least one intent"
        );

        // Structural invariants hold post-summon.
        state
            .assert_invariants()
            .expect("summon_monster preserves structural invariants");
    }

    // ──────────────────────────────────────────────────────────────────
    // Slot-arg dispatch: explicit `MonsterSpawnArgs::*` payload wins over
    // the `position.index` fallback, and the `position.index → slot`
    // tables map each index onto a distinct starting move. These pin the
    // per-arm branches in `build_ai_with_ascension` against
    // arm-deletion mutants.
    // ──────────────────────────────────────────────────────────────────

    /// Helper: starting-move id for `kind` built from explicit args with a
    /// default (index-0) position — exercises the `MonsterSpawnArgs::*`
    /// arm, NOT the position fallback.
    fn move_from_args(kind: MonsterKind, args: MonsterSpawnArgs) -> &'static str {
        build_ai(kind, args, SpawnPosition::default())
            .current_move()
            .id
    }

    /// Helper: starting-move id for `kind` built from `MonsterSpawnArgs::None`
    /// at encounter `index` — exercises the `position.index → slot` fallback.
    fn move_from_index(kind: MonsterKind, index: u8) -> &'static str {
        build_ai(
            kind,
            MonsterSpawnArgs::None,
            SpawnPosition {
                alone: false,
                front: index == 0,
                back: false,
                index,
            },
        )
        .current_move()
        .id
    }

    /// Explicit `MonsterSpawnArgs::*` payloads must take precedence over the
    /// `_ => position`/default fallback. Each case picks a payload whose
    /// resulting move id differs from what the fallback would produce, so
    /// deleting the explicit arm flips the assertion.
    #[test]
    fn explicit_spawn_args_override_position_default() {
        // Exoskeleton: args Third → ENRAGE; default (index 0) → First → SKITTER.
        assert_eq!(
            move_from_args(
                MonsterKind::Exoskeleton,
                MonsterSpawnArgs::Exoskeleton {
                    slot: ExoskeletonSlot::Third,
                },
            ),
            "ENRAGE",
        );
        // Chomper: args scream_first → SCREECH; default → CLAMP.
        assert_eq!(
            move_from_args(
                MonsterKind::Chomper,
                MonsterSpawnArgs::Chomper { scream_first: true },
            ),
            "SCREECH",
        );
        // Myte: args Second → SUCK_MOVE; default (index 0) → First → TOXIC_MOVE.
        assert_eq!(
            move_from_args(
                MonsterKind::Myte,
                MonsterSpawnArgs::Myte {
                    slot: MyteSlot::Second,
                },
            ),
            "SUCK_MOVE",
        );
        // DecimillipedeSegment: args starter_idx 1 → BULK; default (index 0) → WRITHE.
        assert_eq!(
            move_from_args(
                MonsterKind::DecimillipedeSegment,
                MonsterSpawnArgs::DecimillipedeSegment { starter_idx: 1 },
            ),
            "BULK",
        );
        // ScrollOfBiting: args starter_idx 1 → CHEW; default (index 0) → CHOMP.
        assert_eq!(
            move_from_args(
                MonsterKind::ScrollOfBiting,
                MonsterSpawnArgs::ScrollOfBiting { starter_idx: 1 },
            ),
            "CHEW",
        );
        // Axebot: args start_with_boot_up → BOOT_UP; default → RAND.
        assert_eq!(
            move_from_args(
                MonsterKind::Axebot,
                MonsterSpawnArgs::Axebot {
                    start_with_boot_up: true,
                },
            ),
            "BOOT_UP",
        );
        // PunchConstruct (v0.107.1): args starts_with_fast_punch → FAST_PUNCH;
        // default → READY.
        assert_eq!(
            move_from_args(
                MonsterKind::PunchConstruct,
                MonsterSpawnArgs::PunchConstruct {
                    starts_with_fast_punch: true,
                },
            ),
            "FAST_PUNCH",
        );
    }

    /// `position.index → slot` fallback tables: each wired index must map to
    /// its own distinct starting move so deleting any index arm (which would
    /// drop through to the `_` catch-all) flips an assertion.
    #[test]
    fn position_index_selects_distinct_slot_starter() {
        // Inklet: index 1 → Second → WHIRLWIND (index 0/other → JAB). Only
        // the Second arm is observable; index 0 vs Other both yield JAB.
        assert_eq!(move_from_index(MonsterKind::Inklet, 1), "WHIRLWIND");

        // KinFollower: index 0/1 → First/Second → QUICK_SLASH; the `_` arm
        // (Third) opens on POWER_DANCE, so dropping either explicit index
        // arm flips the move.
        assert_eq!(move_from_index(MonsterKind::KinFollower, 0), "QUICK_SLASH");
        assert_eq!(move_from_index(MonsterKind::KinFollower, 1), "QUICK_SLASH");
        assert_eq!(move_from_index(MonsterKind::KinFollower, 2), "POWER_DANCE");

        // Exoskeleton: index 0 → SKITTER, index 2 → ENRAGE. The `_` arm
        // (Fourth → "RAND") resolves — via the constructor's fixed throwaway
        // RNG — deterministically to MANDIBLE, so deleting the index-0 arm
        // (SKITTER → MANDIBLE) and the index-2 arm (ENRAGE → MANDIBLE) both
        // flip these. Index 1 (MANDIBLE) coincides with the `_` resolution,
        // so that arm deletion is EQUIVALENT and is not asserted as a kill —
        // the assertion below merely documents the mapping.
        assert_eq!(move_from_index(MonsterKind::Exoskeleton, 0), "SKITTER");
        assert_eq!(move_from_index(MonsterKind::Exoskeleton, 1), "MANDIBLE");
        assert_eq!(move_from_index(MonsterKind::Exoskeleton, 2), "ENRAGE");

        // Myte: index 0 → First → TOXIC_MOVE; `_` → Second → SUCK_MOVE.
        assert_eq!(move_from_index(MonsterKind::Myte, 0), "TOXIC_MOVE");
        assert_eq!(move_from_index(MonsterKind::Myte, 1), "SUCK_MOVE");

        // Toadpole: index 0 → First → SPIKEN_MOVE; others → WHIRL_MOVE.
        assert_eq!(move_from_index(MonsterKind::Toadpole, 0), "SPIKEN_MOVE");
        assert_eq!(move_from_index(MonsterKind::Toadpole, 1), "WHIRL_MOVE");

        // TwoTailedRat: index 2 → Third → SCREECH. The `_` arm (Other →
        // "RAND") resolves — via the constructor's fixed throwaway RNG — to
        // SCREECH as well, so deleting the index-2 arm is EQUIVALENT (this
        // assertion documents the mapping but does not kill that mutant).
        assert_eq!(move_from_index(MonsterKind::TwoTailedRat, 2), "SCREECH");

        // PhantasmalGardener: index 0/1/2 → FLAIL/BITE/LASH; `_` → ENLARGE.
        assert_eq!(move_from_index(MonsterKind::PhantasmalGardener, 0), "FLAIL");
        assert_eq!(move_from_index(MonsterKind::PhantasmalGardener, 1), "BITE");
        assert_eq!(move_from_index(MonsterKind::PhantasmalGardener, 2), "LASH");
        assert_eq!(
            move_from_index(MonsterKind::PhantasmalGardener, 3),
            "ENLARGE"
        );
    }
}
