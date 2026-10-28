//! Monster system (Phase 4 → STS2-native).
//!
//! This module defines the per-monster discriminant ([`MonsterKind`]),
//! the intent surface ([`Intent`] + [`IntentKindTag`]), and the
//! per-creature intent history ([`IntentHistory`]) that the Phase-5
//! monster DSL uses to drive AI state. The original Phase-4 hand-rolled
//! STS1 stubs (`Cultist` / `JawWorm` / `AcidSlimeS` / `GreenLouse` /
//! `Sentry`) were removed in bead `sts2-rs-v1u5` — they were
//! pre-DSL scaffolding for the Phase 4 milestone, not Python-parity
//! enemies. The Python source-of-truth pin
//! (`sts2-rl-agent@01c094d`) ships only STS2-native enemies
//! (Mawler / Nibbit / Byrdonis / Kin / ...), all of which are wired
//! through the Phase-5 DSL via `sts2-content::monsters::*`.
//!
//! The `MonsterKind::roll_intent` arms below are placeholders for
//! call sites that bypass the AI path (e.g. parity / smoke fixtures
//! that pre-set `intent: None`); the canonical path goes through
//! `MonsterAi::current_move()` and `combat_loop`'s intent-priming step.
//!
//! Mirrors source: `sts2-rl-agent@01c094d:sts2_env/monsters/intents.py`
//! (`Intent` + `IntentType`).

use crate::dotnet_rng::Rng as CombatRng;

// ---------------------------------------------------------------------------
// MonsterId — free-form string identifier for parity export
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MonsterId(pub &'static str);

impl From<MonsterKind> for MonsterId {
    fn from(kind: MonsterKind) -> Self {
        Self(kind.python_name())
    }
}

// ---------------------------------------------------------------------------
// MonsterKind — discriminant for STS2-native enemies
// ---------------------------------------------------------------------------

/// Kind of monster. STS2-native enemies wired through the Phase-5 DSL.
///
/// `#[non_exhaustive]` so adding a variant later does not break downstream
/// `match MonsterKind { ... }` arms — they fail to compile only when the
/// match is exhaustive, which is the correctness invariant we want.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MonsterKind {
    /// Nibbit — DSL-driven (BUTT/SLICE/HISS rotation, see
    /// `sts2-content::monsters::act1_weak::create_nibbit_ai`). HP 45.
    /// The hand-rolled `roll_intent` arm is a placeholder
    /// (Attack 12 = BUTT damage) — `combat_loop`'s intent-priming step
    /// reads `creature.ai.current_move().intents.first()` whenever a
    /// `MonsterAi` is attached, so this arm is only exercised by
    /// `MonsterKind`-only call sites that skip the AI path.
    Nibbit,
    // ── Cliext H3 (bead `sts2-rs-yioi`) — STS2 act1_weak ────────────
    // Real STS2 monsters from the Python pin's `monsters/act1_weak.py`.
    // DSL-driven; the placeholder `roll_intent` arms below are only
    // exercised by `MonsterKind`-only call sites. The DSL factories
    // already exist in `sts2-content::monsters::act1_weak`; this enum
    // change adds the spawn-side wiring that was missed by the closed
    // P5 umbrella `sts2-rs-ux2` (see follow-up bead `sts2-rs-78ks` for
    // the remaining ~117 unwired DSL factories across acts 1-4 +
    // shared).
    /// Shrinker Beetle — HP 38-40. Python id `SHRINKER_BEETLE`.
    ShrinkerBeetle,
    /// Fuzzy Wurm Crawler — HP 55-57. Python id `FUZZY_WURM_CRAWLER`.
    FuzzyWurmCrawler,
    /// Leaf Slime (Small) — HP 11-15. Python id `LEAF_SLIME_S`.
    LeafSlimeS,
    /// Twig Slime (Small) — HP 7-11. Python id `TWIG_SLIME_S`.
    TwigSlimeS,
    /// Leaf Slime (Medium) — HP 32-35. Python id `LEAF_SLIME_M`.
    LeafSlimeM,
    /// Twig Slime (Medium) — HP 26-28. Python id `TWIG_SLIME_M`.
    TwigSlimeM,
    // ── Act 1 — normal encounters (beads sts2-rs-h37x / mv7c / ckj8) ──
    // Wired in the umbrella plan at
    // docs/plans/2026-05-14-expose-all-sts2-enemies-all-ironclad-cards-through-sts2-cli-.md.
    // HP values mirror the Python pin `monsters/act1.py` (fixed values
    // or `rng.next_int(lo, hi)` inclusive). All 22 are DSL-driven via
    // `sts2_content::monsters::act1_overgrowth::create_*_ai`; the `roll_intent`
    // arms below are placeholders for `MonsterKind`-only call sites
    // (same pattern as Nibbit + act1_weak monsters).
    /// `CubexConstruct` — HP 65. Python id `CUBEX_CONSTRUCT`.
    CubexConstruct,
    /// Flyconid — HP 47-49. Python id `FLYCONID`.
    Flyconid,
    /// `EyeWithTeeth` — HP 6 (minion). Python id `EYE_WITH_TEETH`.
    EyeWithTeeth,
    /// Fogmog — HP 74. Python id `FOGMOG`.
    Fogmog,
    /// Inklet — HP 11-17 per C# decomp `Inklet.cs` v0.103.2 (ToughAsc
    /// 12-18). Python pin `01c094d` ships a stale 30-33 HP variant
    /// (Pattern B sim bug — Python overstated by 2-3× per
    /// `docs/perf/2026-05-18-monster-stat-audit.md`); see beads
    /// `sts2-rs-b60z` / `sts2-rs-kgwj`. Move-set audit (C# JAB / WHIRLWIND
    /// / PIERCING_GAZE vs Rust SPLATTER / SUBMERGE) is a separate
    /// follow-up. Python id `INKLET`. Spawns with an
    /// [`crate::monster_ai`]-side `InkletSlot` arg via
    /// `sts2_content::monster_dispatch::MonsterSpawnArgs::Inklet`.
    Inklet,
    /// Mawler — HP 72. Python id `MAWLER`.
    Mawler,
    /// `VineShambler` — HP 61-64 per C# v0.103.2 (normal 61, ToughAsc 64).
    /// Python pin `01c094d` ships HP 40-43 (-34% Pattern-A divergence).
    /// Bead `sts2-rs-titv` (parent `sts2-rs-kgwj`). Python id `VINE_SHAMBLER`.
    VineShambler,
    /// `SlitheringStrangler` — HP 45-49. Python id `SLITHERING_STRANGLER`.
    SlitheringStrangler,
    /// `SnappingJaxfruit` — HP 53-56. Python id `SNAPPING_JAXFRUIT`.
    SnappingJaxfruit,
    // ── Act 1 — RubyRaider elites (bead sts2-rs-h37x) ──
    /// `RubyRaider` Assassin elite — HP 18-23. Python id `ASSASSIN_RUBY_RAIDER`.
    AssassinRubyRaider,
    /// `RubyRaider` Axe elite — HP 20-22. Python id `AXE_RUBY_RAIDER`.
    AxeRubyRaider,
    /// `RubyRaider` Brute elite — HP 30-33. Python id `BRUTE_RUBY_RAIDER`.
    BruteRubyRaider,
    /// `RubyRaider` Crossbow elite — HP 18-21. Python id `CROSSBOW_RUBY_RAIDER`.
    CrossbowRubyRaider,
    /// `RubyRaider` Tracker elite — HP 21-25. Python id `TRACKER_RUBY_RAIDER`.
    TrackerRubyRaider,
    // ── Act 1 — elites / bosses (bead sts2-rs-ckj8) ──
    /// `BygoneEffigy` elite — HP 127. Python id `BYGONE_EFFIGY`.
    BygoneEffigy,
    /// Byrdonis — HP 91-94. Python id `BYRDONIS`.
    Byrdonis,
    /// `PhrogParasite` — HP 80-83. Python id `PHROG_PARASITE`.
    PhrogParasite,
    /// Parafright — HP 21 per C# decomp `Parafright.cs` (sts2-rs-ksa5
    /// bumped from Python pin 01c094d's HP 1 minion stub). Currently
    /// only spawned by Vantom's CONSUME, but the C# stat block applies
    /// regardless of spawn source.
    Parafright,
    /// Vantom boss — HP 173-183 per C# v0.103.2 (normal 173,
    /// ToughAsc 183; min=max). Python pin `01c094d` ships HP 206
    /// (+19% Pattern divergence; the 2026-05-18 monster-stat audit
    /// rebases sts2-rs to C# canonical). Bead `sts2-rs-hi0z`
    /// (parent `sts2-rs-kgwj`); full AI/moveset rewrite is a
    /// follow-up (Vantom CONSUME/Parafright spawn semantics tie
    /// into act1 minion plumbing). Python id `VANTOM`.
    Vantom,
    /// `CeremonialBeast` boss — HP 252. Python id `CEREMONIAL_BEAST`.
    CeremonialBeast,
    /// `KinPriest` — HP 190 per C# decomp `KinPriest.cs` v0.103.2
    /// (normal 190, ToughAsc 199; min=max). Python pin `01c094d` ships
    /// a stale 119 HP / 2-move (CONVERSION-SMITE) variant we
    /// deliberately diverge from per the monster-stat audit
    /// (`docs/perf/2026-05-18-monster-stat-audit.md`, Pattern A -37%);
    /// see beads `sts2-rs-1jlo` / `sts2-rs-kgwj`. Python id
    /// `KIN_PRIEST`. Spawns alongside two `KinFollower`s in the
    /// canonical `TheKin` encounter.
    KinPriest,
    /// `KinFollower` — HP 65-71. Python id `KIN_FOLLOWER`. Spawns with
    /// a `KinFollowerSlot` arg via
    /// `sts2_content::monster_dispatch::MonsterSpawnArgs::KinFollower`.
    KinFollower,
    // ── Act 2 — minions (incremental wiring; full act2 port is the
    // `78ks`/`ux2-*` umbrella) ──────────────────────────────────────
    /// Wriggler — SPAWNED_MOVE (Stun) → alternating BITE / WRIGGLE.
    /// Python id `WRIGGLER`. HP 17-21 per C# decomp
    /// `MegaCrit.Sts2.Core.Models.Monsters/Wriggler.cs` (Min/MaxInitialHp
    /// 17/21 base). The Python pin at `01c094d` ships an 8-10 HP /
    /// single-BITE stub which is a content bug — see
    /// `docs/perf/2026-05-18-monster-stat-audit.md` and bead
    /// `sts2-rs-n76j`. Surfaces in the hint pipeline because Wrigglers
    /// are spawned mid-combat on an infested enemy's death (bead
    /// `sts2-rs-2u9w1`). (`LouseProgenitor` does NOT summon them — that
    /// was a Python-pin fabrication removed under bead `sts2-rs-ybtmu`.)
    Wriggler,
    // ── Act 2 — Hive (bead `sts2-rs-6ynt.10`) ──
    // HP ranges mirror `sts2_env/monsters/act2.py` at pin 01c094d.
    // Factory functions in `sts2-content::monsters::act2_hive` already
    // exist; this enum + dispatch wiring lets `Scenario::from_monsters`
    // and the parity bridge spawn act2 enemies. DSL-driven; the
    // `roll_intent` arms below are placeholders for `MonsterKind`-only
    // call sites that skip the AI path.
    /// `ThievingHopper` — HP 79-84 per C# decomp
    /// `MegaCrit.Sts2.Core.Models.Monsters/ThievingHopper.cs` v0.103.2
    /// (normal 79, ToughAsc 84; min=max). Python pin `01c094d` ships
    /// a stale 15-18 HP / 1-move MUG variant; bead `sts2-rs-l8yi`
    /// rebaselined the move chain to the C# 5-move sequence and bead
    /// `sts2-rs-ctib` completed the HP bump that was lost in a
    /// sibling-pane race during l8yi. Python id `THIEVING_HOPPER`.
    ThievingHopper,
    /// Tunneler — HP 87-92 per C# decomp `Tunneler.cs` v0.103.2
    /// (normal 87, ToughAsc 92; min=max). Python pin `01c094d` ships a
    /// stale 24-28 HP / 2-move variant we deliberately diverge from per
    /// the monster-stat audit; see beads `sts2-rs-ii8x` /
    /// `sts2-rs-kgwj` and `docs/perf/2026-05-18-monster-stat-audit.md`.
    /// Python id `TUNNELER`.
    Tunneler,
    /// `BowlbugEgg` — HP 21-22. Python id `BOWLBUG_EGG`.
    BowlbugEgg,
    /// `BowlbugNectar` — HP 35-38. Python id `BOWLBUG_NECTAR`.
    BowlbugNectar,
    /// `BowlbugRock` — HP 45-48. Python id `BOWLBUG_ROCK`.
    BowlbugRock,
    /// `BowlbugSilk` — HP 40-43. Python id `BOWLBUG_SILK`.
    BowlbugSilk,
    /// Exoskeleton — HP 24-28. Python id `EXOSKELETON`. Spawns with
    /// an `ExoskeletonSlot` arg via
    /// `MonsterSpawnArgs::Exoskeleton`.
    Exoskeleton,
    /// Chomper — HP 60-64. Python id `CHOMPER`. Spawns with a
    /// `scream_first` flag via `MonsterSpawnArgs::Chomper`.
    Chomper,
    /// `HunterKiller` — HP 121 (fixed) per C# v0.103.2 (normal 121,
    /// ToughAsc 126; min=max). Python pin `01c094d` ships a stale
    /// 60-65 HP / 2-move variant we deliberately diverge from per
    /// the monster-stat audit; bead `sts2-rs-2srb` (parent `kgwj`)
    /// and `docs/perf/2026-05-18-monster-stat-audit.md`. Python id
    /// `HUNTER_KILLER`.
    HunterKiller,
    /// `LouseProgenitor` — HP 52-56. Python id `LOUSE_PROGENITOR`.
    LouseProgenitor,
    /// Myte — HP 61-67 per C# decomp `Myte.cs` v0.103.2 (normal 61-67,
    /// ToughAsc 64-69). Python pin `01c094d` ships a stale 22-26 HP /
    /// 2-move (RAND BITE/INFEST) variant we deliberately diverge from
    /// per the monster-stat audit; see beads `sts2-rs-t85w` /
    /// `sts2-rs-kgwj` and `docs/perf/2026-05-18-monster-stat-audit.md`.
    /// Python id `MYTE`.
    Myte,
    /// `ToughEgg` — HP 14-18 per C# decomp `ToughEgg.cs` v0.103.2
    /// (`MinInitialHp=14`/`MaxInitialHp=18` normal; ToughAsc 15-19).
    /// Python pin `01c094d` ships 10-12; rebaselined by sts2-rs-7twl.
    /// Python id `TOUGH_EGG`. Spawned mid-combat by Ovicopter.
    ToughEgg,
    /// Ovicopter — HP 124-130 (asc-0) per C# decomp
    /// `MegaCrit.Sts2.Core.Models.Monsters/Ovicopter.cs` v0.103.2
    /// (normal 124-130, ToughAsc 126-132). Python pin `01c094d` ships
    /// a stale 67-72 HP / 2-move (LAY_EGGS↔DIVE 14) variant we
    /// deliberately diverge from per the monster-stat audit; see beads
    /// `sts2-rs-7qd1` / `sts2-rs-kgwj` and
    /// `docs/perf/2026-05-18-monster-stat-audit.md`. Python id
    /// `OVICOPTER`.
    Ovicopter,
    /// `SlumberingBeetle` — HP 66-70. Python id `SLUMBERING_BEETLE`.
    SlumberingBeetle,
    /// `SpinyToad` — HP 116-119. Python id `SPINY_TOAD`.
    SpinyToad,
    /// `TheObscura` — HP 123-129 per C# decomp `TheObscura.cs`
    /// v0.103.2 (normal 123, ToughAsc 129; min=max). Python pin
    /// `01c094d` ships a stale 36-39 HP / 3-move (DARKNESS-CHOMP-STRIKE)
    /// variant we deliberately diverge from per the monster-stat
    /// audit; see beads `sts2-rs-w1kh` / `sts2-rs-kgwj` and
    /// `docs/perf/2026-05-18-monster-stat-audit.md`. Python id
    /// `THE_OBSCURA`.
    TheObscura,
    /// `DecimillipedeSegment` — HP 42-48. Python id
    /// `DECIMILLIPEDE_SEGMENT`. Spawns with a `starter_idx` arg via
    /// `MonsterSpawnArgs::DecimillipedeSegment`.
    DecimillipedeSegment,
    /// Entomancer — HP 145 (fixed). Python id `ENTOMANCER`.
    Entomancer,
    /// `InfestedPrism` — HP 200-215 per C# decomp
    /// `MegaCrit.Sts2.Core.Models.Monsters/InfestedPrism.cs` v0.103.2
    /// (normal 200, ToughAsc 215; min=max). Python pin `01c094d` ships
    /// a stale 40-45 HP variant; bead `sts2-rs-w4gq` rebaselined the
    /// Rust port to the C# baseline (Pattern-A divergence). Python id
    /// `INFESTED_PRISM`. Spawns with an `InfestedPrismSlot` arg via
    /// `MonsterSpawnArgs::InfestedPrism`.
    InfestedPrism,
    /// `TheInsatiable` boss — HP 321 (fixed). Python id
    /// `THE_INSATIABLE`.
    TheInsatiable,
    /// `KnowledgeDemon` boss — HP 379 (fixed). Python id
    /// `KNOWLEDGE_DEMON`.
    KnowledgeDemon,
    /// Crusher (Kaiser Crab boss component) — HP 199. Python id
    /// `CRUSHER`.
    Crusher,
    /// Rocket (Kaiser Crab boss component) — HP 199 (C# `Rocket.cs`
    /// MinInitialHp = MaxInitialHp = 199 asc-0; was 189). Python id
    /// `ROCKET`.
    Rocket,
    // ── Act 3 — Glory (bead `sts2-rs-6ynt.10`) ──
    // HP values mirror `sts2_env/monsters/act3.py` at pin 01c094d.
    /// `DevotedSculptor` — HP 162 (fixed). Python id `DEVOTED_SCULPTOR`.
    DevotedSculptor,
    /// `ScrollOfBiting` — HP 24-26. Python id `SCROLL_OF_BITING`.
    ScrollOfBiting,
    /// `TurretOperator` — HP 41 (Tough Ascension: 51). Python id
    /// `TURRET_OPERATOR`. Rebaselined to C# v0.103.2 per
    /// `MegaCrit.Sts2.Core.Models.Monsters/TurretOperator.cs`
    /// (MinInitialHp = MaxInitialHp = 41, sts2-rs-tnok). Python pin
    /// `01c094d` shipped 28-30 (-27% delta). 3-move chain rewrite +
    /// Fire-3 attack variant deferred to a follow-up bead.
    TurretOperator,
    /// Axebot — HP 40-44. Python id `AXEBOT`. Spawns with a
    /// `start_with_boot_up` flag via `MonsterSpawnArgs::Axebot`.
    Axebot,
    /// Zapbot — HP 18-23 per C# decomp `Zapbot.cs` v0.103.2 (ToughAsc
    /// 19-24). Python pin `01c094d` ships a stale 23-28 HP variant we
    /// deliberately diverge from per the monster-stat audit
    /// (`docs/perf/2026-05-18-monster-stat-audit.md`, Pattern C +28%
    /// HP overstatement); see beads `sts2-rs-znyy` / `sts2-rs-kgwj`.
    /// Python id `ZAPBOT`. Spawned by Fabricator.
    Zapbot,
    /// Stabbot — HP 18-23 per C# decomp `Stabbot.cs` v0.103.2 (ToughAsc
    /// 19-24); Stab damage 11 (Deadly 12). Python pin `01c094d` ships
    /// stale 23-28 HP and Stab 3 MultiAttack ×3; see beads
    /// `sts2-rs-znyy` / `sts2-rs-kgwj`. Python id `STABBOT`. Spawned
    /// by Fabricator.
    Stabbot,
    /// Guardbot — HP 16-20 per C# decomp `Guardbot.cs` v0.103.2
    /// (ToughAsc 17-21). Python pin `01c094d` ships stale 21-25 HP;
    /// see beads `sts2-rs-znyy` / `sts2-rs-kgwj`. Python id `GUARDBOT`.
    /// Spawned by Fabricator.
    Guardbot,
    /// Noisebot — HP 18-23 per C# decomp `Noisebot.cs` v0.103.2
    /// (ToughAsc 19-24). Python pin `01c094d` ships stale 23-28 HP;
    /// see beads `sts2-rs-znyy` / `sts2-rs-kgwj`. Python id `NOISEBOT`.
    /// Spawned by Fabricator.
    Noisebot,
    /// Fabricator — HP 150 (fixed). Python id `FABRICATOR`.
    Fabricator,
    /// `FrogKnight` — HP 191 (fixed). Python id `FROG_KNIGHT`.
    FrogKnight,
    /// `GlobeHead` — HP 148 (fixed). Python id `GLOBE_HEAD`.
    GlobeHead,
    /// `OwlMagistrate` — HP 234 (fixed) per C# v0.103.2. Python id `OWL_MAGISTRATE`.
    /// Bead `sts2-rs-yxvi` rebased off Python pin to C# canonical stats.
    OwlMagistrate,
    /// `SlimedBerserker` — HP 266-276 per C# decomp
    /// `MegaCrit.Sts2.Core.Models.Monsters/SlimedBerserker.cs` v0.103.2
    /// (normal 266, ToughAsc 276; min=max). Python pin `01c094d` ships
    /// a stale 60-65 HP / 3-move SLASH/SLAM/ENRAGE variant we
    /// deliberately diverge from per the monster-stat audit; see beads
    /// `sts2-rs-6v3s` / `sts2-rs-kgwj` and
    /// `docs/perf/2026-05-18-monster-stat-audit.md`. Python id
    /// `SLIMED_BERSERKER`.
    SlimedBerserker,
    /// `TheLost` — HP 50-54. Python id `THE_LOST`.
    TheLost,
    /// `TheForgotten` — HP 106 (C# `TheForgotten.cs` MinInitialHp =
    /// MaxInitialHp = 106; 111 ToughEnemies). The prior 50-54 was a stub.
    /// Python id `THE_FORGOTTEN`.
    TheForgotten,
    /// `FlailKnight` elite — HP 101 (fixed). Python id `FLAIL_KNIGHT`.
    FlailKnight,
    /// `MysteriousKnight` elite — HP 101 (fixed). Python id
    /// `MYSTERIOUS_KNIGHT`.
    MysteriousKnight,
    /// `LivingShield` — HP 55 (fixed). Python id `LIVING_SHIELD`.
    /// Spawned by Magi Knight.
    LivingShield,
    /// `MagiKnight` elite — HP 82 (C# `MagiKnight.cs` MinInitialHp =
    /// MaxInitialHp = 82 asc-0; was 101). Python id `MAGI_KNIGHT`.
    MagiKnight,
    /// `SpectralKnight` elite — HP 101 (fixed). Python id
    /// `SPECTRAL_KNIGHT`.
    SpectralKnight,
    /// `MechaKnight` elite — HP 300 per C# decomp `MechaKnight.cs`
    /// (sts2-rs-dpov bumped from Python pin 01c094d's 155 — Pattern-A
    /// per audit). ToughAsc 320 (ascension is identity at pin, so we
    /// stay on the normal-tier 300).
    MechaKnight,
    /// Osty — HP 1 (fixed). Python id `OSTY`. Spawned by Soul Nexus.
    Osty,
    /// `SoulNexus` elite — HP 234 per C# decomp `SoulNexus.cs` v0.103.2
    /// (normal 234, ToughAsc 254; min=max). Python pin `01c094d` ships
    /// a stale 155 HP / 3-state SUMMON-SOUL_DRAIN-SPIRIT_BARRAGE
    /// variant we deliberately diverge from per the monster-stat
    /// audit (`docs/perf/2026-05-18-monster-stat-audit.md`, Pattern A
    /// -34%); see beads `sts2-rs-blwi` / `sts2-rs-kgwj`. Python id
    /// `SOUL_NEXUS`.
    SoulNexus,
    /// Aeonglass — Act-3 boss (v0.107.1, replaces Doormaker). HP
    /// 512 (A0) / 535 (ToughEnemies). C#
    /// `MegaCrit.Sts2.Core.Models.Monsters/Aeonglass.cs`. On spawn applies
    /// `WitheringPresence(6)` then `Artifact(3)` — BOTH to ITSELF
    /// (`Aeonglass.cs:78-84`: `PowerCmd.Apply(wp, base.Creature, 6m, …)`;
    /// only the power's `Target` field points at the player), then
    /// cycles Ebb (attack+block) → Eye Lasers (multi-attack) → Increasing
    /// Intensity (add Wither + grow Strength). Bead
    /// `sts2-rs-v0107-content-port-wb8lk.2`.
    Aeonglass,
    /// Queen boss — HP 400 per C# decomp `Queen.cs` (sts2-rs-gizi
    /// bumped from Python pin 01c094d's 302 — Pattern-A per audit;
    /// ToughAsc 419, ascension is identity at pin so we stay on the
    /// normal-tier 400). Move-set rebaseline (PUPPET_STRINGS / YOURE_MINE /
    /// BURN_BRIGHT_FOR_ME / OFF_WITH_YOUR_HEAD / EXECUTION / ENRAGE) is
    /// a separate follow-up — the C# state machine has 6 moves vs
    /// Python's 3, including conditional Enrage/BurnBrightForMe states
    /// that need additional infra to model faithfully.
    Queen,
    /// `TestSubject` boss — HP 255. Python id `TEST_SUBJECT`.
    TestSubject,
    // ── Shared / event / dummy monsters (bead `sts2-rs-qz5p` /
    //    parent `sts2-rs-78ks` rc-monster-wiring-gap) ──
    // Ported from sts2-rl-agent@01c094d:sts2_env/monsters/shared.py
    // (`create_*` factories with no spawn-arg config). DSL-driven; the
    // placeholder `roll_intent` arms below are only exercised by
    // `MonsterKind`-only call sites (same pattern as Nibbit + acts 1-3).
    /// Architect — HP 9999 (fixed). Python id `ARCHITECT`. Pet /
    /// structural element with `IntentType.UNKNOWN`.
    Architect,
    /// Byrdpip — HP 9999 (fixed). Python id `BYRDPIP`. Pet companion;
    /// `IntentType.UNKNOWN`.
    Byrdpip,
    /// `PaelsLegion` — HP 9999 (fixed). Python id `PAELS_LEGION`. Pet
    /// companion; `IntentType.UNKNOWN`.
    PaelsLegion,
    /// `FakeMerchantMonster` — HP 165 (fixed). Python id
    /// `FAKE_MERCHANT_MONSTER`. Event-combat boss (SWIPE 13 baseline).
    FakeMerchantMonster,
    /// `TorchHeadAmalgam` — HP 199 (fixed). Python id
    /// `TORCH_HEAD_AMALGAM`. Tackle 18 baseline.
    TorchHeadAmalgam,
    /// `TheAdversaryMkOne` — HP 100 (fixed). Python id
    /// `THE_ADVERSARY_MK_ONE`. Phase 1 of the 3-phase boss; phase
    /// advancement glue lives in the encounter-script / combat_loop
    /// layer (see module doc on shared.rs).
    TheAdversaryMkOne,
    /// `TheAdversaryMkTwo` — HP 200 (fixed). Python id
    /// `THE_ADVERSARY_MK_TWO`. Phase 2 of the 3-phase boss.
    TheAdversaryMkTwo,
    /// `TheAdversaryMkThree` — HP 300 (fixed). Python id
    /// `THE_ADVERSARY_MK_THREE`. Phase 3 of the 3-phase boss.
    TheAdversaryMkThree,
    /// `BattleFriendV1` — HP 75 (fixed). Python id `BATTLE_FRIEND_V1`.
    /// Training dummy carrying `PowerKind::BattlewornDummyTimeLimit(3)`.
    BattleFriendV1,
    /// `BattleFriendV2` — HP 150 (fixed). Python id `BATTLE_FRIEND_V2`.
    /// Training dummy carrying `PowerKind::BattlewornDummyTimeLimit(3)`.
    BattleFriendV2,
    /// `BattleFriendV3` — HP 300 (fixed). Python id `BATTLE_FRIEND_V3`.
    /// Training dummy carrying `PowerKind::BattlewornDummyTimeLimit(3)`.
    BattleFriendV3,
    /// `BigDummy` — HP 9999 (fixed). Python id `BIG_DUMMY`. Test dummy
    /// emitting `IntentType.UNKNOWN`.
    BigDummy,
    /// `OneHpMonster` — HP 1 (fixed). Python id `ONE_HP_MONSTER`. Test
    /// dummy emitting `IntentType.UNKNOWN`.
    OneHpMonster,
    /// `TenHpMonster` — HP 10 (fixed). Python id `TEN_HP_MONSTER`. Test
    /// dummy emitting `IntentType.UNKNOWN`.
    TenHpMonster,
    /// `SingleAttackMoveMonster` — HP 999 (fixed). Python id
    /// `SINGLE_ATTACK_MOVE_MONSTER`. Test template (POKE 1).
    SingleAttackMoveMonster,
    /// `MultiAttackMoveMonster` — HP 999 (fixed). Python id
    /// `MULTI_ATTACK_MOVE_MONSTER`. Test template (POKE 1 × 5).
    MultiAttackMoveMonster,
    // ── 78ks-slotargs (bead `sts2-rs-v9zv`) ── Decimillipede segment
    // visual wrappers + the slot-driven DenseVegetationWriggler. All
    // four reuse existing per-shape `MonsterSpawnArgs` payloads (the
    // Decimillipede trio shares `DecimillipedeSegment { starter_idx }`
    // because the Python factories are visual-only wrappers around the
    // shared AI; `DenseVegetationWriggler { slot }` carries the
    // 4-slot starter-move discriminant).
    /// `DecimillipedeSegmentFront` — HP 42-48 (Python id
    /// `DECIMILLIPEDE_SEGMENT_FRONT`). Visual wrapper around
    /// `DecimillipedeSegment`; reuses `MonsterSpawnArgs::DecimillipedeSegment`.
    DecimillipedeSegmentFront,
    /// `DecimillipedeSegmentMiddle` — HP 42-48 (Python id
    /// `DECIMILLIPEDE_SEGMENT_MIDDLE`). Visual wrapper.
    DecimillipedeSegmentMiddle,
    /// `DecimillipedeSegmentBack` — HP 42-48 (Python id
    /// `DECIMILLIPEDE_SEGMENT_BACK`). Visual wrapper.
    DecimillipedeSegmentBack,
    /// `DenseVegetationWriggler` — HP 17-21 (Python id
    /// `DENSE_VEGETATION_WRIGGLER`). Event-combat monster with a
    /// 4-slot starter-move table; spawns via
    /// `MonsterSpawnArgs::DenseVegetationWriggler { slot }`.
    DenseVegetationWriggler,
    // ── act4 import (bead `sts2-rs-551m`) — first act4 monster wired
    //    into MonsterKind dispatch. Python encounter
    //    `CONSTRUCT_MENAGERIE_NORMAL` spawns 1 PunchConstruct + 2
    //    CubexConstructs; until this lands a third CubexConstruct stood
    //    in as a placeholder. ──
    /// `PunchConstruct` — HP 55 (fixed). Python id `PUNCH_CONSTRUCT`.
    /// v0.107.1 cycle: READY (Defend block 10) → FAST_PUNCH (5 × 2 +
    /// Frail 1) → STRONG_PUNCH (14) → READY. Carries `PowerKind::Artifact(1)`
    /// from the factory's spawn-side power application. Optional
    /// `starts_with_fast_punch` flag routes via
    /// `MonsterSpawnArgs::PunchConstruct { starts_with_fast_punch }`.
    PunchConstruct,
    // ── act4 bulk wiring (bead `sts2-rs-y14x`) — remaining 21 act4
    //    monsters from `sts2_env/monsters/act4.py`. Slot-arg factories
    //    (`CorpseSlug` starter_idx, `Toadpole`/`TwoTailedRat` slot)
    //    default to the first slot at dispatch; per-slot variants can
    //    be added under follow-up if encounters need them. ──
    /// `CorpseSlug` — HP 25-27. Python id `CORPSE_SLUG`.
    CorpseSlug,
    /// `Seapunk` — HP 35-38. Python id `SEAPUNK`.
    Seapunk,
    /// `SludgeSpinner` — HP 20-23. Python id `SLUDGE_SPINNER`.
    SludgeSpinner,
    /// `Toadpole` — HP 14-16. Python id `TOADPOLE`.
    Toadpole,
    /// `CalcifiedCultist` — HP 38-41. Python id `CALCIFIED_CULTIST`.
    CalcifiedCultist,
    /// `DampCultist` — HP 51-53. Python id `DAMP_CULTIST`.
    DampCultist,
    /// `FossilStalker` — HP 51-53. Python id `FOSSIL_STALKER`.
    FossilStalker,
    /// `SneakyGremlin` — HP 9-12. Python id `SNEAKY_GREMLIN`.
    SneakyGremlin,
    /// `FatGremlin` — HP 13-17. Python id `FAT_GREMLIN`.
    FatGremlin,
    /// `GremlinMerc` — HP 47-49. Python id `GREMLIN_MERC`.
    GremlinMerc,
    /// `HauntedShip` — HP 58-62. Python id `HAUNTED_SHIP`.
    HauntedShip,
    /// `GasBomb` — HP 7 (fixed) per C# v0.103.2 (normal 7, ToughAsc
    /// 8; min=max). Python pin `01c094d` ships HP 10 (+43%) which the
    /// 2026-05-18 monster-stat audit flagged as a likely sim typo
    /// (Pattern-B in `docs/perf/2026-05-18-monster-stat-audit.md`).
    /// Bead `sts2-rs-jd5g` (parent `sts2-rs-kgwj`). Python id
    /// `GAS_BOMB`.
    GasBomb,
    /// `LivingFog` — HP 80 (C# `LivingFog.cs` MinInitialHp=MaxInitialHp=80;
    /// the prior 70-74 was the Python pin). Python id `LIVING_FOG`.
    LivingFog,
    /// `SewerClam` — HP 50-54. Python id `SEWER_CLAM`.
    SewerClam,
    /// `TwoTailedRat` — HP 17-21 per C# decomp `TwoTailedRat.cs`
    /// (sts2-rs-54ua corrected from Python pin 01c094d's 25-28
    /// overshoot; Pattern-B / sim bug per audit). Move-set rebaseline
    /// (GNAW/TAIL_WHIP → SCRATCH/DISEASE_BITE/SCREECH/CALL_FOR_BACKUP)
    /// is a separate follow-up — see `docs/perf/2026-05-18-monster-
    /// stat-audit.md` and the bead description for details.
    TwoTailedRat,
    /// `PhantasmalGardener` — HP 26-32. Python id `PHANTASMAL_GARDENER`.
    /// C# decomp `MegaCrit.Sts2.Core.Models.Monsters/PhantasmalGardener.cs`:
    /// MinInitialHp=26 (normal)/27 (ToughEnemies),
    /// MaxInitialHp=31 (normal)/32 (ToughEnemies). The Python pin
    /// `sts2-rl-agent@01c094d:sts2_env/monsters/act4.py` doubles these
    /// values (60-65) — Rust follows the C# ground truth per
    /// `sts2-rs-fwbs`.
    PhantasmalGardener,
    /// `SkulkingColony` — HP 70-75 per C# v0.103.2 (normal 70,
    /// ToughAsc 75; min=max). Python pin `01c094d` ships HP 140
    /// (+100%) — a likely 2x sim typo flagged by the 2026-05-18
    /// monster-stat audit as Pattern-B. Bead `sts2-rs-1xm2`
    /// (parent `sts2-rs-kgwj`) rebases to canonical stats; see
    /// `docs/perf/2026-05-18-monster-stat-audit.md`. Python id
    /// `SKULKING_COLONY`.
    SkulkingColony,
    /// `TerrorEel` — HP 140 (fixed; C# v0.103.2). Python id `TERROR_EEL`.
    TerrorEel,
    /// `WaterfallGiant` — HP 240 (Min=Max; 250 ToughEnemies). C# v0.103.2
    /// `WaterfallGiant.MinInitialHp`. Python id `WATERFALL_GIANT`.
    WaterfallGiant,
    /// `SoulFysh` — HP 270 (fixed). Python id `SOUL_FYSH`.
    SoulFysh,
    /// `LagavulinMatriarch` — HP 222 (fixed). Python id `LAGAVULIN_MATRIARCH`.
    LagavulinMatriarch,
}

/// Canonical starting-HP spec for a [`MonsterKind`]: either a fixed value
/// (consumes no RNG read) or an inclusive `[min, max]` range rolled off one
/// RNG read. Produced by [`MonsterKind::hp_roll`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HpRoll {
    Fixed(i32),
    Range(i32, i32),
}

impl MonsterKind {
    /// Canonical roster of every `MonsterKind` variant currently wired
    /// into the engine (Python-name table + factory + dispatch). Used
    /// by cross-crate regression guards (e.g. the scenario-resolver
    /// coverage test in `sts2-cli`) to assert that adding a variant
    /// to the enum without wiring downstream lookups fails at
    /// `cargo test`, not at the next replay-harness re-run.
    ///
    /// **Must stay in sync with [`MonsterKind::python_name`]'s match
    /// arms** — the `monster_kinds_table_matches_python_names` test
    /// in `sts2-py::state_dict` locks the bidirectional invariant
    /// (every kind has a name; every name resolves back).
    ///
    /// Bead `sts2-rs-23ky`.
    pub const ALL_KNOWN: &'static [Self] = &[
        Self::Nibbit,
        Self::ShrinkerBeetle,
        Self::FuzzyWurmCrawler,
        Self::LeafSlimeS,
        Self::TwigSlimeS,
        Self::LeafSlimeM,
        Self::TwigSlimeM,
        Self::CubexConstruct,
        Self::Flyconid,
        Self::EyeWithTeeth,
        Self::Fogmog,
        Self::Inklet,
        Self::Mawler,
        Self::VineShambler,
        Self::SlitheringStrangler,
        Self::SnappingJaxfruit,
        Self::AssassinRubyRaider,
        Self::AxeRubyRaider,
        Self::BruteRubyRaider,
        Self::CrossbowRubyRaider,
        Self::TrackerRubyRaider,
        Self::BygoneEffigy,
        Self::Byrdonis,
        Self::PhrogParasite,
        Self::Parafright,
        Self::Vantom,
        Self::CeremonialBeast,
        Self::KinPriest,
        Self::KinFollower,
        Self::Wriggler,
        Self::ThievingHopper,
        Self::Tunneler,
        Self::BowlbugEgg,
        Self::BowlbugNectar,
        Self::BowlbugRock,
        Self::BowlbugSilk,
        Self::Exoskeleton,
        Self::Chomper,
        Self::HunterKiller,
        Self::LouseProgenitor,
        Self::Myte,
        Self::ToughEgg,
        Self::Ovicopter,
        Self::SlumberingBeetle,
        Self::SpinyToad,
        Self::TheObscura,
        Self::DecimillipedeSegment,
        Self::Entomancer,
        Self::InfestedPrism,
        Self::TheInsatiable,
        Self::KnowledgeDemon,
        Self::Crusher,
        Self::Rocket,
        Self::DevotedSculptor,
        Self::ScrollOfBiting,
        Self::TurretOperator,
        Self::Axebot,
        Self::Zapbot,
        Self::Stabbot,
        Self::Guardbot,
        Self::Noisebot,
        Self::Fabricator,
        Self::FrogKnight,
        Self::GlobeHead,
        Self::OwlMagistrate,
        Self::SlimedBerserker,
        Self::TheLost,
        Self::TheForgotten,
        Self::FlailKnight,
        Self::MysteriousKnight,
        Self::LivingShield,
        Self::MagiKnight,
        Self::SpectralKnight,
        Self::MechaKnight,
        Self::Osty,
        Self::SoulNexus,
        Self::Aeonglass,
        Self::Queen,
        Self::TestSubject,
        Self::Architect,
        Self::Byrdpip,
        Self::PaelsLegion,
        Self::FakeMerchantMonster,
        Self::TorchHeadAmalgam,
        Self::TheAdversaryMkOne,
        Self::TheAdversaryMkTwo,
        Self::TheAdversaryMkThree,
        Self::BattleFriendV1,
        Self::BattleFriendV2,
        Self::BattleFriendV3,
        Self::BigDummy,
        Self::OneHpMonster,
        Self::TenHpMonster,
        Self::SingleAttackMoveMonster,
        Self::MultiAttackMoveMonster,
        Self::DecimillipedeSegmentFront,
        Self::DecimillipedeSegmentMiddle,
        Self::DecimillipedeSegmentBack,
        Self::DenseVegetationWriggler,
        Self::PunchConstruct,
        Self::CorpseSlug,
        Self::Seapunk,
        Self::SludgeSpinner,
        Self::Toadpole,
        Self::CalcifiedCultist,
        Self::DampCultist,
        Self::FossilStalker,
        Self::SneakyGremlin,
        Self::FatGremlin,
        Self::GremlinMerc,
        Self::HauntedShip,
        Self::GasBomb,
        Self::LivingFog,
        Self::SewerClam,
        Self::TwoTailedRat,
        Self::PhantasmalGardener,
        Self::SkulkingColony,
        Self::TerrorEel,
        Self::WaterfallGiant,
        Self::SoulFysh,
        Self::LagavulinMatriarch,
    ];

    /// Python-name string for the parity exporter's `enemies[*].id` field.
    /// Mirrors `sts2_env/parity/bridge_replay.py:321`
    /// (`enemy.monster_id or "UNKNOWN"`).
    #[must_use]
    pub const fn python_name(self) -> &'static str {
        match self {
            MonsterKind::Nibbit => "Nibbit",
            // Cliext H3: STS2 act1_weak. PascalCase here matches the
            // existing parity rows; the Python source-of-truth `monster_id`
            // is UPPER_SNAKE (e.g. `SHRINKER_BEETLE`) — when the parity
            // bridge round-trips these end-to-end, a UPPER_SNAKE mapping
            // is needed. A separate parity bead owns that remap.
            MonsterKind::ShrinkerBeetle => "ShrinkerBeetle",
            MonsterKind::FuzzyWurmCrawler => "FuzzyWurmCrawler",
            MonsterKind::LeafSlimeS => "LeafSlimeS",
            MonsterKind::TwigSlimeS => "TwigSlimeS",
            MonsterKind::LeafSlimeM => "LeafSlimeM",
            MonsterKind::TwigSlimeM => "TwigSlimeM",
            // ── Act 1 — beads sts2-rs-h37x / mv7c / ckj8 ──
            MonsterKind::CubexConstruct => "CubexConstruct",
            MonsterKind::Flyconid => "Flyconid",
            MonsterKind::EyeWithTeeth => "EyeWithTeeth",
            MonsterKind::Fogmog => "Fogmog",
            MonsterKind::Inklet => "Inklet",
            MonsterKind::Mawler => "Mawler",
            MonsterKind::VineShambler => "VineShambler",
            MonsterKind::SlitheringStrangler => "SlitheringStrangler",
            MonsterKind::SnappingJaxfruit => "SnappingJaxfruit",
            MonsterKind::AssassinRubyRaider => "AssassinRubyRaider",
            MonsterKind::AxeRubyRaider => "AxeRubyRaider",
            MonsterKind::BruteRubyRaider => "BruteRubyRaider",
            MonsterKind::CrossbowRubyRaider => "CrossbowRubyRaider",
            MonsterKind::TrackerRubyRaider => "TrackerRubyRaider",
            MonsterKind::BygoneEffigy => "BygoneEffigy",
            MonsterKind::Byrdonis => "Byrdonis",
            MonsterKind::PhrogParasite => "PhrogParasite",
            MonsterKind::Parafright => "Parafright",
            MonsterKind::Vantom => "Vantom",
            MonsterKind::CeremonialBeast => "CeremonialBeast",
            MonsterKind::KinPriest => "KinPriest",
            MonsterKind::KinFollower => "KinFollower",
            MonsterKind::Wriggler => "Wriggler",
            // ── Act 2 (sts2-rs-6ynt.10) ──
            MonsterKind::ThievingHopper => "ThievingHopper",
            MonsterKind::Tunneler => "Tunneler",
            MonsterKind::BowlbugEgg => "BowlbugEgg",
            MonsterKind::BowlbugNectar => "BowlbugNectar",
            MonsterKind::BowlbugRock => "BowlbugRock",
            MonsterKind::BowlbugSilk => "BowlbugSilk",
            MonsterKind::Exoskeleton => "Exoskeleton",
            MonsterKind::Chomper => "Chomper",
            MonsterKind::HunterKiller => "HunterKiller",
            MonsterKind::LouseProgenitor => "LouseProgenitor",
            MonsterKind::Myte => "Myte",
            MonsterKind::ToughEgg => "ToughEgg",
            MonsterKind::Ovicopter => "Ovicopter",
            MonsterKind::SlumberingBeetle => "SlumberingBeetle",
            MonsterKind::SpinyToad => "SpinyToad",
            MonsterKind::TheObscura => "TheObscura",
            MonsterKind::DecimillipedeSegment => "DecimillipedeSegment",
            MonsterKind::Entomancer => "Entomancer",
            MonsterKind::InfestedPrism => "InfestedPrism",
            MonsterKind::TheInsatiable => "TheInsatiable",
            MonsterKind::KnowledgeDemon => "KnowledgeDemon",
            MonsterKind::Crusher => "Crusher",
            MonsterKind::Rocket => "Rocket",
            // ── Act 3 (sts2-rs-6ynt.10) ──
            MonsterKind::DevotedSculptor => "DevotedSculptor",
            MonsterKind::ScrollOfBiting => "ScrollOfBiting",
            MonsterKind::TurretOperator => "TurretOperator",
            MonsterKind::Axebot => "Axebot",
            MonsterKind::Zapbot => "Zapbot",
            MonsterKind::Stabbot => "Stabbot",
            MonsterKind::Guardbot => "Guardbot",
            MonsterKind::Noisebot => "Noisebot",
            MonsterKind::Fabricator => "Fabricator",
            MonsterKind::FrogKnight => "FrogKnight",
            MonsterKind::GlobeHead => "GlobeHead",
            MonsterKind::OwlMagistrate => "OwlMagistrate",
            MonsterKind::SlimedBerserker => "SlimedBerserker",
            MonsterKind::TheLost => "TheLost",
            MonsterKind::TheForgotten => "TheForgotten",
            MonsterKind::FlailKnight => "FlailKnight",
            MonsterKind::MysteriousKnight => "MysteriousKnight",
            MonsterKind::LivingShield => "LivingShield",
            MonsterKind::MagiKnight => "MagiKnight",
            MonsterKind::SpectralKnight => "SpectralKnight",
            MonsterKind::MechaKnight => "MechaKnight",
            MonsterKind::Osty => "Osty",
            MonsterKind::SoulNexus => "SoulNexus",
            MonsterKind::Aeonglass => "Aeonglass",
            MonsterKind::Queen => "Queen",
            MonsterKind::TestSubject => "TestSubject",
            // ── Shared (sts2-rs-qz5p) ──
            MonsterKind::Architect => "Architect",
            MonsterKind::Byrdpip => "Byrdpip",
            MonsterKind::PaelsLegion => "PaelsLegion",
            MonsterKind::FakeMerchantMonster => "FakeMerchantMonster",
            MonsterKind::TorchHeadAmalgam => "TorchHeadAmalgam",
            MonsterKind::TheAdversaryMkOne => "TheAdversaryMkOne",
            MonsterKind::TheAdversaryMkTwo => "TheAdversaryMkTwo",
            MonsterKind::TheAdversaryMkThree => "TheAdversaryMkThree",
            MonsterKind::BattleFriendV1 => "BattleFriendV1",
            MonsterKind::BattleFriendV2 => "BattleFriendV2",
            MonsterKind::BattleFriendV3 => "BattleFriendV3",
            MonsterKind::BigDummy => "BigDummy",
            MonsterKind::OneHpMonster => "OneHpMonster",
            MonsterKind::TenHpMonster => "TenHpMonster",
            MonsterKind::SingleAttackMoveMonster => "SingleAttackMoveMonster",
            MonsterKind::MultiAttackMoveMonster => "MultiAttackMoveMonster",
            // ── 78ks-slotargs (bead `sts2-rs-v9zv`) ──
            MonsterKind::DecimillipedeSegmentFront => "DecimillipedeSegmentFront",
            MonsterKind::DecimillipedeSegmentMiddle => "DecimillipedeSegmentMiddle",
            MonsterKind::DecimillipedeSegmentBack => "DecimillipedeSegmentBack",
            MonsterKind::DenseVegetationWriggler => "DenseVegetationWriggler",
            // ── act4 (sts2-rs-551m) ──
            MonsterKind::PunchConstruct => "PunchConstruct",
            // ── act4 bulk wiring (bead `sts2-rs-y14x`) ──
            MonsterKind::CorpseSlug => "CorpseSlug",
            MonsterKind::Seapunk => "Seapunk",
            MonsterKind::SludgeSpinner => "SludgeSpinner",
            MonsterKind::Toadpole => "Toadpole",
            MonsterKind::CalcifiedCultist => "CalcifiedCultist",
            MonsterKind::DampCultist => "DampCultist",
            MonsterKind::FossilStalker => "FossilStalker",
            MonsterKind::SneakyGremlin => "SneakyGremlin",
            MonsterKind::FatGremlin => "FatGremlin",
            MonsterKind::GremlinMerc => "GremlinMerc",
            MonsterKind::HauntedShip => "HauntedShip",
            MonsterKind::GasBomb => "GasBomb",
            MonsterKind::LivingFog => "LivingFog",
            MonsterKind::SewerClam => "SewerClam",
            MonsterKind::TwoTailedRat => "TwoTailedRat",
            MonsterKind::PhantasmalGardener => "PhantasmalGardener",
            MonsterKind::SkulkingColony => "SkulkingColony",
            MonsterKind::TerrorEel => "TerrorEel",
            MonsterKind::WaterfallGiant => "WaterfallGiant",
            MonsterKind::SoulFysh => "SoulFysh",
            MonsterKind::LagavulinMatriarch => "LagavulinMatriarch",
        }
    }

    /// The monster's canonical starting-HP spec: a fixed value (no RNG
    /// read, e.g. Nibbit/TurretOperator) or an inclusive `[min, max]`
    /// range rolled off a single RNG read. Single source of truth for
    /// both [`Self::starting_hp`] and [`Self::starting_hp_unique`].
    ///
    /// Match arms are a table mirroring the per-monster HP ranges (Python
    /// pin where C# wasn't yet audited, C# decomp where it was);
    /// coincidental body collisions across monsters (e.g.
    /// Inklet/BruteRubyRaider both 30..=33) are by design — merging
    /// them would obscure the per-monster source-of-truth pin.
    #[allow(clippy::match_same_arms)]
    fn hp_roll(self, ascension: u8) -> HpRoll {
        // `ascension` selects the C# `GetValueIfAscension(ToughEnemies, …)`
        // branch per species (HP scales at ascension >= 8). Most arms are
        // still ascension-identity (their ToughEnemies value lives only in
        // the doc-comment) pending the per-monster fan-out; the
        // FuzzyWurmCrawler arm is the wired vertical slice.
        use crate::ascension::{get_value_if_ascension, AscensionLevel::ToughEnemies};
        match self {
            // Nibbit HP range 42..=46 (C# Nibbit.cs, sts2-rs-brxpe).
            // The `_rng` deliberately goes unused on this arm.
            // sts2-rs-brxpe: C# `Nibbit.cs` MinInitialHp=42 / MaxInitialHp=46
            // (asc-0; 44/48 ToughEnemies). Was a stub Fixed(45).
            MonsterKind::Nibbit => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 44, 42),
                get_value_if_ascension(ascension, ToughEnemies, 48, 46),
            ),
            // Cliext H3 — ranges from `sts2_env/monsters/act1_weak.py`
            // (inclusive on both ends, matching `rng.next_int`).
            MonsterKind::ShrinkerBeetle => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 40, 38),
                get_value_if_ascension(ascension, ToughEnemies, 42, 40),
            ),
            // C# `FuzzyWurmCrawler.cs`:
            // MinInitialHp => GetValueIfAscension(ToughEnemies, 58, 55),
            // MaxInitialHp => GetValueIfAscension(ToughEnemies, 59, 57).
            MonsterKind::FuzzyWurmCrawler => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 58, 55),
                get_value_if_ascension(ascension, ToughEnemies, 59, 57),
            ),
            MonsterKind::LeafSlimeS => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 12, 11),
                get_value_if_ascension(ascension, ToughEnemies, 16, 15),
            ),
            MonsterKind::TwigSlimeS => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 8, 7),
                get_value_if_ascension(ascension, ToughEnemies, 12, 11),
            ),
            MonsterKind::LeafSlimeM => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 33, 32),
                get_value_if_ascension(ascension, ToughEnemies, 36, 35),
            ),
            MonsterKind::TwigSlimeM => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 27, 26),
                get_value_if_ascension(ascension, ToughEnemies, 29, 28),
            ),
            // ── Act 1 (sts2-rs-h37x / mv7c / ckj8) ── ranges mirror
            // `sts2_env/monsters/act1.py` at pin 01c094d. Fixed HPs
            // are `rng.gen_range(n..=n)` so the call still consumes an
            // rng read at the same call site.
            MonsterKind::CubexConstruct => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 70, 65),
                get_value_if_ascension(ascension, ToughEnemies, 70, 65),
            ),
            MonsterKind::Flyconid => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 51, 47),
                get_value_if_ascension(ascension, ToughEnemies, 53, 49),
            ),
            MonsterKind::EyeWithTeeth => HpRoll::Range(6, 6),
            MonsterKind::Fogmog => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 78, 74),
                get_value_if_ascension(ascension, ToughEnemies, 78, 74),
            ),
            MonsterKind::Inklet => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 12, 11),
                get_value_if_ascension(ascension, ToughEnemies, 18, 17),
            ),
            MonsterKind::Mawler => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 76, 72),
                get_value_if_ascension(ascension, ToughEnemies, 76, 72),
            ),
            // sts2-rs-9rwte: C# `VineShambler.cs` Min=Max=61 at Asc-0
            // (ToughAsc=64). Ascension is identity at pin so Asc-0
            // is the only reachable tier — the 62..=64 over-rolls
            // were polluting parity-fuzz INIT diffs.
            MonsterKind::VineShambler => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 64, 61),
                get_value_if_ascension(ascension, ToughEnemies, 64, 61),
            ),
            // Audit follow-up to sts2-rs-9rwte/km8cp/zi9qu: C#
            // `SlitheringStrangler.cs` Asc-0 Min=53/Max=55 (ToughAsc
            // 54..=56). Prior 45..=49 was a Python-pin value that
            // under-rolled the C# range by ~8 HP.
            MonsterKind::SlitheringStrangler => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 54, 53),
                get_value_if_ascension(ascension, ToughEnemies, 56, 55),
            ),
            // sts2-rs-4poa: bumped 53..=56 → 31..=33 per C# decomp
            // `SnappingJaxfruit.cs` (MinInitialHp=31..MaxInitialHp=33
            // normal; ToughAsc 34..36). Python pin overshot by ~70%.
            MonsterKind::SnappingJaxfruit => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 34, 31),
                get_value_if_ascension(ascension, ToughEnemies, 36, 33),
            ),
            MonsterKind::AssassinRubyRaider => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 19, 18),
                get_value_if_ascension(ascension, ToughEnemies, 24, 23),
            ),
            MonsterKind::AxeRubyRaider => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 21, 20),
                get_value_if_ascension(ascension, ToughEnemies, 23, 22),
            ),
            MonsterKind::BruteRubyRaider => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 31, 30),
                get_value_if_ascension(ascension, ToughEnemies, 34, 33),
            ),
            MonsterKind::CrossbowRubyRaider => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 19, 18),
                get_value_if_ascension(ascension, ToughEnemies, 22, 21),
            ),
            MonsterKind::TrackerRubyRaider => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 22, 21),
                get_value_if_ascension(ascension, ToughEnemies, 26, 25),
            ),
            MonsterKind::BygoneEffigy => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 132, 127),
                get_value_if_ascension(ascension, ToughEnemies, 132, 127),
            ),
            // sts2-rs-zi9qu: C# `Byrdonis.cs` Asc-0 Min=81/Max=84
            // (ToughAsc fixed 90). Previous 91..=94 was the ToughAsc
            // band offset by +1 — wrong tier entirely.
            MonsterKind::Byrdonis => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 90, 81),
                get_value_if_ascension(ascension, ToughEnemies, 90, 84),
            ),
            // Audit follow-up to sts2-rs-9rwte/km8cp/zi9qu: C#
            // `PhrogParasite.cs` Asc-0 Min=61/Max=64 (ToughAsc
            // 66..=68). Prior 80..=83 was a Python-pin value
            // ~20 HP higher than the actual game — meaningful skew
            // for the PHROG_PARASITE_ELITE encounter MCTS gates.
            MonsterKind::PhrogParasite => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 66, 61),
                get_value_if_ascension(ascension, ToughEnemies, 68, 64),
            ),
            // sts2-rs-ksa5: bumped 1..=1 → 21..=21 per C# decomp
            // `Parafright.cs` (was a Python minion stub HP=1).
            MonsterKind::Parafright => HpRoll::Range(21, 21),
            // sts2-rs-km8cp: C# `Vantom.cs` Min=Max=173 at Asc-0
            // (ToughAsc=183). Act-1 IC boss; previous range was
            // skewing MCTS winrate gates against Vantom by ~5 HP avg.
            MonsterKind::Vantom => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 183, 173),
                get_value_if_ascension(ascension, ToughEnemies, 183, 173),
            ),
            MonsterKind::CeremonialBeast => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 262, 252),
                get_value_if_ascension(ascension, ToughEnemies, 262, 252),
            ),
            MonsterKind::KinPriest => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 199, 190),
                get_value_if_ascension(ascension, ToughEnemies, 199, 190),
            ),
            // sts2-rs-elt9: bumped 65..=71 → 58..=59 per C# decomp
            // `KinFollower.cs` (MinInitialHp=58/MaxInitialHp=59 normal;
            // ToughAsc 62..63). Python pin overshot by ~12%.
            MonsterKind::KinFollower => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 62, 58),
                get_value_if_ascension(ascension, ToughEnemies, 63, 59),
            ),
            // Act 2 minion — HP from C# decomp `Wriggler.cs` Min/MaxInitialHp
            // (17/21 base). Python pin `act2.py:create_wriggler` is buggy
            // (8-10); bead `sts2-rs-n76j` overrides to canonical range.
            MonsterKind::Wriggler => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 18, 17),
                get_value_if_ascension(ascension, ToughEnemies, 22, 21),
            ),
            // ── Act 2 (sts2-rs-6ynt.10) ──
            // sts2-rs-ctib: bumped 15..=18 → 79..=84 per C# decomp
            // (ThievingHopper.cs HP=79 normal / 84 ToughAsc). Completes
            // the l8yi Pattern-A rebaseline; the HP edit was lost in a
            // sibling-pane race during the l8yi commit window (see
            // 128bf77 for the matching act2.rs follow-up).
            // sts2-rs-u27n6: C# `ThievingHopper.cs` MaxInitialHp => MinInitialHp;
            // asc-0 Min=79 (84 ToughEnemies). Was Range(79,84) — the 84 was
            // the Tough-asc ceiling, not a sampled max (ascension identity at
            // pin 01c094d).
            MonsterKind::ThievingHopper => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 84, 79),
                get_value_if_ascension(ascension, ToughEnemies, 84, 79),
            ),
            // sts2-rs-u27n6: C# `Tunneler.cs` MaxInitialHp => MinInitialHp;
            // asc-0 Min=87 (92 ToughEnemies). Was Range(87,92).
            MonsterKind::Tunneler => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 92, 87),
                get_value_if_ascension(ascension, ToughEnemies, 92, 87),
            ),
            MonsterKind::BowlbugEgg => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 23, 21),
                get_value_if_ascension(ascension, ToughEnemies, 24, 22),
            ),
            MonsterKind::BowlbugNectar => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 36, 35),
                get_value_if_ascension(ascension, ToughEnemies, 39, 38),
            ),
            MonsterKind::BowlbugRock => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 46, 45),
                get_value_if_ascension(ascension, ToughEnemies, 49, 48),
            ),
            MonsterKind::BowlbugSilk => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 41, 40),
                get_value_if_ascension(ascension, ToughEnemies, 44, 43),
            ),
            MonsterKind::Exoskeleton => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 25, 24),
                get_value_if_ascension(ascension, ToughEnemies, 29, 28),
            ),
            MonsterKind::Chomper => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 63, 60),
                get_value_if_ascension(ascension, ToughEnemies, 67, 64),
            ),
            // sts2-rs-u27n6: C# `HunterKiller.cs` MaxInitialHp => MinInitialHp;
            // asc-0 Min=121 (126 ToughEnemies). Was Range(121,126).
            MonsterKind::HunterKiller => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 126, 121),
                get_value_if_ascension(ascension, ToughEnemies, 126, 121),
            ),
            // sts2-rs-4nv5r: C# `LouseProgenitor.cs` Min=134/Max=136 (asc-0;
            // 138/141 ToughEnemies). Prior 52-56 was a stub (~2.5x low).
            MonsterKind::LouseProgenitor => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 138, 134),
                get_value_if_ascension(ascension, ToughEnemies, 141, 136),
            ),
            MonsterKind::Myte => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 64, 61),
                get_value_if_ascension(ascension, ToughEnemies, 69, 67),
            ),
            // sts2-rs-7twl: bumped 10..=12 → 14..=18 per C# decomp
            // `ToughEgg.cs` (MinInitialHp=14/MaxInitialHp=18 normal;
            // ToughAsc 15..=19). Python undershot by ~30%.
            MonsterKind::ToughEgg => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 15, 14),
                get_value_if_ascension(ascension, ToughEnemies, 19, 18),
            ),
            // sts2-rs-rbtn9: C# `Ovicopter.cs` Min=124/Max=130 (asc-0;
            // 126/132 ToughEnemies). Max was 132.
            MonsterKind::Ovicopter => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 126, 124),
                get_value_if_ascension(ascension, ToughEnemies, 132, 130),
            ),
            // sts2-rs-4nv5r: C# `SlumberingBeetle.cs` Min=Max=86 (asc-0; 89
            // ToughEnemies). Prior 66-70 was a stub.
            MonsterKind::SlumberingBeetle => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 89, 86),
                get_value_if_ascension(ascension, ToughEnemies, 89, 86),
            ),
            MonsterKind::SpinyToad => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 121, 116),
                get_value_if_ascension(ascension, ToughEnemies, 124, 119),
            ),
            // sts2-rs-u27n6: C# `TheObscura.cs` MaxInitialHp => MinInitialHp;
            // asc-0 Min=123 (129 ToughEnemies). Was Range(123,129).
            MonsterKind::TheObscura => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 129, 123),
                get_value_if_ascension(ascension, ToughEnemies, 129, 123),
            ),
            // sts2-rs-4nv5r: C# `DecimillipedeSegment.cs` Min=40/Max=46
            // (asc-0; 46/52 ToughEnemies). Was 42-48. The ToughEnemies pair
            // was transcribed into the comment but never applied — the arm
            // was ascension-identity, so an A8+ Decimillipede rolled the A0
            // band. Both `MinInitialHp` and `MaxInitialHp` are
            // `GetValueIfAscension(ToughEnemies, …)` in the decomp
            // (`v0.107.1/…/DecimillipedeSegment.cs:64-66`); the band width is
            // unchanged (7 values) so the `niche` cursor is unaffected.
            MonsterKind::DecimillipedeSegment => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 46, 40),
                get_value_if_ascension(ascension, ToughEnemies, 52, 46),
            ),
            MonsterKind::Entomancer => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 155, 145),
                get_value_if_ascension(ascension, ToughEnemies, 155, 145),
            ),
            // sts2-rs-u27n6: C# `InfestedPrism.cs` MaxInitialHp => MinInitialHp;
            // asc-0 Min=200 (215 ToughEnemies). Was Range(200,215) — the 215
            // was the Tough-asc ceiling, not a sampled max (ascension identity
            // at pin 01c094d).  Earlier w4gq bumped from 40..=45 stub.
            // `…wb8lk.11`: v0.107.1 InfestedPrism HP 200 → 161 (A0).
            // Decomp `v0.107.1/…/InfestedPrism.cs:34`
            // Min=Max GetValueIfAscension(ToughEnemies, 171, 161).
            MonsterKind::InfestedPrism => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 171, 161),
                get_value_if_ascension(ascension, ToughEnemies, 171, 161),
            ),
            MonsterKind::TheInsatiable => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 341, 321),
                get_value_if_ascension(ascension, ToughEnemies, 341, 321),
            ),
            MonsterKind::KnowledgeDemon => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 399, 379),
                get_value_if_ascension(ascension, ToughEnemies, 399, 379),
            ),
            // sts2-rs-g3vq4: C# `Crusher.cs` MinInitialHp = MaxInitialHp =
            // 209 (asc-0; 219 ToughEnemies). Was 199.
            MonsterKind::Crusher => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 219, 209),
                get_value_if_ascension(ascension, ToughEnemies, 219, 209),
            ),
            // sts2-rs-32pq0: C# `Rocket.cs` MinInitialHp = MaxInitialHp =
            // 199 (asc-0; 209 ToughEnemies). Was 189.
            MonsterKind::Rocket => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 209, 199),
                get_value_if_ascension(ascension, ToughEnemies, 209, 199),
            ),
            // ── Act 3 (sts2-rs-6ynt.10) ──
            MonsterKind::DevotedSculptor => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 172, 162),
                get_value_if_ascension(ascension, ToughEnemies, 172, 162),
            ),
            // sts2-rs-g3vq4: C# `ScrollOfBiting.cs` MinInitialHp = 31,
            // MaxInitialHp = 38 (asc-0; 32/39 ToughEnemies). Was 24-26.
            // `…wb8lk.11`: v0.107.1 ScrollOfBiting HP 31–38 → 30–37 (A0),
            // ToughEnemies 32–39 → 33–39. Decomp `v0.107.1/…/ScrollOfBiting.cs:37,39`
            // Min GetValueIfAscension(ToughEnemies, 33, 30),
            // Max GetValueIfAscension(ToughEnemies, 39, 37).
            MonsterKind::ScrollOfBiting => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 33, 30),
                get_value_if_ascension(ascension, ToughEnemies, 39, 37),
            ),
            MonsterKind::TurretOperator => {
                HpRoll::Fixed(get_value_if_ascension(ascension, ToughEnemies, 51, 41))
            }
            // `…wb8lk.11`: v0.107.1 Axebot HP 40–44 → 70–78 (A0).
            // Decomp `v0.107.1/…/Axebot.cs:44-46`
            // Min GetValueIfAscension(ToughEnemies, 76, 70),
            // Max GetValueIfAscension(ToughEnemies, 86, 78).
            MonsterKind::Axebot => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 76, 70),
                get_value_if_ascension(ascension, ToughEnemies, 86, 78),
            ),
            MonsterKind::Zapbot => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 19, 18),
                get_value_if_ascension(ascension, ToughEnemies, 24, 23),
            ),
            MonsterKind::Stabbot => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 19, 18),
                get_value_if_ascension(ascension, ToughEnemies, 24, 23),
            ),
            MonsterKind::Guardbot => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 17, 16),
                get_value_if_ascension(ascension, ToughEnemies, 21, 20),
            ),
            MonsterKind::Noisebot => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 19, 18),
                get_value_if_ascension(ascension, ToughEnemies, 24, 23),
            ),
            MonsterKind::Fabricator => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 155, 150),
                get_value_if_ascension(ascension, ToughEnemies, 155, 150),
            ),
            MonsterKind::FrogKnight => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 199, 191),
                get_value_if_ascension(ascension, ToughEnemies, 199, 191),
            ),
            MonsterKind::GlobeHead => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 158, 148),
                get_value_if_ascension(ascension, ToughEnemies, 158, 148),
            ),
            // `…wb8lk.11`: v0.107.1 OwlMagistrate HP 234 → 231 (A0),
            // ToughEnemies 243 → 247. Decomp `v0.107.1/…/OwlMagistrate.cs:35`
            // Min=Max GetValueIfAscension(ToughEnemies, 247, 231).
            MonsterKind::OwlMagistrate => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 247, 231),
                get_value_if_ascension(ascension, ToughEnemies, 247, 231),
            ),
            // sts2-rs-odxkd: C# `SlimedBerserker.cs` MaxInitialHp => MinInitialHp;
            // asc-0 Min=266 (276 ToughEnemies). Was Range(266,276) — the 276
            // was the Tough-asc ceiling, unreachable at pin 01c094d.
            // `…wb8lk.11`: v0.107.1 SlimedBerserker HP 266 → 261 (A0),
            // ToughEnemies 276 → 281. Decomp `v0.107.1/…/SlimedBerserker.cs:33`
            // Min=Max GetValueIfAscension(ToughEnemies, 281, 261).
            MonsterKind::SlimedBerserker => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 281, 261),
                get_value_if_ascension(ascension, ToughEnemies, 281, 261),
            ),
            // sts2-rs-g3vq4: C# `TheLost.cs` MinInitialHp = MaxInitialHp =
            // 93 (asc-0; 99 ToughEnemies). Prior 50-54 was a stub.
            MonsterKind::TheLost => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 99, 93),
                get_value_if_ascension(ascension, ToughEnemies, 99, 93),
            ),
            // sts2-rs-u278v: C# `TheForgotten.cs` MinInitialHp = MaxInitialHp
            // = 106 (asc-0; 111 ToughEnemies). Prior 50-54 was a stub.
            MonsterKind::TheForgotten => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 111, 106),
                get_value_if_ascension(ascension, ToughEnemies, 111, 106),
            ),
            MonsterKind::FlailKnight => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 108, 101),
                get_value_if_ascension(ascension, ToughEnemies, 108, 101),
            ),
            MonsterKind::MysteriousKnight => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 108, 101),
                get_value_if_ascension(ascension, ToughEnemies, 108, 101),
            ),
            MonsterKind::LivingShield => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 65, 55),
                get_value_if_ascension(ascension, ToughEnemies, 65, 55),
            ),
            // sts2-rs-32pq0: C# `MagiKnight.cs` MinInitialHp = MaxInitialHp
            // = 82 (asc-0; 89 ToughEnemies). Was 101.
            MonsterKind::MagiKnight => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 89, 82),
                get_value_if_ascension(ascension, ToughEnemies, 89, 82),
            ),
            // sts2-rs-g3vq4: C# `SpectralKnight.cs` MinInitialHp =
            // MaxInitialHp = 93 (asc-0; 97 ToughEnemies). Was 101.
            MonsterKind::SpectralKnight => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 97, 93),
                get_value_if_ascension(ascension, ToughEnemies, 97, 93),
            ),
            // sts2-rs-dpov: bumped 155..=155 → 300..=300 per C#
            // decomp `MechaKnight.cs` (MinInitialHp=300 normal /
            // 320 ToughAsc). Pattern-A audit row.
            MonsterKind::MechaKnight => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 320, 300),
                get_value_if_ascension(ascension, ToughEnemies, 320, 300),
            ),
            MonsterKind::Osty => HpRoll::Range(1, 1),
            MonsterKind::SoulNexus => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 254, 234),
                get_value_if_ascension(ascension, ToughEnemies, 254, 234),
            ),
            // Aeonglass (v0.107.1): fixed HP, A0=512 / ToughEnemies(A8)=535.
            // C# `Aeonglass.MinInitialHp == MaxInitialHp ==
            // GetValueIfAscension(ToughEnemies, 535, 512)`.
            MonsterKind::Aeonglass => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 535, 512),
                get_value_if_ascension(ascension, ToughEnemies, 535, 512),
            ),
            // sts2-rs-gizi: bumped 302..=302 → 400..=400 per C# decomp
            // `Queen.cs` (MinInitialHp=400 normal / 419 ToughAsc).
            // Pattern-A audit row.
            MonsterKind::Queen => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 419, 400),
                get_value_if_ascension(ascension, ToughEnemies, 419, 400),
            ),
            // sts2-rs-1b86: bumped 255..=255 → 100..=100 per C#
            // decomp `TestSubject.cs` (FirstFormHp=100 normal /
            // 111 ToughAsc; revives to SecondFormHp=200/212 on death
            // — Revive() mechanic is a separate follow-up). Pattern-A.
            MonsterKind::TestSubject => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 111, 100),
                get_value_if_ascension(ascension, ToughEnemies, 111, 100),
            ),
            // ── Shared (sts2-rs-qz5p) ── fixed HPs from
            // sts2_env/monsters/shared.py. `rng.gen_range(n..=n)` keeps
            // the call site rng-symmetric with the rest of the table.
            MonsterKind::Architect
            | MonsterKind::Byrdpip
            | MonsterKind::PaelsLegion
            | MonsterKind::BigDummy => HpRoll::Range(9999, 9999),
            MonsterKind::FakeMerchantMonster => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 175, 165),
                get_value_if_ascension(ascension, ToughEnemies, 175, 165),
            ),
            MonsterKind::TorchHeadAmalgam => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 211, 199),
                get_value_if_ascension(ascension, ToughEnemies, 211, 199),
            ),
            MonsterKind::TheAdversaryMkOne => HpRoll::Range(100, 100),
            MonsterKind::TheAdversaryMkTwo => HpRoll::Range(200, 200),
            MonsterKind::TheAdversaryMkThree => HpRoll::Range(300, 300),
            MonsterKind::BattleFriendV1 => HpRoll::Range(75, 75),
            MonsterKind::BattleFriendV2 => HpRoll::Range(150, 150),
            MonsterKind::BattleFriendV3 => HpRoll::Range(300, 300),
            MonsterKind::OneHpMonster => HpRoll::Range(1, 1),
            MonsterKind::TenHpMonster => HpRoll::Range(10, 10),
            MonsterKind::SingleAttackMoveMonster | MonsterKind::MultiAttackMoveMonster => {
                HpRoll::Range(999, 999)
            }
            // ── 78ks-slotargs (bead `sts2-rs-v9zv`) ──
            // DecimillipedeSegment variants share the 42..=48 base
            // range (Python: HpRoll::Range(42, 48), act2.py:455 +
            // wrappers at 501-522). DenseVegetationWriggler is 17..=21
            // (shared.py:204, HpRoll::Range(17, 21)).
            // sts2-rs-1k2w9: DecimillipedeSegment{Front,Middle,Back} all
            // `extend DecimillipedeSegment` in C# without overriding HP, so
            // they inherit its Min=40/Max=46 (asc-0; 46/52 ToughEnemies —
            // sts2-rs-4nv5r). Was a stub 42-48, then an ascension-identity
            // 40-46; the ToughEnemies pair now applies here too, since the
            // subclasses inherit the base's ascension-aware properties.
            MonsterKind::DecimillipedeSegmentFront
            | MonsterKind::DecimillipedeSegmentMiddle
            | MonsterKind::DecimillipedeSegmentBack => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 46, 40),
                get_value_if_ascension(ascension, ToughEnemies, 52, 46),
            ),
            MonsterKind::DenseVegetationWriggler => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 18, 17),
                get_value_if_ascension(ascension, ToughEnemies, 22, 21),
            ),
            // ── act4 (sts2-rs-551m) ── HP 55 fixed
            // (sts2_env/monsters/act4.py:353).
            MonsterKind::PunchConstruct => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 60, 55),
                get_value_if_ascension(ascension, ToughEnemies, 60, 55),
            ),
            // ── act4 bulk wiring (bead `sts2-rs-y14x`) ──
            MonsterKind::CorpseSlug => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 27, 25),
                get_value_if_ascension(ascension, ToughEnemies, 29, 27),
            ),
            // sts2-rs-29kfv: C# `Seapunk.cs` MinInitialHp=44/MaxInitialHp=46
            // at Asc-0 (ToughEnemies 47..=49). Prior 35..=38 was a stale
            // Python-pin value ~9 HP low.
            MonsterKind::Seapunk => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 47, 44),
                get_value_if_ascension(ascension, ToughEnemies, 49, 46),
            ),
            // sts2-rs-hg6yy: C# `SludgeSpinner.cs` MinInitialHp=37/MaxInitialHp=39
            // (asc-0; 41..=42 ToughEnemies). Was 20..=23 — a ~17 HP-low stub.
            MonsterKind::SludgeSpinner => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 41, 37),
                get_value_if_ascension(ascension, ToughEnemies, 42, 39),
            ),
            // sts2-rs-hg6yy: C# `Toadpole.cs` MinInitialHp=21/MaxInitialHp=25
            // (asc-0; 22..=26 ToughEnemies). Was 14..=16 — ~7-9 HP low.
            MonsterKind::Toadpole => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 22, 21),
                get_value_if_ascension(ascension, ToughEnemies, 26, 25),
            ),
            MonsterKind::CalcifiedCultist => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 39, 38),
                get_value_if_ascension(ascension, ToughEnemies, 42, 41),
            ),
            MonsterKind::DampCultist => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 52, 51),
                get_value_if_ascension(ascension, ToughEnemies, 54, 53),
            ),
            MonsterKind::FossilStalker => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 54, 51),
                get_value_if_ascension(ascension, ToughEnemies, 56, 53),
            ),
            // sts2-rs-hg6yy: C# `SneakyGremlin.cs` MinInitialHp=10/MaxInitialHp=14
            // (asc-0; 11..=15 ToughEnemies). Was 9..=12.
            MonsterKind::SneakyGremlin => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 11, 10),
                get_value_if_ascension(ascension, ToughEnemies, 15, 14),
            ),
            MonsterKind::FatGremlin => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 14, 13),
                get_value_if_ascension(ascension, ToughEnemies, 18, 17),
            ),
            MonsterKind::GremlinMerc => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 51, 47),
                get_value_if_ascension(ascension, ToughEnemies, 53, 49),
            ),
            // sts2-rs-4nv5r: C# `HauntedShip.cs` Min=Max=63 (asc-0; 67
            // ToughEnemies). Was 58-62.
            MonsterKind::HauntedShip => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 67, 63),
                get_value_if_ascension(ascension, ToughEnemies, 67, 63),
            ),
            // sts2-rs-hg6yy: C# `GasBomb.cs` Min=Max=7 (asc-0; 8 ToughEnemies).
            // Was Range(7, 8) — C# uses MaxInitialHp => MinInitialHp.
            MonsterKind::GasBomb => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 8, 7),
                get_value_if_ascension(ascension, ToughEnemies, 8, 7),
            ),
            MonsterKind::LivingFog => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 82, 80),
                get_value_if_ascension(ascension, ToughEnemies, 82, 80),
            ),
            // sts2-rs-hg6yy: C# `SewerClam.cs` Min=Max=56 (asc-0; 58 ToughEnemies;
            // MaxInitialHp => MinInitialHp). Was Range(50, 54) — 2-6 HP low + spread.
            MonsterKind::SewerClam => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 58, 56),
                get_value_if_ascension(ascension, ToughEnemies, 58, 56),
            ),
            // sts2-rs-54ua: bumped 25..=28 → 17..=21 per C# decomp
            // `TwoTailedRat.cs` (MinInitialHp=17 / MaxInitialHp=21,
            // ToughAsc 18..=22; ascension is identity at pin so we
            // stay on the normal-tier range). Pattern-B sim bug.
            MonsterKind::TwoTailedRat => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 18, 17),
                get_value_if_ascension(ascension, ToughEnemies, 22, 21),
            ),
            // sts2-rs-4nv5r: C# `PhantasmalGardener.cs` Min=26/Max=31 (asc-0;
            // 27/32 ToughEnemies). Max was 32.
            MonsterKind::PhantasmalGardener => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 27, 26),
                get_value_if_ascension(ascension, ToughEnemies, 32, 31),
            ),
            // `…wb8lk.11`: v0.107.1 SkulkingColony HP 70 → 75 (A0),
            // ToughEnemies 75 → 80. Decomp `v0.107.1/…/SkulkingColony.cs:37`
            // Min=Max GetValueIfAscension(ToughEnemies, 80, 75).
            MonsterKind::SkulkingColony => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 80, 75),
                get_value_if_ascension(ascension, ToughEnemies, 80, 75),
            ),
            // sts2-rs-7iuyk: C# TerrorEel.cs MinInitialHp == MaxInitialHp == 140
            // (150 ToughEnemies; identity at pin 01c094d). Was a fabricated 130.
            MonsterKind::TerrorEel => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 150, 140),
                get_value_if_ascension(ascension, ToughEnemies, 150, 140),
            ),
            // C# WaterfallGiant.MinInitialHp = 240 (Min=Max; 250 ToughEnemies,
            // ascension identity at pin 01c094d). Was a fabricated 310.
            MonsterKind::WaterfallGiant => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 250, 240),
                get_value_if_ascension(ascension, ToughEnemies, 250, 240),
            ),
            // sts2-rs-hg6yy: C# `SoulFysh.cs` Min=Max=211 (asc-0; 221 ToughEnemies;
            // MaxInitialHp => MinInitialHp). Was Range(270, 270) — 59 HP high.
            MonsterKind::SoulFysh => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 221, 211),
                get_value_if_ascension(ascension, ToughEnemies, 221, 211),
            ),
            MonsterKind::LagavulinMatriarch => HpRoll::Range(
                get_value_if_ascension(ascension, ToughEnemies, 233, 222),
                get_value_if_ascension(ascension, ToughEnemies, 233, 222),
            ),
        }
    }

    /// Roll the monster's starting HP from its canonical range. Inclusive
    /// on both ends, matching Python's `random.randint(low, high)` /
    /// C# `Rng.NextInt(min, max+1)`. Fixed-HP species consume no RNG read
    /// (pre-existing behavior; see [`Self::hp_roll`]); ranged species
    /// consume exactly one. Use [`Self::starting_hp_unique`] on the
    /// enemy-spawn path so same-kind monsters get distinct HP.
    #[must_use]
    pub fn starting_hp(self, rng: &mut CombatRng, ascension: u8) -> i32 {
        match self.hp_roll(ascension) {
            HpRoll::Fixed(v) => v,
            HpRoll::Range(min, max) => rng.next_int(min, max),
        }
    }

    /// The low end of [`Self::hp_roll`] — C# `MonsterModel.MinInitialHp`,
    /// read WITHOUT rolling anything.
    ///
    /// Events preview a fight they haven't started yet off this value:
    /// `BattlewornDummy.GenerateInitialOptions` binds each setting's
    /// description to `ModelDb.Monster<BattleFriendV{1,2,3}>().MinInitialHp`
    /// so the option reads "Fight a 75 HP dummy." (the multiplayer scaling it
    /// wraps the read in is identity at one player,
    /// `Creature.ScaleHpForMultiplayer`). Fixed-HP species return their single
    /// value; the `ascension` argument feeds the same
    /// `GetValueIfAscension(ToughEnemies, …)` branch the roll uses, because C#
    /// overrides like `Nibbit.MinInitialHp` are themselves ascension-aware.
    #[must_use]
    pub fn min_initial_hp(self, ascension: u8) -> i32 {
        match self.hp_roll(ascension) {
            HpRoll::Fixed(v) | HpRoll::Range(v, _) => v,
        }
    }

    /// Roll starting HP while guaranteeing distinctness from HP values
    /// already taken by same-side creatures. Port of C#
    /// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Entities.Creatures/Creature.cs:319`
    /// `SetUniqueMonsterHpValue`: build the inclusive `[Min, Max]` set,
    /// remove HP values already used by other same-side creatures
    /// (`taken`), then pick via `Rng.NextItem` — i.e. `NextInt(0, count)`
    /// indexing the ascending remaining set (.NET `HashSet<int>` built by
    /// ascending `Range(..).ToHashSet()` then `ExceptWith` enumerates in
    /// ascending order). When the set is exhausted it falls back to
    /// `NextInt(min, max+1)`. Every branch — including the fixed-HP one
    /// below — consumes exactly one Niche read, matching C#
    /// (`CreateCreature` rolls HP for every enemy). Beads `sts2-rs-t6sh2`
    /// (cross-kind exclusion) + `sts2-rs-q6bix` (fixed-HP read).
    ///
    /// When `taken` excludes nothing in range (single / distinct-kind
    /// encounters), this returns the *same* value as [`Self::starting_hp`]
    /// off the same RNG cursor — both compute `min + floor(sample *
    /// (max - min + 1))` — so existing INIT-HP parity is preserved; only
    /// `>= 2` same-kind monsters (e.g. 2× `LeafSlimeS`) diverge, which is
    /// the bug this fixes. Fixed-HP species draw `NextItem({v})` =
    /// `NextInt(0, 1)` (one read, always `v`) so the Niche cursor stays in
    /// lockstep with C# for any LATER same-side monster.
    #[must_use]
    pub fn starting_hp_unique(
        self,
        rng: &mut CombatRng,
        taken: &std::collections::HashSet<i32>,
        ascension: u8,
    ) -> i32 {
        match self.hp_roll(ascension) {
            HpRoll::Fixed(v) => {
                // C# `CombatState.CreateCreature` calls
                // `SetUniqueMonsterHpValue` for EVERY enemy
                // unconditionally (`CombatState.cs:387`, no range guard).
                // A fixed-HP monster (`MinInitialHp == MaxInitialHp == v`)
                // still builds the 1-element set `{v}` and draws
                // `NextItem({v})` = `NextInt(0, 1)`, which advances the
                // Niche `Counter` (`Rng.cs:57-65` — `Counter++` for any
                // `min < max`, and `0 < 1`). The value is always `v`, but
                // the READ must happen: skipping it (the prior no-read
                // fast path) desynced the Niche cursor by one for every
                // later same-side monster, drifting their range-HP rolls
                // (sweep `niche cs=N rs=N-1`, e.g. TURRET_OPERATOR_WEAK;
                // bead `sts2-rs-q6bix`). Consume exactly one read — the
                // `ExceptWith`-emptied fallback `NextInt(v, v+1)` is also
                // one read returning `v`, so `NextInt(0, 1)` is
                // cursor-faithful regardless of exclusions.
                let _ = rng.next_int_exclusive(0, 1);
                v
            }
            HpRoll::Range(min, max) => {
                let candidates: Vec<i32> = (min..=max).filter(|v| !taken.contains(v)).collect();
                if candidates.is_empty() {
                    // C# `hashSet.Count <= 0` branch: NextInt(min, max+1).
                    rng.next_int(min, max)
                } else {
                    // C# `NextItem(hashSet)` = NextInt(0, count) into the
                    // ascending remaining set.
                    let count =
                        i32::try_from(candidates.len()).expect("HP candidate count fits in i32");
                    let idx = rng.next_int_exclusive(0, count);
                    candidates[usize::try_from(idx).expect("NextInt(0, count) is non-negative")]
                }
            }
        }
    }

    /// Post-roll maxHp fix-up applied at spawn, after
    /// [`Self::starting_hp_unique`]. Identity for every species except the
    /// Decimillipede segments, which run C#
    /// `DecimillipedeSegment.AfterAddedToRoom`
    /// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/DecimillipedeSegment.cs`):
    /// round the rolled maxHp UP to even, then bump `+2` (wrapping
    /// `MaxInitialHp`→`MinInitialHp`) until it no longer collides with a
    /// teammate's maxHp. C# reads the *already-finalized* maxHp of the
    /// segments added before it, so callers must pass the transformed
    /// values of prior same-side creatures (the same set already excluded
    /// by `starting_hp_unique`).
    ///
    /// The wrap bounds are the segment's own `MinInitialHp`/`MaxInitialHp`
    /// run through `ScaleHpForMultiplayer` — which collapses to the identity
    /// single-player — and BOTH properties are
    /// `AscensionHelper.GetValueIfAscension(ToughEnemies, …)`
    /// (`v0.107.1/…/DecimillipedeSegment.cs:64-66`, `:137-140`). So the wrap
    /// band is `[40, 46]` below A8 and `[46, 52]` at A8+; the previously
    /// hard-coded `[40, 46]` sent an A8 segment that collided at 52 down to
    /// 40, four points below the ToughEnemies floor. Bead `sts2-rs-8mpo1`.
    #[must_use]
    pub fn finalize_spawn_max_hp(
        self,
        rolled: i32,
        teammate_max_hps: &std::collections::HashSet<i32>,
        ascension: u8,
    ) -> i32 {
        use crate::ascension::{get_value_if_ascension, AscensionLevel::ToughEnemies};
        let (min_hp, max_hp) = match self {
            MonsterKind::DecimillipedeSegment
            | MonsterKind::DecimillipedeSegmentFront
            | MonsterKind::DecimillipedeSegmentMiddle
            | MonsterKind::DecimillipedeSegmentBack => (
                get_value_if_ascension(ascension, ToughEnemies, 46, 40),
                get_value_if_ascension(ascension, ToughEnemies, 52, 46),
            ),
            _ => return rolled,
        };
        // C#: `if (maxHp % 2 == 1) maxHp++;` — round up to even.
        let mut hp = if rolled % 2 == 1 { rolled + 1 } else { rolled };
        // C#: `while (teammates.Any(c => c.MaxHp == maxHp)) { maxHp += 2;
        //   if (maxHp > Max) maxHp = Min; }`. At most two teammates are
        // present when the third segment spawns, so a free even value in
        // `{40, 42, 44, 46}` (A0) / `{46, 48, 50, 52}` (A8+) always exists —
        // the loop terminates.
        while teammate_max_hps.contains(&hp) {
            hp += 2;
            if hp > max_hp {
                hp = min_hp;
            }
        }
        hp
    }
}

/// Roll every enemy's spawn `max_hp` for one combat, faithful to C#'s
/// **two-pass** creature setup.
///
/// C# splits HP assignment across two distinct iterations over the enemy
/// list:
///
/// 1. **Creation pass** — `CombatState.CreateCreature`
///    (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Combat/CombatState.cs:388`)
///    calls `Creature.SetUniqueMonsterHpValue(creaturesOnSide, Rng.Niche)`
///    for each enemy in spawn order. The exclusion set is the **raw** rolled
///    `MaxHp` of the teammates created *before* it — `AfterAddedToRoom` has
///    not run yet, so no even-rounding has happened.
/// 2. **Finalize pass** — `CombatManager.StartCombatInternal`
///    (`CombatManager.cs:309-318`) walks `_state.Creatures` a *second* time,
///    awaiting `AfterCreatureAdded` → `creature.AfterAddedToRoom`. The
///    Decimillipede segment override
///    (`DecimillipedeSegment.cs:119-143`) even-rounds its own rolled `MaxHp`
///    and bumps `+2` (wrapping `[40, 46]`) off `GetTeammatesOf(this)` — whose
///    `MaxHp` values are **finalized for teammates earlier in the iteration
///    and still raw for teammates later** in it.
///
/// For every non-Decimillipede roster `finalize_spawn_max_hp` is the
/// identity, so the two passes collapse to the single inline roll and the
/// result is byte-identical (same `niche` reads, same values). The split
/// only changes values when a multi-segment Decimillipede is present — the
/// case the prior inline `roll → finalize → insert` loop got subtly wrong
/// (it excluded the *finalized* teammate HP in pass 1 and saw an
/// all-finalized teammate set in pass 2). Bead `sts2-rs-8mpo1` / `jenwp`
/// (M3b step-c).
///
/// `niche` cursor advancement is unchanged from the inline loop: one read
/// per enemy in spawn order during pass 1; the finalize pass consumes no RNG.
#[must_use]
pub fn roll_unique_max_hps(
    kinds: &[MonsterKind],
    niche: &mut CombatRng,
    ascension: u8,
) -> Vec<i32> {
    use std::collections::HashSet;
    // Pass 1: raw rolls, side-wide exclusion of prior creatures' RAW MaxHp.
    let mut taken_raw: HashSet<i32> = HashSet::new();
    let raws: Vec<i32> = kinds
        .iter()
        .map(|&kind| {
            let r = kind.starting_hp_unique(niche, &taken_raw, ascension);
            taken_raw.insert(r);
            r
        })
        .collect();
    // Pass 2: finalize each, dedup against the C# mixed teammate set —
    // finalized values for earlier indices, raw values for later ones.
    let mut finals = raws.clone();
    for i in 0..kinds.len() {
        let teammates: HashSet<i32> = finals
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != i)
            .map(|(_, &v)| v)
            .collect();
        finals[i] = kinds[i].finalize_spawn_max_hp(raws[i], &teammates, ascension);
    }
    finals
}

impl MonsterKind {
    /// Per-species static buffs applied at creature spawn. Mirrors
    /// Python's `creature.apply_power(...)` calls at the tail of each
    /// `create_<monster>` factory in
    /// `sts2-rl-agent@01c094d:sts2_env/monsters/*.py`. The returned
    /// slice is consumed by
    /// [`crate::CombatState::apply_creature_spawn_buffs`], which the
    /// `spawn_enemy` helper calls automatically.
    ///
    /// Empty slice means "no on-spawn self-buff" — that's the default
    /// for every monster whose AI carries its buffs through `Buff`
    /// intent moves at runtime (Cubex CHARGE_UP, Mawler ROAR, etc.).
    /// The 5 species enumerated below have buffs that fire at
    /// construction time — they're statically present on every
    /// fresh spawn before any AI moves resolve.
    ///
    /// Bead `sts2-rs-3jo9`.
    /// A9 `DeadlyEnemies` replacements for the [`Self::on_spawn_self_buffs`]
    /// amounts C# resolves through
    /// `AscensionHelper.GetValueIfAscension(DeadlyEnemies, …)`. The base
    /// table carries the A0 amount (which every parity fixture pins);
    /// this returns the A9 amount, or `None` when the amount is flat.
    /// Same base-table + A9-override shape as
    /// `sts2_content::ascension_scaling`.
    #[must_use]
    pub fn on_spawn_self_buff_deadly(self, power: crate::powers::PowerKind) -> Option<i32> {
        use crate::powers::PowerKind;
        match (self, power) {
            // C# `CorpseSlug.cs:51,88` `RavenousStr => (.., 5, 4)`.
            (MonsterKind::CorpseSlug, PowerKind::Ravenous) => Some(5),
            // C# `TestSubject.cs:88,179` `EnrageAmount => (.., 3, 2)`.
            (MonsterKind::TestSubject, PowerKind::Enrage) => Some(3),
            _ => None,
        }
    }

    /// A8 `ToughEnemies` replacements for the [`Self::on_spawn_self_buffs`]
    /// amounts C# resolves through
    /// `AscensionHelper.GetValueIfAscension(ToughEnemies, …)`; `None` when the
    /// amount doesn't scale there. Same shape as
    /// [`Self::on_spawn_self_buff_deadly`].
    #[must_use]
    pub fn on_spawn_self_buff_tough(self, power: crate::powers::PowerKind) -> Option<i32> {
        use crate::powers::PowerKind;
        match (self, power) {
            // C# `Vantom.cs:64` `SlipperyAmt => (ToughEnemies, 9, 8)`.
            (MonsterKind::Vantom, PowerKind::Slippery) => Some(9),
            // C# `SewerClam.cs:32` Plating `(ToughEnemies, 9, 8)`.
            (MonsterKind::SewerClam, PowerKind::Plating) => Some(9),
            // C# `FrogKnight.cs:43` `PlatingAmount => (.., 19, 15)`.
            (MonsterKind::FrogKnight, PowerKind::Plating) => Some(19),
            // C# `SlumberingBeetle.cs:45` `PlatingAmount => (.., 18, 15)`.
            (MonsterKind::SlumberingBeetle, PowerKind::Plating) => Some(18),
            // C# `PhantasmalGardener.cs:49` `SkittishAmount => (.., 7, 6)`.
            (MonsterKind::PhantasmalGardener, PowerKind::Skittish) => Some(7),
            // C# `TerrorEel.cs:39` `ShriekAmount => (.., 75, 70)`.
            (MonsterKind::TerrorEel, PowerKind::Shriek) => Some(75),
            // C# `LouseProgenitor.cs:65,70` CurlUp(`CurlBlock => (.., 18, 14)`).
            (MonsterKind::LouseProgenitor, PowerKind::CurlUp) => Some(18),
            _ => None,
        }
    }

    #[must_use]
    pub fn on_spawn_self_buffs(self) -> &'static [(crate::powers::PowerKind, i32)] {
        use crate::powers::PowerKind;
        match self {
            // sts2-rs-5dp60: VineShambler.cs has no AfterAddedToRoom
            // override in C# — the Python pin invented Thorns(3) on
            // spawn. Removed to match canonical game behavior; the
            // monster's swipe/vines/chomp move state machine retains
            // its (non-Thorns) damage profile.
            //
            // SlitheringStrangler.cs has no AfterAddedToRoom override
            // either. Its `ConstrictMove` applies ConstrictPower(3)
            // to the *player target*, not itself — Python's "on-spawn
            // Constrict(3) on self" was a misread.
            //
            // SnappingJaxfruit.cs's AfterAddedToRoom plays SFX only.
            // The +2 Strength is applied mid-move after the charged
            // attack lands, not at spawn time.
            //
            // Myte.cs has no AfterAddedToRoom; the +Strength applied
            // by `SuckStrength` happens mid-move, not on spawn.
            //
            // All four removed; their move state machines still apply
            // the correct powers at the right turn.

            // sts2-rs-5dp60: C# PhrogParasite.cs:35 AfterAddedToRoom
            // applies InfestedPower(4) (not CurlUp). InfestedPower
            // spawns 4 Wrigglers on death — that on-death wiring is
            // a separate port concern; this fixes the on-spawn power
            // identity so parity-fuzz INIT diffs go green.
            MonsterKind::PhrogParasite => &[(PowerKind::Infested, 4)],
            // Bead `…wb8lk.11`: v0.107.1 nerfed Vantom's spawn Slippery
            // 9→8 at A0. Decomp `v0.107.1/…/Vantom.cs:64`
            // `SlipperyAmt => GetValueIfAscension(ToughEnemies, 9, 8)` —
            // A0=8, A8(ToughEnemies)=9. The on-spawn buff table is not
            // ascension-aware, so the A8 bump to 9 is not modeled here
            // (consistent with the other static on-spawn entries); the A0
            // value 8 is the live-parity-correct one. First 8 hits to
            // Vantom deal 1 damage each (SlipperyPower landed in
            // sts2-rs-12xvk; see hooks.rs Osty HP-loss arm).
            MonsterKind::Vantom => &[(PowerKind::Slippery, 8)],
            // sts2-rs-yqdlt: C# `BygoneEffigy.cs:35` AfterAddedToRoom
            // applies SlowPower(1) to self. Discovered by parity-fuzz
            // INIT diff (powers.len 0 vs 1 in 3/100 trials).
            MonsterKind::BygoneEffigy => &[(PowerKind::Slow, 1)],
            // sts2-rs-o1mwv (C.2): C# `EyeWithTeeth.cs:30`
            // `AfterAddedToRoom` applies `IllusionPower(1)` to self,
            // whose `AfterApplied` (IllusionPower.cs:67) chains
            // `MinionPower(1)` when the owner lacks it. Net spawn
            // powers: `[Illusion(1), Minion(1)]` (order matters for
            // the `monsters[0].powers.len` / `powers[*].kind` parity
            // buckets on FOGMOG trials).
            //
            // TODO(o1mwv): this hardcodes the chained Minion rather
            // than modeling `IllusionPower::AfterApplied`. The Illusion
            // *revive* mechanic (IllusionPower.cs AfterDeath →
            // REVIVE_MOVE heal-to-full + ShouldCreatureBeRemovedFrom-
            // CombatAfterDeath) and Minion's secondary-enemy / non-
            // fatal-death flags are NOT modeled — separate beads.
            MonsterKind::EyeWithTeeth => &[(PowerKind::Illusion, 1), (PowerKind::Minion, 1)],
            // C# `Byrdonis.cs:35` AfterAddedToRoom applies TerritorialPower(1)
            // to self. Territorial is a Counter Buff whose AfterTurnEnd grants
            // the owner Strength == Amount each of its own turn-ends (wired in
            // hooks::after_turn_end). Discovered by parity-fuzz INIT diff
            // (BYRDONIS_ELITE monsters[0].powers.len 0 vs 1, 33/500 trials).
            MonsterKind::Byrdonis => &[(PowerKind::Territorial, 1)],
            // sts2-rs-7iuyk (29kfv): C# `TerrorEel.cs:71` AfterAddedToRoom
            // applies `ShriekPower(70)` to self (75 ToughEnemies; identity at
            // pin 01c094d). ShriekPower is a Counter Debuff with an
            // `AfterDamageReceived`/Stun mechanic (ShriekPower.cs:25) that is
            // NOT modeled here — this only registers the on-spawn power for
            // INIT/powers.len parity, matching the Illusion/Minion convention
            // above. Deeper Shriek behavior is a separate bead.
            MonsterKind::TerrorEel => &[(PowerKind::Shriek, 70)],
            // sts2-rs-opbps: C# `MechaKnight.cs:72-75` AfterAddedToRoom
            // applies `ArtifactPower(3)` to self
            // (`PowerCmd.Apply<ArtifactPower>(base.Creature, 3m, …)`).
            // Artifact negates the next 3 debuffs applied to the elite —
            // registering it on spawn matches the C# initial power set
            // (INIT/powers.len parity) the same way as the Illusion/
            // Shriek arms above.
            MonsterKind::MechaKnight => &[(PowerKind::Artifact, 3)],
            // sts2-rs-en6mn (slice of jnf4a): C# `TestSubject.cs:169-173`
            // AfterAddedToRoom applies AdaptablePower(1) to self
            // (`PowerCmd.Apply<AdaptablePower>(base.Creature, 1m, …)`).
            // Adaptable's DoRevive() + on-death intercept
            // (TestSubject.cs:225-273) and the phase-2/3 movesets are
            // still under the parent jnf4a bead; this entry only
            // registers the on-spawn power identity for INIT/powers.len
            // parity, following the Illusion/Minion/Shriek/Artifact
            // convention above.
            //
            // sts2-rs-vawqf (uy6g9 follow-up): C# AfterAddedToRoom also
            // applies `EnragePower(EnrageAmount)` (`TestSubject.cs:171`)
            // where EnrageAmount = 2 at non-ascension
            // (`AscensionHelper.GetValueIfAscension(DeadlyEnemies, 3, 2)`).
            // Ascension is identity at pin `01c094d` per
            // `project_ascension_identity_at_pin`, so the static `2` is
            // canonical here.
            MonsterKind::TestSubject => &[(PowerKind::Adaptable, 1), (PowerKind::Enrage, 2)],
            // sts2-rs-7h9y2: C# `Axebot.cs:76-82` AfterAddedToRoom
            // applies `StockPower(StockAmount)` when StockAmount > 0;
            // default StockAmount = 2 (Axebot.cs:50-54
            // `_stockOverrideAmount.GetValueOrDefault(2)`). Axebot
            // appears as a standalone encounter (encounters.rs:426,430)
            // with no Fabricator override at this pin, so the static
            // default 2 is the correct registration. Stock's listener
            // (StockPower mechanic) is already modeled engine-side —
            // this only closes the INIT/powers.len parity gap.
            MonsterKind::Axebot => &[(PowerKind::Stock, 2)],
            // sts2-rs-u97h0: C# `FrogKnight.cs:59-64` AfterAddedToRoom
            // applies `PlatingPower(PlatingAmount)`; PlatingAmount = 15
            // non-ascension (19 ToughEnemies; identity at pin 01c094d).
            // FrogKnight is a standalone encounter — no minion-spawn
            // double-apply concern. Plating's listener is already
            // modeled (existing SlumberingBeetle entry shares the kind).
            MonsterKind::FrogKnight => &[(PowerKind::Plating, 15)],
            // sts2-rs-enb55: C# `SewerClam.cs:28-33` AfterAddedToRoom
            // applies `PlatingPower(8)` non-asc (9 ToughEnemies;
            // identity at pin 01c094d). Plating listener already
            // modeled. Closes INIT/powers.len parity gap.
            MonsterKind::SewerClam => &[(PowerKind::Plating, 8)],
            // sts2-rs-9djjh: All three BattleFriend variants apply
            // `BattlewornDummyTimeLimitPower(3)` on spawn per identical
            // C# `BattleFriendV{1,2,3}.cs:32-35` AfterAddedToRoom
            // (`PowerCmd.Apply<BattlewornDummyTimeLimitPower>(base.Creature, 3m, null, null)`).
            // BattlewornDummyTimeLimit's listener is already in
            // power_ids_generated.rs:136. Closes INIT/powers.len parity
            // gap for BattleFriend training-dummy encounters.
            MonsterKind::BattleFriendV1 => &[(PowerKind::BattlewornDummyTimeLimit, 3)],
            MonsterKind::BattleFriendV2 => &[(PowerKind::BattlewornDummyTimeLimit, 3)],
            MonsterKind::BattleFriendV3 => &[(PowerKind::BattlewornDummyTimeLimit, 3)],
            // sts2-rs-43ky2: C# `TorchHeadAmalgam.cs:40-44`
            // AfterAddedToRoom applies `MinionPower(1)` to self
            // (`PowerCmd.Apply<MinionPower>(base.Creature, 1m, …)`).
            // TorchHeadAmalgam is paired with Queen in encounters; Queen
            // references TorchHead at Queen.cs:120,215 only via
            // `Enemies.First((c) => c.Monster is TorchHeadAmalgam)` —
            // does NOT call `Apply<MinionPower>` itself, so no double-
            // apply concern. Minion's listener is already modeled
            // (EyeWithTeeth/KinFollower share it).
            MonsterKind::TorchHeadAmalgam => &[(PowerKind::Minion, 1)],
            // sts2-rs-8442w: C# `Rocket.cs` AfterAddedToRoom applies
            // three powers — `SurroundedPower(1)` to *opponents* (target
            // = `base.CombatState.GetOpponentsOf(base.Creature)`),
            // `BackAttackRightPower(1)` to self, and `CrabRagePower(1)`
            // to self. The Surrounded application is opponent-targeted
            // and NOT a self-buff, so it lives in the opponent-targeted
            // table [`Self::on_spawn_opponent_debuffs`] (bead
            // `sts2-rs-ybtmu`) rather than this self-only one.
            MonsterKind::Rocket => &[(PowerKind::BackAttackRight, 1), (PowerKind::CrabRage, 1)],
            // sts2-rs-cezr9 (corrects ysele): SpinyToad's C# `AfterAddedToRoom`
            // is actually `Task.CompletedTask` — there is NO Thorns apply
            // on spawn. The +5 Thorns is gated behind the `SpikesMove`
            // (PROTRUDING_SPIKES) state-machine move, and stripped by
            // `ExplosionMove` (-5m). The ysele audit-doc misattributed
            // Thorns(5) to SpinyToad's AfterAddedToRoom; the actual
            // C# only adds Thorns inside SpikesMove. Wiring the spawn
            // buff caused the toad to enter combat already spiked, and
            // every SPINES then stacked to 10/15/… until cezr9 added
            // the EXPLOSION strip. Empty arm now (no on-spawn powers).
            // sts2-rs-ztx5u: AfterAddedToRoom `ArtifactPower` applies on
            // these normal robots/foes per C# decomp (negates the next N
            // debuffs). Registered for INIT/powers.len parity following
            // the convention above; Artifact's debuff-negation hook is
            // already modeled engine-side.
            //   `Chomper.cs`        → ArtifactPower(2)
            //   `CubexConstruct.cs` → ArtifactPower(1)
            //   `PunchConstruct.cs` → ArtifactPower(1)
            MonsterKind::Chomper => &[(PowerKind::Artifact, 2)],
            MonsterKind::CubexConstruct => &[(PowerKind::Artifact, 1)],
            MonsterKind::PunchConstruct => &[(PowerKind::Artifact, 1)],
            // sts2-rs-ztx5u: `Inklet.cs` AfterAddedToRoom applies
            // `SlipperyPower(1)` to self (first incoming hit deals 1 dmg,
            // same mechanic as Vantom's Slippery wall above).
            MonsterKind::Inklet => &[(PowerKind::Slippery, 1)],
            // sts2-rs-rop3f: on-spawn self-buffs verified against C#
            // v0.103.2 `*.cs::AfterAddedToRoom` (non-ascension identity
            // amounts). Registered for INIT/powers.len parity following
            // the Artifact/Shriek convention above; where a power's
            // behavior hook is not yet modeled, the on-spawn power
            // identity is still correct and the deeper mechanic is a
            // separate concern.
            //   `GlobeHead.cs:41`   → GalvanicPower(6)
            MonsterKind::GlobeHead => &[(PowerKind::Galvanic, 6)],
            //   `SlumberingBeetle.cs:75-76` → PlatingPower(15) then
            //   SlumberPower(3). PlatingAmount=15 non-asc (18 ToughEnemies).
            //   Order matches C# (Plating applied first).
            MonsterKind::SlumberingBeetle => &[(PowerKind::Plating, 15), (PowerKind::Slumber, 3)],
            //   `Entomancer.cs:37`  → PersonalHivePower(1)
            MonsterKind::Entomancer => &[(PowerKind::PersonalHive, 1)],
            //   `ThievingHopper.cs:151` → EscapeArtistPower(5)
            MonsterKind::ThievingHopper => &[(PowerKind::EscapeArtist, 5)],
            //   `Zapbot.cs:36`      → HighVoltagePower(2)
            MonsterKind::Zapbot => &[(PowerKind::HighVoltage, 2)],
            //   `Exoskeleton.cs:48` → HardToKillPower(9)
            MonsterKind::Exoskeleton => &[(PowerKind::HardToKill, 9)],
            //   `InfestedPrism.cs:59` → VitalSparkPower(VitalSparkAmount).
            //   `…wb8lk.11`: v0.107.1 reworked the spawn amount from a flat
            //   1m to `VitalSparkAmount = GetValueIfAscension(DeadlyEnemies,
            //   3, 2)` (A0=2, A9=3). The on-spawn table is not ascension-
            //   aware, so we register the A0 value 2; the A9 bump to 3 is a
            //   declared deferral (same limitation as the other static
            //   on-spawn arms). VitalSpark is the in-combat Tainted applier
            //   (bead `…wb8lk.4`).
            MonsterKind::InfestedPrism => &[(PowerKind::VitalSpark, 2)],
            // sts2-rs-rop3f (batch 2): further C# `AfterAddedToRoom`
            // self-powers, verified against the decomp (non-ascension
            // identity amounts). Same INIT/powers.len-parity convention.
            //   `FossilStalker.cs`   → SuckPower(3)
            MonsterKind::FossilStalker => &[(PowerKind::Suck, 3)],
            //   `BowlbugRock.cs`     → ImbalancedPower(1)
            MonsterKind::BowlbugRock => &[(PowerKind::Imbalanced, 1)],
            //   `DecimillipedeSegment.cs` → ReattachPower(25). The
            //   Front/Middle/Back subclasses (`DecimillipedeSegment{Front,
            //   Middle,Back}.cs`) inherit `AfterAddedToRoom` unchanged, so
            //   they grant the same Reattach(25). The `DECIMILLIPEDE_ELITE`
            //   roster spawns the three positional variants (bead
            //   `sts2-rs-8mpo1`); without this arm the parity override path —
            //   which resolves C#'s `DECIMILLIPEDE_SEGMENT_{FRONT,MIDDLE,BACK}`
            //   ids to those kinds — dropped the power (`powers.len` 1-vs-0).
            MonsterKind::DecimillipedeSegment
            | MonsterKind::DecimillipedeSegmentFront
            | MonsterKind::DecimillipedeSegmentMiddle
            | MonsterKind::DecimillipedeSegmentBack => &[(PowerKind::Reattach, 25)],
            //   `SkulkingColony.cs:56` → HardenedShellPower(20). `…wb8lk.11`:
            //   v0.107.1 bumped the spawn HardenedShell 15→20.
            MonsterKind::SkulkingColony => &[(PowerKind::HardenedShell, 20)],
            //   `LouseProgenitor.cs:69` → CurlUpPower(CurlBlock=14 non-asc)
            MonsterKind::LouseProgenitor => &[(PowerKind::CurlUp, 14)],
            //   `PhantasmalGardener.cs:92` → SkittishPower(SkittishAmount=6 non-asc)
            MonsterKind::PhantasmalGardener => &[(PowerKind::Skittish, 6)],
            //   `CorpseSlug.cs:85`   → RavenousPower(RavenousStr=4 non-asc)
            MonsterKind::CorpseSlug => &[(PowerKind::Ravenous, 4)],
            // sts2-rs-jj9gv: further C# `AfterAddedToRoom` self-powers,
            // verified against the v0.103.2 decomp (non-ascension identity
            // amounts). Same INIT/powers.len-parity convention as above.
            //   `TheLost.cs`        → PossessStrengthPower(1)
            MonsterKind::TheLost => &[(PowerKind::PossessStrength, 1)],
            //   `ScrollOfBiting.cs` → PaperCutsPower(2)
            MonsterKind::ScrollOfBiting => &[(PowerKind::PaperCuts, 2)],
            //   `MysteriousKnight.cs` → StrengthPower(6) then PlatingPower(6)
            //   (both to self; order matches C# — Strength applied first).
            MonsterKind::MysteriousKnight => &[(PowerKind::Strength, 6), (PowerKind::Plating, 6)],
            // sts2-rs-arn7n (batch 2): further C# `AfterAddedToRoom`
            // self-powers (literal, non-ascension amounts). Same
            // INIT/powers.len-parity convention as above.
            //   `Crusher.cs`            → BackAttackLeftPower(1) then CrabRagePower(1)
            MonsterKind::Crusher => &[(PowerKind::BackAttackLeft, 1), (PowerKind::CrabRage, 1)],
            //   `LivingShield.cs`       → RampartPower(25)
            MonsterKind::LivingShield => &[(PowerKind::Rampart, 25)],
            //   `LagavulinMatriarch.cs` → PlatingPower(12) then AsleepPower(3)
            //   (both self; order matches C# — Plating applied first).
            MonsterKind::LagavulinMatriarch => &[(PowerKind::Plating, 12), (PowerKind::Asleep, 3)],
            // sts2-rs-tfsj5 (batch 3): further C# `AfterAddedToRoom`
            // self-powers (literal amounts). Same INIT/powers.len-parity
            // convention. (TheAdversaryMkOne is omitted — its C# applies
            // ArtifactPower(0), a no-op stack.)
            //   `Parafright.cs` → IllusionPower(1) — summoned by TheObscura
            //   via spawn_enemy_at_front (auto-applies on-spawn buffs).
            //   C# `IllusionPower.AfterApplied` (IllusionPower.cs:59-65)
            //   grants MinionPower(1) to any creature that gains Illusion
            //   without it, so the Parafright reads [Illusion, Minion] like
            //   the EyeWithTeeth arm above. Bead `sts2-rs-ybtmu` — the
            //   missing Minion entry left the summoned Parafright at
            //   powers.len 1 vs C#'s 2 (Act-2/3 coverage sweep,
            //   encounter:THE_OBSCURA_NORMAL).
            MonsterKind::Parafright => &[(PowerKind::Illusion, 1), (PowerKind::Minion, 1)],
            //   `GremlinMerc.cs` → SurprisePower(1) then ThieveryPower(20)
            //   (both on self; the Thievery's player-Target is a behavior
            //   nuance, separate from this powers.len-parity registration).
            MonsterKind::GremlinMerc => &[(PowerKind::Surprise, 1), (PowerKind::Thievery, 20)],
            //   `TheAdversaryMkTwo.cs`   → ArtifactPower(1)
            //   `TheAdversaryMkThree.cs` → ArtifactPower(2)
            MonsterKind::TheAdversaryMkTwo => &[(PowerKind::Artifact, 1)],
            MonsterKind::TheAdversaryMkThree => &[(PowerKind::Artifact, 2)],
            // sts2-rs-qw18q: on-spawn `MinionPower(1)` for minions reached
            // via the auto-buffing `spawn_enemy` path (NOT the Fabricator
            // bots, which apply Minion manually in their summon effect).
            // Mirrors the EyeWithTeeth Minion arm above.
            //   `KinFollower.cs` → MinionPower(1) — encounter-placed
            //   `GasBomb.cs`     → MinionPower(1) — LivingFog BLOAT spawn
            MonsterKind::KinFollower => &[(PowerKind::Minion, 1)],
            MonsterKind::GasBomb => &[(PowerKind::Minion, 1)],
            // sts2-rs-u278v: C# `TheForgotten.cs:36-38` AfterAddedToRoom
            // applies `PossessSpeedPower(1)` to self. (The DREAD dynamic-
            // damage scaling + moveset rebase remain in sts2-rs-96y80.)
            MonsterKind::TheForgotten => &[(PowerKind::PossessSpeed, 1)],
            // Aeonglass (v0.107.1): C# `Aeonglass.AfterAddedToRoom`
            // (`Aeonglass.cs:78-84`) applies BOTH `WitheringPresence(6)` AND
            // `ArtifactPower(3)` to ITSELF: `PowerCmd.Apply(wp,
            // base.Creature, 6m, base.Creature, null)` — owner == target ==
            // the monster. WitheringPresence is `Instanced` and logically
            // targets the player, but the power *lives on Aeonglass*, so the
            // player carries no power (C# parity: monster 2 powers, player 0).
            // The reaction (player card plays decrement the monster's
            // `CardsLeft`) is in `crate::hooks`. Bead `…wb8lk.2`.
            //
            // ORDER matters for the serialized `monsters[*].powers[*]` list:
            // C# applies WitheringPresence FIRST (inside the opponents loop,
            // `Aeonglass.cs:78-83`) and Artifact SECOND (`:84`) — the prior
            // Artifact-first order diverged on the wire and, latently, would
            // have let an Artifact-strip observe the wrong slot.
            MonsterKind::Aeonglass => {
                &[(PowerKind::WitheringPresence, 6), (PowerKind::Artifact, 3)]
            }
            _ => &[],
        }
    }

    /// Bead `sts2-rs-ybtmu`: static on-spawn powers a monster applies to
    /// its **opponents** (the player) inside C# `AfterAddedToRoom`, the
    /// opponent-targeted counterpart of [`Self::on_spawn_self_buffs`].
    /// Applied to the player by
    /// [`crate::CombatState::apply_creature_spawn_buffs`] after the
    /// self-buffs.
    ///
    /// Only `Rocket.cs` uses this at the current pin:
    /// `PowerCmd.Apply<SurroundedPower>(GetOpponentsOf(Creature), 1)` —
    /// the KaiserCrab boss arm surrounds the player so the other arm's
    /// back-attack lands 1.5× (see `SurroundedPower` in
    /// `crates/sts2-core/src/hooks.rs`).
    #[must_use]
    pub fn on_spawn_opponent_debuffs(self) -> &'static [(crate::powers::PowerKind, i32)] {
        use crate::powers::PowerKind;
        match self {
            MonsterKind::Rocket => &[(PowerKind::Surrounded, 1)],
            // Aeonglass's WitheringPresence is monster-owned (see
            // `on_spawn_self_buffs`), NOT an opponent debuff.
            _ => &[],
        }
    }

    /// Bead `sts2-rs-caz92`: static on-spawn block grant. Parallels
    /// [`Self::on_spawn_self_buffs`] but for `CreatureCmd.GainBlock`
    /// calls inside C# `AfterAddedToRoom`. Returns `0` for the default
    /// case (no spawn-time block). Consumed by
    /// [`crate::CombatState::apply_creature_spawn_buffs`] which adds
    /// the amount to the creature's starting block after powers apply.
    ///
    /// Currently only `CubexConstruct.cs` grants block at spawn (13
    /// via `ValueProp.Move` — powered, but Frail/Dexterity don't apply
    /// at construction so it lands raw). Other monsters that gain
    /// block use mid-combat `Buff` intents, not spawn-time grants.
    #[must_use]
    pub fn on_spawn_block(self) -> i32 {
        match self {
            // CubexConstruct.cs:84 — GainBlock(creature, 13, Move).
            MonsterKind::CubexConstruct => 13,
            _ => 0,
        }
    }

    /// Bead `sts2-rs-y4e4z`: minimum MCTS rollout depth for combats
    /// containing this monster. `None` = no recommendation (caller
    /// honors its own default). `Some(d)` = the rollout horizon must
    /// reach at least `d` plies to evaluate this enemy faithfully.
    ///
    /// **Vantom (50)**: 4-turn cycle (InkBlot → InkyLance → Dismember
    /// → Prepare) + SlipperyPower(9) wall on incoming damage. Depth-10
    /// rollouts terminate before Dismember (turn 3 of the cycle)
    /// resolves, so the search doesn't see the cliff or the post-
    /// Slippery damage potential. Empirical sweep
    /// (`docs/perf/2026-05-24-vantom-depth-sweep.md`, 10 fixtures ×
    /// sims=10000): depth=10 → 0%, depth=20 → 10%, depth=50 → 60%,
    /// depth=100 → 50%. Knee at 50.
    #[must_use]
    pub fn recommended_rollout_depth(self) -> Option<u32> {
        match self {
            MonsterKind::Vantom => Some(50),
            _ => None,
        }
    }

    /// No-AI fallback intent: a single generic [`Intent::Attack`].
    ///
    /// In the canonical combat path every enemy carries a [`MonsterAi`]
    /// (attached via `sts2-content::monster_dispatch::build_ai`) and
    /// `combat_loop`'s intent-priming step reads
    /// `creature.ai.current_move().intents.first()`. This method is only
    /// reached when `creature.ai.is_none()` — i.e. by `ai: None` test
    /// fixtures that use it as a deliberately-minimal monster brain to
    /// fuzz the combat loop (`random_playout_fuzz`, `parity_shrinker`,
    /// the `enemy_turn_exec` unit tests). Those harnesses need the
    /// monster to *attack* so the playout terminates, so a bare
    /// `Unknown` won't do.
    ///
    /// This previously held a per-`MonsterKind` intent table (~130 arms)
    /// mirrored from the Python sim's `intents.py`. That was a drift
    /// trap: the authoritative per-monster intent lives in the DSL move
    /// state machine (C# `MonsterModel.GenerateMoveStateMachine`, which
    /// has no parallel table), so the duplicated literals silently
    /// diverged (e.g. AssassinRubyRaider 15 vs real KILLSHOT 11,
    /// SoulFysh 18 vs 16/7/11). Collapsed to one generic attack so the
    /// fallback can never lie about a specific monster again
    /// (sts2-rs-8c7lb). `self`/`_rng`/`_history` are retained for the
    /// `combat_loop` call-site signature.
    ///
    /// # ⚠️ Do NOT use this for faithfulness/parity tests
    ///
    /// This return value is a **generic stub, not the monster's real
    /// behaviour** — it is `Attack{5}` for *every* `MonsterKind`,
    /// regardless of the monster's actual moveset, intents, damage, or
    /// transitions. It is correct *only* for tests of the engine itself
    /// (combat-loop invariants, the re-roll mechanism, damage/RNG
    /// determinism) that need a minimal attacker and do not assert
    /// game-faithful enemy behaviour.
    ///
    /// Any test that checks a monster behaves like the real game (exact
    /// damage, move cycle, telegraph, spawn powers, death removal) MUST
    /// attach the real DSL state machine via
    /// `sts2-content::monster_dispatch::build_ai` and read intents from
    /// `creature.ai.current_move()` — never from this fallback. Relying
    /// on this for faithfulness will silently pass against a wrong value
    /// (that is exactly how the AssassinRubyRaider/SoulFysh divergences
    /// above went undetected: no faithfulness test touches this path).
    #[must_use]
    #[allow(clippy::unused_self)]
    pub fn roll_intent(self, _rng: &mut CombatRng, _history: IntentHistory) -> Intent {
        Intent::Attack { damage: 5 }
    }
}

// ---------------------------------------------------------------------------
// Intent — first-class enum mirroring Python's IntentType + damage/hits
// ---------------------------------------------------------------------------

/// The intent surfaced for a single enemy on the upcoming turn.
///
/// Mirrors Python `sts2_env/monsters/intents.py::Intent` but as a sum
/// type so the listener can pattern-match without having to dig into a
/// `damage: i32` field that is meaningless on `Defend`/`Buff`/`Debuff`.
///
/// The parity exporter's intent-emit step
/// (`combat_state_to_bridge_state` lines 333-339) reads
/// `first_intent.intent_type.name` (the Python `IntentType.X.name`
/// string), `first_intent.damage`, and `first_intent.hits`. Our
/// [`Intent::python_name`] returns a string matching `IntentType.X.name`
/// for every variant; [`Intent::damage`] and [`Intent::hits`] return the
/// Python defaults (0 and 1 respectively) for non-attack variants —
/// matching `Intent.damage = 0, hits = 1` defaults at intents.py:14-15.
/// TestSubject form HP constants — used by the future Revive impl
/// (bead `sts2-rs-jnf4a`) to set `current_hp/max_hp` when the
/// AdaptablePower.DoRevive() chain fires on TestSubject's death.
///
/// Mirrors C# `TestSubject.cs:72-80`:
/// [decompiled C# omitted — see the cited path]
///
/// Non-ascension identity (ascension is identity at pin `01c094d`
/// per `project_ascension_identity_at_pin`). The ToughEnemies
/// branch is included as a doc-comment reference for the eventual
/// ascension-aware port.
///
/// Bead `sts2-rs-7jgab` (slice of `jnf4a`).
pub mod test_subject {
    /// Initial spawn HP (matches `MonsterKind::TestSubject` HpRoll
    /// at `Range(100, 100)` in this file). 111 on ToughEnemies.
    pub const FIRST_FORM_HP: i32 = 100;
    /// HP set on first Revive (TestSubject.cs:267
    /// `await Revive(SecondFormHp);`). 212 on ToughEnemies.
    pub const SECOND_FORM_HP: i32 = 200;
    /// HP set on second Revive (TestSubject.cs:271
    /// `await Revive(ThirdFormHp);`). 313 on ToughEnemies.
    pub const THIRD_FORM_HP: i32 = 300;

    // ── Phase-3 move constants (sts2-rs-94lp7) ─────────────────────
    //
    // C# `TestSubject.cs:88-157` defines the MULTI_CLAW phase-3 attack
    // shape. Non-ascension identity; DeadlyEnemies branch noted as a
    // doc-comment reference for the eventual ascension port.

    /// Per-hit damage of `MULTI_CLAW_MOVE` (C# `MultiClawDamage` at
    /// `TestSubject.cs:88`). 11 on DeadlyEnemies.
    pub const MULTI_CLAW_DAMAGE: i32 = 10;
    /// Base hit count of `MULTI_CLAW_MOVE` (C# `BaseMultiClawCount`
    /// at `TestSubject.cs:90`). Each repeat of MULTI_CLAW adds one
    /// more hit via `ExtraMultiClawCount++` (`TestSubject.cs:304`),
    /// so the actual hit count is `BASE_MULTI_CLAW_COUNT +
    /// extra_multi_claw_count`. The `extra` is per-combat scratch
    /// — the future port should hold it on the AI state, not on a
    /// global.
    pub const BASE_MULTI_CLAW_COUNT: i32 = 3;

    // ── Phase-1 / phase-3 attack damage (sts2-rs-9o3gh) ────────────

    /// Damage of `BITE_MOVE` (C# `BiteDamage` at `TestSubject.cs:84`).
    /// 22 on DeadlyEnemies.
    pub const BITE_DAMAGE: i32 = 20;
    /// Damage of `SKULL_BASH_MOVE` (C# `SkullBashDamage` at
    /// `TestSubject.cs:86`). 16 on DeadlyEnemies.
    pub const SKULL_BASH_DAMAGE: i32 = 14;
    /// Per-hit damage of `PHASE3_LACERATE_MOVE` (C#
    /// `Phase3LacerateDamage` at `TestSubject.cs:92`). 11 on
    /// DeadlyEnemies. Hit-count comes from C# `MultiAttackIntent(.., 3)`.
    pub const PHASE3_LACERATE_DAMAGE: i32 = 10;
    /// Damage of `BIG_POUNCE_MOVE` (C# `BigPounceDamage` at
    /// `TestSubject.cs:94`). Not ascension-scaled in C#.
    pub const BIG_POUNCE_DAMAGE: i32 = 45;
    /// Burn-card count applied by `BURNING_GROWL_MOVE` (C#
    /// `BurningGrowlBurnCount` at `TestSubject.cs:96`). 5 on
    /// DeadlyEnemies. `BURNING_GROWL` also applies StrengthPower(2)
    /// per the C# body — that's a separate concern.
    pub const BURNING_GROWL_BURN_COUNT: i32 = 3;

    /// `BurningGrowlBurnCount` at `AscensionLevel.DeadlyEnemies`
    /// (`TestSubject.cs:102` `GetValueIfAscension(DeadlyEnemies, 5, 3)`).
    pub const BURNING_GROWL_BURN_COUNT_DEADLY: i32 = 5;

    /// `BurningGrowlStrengthGain` — `TestSubject.cs:104`
    /// `GetValueIfAscension(DeadlyEnemies, 3, 2)`.
    pub const BURNING_GROWL_STRENGTH: i32 = 2;
    /// A9 `BurningGrowlStrengthGain`. See [`BURNING_GROWL_STRENGTH`].
    pub const BURNING_GROWL_STRENGTH_DEADLY: i32 = 3;

    // ── Revive() power amounts (sts2-rs-oj1y1) ─────────────────────
    //
    // Per C# `TestSubject.cs:243-280` the Adaptable.DoRevive chain
    // applies these powers as the TestSubject transitions between
    // forms. The full flow:
    //   - Respawns == 1 → `Revive(SecondFormHp)` +
    //     `Apply<PainfulStabsPower>(base.Creature, 1m, ...)`
    //     (`TestSubject.cs:267-268`).
    //   - Respawns == 2 → `Revive(ThirdFormHp)` +
    //     `Apply<NemesisPower>(base.Creature, 1m, ...)` +
    //     `Remove<PainfulStabsPower>(base.Creature)`
    //     (`TestSubject.cs:271-274`).
    //
    // These are amounts only — the actual Revive flow is jnf4a.

    /// PainfulStabs stack applied on first Revive
    /// (`TestSubject.cs:268`). Removed on second Revive.
    pub const PAINFUL_STABS_AMOUNT: i32 = 1;
    /// Nemesis stack applied on second Revive
    /// (`TestSubject.cs:272`).
    pub const NEMESIS_AMOUNT: i32 = 1;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Intent {
    /// `IntentType.ATTACK` — single-hit attack.
    /// Python: `attack_intent(damage)`.
    Attack { damage: i32 },
    /// `IntentType.MULTI_ATTACK` — `hits` strikes of `damage` each.
    /// Python: `multi_attack_intent(damage, hits)`.
    MultiAttack { damage: i32, hits: u8 },
    /// `IntentType.DEFEND` — gain block. The block amount is the
    /// monster's per-move constant; Python's `defend_intent()` carries
    /// no payload (`intents.py:35-36`) — the Rust enrichment encodes
    /// the per-move constant at the call site so per-monster audits
    /// can verify it without a separate effect_fn lookup. Parity
    /// exporter remains payload-blind (Python emits no block field
    /// on intents). Bead `sts2-rs-1usm`.
    ///
    /// `block = 0` is the "unspecified / look at the move's
    /// effect_fn" sentinel — audits should refine in-place.
    Defend { block: i32 },
    /// `IntentType.BUFF` — apply self-buff (Strength, Ritual, etc).
    /// Python's `buff_intent()` is unit-payload (`intents.py:39-40`);
    /// the Rust enrichment encodes the buff `kind` + `amount` at the
    /// call site so per-monster audits can verify them without a
    /// separate effect_fn lookup. Bead `sts2-rs-1usm`.
    ///
    /// **Unspecified sentinel:** `kind = PowerKind::Strength,
    /// amount = 0` is the placeholder used by the mechanical
    /// migration of pre-1usm callsites — it means "the actual buff
    /// specifics live in the move's effect_fn closure; an audit bead
    /// should refine these fields against the Python pin".
    Buff {
        kind: crate::powers::PowerKind,
        amount: i32,
    },
    /// `IntentType.DEBUFF` — apply weak-strength debuff to the player.
    /// Same shape as [`Self::Buff`]; see that variant's doc-comment
    /// for the unspecified sentinel convention. Bead `sts2-rs-1usm`.
    ///
    /// **Unspecified sentinel:** `kind = PowerKind::Weak,
    /// amount = 0` is the placeholder used by the mechanical
    /// migration.
    Debuff {
        kind: crate::powers::PowerKind,
        amount: i32,
    },
    /// `IntentType.DEBUFF_STRONG` — apply a stronger debuff (e.g. Vulnerable+Weak).
    DebuffStrong,
    /// `IntentType.SLEEP` — skip turn (Lagavulin).
    Sleep,
    /// `IntentType.STUN` — stunned, skip turn.
    Stun,
    /// `IntentType.STATUS_CARD` — add status card to player's discard.
    StatusCard,
    /// `IntentType.UNKNOWN` — telegraphed when stunned and the next
    /// move has not been chosen yet.
    Unknown,
    /// `IntentType.SUMMON` — spawn one or more new enemies into the
    /// fight. Mirrors Python `Intent(IntentType.SUMMON)`
    /// (`sts2-rl-agent@01c094d:sts2_env/monsters/intents.py`). Used by
    /// Vantom's `CONSUME` state to telegraph the 2 Parafrights spawn
    /// (`sts2_env/monsters/act1.py:586`). Bead `sts2-rs-47p5`.
    Summon,
    /// `IntentType.HEAL` — restore HP to self (or to an ally). Mirrors
    /// Python `Intent(IntentType.HEAL)`. Used by SoulNexus's
    /// `SOUL_DRAIN` slot (Attack + Heal multi-intent at
    /// `sts2_env/monsters/act3.py:671`) and DecimillipedeSegment's
    /// `REATTACH` (`sts2_env/monsters/act2.py:488`). The healing
    /// arithmetic itself dispatches via the per-move `effect_fn` (same
    /// shape as `Intent::Summon` and the Phase-5 dispatcher landed in
    /// bead `sts2-rs-vn0m`); this variant only carries the UI / parity
    /// telegraph tag. Bead `sts2-rs-va5r`.
    Heal,
    /// `IntentType.DEATH_BLOW` — single-hit attack that kills the
    /// dealer when the move resolves. Mirrors C#
    /// `MegaCrit.Sts2.Core.MonsterMoves.Intents.DeathBlowIntent`
    /// (a `SingleAttackIntent` with `IntentType.DeathBlow`) — the
    /// move body in C# is `DamageCmd.Attack(...)` followed by
    /// `CreatureCmd.Kill(base.Creature)`. Used by GasBomb EXPLODE
    /// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/GasBomb.cs`)
    /// and WaterfallGiant AboutToBlow → EXPLODE (deferred sibling
    /// bead `sts2-rs-wz8qc`). Bead `sts2-rs-f99e7`.
    DeathBlow { damage: i32 },
    /// `IntentType.ESCAPE` — the monster flees combat. Mirrors C#
    /// `MegaCrit.Sts2.Core.MonsterMoves.Intents.EscapeIntent`
    /// (an `AbstractIntent` with `IntentType.Escape`, prefix
    /// `"ESCAPE"`). Used by FatGremlin FLEE
    /// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/FatGremlin.cs`:
    /// `FleeMove` ends with `CreatureCmd.Escape(base.Creature)`).
    /// Carries no payload — the per-move effect is "remove self from
    /// combat", handled at the `perform_intent` dispatch site (Rust
    /// keeps corpses, so removal manifests as `current_hp = 0` on the
    /// fleeing creature; same convention as `Intent::DeathBlow` /
    /// `project_dead_enemy_array_removal_divergence`). Bead
    /// `sts2-rs-8f336`.
    Escape,
}

impl Intent {
    /// Whether the game shows this debuff as a `CardDebuffIntent`: its power
    /// afflicts the player's cards or takes one (`VineShambler.cs:47`
    /// Tangled, `LivingFog.cs:64` Smoggy, `Queen.cs:135` Chains of Binding,
    /// `ThievingHopper.cs:166` the steal its `SwipePower` holds). The engine
    /// keeps these as [`Intent::Debuff`]; only the wire's name differs.
    #[must_use]
    pub const fn is_card_debuff(self) -> bool {
        matches!(
            self,
            Self::Debuff {
                kind: crate::powers::PowerKind::Tangled
                    | crate::powers::PowerKind::Smoggy
                    | crate::powers::PowerKind::ChainsOfBinding
                    | crate::powers::PowerKind::Swipe,
                ..
            }
        )
    }

    /// Python `IntentType.<self>.name` exactly. Used by the parity
    /// exporter's `enemy_data["intent"]` field
    /// (`bridge_replay.py:337`).
    #[must_use]
    pub const fn python_name(self) -> &'static str {
        match self {
            Intent::Attack { .. } => "ATTACK",
            Intent::MultiAttack { .. } => "MULTI_ATTACK",
            Intent::Defend { .. } => "DEFEND",
            Intent::Buff { .. } => "BUFF",
            Intent::Debuff { .. } => "DEBUFF",
            Intent::DebuffStrong => "DEBUFF_STRONG",
            Intent::Sleep => "SLEEP",
            Intent::Stun => "STUN",
            Intent::StatusCard => "STATUS_CARD",
            Intent::Unknown => "UNKNOWN",
            Intent::Summon => "SUMMON",
            Intent::Heal => "HEAL",
            Intent::DeathBlow { .. } => "DEATH_BLOW",
            Intent::Escape => "ESCAPE",
        }
    }

    /// Damage component for the parity exporter's `intent_damage` field.
    /// Mirrors Python `Intent.damage` default of 0 for non-attack moves
    /// (`intents.py:15`).
    #[must_use]
    pub const fn damage(self) -> i32 {
        match self {
            Intent::Attack { damage }
            | Intent::MultiAttack { damage, .. }
            | Intent::DeathBlow { damage } => damage,
            _ => 0,
        }
    }

    /// Hit count for the parity exporter's `intent_hits` field. Mirrors
    /// Python `Intent.hits` default of 1 (`intents.py:16`).
    #[must_use]
    pub const fn hits(self) -> u8 {
        match self {
            Intent::MultiAttack { hits, .. } => hits,
            _ => 1,
        }
    }

    /// Discriminant tag — used by [`IntentHistory::last_kind`] to enforce
    /// "no two attacks in a row" / repeat-limit rules without the caller
    /// having to destructure the inner damage/hits fields.
    #[must_use]
    pub const fn kind(self) -> IntentKindTag {
        match self {
            Intent::Attack { .. } => IntentKindTag::Attack,
            Intent::MultiAttack { .. } => IntentKindTag::MultiAttack,
            Intent::Defend { .. } => IntentKindTag::Defend,
            Intent::Buff { .. } => IntentKindTag::Buff,
            Intent::Debuff { .. } => IntentKindTag::Debuff,
            Intent::DebuffStrong => IntentKindTag::DebuffStrong,
            Intent::Sleep => IntentKindTag::Sleep,
            Intent::Stun => IntentKindTag::Stun,
            Intent::StatusCard => IntentKindTag::StatusCard,
            Intent::Unknown => IntentKindTag::Unknown,
            Intent::Summon => IntentKindTag::Summon,
            Intent::Heal => IntentKindTag::Heal,
            Intent::DeathBlow { .. } => IntentKindTag::DeathBlow,
            Intent::Escape => IntentKindTag::Escape,
        }
    }
}

/// Variant tag for [`IntentHistory::last_kind`]. Strips the inner data so
/// the history struct stays POD-`Copy` and small.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IntentKindTag {
    Attack,
    MultiAttack,
    Defend,
    Buff,
    Debuff,
    DebuffStrong,
    Sleep,
    Stun,
    StatusCard,
    Unknown,
    /// Companion of [`Intent::Summon`]. Bead `sts2-rs-47p5`.
    Summon,
    /// Companion of [`Intent::Heal`]. Bead `sts2-rs-va5r`.
    Heal,
    /// Companion of [`Intent::DeathBlow`]. Bead `sts2-rs-f99e7`.
    DeathBlow,
    /// Companion of [`Intent::Escape`]. Bead `sts2-rs-8f336`.
    Escape,
}

// ---------------------------------------------------------------------------
// IntentHistory — per-monster move-history state for repeat-limit rules
// ---------------------------------------------------------------------------

/// Per-monster intent-roll history. Used by the Phase-5 DSL state
/// machine to enforce repeat-limit rules ("no two attacks in a row",
/// "max 2 bites in a row", …).
///
/// Kept POD-`Copy` so callers can stash one in the per-enemy state without
/// dynamic allocation. `consecutive` tracks how many times in a row the
/// same `last_kind` has fired.
///
/// Mirrors Python's `MoveRepeatType::CAN_REPEAT_X_TIMES` /
/// `CANNOT_REPEAT` semantics
/// (`sts2_env/core/enums.py:174-178`,
/// `sts2_env/monsters/state_machine.py::RandomBranchState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct IntentHistory {
    /// The previous intent's variant tag, if any. `None` on turn 0.
    pub last_kind: Option<IntentKindTag>,
    /// How many turns in a row `last_kind` has fired. `0` if `last_kind`
    /// is `None`; `1` if this is the first turn it fired.
    pub consecutive: u8,
    /// Zero-indexed turn count. Caller increments after each roll.
    pub turn_index: u32,
}

impl IntentHistory {
    /// Construct a fresh history for an enemy that has not yet rolled.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last_kind: None,
            consecutive: 0,
            turn_index: 0,
        }
    }

    /// Record that `intent` just fired. Increments `consecutive` if the
    /// kind tag matches the previous one, otherwise resets to 1.
    /// `turn_index` advances by 1.
    pub fn record(&mut self, intent: Intent) {
        let tag = intent.kind();
        if self.last_kind == Some(tag) {
            self.consecutive = self.consecutive.saturating_add(1);
        } else {
            self.consecutive = 1;
        }
        self.last_kind = Some(tag);
        self.turn_index = self.turn_index.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::powers::PowerKind;
    fn rng() -> crate::dotnet_rng::Rng {
        crate::dotnet_rng::Rng::new(0)
    }

    /// `sts2-rs-opbps`: MechaKnight's C# `AfterAddedToRoom`
    /// (`MechaKnight.cs:72-75`) applies `ArtifactPower(3)` to self.
    #[test]
    fn mecha_knight_on_spawn_grants_artifact_three() {
        use crate::powers::PowerKind;
        assert_eq!(
            MonsterKind::MechaKnight.on_spawn_self_buffs(),
            &[(PowerKind::Artifact, 3)],
            "MechaKnight must gain Artifact(3) on spawn"
        );
    }

    /// `sts2-rs-ztx5u`: AfterAddedToRoom on-spawn powers for non-29kfv
    /// normals — Artifact (Chomper/CubexConstruct/PunchConstruct) and
    /// Slippery (Inklet) — vs the C# decomp.
    #[test]
    fn normal_monsters_on_spawn_artifact_and_slippery() {
        use crate::powers::PowerKind;
        let cases = [
            (MonsterKind::Chomper, PowerKind::Artifact, 2),
            (MonsterKind::CubexConstruct, PowerKind::Artifact, 1),
            (MonsterKind::PunchConstruct, PowerKind::Artifact, 1),
            (MonsterKind::Inklet, PowerKind::Slippery, 1),
        ];
        for (kind, power, amount) in cases {
            assert_eq!(
                kind.on_spawn_self_buffs(),
                &[(power, amount)],
                "{kind:?} on-spawn power mismatch"
            );
        }
    }

    /// `sts2-rs-jj9gv`: on-spawn self-powers from C# v0.103.2
    /// `AfterAddedToRoom` (non-ascension amounts). MysteriousKnight applies
    /// two in C# order (Strength then Plating).
    #[test]
    fn jj9gv_on_spawn_self_buffs_match_csharp() {
        use crate::powers::PowerKind;
        assert_eq!(
            MonsterKind::TheLost.on_spawn_self_buffs(),
            &[(PowerKind::PossessStrength, 1)]
        );
        assert_eq!(
            MonsterKind::ScrollOfBiting.on_spawn_self_buffs(),
            &[(PowerKind::PaperCuts, 2)]
        );
        assert_eq!(
            MonsterKind::MysteriousKnight.on_spawn_self_buffs(),
            &[(PowerKind::Strength, 6), (PowerKind::Plating, 6)],
            "Strength(6) before Plating(6), matching C# apply order"
        );
    }

    /// `sts2-rs-arn7n`: on-spawn self-powers batch 2 from C# v0.103.2
    /// `AfterAddedToRoom` (literal amounts). Crusher and LagavulinMatriarch
    /// apply two powers each in C# order.
    #[test]
    fn arn7n_on_spawn_self_buffs_match_csharp() {
        use crate::powers::PowerKind;
        assert_eq!(
            MonsterKind::Crusher.on_spawn_self_buffs(),
            &[(PowerKind::BackAttackLeft, 1), (PowerKind::CrabRage, 1)],
            "BackAttackLeft(1) before CrabRage(1), matching C# apply order"
        );
        assert_eq!(
            MonsterKind::LivingShield.on_spawn_self_buffs(),
            &[(PowerKind::Rampart, 25)]
        );
        // sts2-rs-ybtmu: Rocket applies SurroundedPower(1) to the player
        // (opponent-targeted), separate from its self-buffs.
        assert_eq!(
            MonsterKind::Rocket.on_spawn_opponent_debuffs(),
            &[(PowerKind::Surrounded, 1)],
            "Rocket surrounds the player on spawn"
        );
        assert!(
            MonsterKind::Crusher.on_spawn_opponent_debuffs().is_empty(),
            "non-Rocket monsters have no opponent-targeted spawn powers"
        );
        assert_eq!(
            MonsterKind::LagavulinMatriarch.on_spawn_self_buffs(),
            &[(PowerKind::Plating, 12), (PowerKind::Asleep, 3)],
            "Plating(12) before Asleep(3), matching C# apply order"
        );
    }

    /// `sts2-rs-tfsj5`: on-spawn self-powers batch 3 from C# v0.103.2
    /// `AfterAddedToRoom`. GremlinMerc applies Surprise then Thievery.
    #[test]
    fn tfsj5_on_spawn_self_buffs_match_csharp() {
        use crate::powers::PowerKind;
        assert_eq!(
            MonsterKind::Parafright.on_spawn_self_buffs(),
            // C# IllusionPower.AfterApplied grants MinionPower(1) — bead
            // `sts2-rs-ybtmu` (matches the EyeWithTeeth arm).
            &[(PowerKind::Illusion, 1), (PowerKind::Minion, 1)]
        );
        assert_eq!(
            MonsterKind::GremlinMerc.on_spawn_self_buffs(),
            &[(PowerKind::Surprise, 1), (PowerKind::Thievery, 20)],
            "Surprise(1) before Thievery(20), matching C# apply order"
        );
        assert_eq!(
            MonsterKind::TheAdversaryMkTwo.on_spawn_self_buffs(),
            &[(PowerKind::Artifact, 1)]
        );
        assert_eq!(
            MonsterKind::TheAdversaryMkThree.on_spawn_self_buffs(),
            &[(PowerKind::Artifact, 2)]
        );
    }

    /// `sts2-rs-qw18q`: on-spawn MinionPower for minions reached via the
    /// auto-buffing `spawn_enemy` path (KinFollower encounter-placed,
    /// GasBomb via LivingFog BLOAT). Fabricator bots are excluded — they
    /// apply Minion manually in their summon effect.
    #[test]
    fn qw18q_on_spawn_minion_power_match_csharp() {
        use crate::powers::PowerKind;
        assert_eq!(
            MonsterKind::KinFollower.on_spawn_self_buffs(),
            &[(PowerKind::Minion, 1)]
        );
        assert_eq!(
            MonsterKind::GasBomb.on_spawn_self_buffs(),
            &[(PowerKind::Minion, 1)]
        );
    }

    /// `sts2-rs-u278v`: TheForgotten on-spawn PossessSpeed(1) + HP 106
    /// (C# `TheForgotten.cs`). The DREAD dynamic-damage rebase is 96y80.
    #[test]
    fn u278v_the_forgotten_on_spawn_possess_speed_and_hp() {
        use crate::powers::PowerKind;
        assert_eq!(
            MonsterKind::TheForgotten.on_spawn_self_buffs(),
            &[(PowerKind::PossessSpeed, 1)]
        );
        assert_eq!(
            MonsterKind::TheForgotten.hp_roll(0),
            HpRoll::Range(106, 106),
            "C# MinInitialHp = MaxInitialHp = 106"
        );
    }

    /// `sts2-rs-g3vq4`: monster initial-HP parity vs C# Min/MaxInitialHp
    /// (asc-0; ToughEnemies values are higher). Prior values were stubs or
    /// drifted.
    #[test]
    fn g3vq4_monster_hp_matches_csharp() {
        let cases = [
            (MonsterKind::SpectralKnight, 93, 93),
            (MonsterKind::Crusher, 209, 209),
            (MonsterKind::TheLost, 93, 93),
            (MonsterKind::ScrollOfBiting, 30, 37),
            // sts2-rs-32pq0 (batch 2)
            (MonsterKind::MagiKnight, 82, 82),
            (MonsterKind::Rocket, 199, 199),
            // sts2-rs-brxpe (act2)
            (MonsterKind::Nibbit, 42, 46),
            // sts2-rs-4nv5r (act2/shared batch 3)
            (MonsterKind::LouseProgenitor, 134, 136),
            (MonsterKind::SlumberingBeetle, 86, 86),
            (MonsterKind::HauntedShip, 63, 63),
            (MonsterKind::PhantasmalGardener, 26, 31),
            (MonsterKind::DecimillipedeSegment, 40, 46),
            // sts2-rs-rbtn9
            (MonsterKind::Ovicopter, 124, 130),
            // sts2-rs-1k2w9 (Decimillipede trio inherits DecimillipedeSegment)
            (MonsterKind::DecimillipedeSegmentFront, 40, 46),
            (MonsterKind::DecimillipedeSegmentMiddle, 40, 46),
            (MonsterKind::DecimillipedeSegmentBack, 40, 46),
            // sts2-rs-hg6yy (act1_underdocks batch 4)
            (MonsterKind::SludgeSpinner, 37, 39),
            (MonsterKind::Toadpole, 21, 25),
            (MonsterKind::SneakyGremlin, 10, 14),
            (MonsterKind::GasBomb, 7, 7),
            (MonsterKind::SewerClam, 56, 56),
            (MonsterKind::SkulkingColony, 75, 75),
            (MonsterKind::SoulFysh, 211, 211),
            // sts2-rs-u27n6 (act2_hive batch 5: C# Min=Max-only species
            // that had been ported as ascension-spread ranges)
            (MonsterKind::ThievingHopper, 79, 79),
            (MonsterKind::Tunneler, 87, 87),
            (MonsterKind::HunterKiller, 121, 121),
            (MonsterKind::TheObscura, 123, 123),
            (MonsterKind::InfestedPrism, 161, 161),
            // sts2-rs-odxkd; `…wb8lk.11`: v0.107.1 261 (was 266)
            (MonsterKind::SlimedBerserker, 261, 261),
        ];
        for (kind, lo, hi) in cases {
            assert_eq!(kind.hp_roll(0), HpRoll::Range(lo, hi), "{kind:?} HP");
        }
    }

    /// Ascension HP scaling vertical slice (FuzzyWurmCrawler). C#
    /// `FuzzyWurmCrawler.cs`:
    /// `MinInitialHp => GetValueIfAscension(ToughEnemies, 58, 55)`,
    /// `MaxInitialHp => GetValueIfAscension(ToughEnemies, 59, 57)`.
    /// ToughEnemies activates at ascension >= 8.
    #[test]
    fn fuzzy_wurm_crawler_hp_scales_at_ascension_8() {
        use MonsterKind::FuzzyWurmCrawler as Wurm;
        assert_eq!(Wurm.hp_roll(0), HpRoll::Range(55, 57), "A0 base HP");
        assert_eq!(
            Wurm.hp_roll(7),
            HpRoll::Range(55, 57),
            "A7 below ToughEnemies"
        );
        assert_eq!(
            Wurm.hp_roll(8),
            HpRoll::Range(58, 59),
            "A8 activates ToughEnemies"
        );
        assert_eq!(
            Wurm.hp_roll(10),
            HpRoll::Range(58, 59),
            "A10 keeps scaled HP"
        );
    }

    /// Comprehensive HP ToughEnemies (>= A8) scaling, transcribed from the
    /// C# decomp per-monster `MinInitialHp`/`MaxInitialHp`
    /// `GetValueIfAscension(ToughEnemies, …)` and validated against the
    /// `hp_roll` arms. `hp_roll(0)` must give the base range, `hp_roll(8)`
    /// the ToughEnemies range. (Decimillipede segments were deferred while
    /// `finalize_spawn_max_hp` still hard-coded its `[40, 46]` wrap bounds;
    /// both now take `ascension`, so the four segment kinds are covered
    /// below.)
    #[test]
    fn hp_tough_enemies_scaling_matches_csharp() {
        let cases: &[(MonsterKind, i32, i32, i32, i32)] = &[
            (MonsterKind::AssassinRubyRaider, 18, 23, 19, 24),
            (MonsterKind::AxeRubyRaider, 20, 22, 21, 23),
            (MonsterKind::Axebot, 70, 78, 76, 86),
            (MonsterKind::BowlbugEgg, 21, 22, 23, 24),
            (MonsterKind::BowlbugNectar, 35, 38, 36, 39),
            (MonsterKind::BowlbugRock, 45, 48, 46, 49),
            (MonsterKind::BowlbugSilk, 40, 43, 41, 44),
            (MonsterKind::BruteRubyRaider, 30, 33, 31, 34),
            (MonsterKind::BygoneEffigy, 127, 127, 132, 132),
            (MonsterKind::Byrdonis, 81, 84, 90, 90),
            (MonsterKind::CalcifiedCultist, 38, 41, 39, 42),
            (MonsterKind::CeremonialBeast, 252, 252, 262, 262),
            (MonsterKind::Chomper, 60, 64, 63, 67),
            (MonsterKind::CorpseSlug, 25, 27, 27, 29),
            (MonsterKind::CrossbowRubyRaider, 18, 21, 19, 22),
            (MonsterKind::Crusher, 209, 209, 219, 219),
            (MonsterKind::CubexConstruct, 65, 65, 70, 70),
            (MonsterKind::DampCultist, 51, 53, 52, 54),
            // `DecimillipedeSegment.cs:64-66`
            // Min => GetValueIfAscension(ToughEnemies, 46, 40),
            // Max => GetValueIfAscension(ToughEnemies, 52, 46). The
            // Front/Middle/Back subclasses inherit both properties verbatim.
            (MonsterKind::DecimillipedeSegment, 40, 46, 46, 52),
            (MonsterKind::DecimillipedeSegmentFront, 40, 46, 46, 52),
            (MonsterKind::DecimillipedeSegmentMiddle, 40, 46, 46, 52),
            (MonsterKind::DecimillipedeSegmentBack, 40, 46, 46, 52),
            (MonsterKind::DenseVegetationWriggler, 17, 21, 18, 22),
            (MonsterKind::DevotedSculptor, 162, 162, 172, 172),
            (MonsterKind::Aeonglass, 512, 512, 535, 535),
            (MonsterKind::Entomancer, 145, 145, 155, 155),
            (MonsterKind::Exoskeleton, 24, 28, 25, 29),
            (MonsterKind::Fabricator, 150, 150, 155, 155),
            (MonsterKind::FakeMerchantMonster, 165, 165, 175, 175),
            (MonsterKind::FatGremlin, 13, 17, 14, 18),
            (MonsterKind::FlailKnight, 101, 101, 108, 108),
            (MonsterKind::Flyconid, 47, 49, 51, 53),
            (MonsterKind::Fogmog, 74, 74, 78, 78),
            (MonsterKind::FossilStalker, 51, 53, 54, 56),
            (MonsterKind::FrogKnight, 191, 191, 199, 199),
            (MonsterKind::FuzzyWurmCrawler, 55, 57, 58, 59),
            (MonsterKind::GasBomb, 7, 7, 8, 8),
            (MonsterKind::GlobeHead, 148, 148, 158, 158),
            (MonsterKind::GremlinMerc, 47, 49, 51, 53),
            (MonsterKind::Guardbot, 16, 20, 17, 21),
            (MonsterKind::HauntedShip, 63, 63, 67, 67),
            (MonsterKind::HunterKiller, 121, 121, 126, 126),
            (MonsterKind::InfestedPrism, 161, 161, 171, 171),
            (MonsterKind::Inklet, 11, 17, 12, 18),
            (MonsterKind::KinFollower, 58, 59, 62, 63),
            (MonsterKind::KinPriest, 190, 190, 199, 199),
            (MonsterKind::KnowledgeDemon, 379, 379, 399, 399),
            (MonsterKind::LagavulinMatriarch, 222, 222, 233, 233),
            (MonsterKind::LeafSlimeM, 32, 35, 33, 36),
            (MonsterKind::LeafSlimeS, 11, 15, 12, 16),
            (MonsterKind::LivingFog, 80, 80, 82, 82),
            (MonsterKind::LivingShield, 55, 55, 65, 65),
            (MonsterKind::LouseProgenitor, 134, 136, 138, 141),
            (MonsterKind::MagiKnight, 82, 82, 89, 89),
            (MonsterKind::Mawler, 72, 72, 76, 76),
            (MonsterKind::MechaKnight, 300, 300, 320, 320),
            (MonsterKind::MysteriousKnight, 101, 101, 108, 108),
            (MonsterKind::MysteriousKnight, 101, 101, 108, 108),
            (MonsterKind::Myte, 61, 67, 64, 69),
            (MonsterKind::Nibbit, 42, 46, 44, 48),
            (MonsterKind::Noisebot, 18, 23, 19, 24),
            (MonsterKind::Ovicopter, 124, 130, 126, 132),
            (MonsterKind::OwlMagistrate, 231, 231, 247, 247),
            (MonsterKind::PhantasmalGardener, 26, 31, 27, 32),
            (MonsterKind::PhrogParasite, 61, 64, 66, 68),
            (MonsterKind::PunchConstruct, 55, 55, 60, 60),
            (MonsterKind::Queen, 400, 400, 419, 419),
            (MonsterKind::Rocket, 199, 199, 209, 209),
            (MonsterKind::ScrollOfBiting, 30, 37, 33, 39),
            (MonsterKind::Seapunk, 44, 46, 47, 49),
            (MonsterKind::SewerClam, 56, 56, 58, 58),
            (MonsterKind::ShrinkerBeetle, 38, 40, 40, 42),
            (MonsterKind::SkulkingColony, 75, 75, 80, 80),
            (MonsterKind::SlimedBerserker, 261, 261, 281, 281),
            (MonsterKind::SlitheringStrangler, 53, 55, 54, 56),
            (MonsterKind::SludgeSpinner, 37, 39, 41, 42),
            (MonsterKind::SlumberingBeetle, 86, 86, 89, 89),
            (MonsterKind::SnappingJaxfruit, 31, 33, 34, 36),
            (MonsterKind::SneakyGremlin, 10, 14, 11, 15),
            (MonsterKind::SoulFysh, 211, 211, 221, 221),
            (MonsterKind::SoulNexus, 234, 234, 254, 254),
            (MonsterKind::SpectralKnight, 93, 93, 97, 97),
            (MonsterKind::SpinyToad, 116, 119, 121, 124),
            (MonsterKind::Stabbot, 18, 23, 19, 24),
            (MonsterKind::TerrorEel, 140, 140, 150, 150),
            (MonsterKind::TestSubject, 100, 100, 111, 111),
            (MonsterKind::TheForgotten, 106, 106, 111, 111),
            (MonsterKind::TheInsatiable, 321, 321, 341, 341),
            (MonsterKind::TheLost, 93, 93, 99, 99),
            (MonsterKind::TheObscura, 123, 123, 129, 129),
            (MonsterKind::ThievingHopper, 79, 79, 84, 84),
            (MonsterKind::Toadpole, 21, 25, 22, 26),
            (MonsterKind::TorchHeadAmalgam, 199, 199, 211, 211),
            (MonsterKind::ToughEgg, 14, 18, 15, 19),
            (MonsterKind::TrackerRubyRaider, 21, 25, 22, 26),
            (MonsterKind::Tunneler, 87, 87, 92, 92),
            (MonsterKind::TwigSlimeM, 26, 28, 27, 29),
            (MonsterKind::TwigSlimeS, 7, 11, 8, 12),
            (MonsterKind::TwoTailedRat, 17, 21, 18, 22),
            (MonsterKind::Vantom, 173, 173, 183, 183),
            (MonsterKind::VineShambler, 61, 61, 64, 64),
            (MonsterKind::WaterfallGiant, 240, 240, 250, 250),
            (MonsterKind::Wriggler, 17, 21, 18, 22),
            (MonsterKind::Zapbot, 18, 23, 19, 24),
        ];
        for (kind, blo, bhi, tlo, thi) in cases.iter().copied() {
            assert_eq!(
                kind.hp_roll(0),
                HpRoll::Range(blo, bhi),
                "{kind:?} A0 base HP"
            );
            assert_eq!(
                kind.hp_roll(7),
                HpRoll::Range(blo, bhi),
                "{kind:?} A7 below ToughEnemies"
            );
            assert_eq!(
                kind.hp_roll(8),
                HpRoll::Range(tlo, thi),
                "{kind:?} A8 ToughEnemies HP"
            );
        }
        // TurretOperator is the lone Fixed-HP arm (C# Min==Max=41; 51 ToughEnemies).
        assert_eq!(
            MonsterKind::TurretOperator.hp_roll(0),
            HpRoll::Fixed(41),
            "TurretOperator A0"
        );
        assert_eq!(
            MonsterKind::TurretOperator.hp_roll(8),
            HpRoll::Fixed(51),
            "TurretOperator A8 ToughEnemies"
        );
    }

    /// `sts2-rs-rop3f`: on-spawn self-buffs registered from each monster's
    /// C# v0.103.2 `AfterAddedToRoom` (non-ascension amounts). SlumberingBeetle
    /// applies two powers in C# order (Plating then Slumber).
    #[test]
    fn rop3f_on_spawn_self_buffs_match_csharp() {
        use crate::powers::PowerKind;
        assert_eq!(
            MonsterKind::GlobeHead.on_spawn_self_buffs(),
            &[(PowerKind::Galvanic, 6)]
        );
        assert_eq!(
            MonsterKind::SlumberingBeetle.on_spawn_self_buffs(),
            &[(PowerKind::Plating, 15), (PowerKind::Slumber, 3)],
            "Plating(15) before Slumber(3), matching C# apply order"
        );
        assert_eq!(
            MonsterKind::Entomancer.on_spawn_self_buffs(),
            &[(PowerKind::PersonalHive, 1)]
        );
        assert_eq!(
            MonsterKind::ThievingHopper.on_spawn_self_buffs(),
            &[(PowerKind::EscapeArtist, 5)]
        );
        assert_eq!(
            MonsterKind::Zapbot.on_spawn_self_buffs(),
            &[(PowerKind::HighVoltage, 2)]
        );
        assert_eq!(
            MonsterKind::Exoskeleton.on_spawn_self_buffs(),
            &[(PowerKind::HardToKill, 9)]
        );
        assert_eq!(
            MonsterKind::InfestedPrism.on_spawn_self_buffs(),
            &[(PowerKind::VitalSpark, 2)]
        );
        // batch 2
        assert_eq!(
            MonsterKind::FossilStalker.on_spawn_self_buffs(),
            &[(PowerKind::Suck, 3)]
        );
        assert_eq!(
            MonsterKind::BowlbugRock.on_spawn_self_buffs(),
            &[(PowerKind::Imbalanced, 1)]
        );
        assert_eq!(
            MonsterKind::DecimillipedeSegment.on_spawn_self_buffs(),
            &[(PowerKind::Reattach, 25)]
        );
        // The positional Front/Middle/Back variants inherit the same
        // ReattachPower(25) on-spawn grant (bead `sts2-rs-8mpo1`).
        for k in [
            MonsterKind::DecimillipedeSegmentFront,
            MonsterKind::DecimillipedeSegmentMiddle,
            MonsterKind::DecimillipedeSegmentBack,
        ] {
            assert_eq!(k.on_spawn_self_buffs(), &[(PowerKind::Reattach, 25)]);
        }
        assert_eq!(
            MonsterKind::SkulkingColony.on_spawn_self_buffs(),
            &[(PowerKind::HardenedShell, 20)]
        );
        assert_eq!(
            MonsterKind::LouseProgenitor.on_spawn_self_buffs(),
            &[(PowerKind::CurlUp, 14)]
        );
        assert_eq!(
            MonsterKind::PhantasmalGardener.on_spawn_self_buffs(),
            &[(PowerKind::Skittish, 6)]
        );
        assert_eq!(
            MonsterKind::CorpseSlug.on_spawn_self_buffs(),
            &[(PowerKind::Ravenous, 4)]
        );
    }

    /// `sts2-rs-t6sh2`: with nothing taken in range, `starting_hp_unique`
    /// must return the *identical* value to `starting_hp` off the same RNG
    /// cursor (both compute `min + floor(sample * (max-min+1))`). This is
    /// what keeps single / distinct-kind encounters bit-identical.
    #[test]
    fn starting_hp_unique_matches_starting_hp_when_nothing_taken() {
        let empty = std::collections::HashSet::new();
        for kind in [
            MonsterKind::LeafSlimeS,
            MonsterKind::Inklet,
            MonsterKind::Byrdonis,
            MonsterKind::Nibbit, // range-HP species (42..=46): both paths roll identically
        ] {
            let mut a = rng();
            let mut b = rng();
            assert_eq!(
                kind.starting_hp(&mut a, 0),
                kind.starting_hp_unique(&mut b, &empty, 0),
                "{kind:?}: unique-with-empty-taken must equal plain starting_hp"
            );
        }
    }

    /// `sts2-rs-t6sh2`: two same-kind monsters must get distinct HP — the
    /// second roll excludes the first's value from the candidate set
    /// (C# `SetUniqueMonsterHpValue`). `LeafSlimeS` ranges 11..=15 (5
    /// values), so exclusion always leaves a non-empty set.
    #[test]
    fn starting_hp_unique_excludes_taken_values() {
        let mut r = rng();
        let first = MonsterKind::LeafSlimeS.starting_hp_unique(
            &mut r,
            &std::collections::HashSet::new(),
            0,
        );
        let taken: std::collections::HashSet<i32> = [first].into_iter().collect();
        let second = MonsterKind::LeafSlimeS.starting_hp_unique(&mut r, &taken, 0);
        assert_ne!(first, second, "same-kind monsters must get distinct HP");
        assert!((11..=15).contains(&second), "second HP stays in range");
    }

    /// `sts2-rs-t6sh2`: when every value in the range is taken, fall back
    /// to `NextInt(min, max)` (C# `hashSet.Count <= 0` branch) — still one
    /// read, returns a value in range (may collide, matching C#).
    #[test]
    fn starting_hp_unique_falls_back_when_range_exhausted() {
        let taken: std::collections::HashSet<i32> = (11..=15).collect();
        let mut r = rng();
        let hp = MonsterKind::LeafSlimeS.starting_hp_unique(&mut r, &taken, 0);
        assert!(
            (11..=15).contains(&hp),
            "exhausted-set fallback stays in range"
        );
    }

    /// `sts2-rs-8mpo1`: the Decimillipede `AfterAddedToRoom` even-unique
    /// maxHp fix-up. Replays the exact C# v0.103.2 DECIMILLIPEDE_ELITE
    /// turn-0 sequence observed in the coverage sweep: raw rolls
    /// `[44, 43, 41]` (front/middle/back) finalize to `[44, 46, 42]` —
    /// front stays (even, no teammates); middle rounds 43→44, collides with
    /// front, bumps to 46; back rounds 41→42, no collision.
    #[test]
    fn finalize_spawn_max_hp_decimillipede_even_unique() {
        use std::collections::HashSet;
        let mut taken: HashSet<i32> = HashSet::new();

        let front = MonsterKind::DecimillipedeSegmentFront.finalize_spawn_max_hp(44, &taken, 0);
        assert_eq!(front, 44);
        taken.insert(front);

        let middle = MonsterKind::DecimillipedeSegmentMiddle.finalize_spawn_max_hp(43, &taken, 0);
        assert_eq!(
            middle, 46,
            "43 rounds to 44, collides with front, bumps to 46"
        );
        taken.insert(middle);

        let back = MonsterKind::DecimillipedeSegmentBack.finalize_spawn_max_hp(41, &taken, 0);
        assert_eq!(back, 42, "41 rounds to 42, no collision");
    }

    /// The two-pass orchestrator matches the inline `roll → finalize` loop
    /// for a non-Decimillipede roster (every `finalize` is the identity, so
    /// the pass split is invisible) and applies the C# mixed-teammate-set
    /// finalize for a multi-segment Decimillipede. Bead `sts2-rs-8mpo1`.
    #[test]
    fn roll_unique_max_hps_two_pass_matches_inline_for_non_decimillipede() {
        use crate::dotnet_rng::Rng;
        use std::collections::HashSet;

        // Two Ruby-Raider-ish range-HP kinds: no finalize transform, so the
        // two-pass result must equal the old inline loop exactly (same niche
        // reads in the same order, identity finalize).
        let kinds = [MonsterKind::LeafSlimeS, MonsterKind::LeafSlimeS];

        let mut rng_a = Rng::new(12345);
        let two_pass = roll_unique_max_hps(&kinds, &mut rng_a, 0);

        let mut rng_b = Rng::new(12345);
        let mut taken: HashSet<i32> = HashSet::new();
        let inline: Vec<i32> = kinds
            .iter()
            .map(|&k| {
                let rolled = k.starting_hp_unique(&mut rng_b, &taken, 0);
                let hp = k.finalize_spawn_max_hp(rolled, &taken, 0);
                taken.insert(hp);
                hp
            })
            .collect();

        assert_eq!(two_pass, inline);
        assert_eq!(
            rng_a.counter(),
            rng_b.counter(),
            "same niche cursor advance"
        );
    }

    /// A 3-segment Decimillipede: pass 1 excludes RAW teammate HP, pass 2
    /// finalizes each segment off the mixed teammate set (earlier finalized,
    /// later still raw). Whatever the niche rolls, the three finalized HPs
    /// must be distinct even values in `[40, 46]` — the C# invariant the
    /// even-unique fix-up guarantees.
    #[test]
    fn roll_unique_max_hps_decimillipede_segments_distinct_even() {
        use crate::dotnet_rng::Rng;
        let kinds = [
            MonsterKind::DecimillipedeSegmentFront,
            MonsterKind::DecimillipedeSegmentMiddle,
            MonsterKind::DecimillipedeSegmentBack,
        ];
        // Sweep a band of seeds so the assertion isn't seed-lucky, at both
        // ascension tiers: the C# band is `[MinInitialHp, MaxInitialHp]`,
        // which ToughEnemies (A8) shifts 40-46 → 46-52. Before the wrap
        // bounds took `ascension` an A8 collision at 52 wrapped to 40 —
        // below the ToughEnemies floor and, worse, an ODD-parity-safe but
        // out-of-band value the C# can never produce.
        for (ascension, lo, hi) in [(0_u8, 40_i32, 46_i32), (8, 46, 52)] {
            for seed in 0..64_i32 {
                let mut rng = Rng::new(seed);
                let hps = roll_unique_max_hps(&kinds, &mut rng, ascension);
                assert_eq!(hps.len(), 3);
                for &hp in &hps {
                    assert!(
                        (lo..=hi).contains(&hp) && hp % 2 == 0,
                        "A{ascension} seed {seed}: segment HP {hp} not an even value in [{lo},{hi}]"
                    );
                }
                let set: std::collections::HashSet<i32> = hps.iter().copied().collect();
                assert_eq!(
                    set.len(),
                    3,
                    "A{ascension} seed {seed}: segment HPs not distinct: {hps:?}"
                );
            }
        }
    }

    /// The `AfterAddedToRoom` wrap band is ascension-aware: C#
    /// `DecimillipedeSegment.AfterAddedToRoom` wraps on
    /// `ScaleHpForMultiplayer(MaxInitialHp, …)` → `…(MinInitialHp, …)`, and
    /// both properties are `GetValueIfAscension(ToughEnemies, …)`
    /// (`v0.107.1/…/DecimillipedeSegment.cs:64-66, :137-140`). At A8 a
    /// collision at the ceiling 52 must wrap to the ToughEnemies floor 46,
    /// NOT to the A0 floor 40.
    #[test]
    fn finalize_spawn_max_hp_wrap_band_follows_tough_enemies() {
        use std::collections::HashSet;
        let taken: HashSet<i32> = [52].into_iter().collect();
        assert_eq!(
            MonsterKind::DecimillipedeSegmentFront.finalize_spawn_max_hp(52, &taken, 8),
            46,
            "A8 ceiling collision wraps to the ToughEnemies floor"
        );
        // …and an A8 roll that lands mid-band still just bumps by 2.
        let taken2: HashSet<i32> = [48].into_iter().collect();
        assert_eq!(
            MonsterKind::DecimillipedeSegmentMiddle.finalize_spawn_max_hp(47, &taken2, 8),
            50,
            "47 rounds to 48, collides, bumps to 50"
        );
        // The A0 band is unchanged: 46 is the ceiling there, so it wraps to 40.
        let taken3: HashSet<i32> = [46].into_iter().collect();
        assert_eq!(
            MonsterKind::DecimillipedeSegmentBack.finalize_spawn_max_hp(46, &taken3, 0),
            40
        );
    }

    /// The fix-up wraps `Max`→`Min` when the even bump overshoots 46, and
    /// is the identity for non-segment kinds.
    #[test]
    fn finalize_spawn_max_hp_wraps_and_is_identity_elsewhere() {
        use std::collections::HashSet;
        // 46 even, but taken by a teammate → bump to 48 > 46 → wrap to 40.
        let taken: HashSet<i32> = [46].into_iter().collect();
        assert_eq!(
            MonsterKind::DecimillipedeSegment.finalize_spawn_max_hp(46, &taken, 0),
            40
        );
        // Non-segment kind: rolled value passes through untouched even with
        // a colliding "teammate".
        let taken2: HashSet<i32> = [13].into_iter().collect();
        assert_eq!(
            MonsterKind::LeafSlimeS.finalize_spawn_max_hp(13, &taken2, 0),
            13
        );
    }

    #[test]
    fn intent_python_names_match_python_intent_type() {
        // Mirrors `sts2_env/core/enums.py::IntentType` member .name
        // values. Each must be ASCII upper-snake-case.
        assert_eq!(Intent::Attack { damage: 6 }.python_name(), "ATTACK");
        assert_eq!(
            Intent::MultiAttack { damage: 4, hits: 2 }.python_name(),
            "MULTI_ATTACK"
        );
        assert_eq!(Intent::Defend { block: 0 }.python_name(), "DEFEND");
        assert_eq!(
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0
            }
            .python_name(),
            "BUFF"
        );
        assert_eq!(
            Intent::Debuff {
                kind: PowerKind::Weak,
                amount: 0
            }
            .python_name(),
            "DEBUFF"
        );
        assert_eq!(Intent::DebuffStrong.python_name(), "DEBUFF_STRONG");
        assert_eq!(Intent::Sleep.python_name(), "SLEEP");
        assert_eq!(Intent::Stun.python_name(), "STUN");
        assert_eq!(Intent::StatusCard.python_name(), "STATUS_CARD");
        assert_eq!(Intent::Unknown.python_name(), "UNKNOWN");
        // Bead sts2-rs-47p5
        assert_eq!(Intent::Summon.python_name(), "SUMMON");
        // Bead sts2-rs-va5r
        assert_eq!(Intent::Heal.python_name(), "HEAL");
        // Bead sts2-rs-f99e7
        assert_eq!(Intent::DeathBlow { damage: 8 }.python_name(), "DEATH_BLOW");
        assert_eq!(Intent::DeathBlow { damage: 8 }.damage(), 8);
        assert_eq!(Intent::DeathBlow { damage: 8 }.hits(), 1);
        assert_eq!(
            Intent::DeathBlow { damage: 8 }.kind(),
            IntentKindTag::DeathBlow
        );
        // Bead sts2-rs-8f336
        assert_eq!(Intent::Escape.python_name(), "ESCAPE");
        assert_eq!(Intent::Escape.damage(), 0);
        assert_eq!(Intent::Escape.hits(), 1);
        assert_eq!(Intent::Escape.kind(), IntentKindTag::Escape);
    }

    #[test]
    fn intent_heal_zero_damage_and_unit_hits() {
        // Bead sts2-rs-va5r: Heal carries no damage; default hits=1
        // matches Python `Intent.hits` default at `intents.py:16`.
        assert_eq!(Intent::Heal.damage(), 0);
        assert_eq!(Intent::Heal.hits(), 1);
        assert_eq!(Intent::Heal.kind(), IntentKindTag::Heal);
    }

    #[test]
    fn intent_damage_and_hits_default_to_python_intent_defaults() {
        // Python `Intent.damage` default is 0; `Intent.hits` default is 1.
        assert_eq!(Intent::Defend { block: 0 }.damage(), 0);
        assert_eq!(Intent::Defend { block: 0 }.hits(), 1);
        assert_eq!(
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0
            }
            .damage(),
            0
        );
        assert_eq!(
            Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0
            }
            .hits(),
            1
        );
        assert_eq!(Intent::Attack { damage: 9 }.damage(), 9);
        assert_eq!(Intent::Attack { damage: 9 }.hits(), 1);
        assert_eq!(Intent::MultiAttack { damage: 4, hits: 3 }.damage(), 4);
        assert_eq!(Intent::MultiAttack { damage: 4, hits: 3 }.hits(), 3);
    }

    #[test]
    fn intent_history_tracks_consecutive_correctly() {
        let mut h = IntentHistory::new();
        assert_eq!(h.last_kind, None);
        assert_eq!(h.consecutive, 0);
        assert_eq!(h.turn_index, 0);

        h.record(Intent::Attack { damage: 6 });
        assert_eq!(h.last_kind, Some(IntentKindTag::Attack));
        assert_eq!(h.consecutive, 1);
        assert_eq!(h.turn_index, 1);

        h.record(Intent::Attack { damage: 6 });
        assert_eq!(h.consecutive, 2);
        assert_eq!(h.turn_index, 2);

        h.record(Intent::Defend { block: 0 });
        assert_eq!(h.last_kind, Some(IntentKindTag::Defend));
        assert_eq!(h.consecutive, 1);
        assert_eq!(h.turn_index, 3);
    }

    /// Bead `sts2-rs-54ua`: TwoTailedRat HP fuzz — every sample
    /// from `starting_hp` must land in `17..=21` (C# decomp range).
    /// Previous Python pin's 25..=28 was inverted per the audit's
    /// Pattern-B bucket.
    #[test]
    fn ttua_two_tailed_rat_starting_hp_matches_csharp_range() {
        let mut r = rng();
        for _ in 0..200 {
            let hp = MonsterKind::TwoTailedRat.starting_hp(&mut r, 0);
            assert!(
                (17..=21).contains(&hp),
                "TwoTailedRat hp {hp} out of 17..=21"
            );
        }
    }

    /// Bead `sts2-rs-gizi`: Queen HP fuzz — every sample must land
    /// at exactly 400 (C# `MinInitialHp == MaxInitialHp == 400`,
    /// gen_range collapsed to a single value). Pre-rebaseline Python
    /// pin shipped 302 (-25%).
    #[test]
    fn gizi_queen_starting_hp_matches_csharp() {
        let mut r = rng();
        for _ in 0..50 {
            let hp = MonsterKind::Queen.starting_hp(&mut r, 0);
            assert_eq!(hp, 400, "Queen hp {hp} not 400");
        }
    }

    /// Bead `sts2-rs-1b86`: TestSubject FirstFormHp == 100 (C# decomp).
    /// Pre-rebaseline Python pin shipped 255. SecondFormHp (200 on
    /// death-Revive) is a separate follow-up — this bead only locks the
    /// initial HP.
    #[test]
    fn b86_test_subject_starting_hp_is_first_form() {
        let mut r = rng();
        for _ in 0..50 {
            let hp = MonsterKind::TestSubject.starting_hp(&mut r, 0);
            assert_eq!(hp, 100, "TestSubject hp {hp} not FirstFormHp=100");
        }
    }

    /// Bead `sts2-rs-4poa`: SnappingJaxfruit HP fuzz — every sample
    /// must land in `31..=33` (C# normal-tier range). Pre-rebaseline
    /// Python pin shipped 53..=56 (~70% overshoot).
    #[test]
    fn p4oa_snapping_jaxfruit_starting_hp_matches_csharp() {
        let mut r = rng();
        for _ in 0..200 {
            let hp = MonsterKind::SnappingJaxfruit.starting_hp(&mut r, 0);
            assert!(
                (31..=33).contains(&hp),
                "SnappingJaxfruit hp {hp} out of 31..=33"
            );
        }
    }

    /// Bead `sts2-rs-elt9`: KinFollower HP fuzz — `58..=59` per C#
    /// `KinFollower.cs`. Pre-rebaseline Python pin shipped 65..=71.
    #[test]
    fn elt9_kin_follower_starting_hp_matches_csharp() {
        let mut r = rng();
        for _ in 0..200 {
            let hp = MonsterKind::KinFollower.starting_hp(&mut r, 0);
            assert!(
                (58..=59).contains(&hp),
                "KinFollower hp {hp} out of 58..=59"
            );
        }
    }

    #[test]
    fn h3_act1_weak_starting_hp_ranges_match_python_pin() {
        // Cliext H3 (bead sts2-rs-yioi). Ranges mirror
        // sts2_env/monsters/act1_weak.py at pin 01c094d.
        let mut r = rng();
        let cases: &[(MonsterKind, std::ops::RangeInclusive<i32>)] = &[
            (MonsterKind::ShrinkerBeetle, 38..=40),
            (MonsterKind::FuzzyWurmCrawler, 55..=57),
            (MonsterKind::LeafSlimeS, 11..=15),
            (MonsterKind::TwigSlimeS, 7..=11),
            (MonsterKind::LeafSlimeM, 32..=35),
            (MonsterKind::TwigSlimeM, 26..=28),
        ];
        for (kind, range) in cases {
            for _ in 0..200 {
                let hp = kind.starting_hp(&mut r, 0);
                assert!(range.contains(&hp), "{kind:?} hp {hp} out of {range:?}");
            }
        }
    }

    /// Bead `sts2-rs-nmleu`: sanity-pin the `test_subject` mod
    /// constants against the documented C# `TestSubject.cs` values
    /// (non-ascension identity branch). Locks future contributors
    /// out of accidentally drifting them when porting the deeper
    /// Revive / phase-3 flow under parent `sts2-rs-jnf4a`.
    #[test]
    fn test_subject_constants_match_csharp_pin() {
        use super::test_subject::*;
        assert_eq!(FIRST_FORM_HP, 100, "TestSubject.cs:76 FirstFormHp non-asc");
        assert_eq!(
            SECOND_FORM_HP, 200,
            "TestSubject.cs:78 SecondFormHp non-asc"
        );
        assert_eq!(THIRD_FORM_HP, 300, "TestSubject.cs:80 ThirdFormHp non-asc");
        assert_eq!(
            MULTI_CLAW_DAMAGE, 10,
            "TestSubject.cs:88 MultiClawDamage non-asc"
        );
        assert_eq!(
            BASE_MULTI_CLAW_COUNT, 3,
            "TestSubject.cs:90 BaseMultiClawCount"
        );
        assert_eq!(BITE_DAMAGE, 20, "TestSubject.cs:84 BiteDamage non-asc");
        assert_eq!(
            SKULL_BASH_DAMAGE, 14,
            "TestSubject.cs:86 SkullBashDamage non-asc"
        );
        assert_eq!(
            PHASE3_LACERATE_DAMAGE, 10,
            "TestSubject.cs:92 Phase3LacerateDamage non-asc"
        );
        assert_eq!(
            BIG_POUNCE_DAMAGE, 45,
            "TestSubject.cs:94 BigPounceDamage (no asc)"
        );
        assert_eq!(
            BURNING_GROWL_BURN_COUNT, 3,
            "TestSubject.cs:96 BurningGrowlBurnCount non-asc"
        );
        assert_eq!(
            PAINFUL_STABS_AMOUNT, 1,
            "TestSubject.cs:268 first-Revive PainfulStabs amount"
        );
        assert_eq!(
            NEMESIS_AMOUNT, 1,
            "TestSubject.cs:272 second-Revive Nemesis amount"
        );
    }
}
