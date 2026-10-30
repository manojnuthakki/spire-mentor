//! Boss-relic Ancient-event option rolls — task #19, bead `sts2-rs-40ikw`.
//!
//! Acts 2 and 3 begin with an `AncientEventModel` (the boss-relic event)
//! selected by `up_front.NextItem(AllAncients ++ sharedSubset)`
//! (`ActModel.cs:297`; identity captured in
//! [`crate::run_room_gen::GeneratedActRooms::ancient`]). Each ancient's
//! `GenerateInitialOptions` rolls 3 relic offers off a per-event RNG seeded
//! per `EventModel.cs:193`:
//!   `Rng(RunState.Rng.Seed + NetId + deterministic_hash_code(Id.Entry))`
//! (single-player `NetId = 1`), i.e. `with_name(seed + 1, ancient.entry())`.
//! The per-event RNG is ISOLATED from `up_front`, so an imperfect
//! deck-condition predicate only changes that one event's offer identities —
//! it never desyncs the global counter.
//!
//! Port pins (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Events/`):
//! `Orobas.cs`, `Pael.cs`, `Tezcatara.cs` (act 2);
//! `Nonupeipe.cs`, `Tanx.cs`, `Vakuu.cs` (act 3); `Darv.cs` (shared).
//!
//! These functions return the 3 offered [`RelicId`]s in C# option order.
//! Pickup side effects (DistinguishedCape −9 MaxHP, SeaGlass character
//! variant, TouchOfOrobas/ArchaicTooth/DustyTome setup) are handled at the
//! pick site, not here.

use crate::run_room_gen::AncientId;
use crate::DotNetRng as Rng;
use crate::RelicId;

/// Deck/run-state predicates the C# `GenerateInitialOptions` methods read.
/// Supplied by the content layer (it owns card-type lookups); the core roll
/// is a pure function of these + the RNG.
#[derive(Clone, Debug)]
pub struct AncientConditions {
    /// `CurrentActIndex` (0-based): 1 = act 2, 2 = act 3. Gates Darv's
    /// act-specific relic set (`Darv.cs` static ctor).
    pub act_index: i32,
    /// Pael: `Deck.Cards.Count(Goopy.CanEnchant) >= 3` adds PaelsClaw.
    pub goopy_eligible_count: usize,
    /// Pael: `Deck.Cards.Count(c.IsRemovable) >= 5` adds PaelsTooth.
    pub removable_count: usize,
    /// Pael: `!Owner.HasEventPet()` adds PaelsLegion. Ironclad never holds
    /// an event pet at this pin, so this is normally `false`.
    pub has_event_pet: bool,
    /// Tanx: `Deck.Cards.Count(Instinct.CanEnchant) >= 3` adds TriBoomerang.
    pub instinct_eligible_count: usize,
    /// Nonupeipe: `Deck.Cards.Count(Swift.CanEnchant) >= 4` adds
    /// BeautifulBracelet.
    pub swift_eligible_count: usize,
    /// Orobas: count of `UnlockState.Characters` other than the player's —
    /// the size of the `NextItem` pool that picks the SeaGlass character
    /// variant. The pick's identity is cosmetic (offer is `SeaGlass`
    /// regardless) but the RNG read must be consumed. 4 under the headless
    /// 5-character `UnlockState.all`.
    pub num_other_characters: usize,
    /// Orobas `OptionPool3`: TouchOfOrobas / ArchaicTooth, each present iff
    /// its `SetupForPlayer` holds (a starter relic / transcendence starter
    /// card exists), from `RunState::orobas_pool3`. Never empty: the C#
    /// LOCKED placeholder (neither holds) isn't modelled.
    pub orobas_pool3: Vec<RelicId>,
    /// Darv: PandorasBox is offered unless a run modifier clears the deck
    /// (`!Modifiers.Any(ClearsPlayerDeck)`). Standard runs → `true`.
    pub pandoras_allowed: bool,
    /// Tezcatara: `Deck.Cards.Any(c => c.Tags.Contains(Strike) && c.Rarity ==
    /// Basic)` appends `NutritiousSoup` to `OptionPool1` (`Tezcatara.cs:191`).
    pub tezcatara_basic_strike: bool,
}

/// `Rng.NextItem(IList)` — `NextInt(0, count)` (max-exclusive), then index.
/// A single-element list STILL reads (`NextInt(0, 1)`), matching C#.
fn next_item(rng: &mut Rng, lst: &[RelicId]) -> RelicId {
    assert!(!lst.is_empty(), "next_item on empty slice");
    let idx = rng.next_int_exclusive(0, lst.len() as i32) as usize;
    lst[idx]
}

/// Build + seed the per-event RNG for `ancient` from the run master seed,
/// per v0.107.1 `EventModel.cs:238`: the additive player term is
/// `GetPlayerSlotIndex(Owner) == 0` in single-player (`RunState.cs:620`),
/// so just the master seed + name hash (v0.103.2 used `Owner.NetId == 1`;
/// the v0.107.1 `NetId`→`GetPlayerSlotIndex` swap dropped it — bead
/// `sts2-rs-ms8mm`). Mirrors
/// [`crate::runstate::RunState::roll_neow_offers_if_needed`].
#[must_use]
pub fn event_rng(master_seed: i32, ancient: AncientId) -> Rng {
    Rng::with_name(master_seed, ancient.entry())
}

/// Dispatch to the selected ancient's option roll. Neow is handled by
/// [`crate::neow_offers`] (different return shape) and is not rolled here.
#[must_use]
pub fn roll_offers(ancient: AncientId, rng: &mut Rng, cond: &AncientConditions) -> Vec<RelicId> {
    match ancient {
        AncientId::Orobas => roll_orobas(rng, cond),
        AncientId::Pael => roll_pael(rng, cond),
        AncientId::Tezcatara => roll_tezcatara(rng, cond),
        AncientId::Nonupeipe => roll_nonupeipe(rng, cond),
        AncientId::Tanx => roll_tanx(rng, cond),
        AncientId::Vakuu => roll_vakuu(rng),
        AncientId::Darv => roll_darv(rng, cond),
        // Neow's offers are rolled via `neow_offers::roll_neow_offers`
        // (2 positives + 1 curse), not this 3-relic boss-relic surface.
        AncientId::Neow => Vec::new(),
    }
}

// ── Act 2 ───────────────────────────────────────────────────────────────

/// `Tezcatara.GenerateInitialOptions` (`Tezcatara.cs:188-198`) — 3
/// independent `NextItem` picks. `OptionPool1` is `[VeryHotCocoa,
/// YummyCookie]` (`:44-57`); `NutritiousSoup` is **appended last** only when
/// the deck holds a Basic Strike (`:190-194`). `SealOfGold` lives at the END
/// of `OptionPool3` (`:78-93`), not in `OptionPool2` (`:60-73`).
fn roll_tezcatara(rng: &mut Rng, cond: &AncientConditions) -> Vec<RelicId> {
    use RelicId::{
        BiiigHug, GoldenCompass, NutritiousSoup, PumpkinCandle, SealOfGold, Storybook,
        ToastyMittens, ToyBox, VeryHotCocoa, YummyCookie,
    };
    let mut pool1: smallvec::SmallVec<[RelicId; 3]> =
        smallvec::smallvec![VeryHotCocoa, YummyCookie];
    if cond.tezcatara_basic_strike {
        pool1.push(NutritiousSoup);
    }
    let pool2 = [BiiigHug, Storybook, ToastyMittens];
    let pool3 = [GoldenCompass, PumpkinCandle, ToyBox, SealOfGold];
    vec![
        next_item(rng, &pool1),
        next_item(rng, &pool2),
        next_item(rng, &pool3),
    ]
}

/// `Orobas.GenerateInitialOptions`. RNG order: (1) pick the SeaGlass
/// character variant from the other-character pool, (2) `NextFloat() < 1/3`
/// PrismaticGem vs SeaGlass, (3) `NextItem` over `pool1 ++ [item]`,
/// (4) `NextItem(pool2)`, (5) `NextItem(pool3)`.
fn roll_orobas(rng: &mut Rng, cond: &AncientConditions) -> Vec<RelicId> {
    use RelicId::{
        AlchemicalCoffer, Driftwood, ElectricShrymp, GlassEye, PrismaticGem, RadiantPearl,
        SandCastle, SeaGlass,
    };
    // (1) SeaGlass character pick — read consumed, identity is cosmetic.
    if cond.num_other_characters > 0 {
        let _ = rng.next_int_exclusive(0, cond.num_other_characters as i32);
    }
    // (2) PrismaticGem (1/3) vs SeaGlass.
    let item = if rng.next_float(1.0) < 0.333_333_3_f64 {
        PrismaticGem
    } else {
        SeaGlass
    };
    // (3) pool1 ++ [item].
    let list = [ElectricShrymp, GlassEye, SandCastle, item];
    let pool2 = [AlchemicalCoffer, Driftwood, RadiantPearl];
    vec![
        next_item(rng, &list),
        next_item(rng, &pool2),
        next_item(rng, &cond.orobas_pool3),
    ]
}

/// `Pael.GenerateInitialOptions`. OptionPool2 starts `[PaelsWing]`, gains
/// PaelsClaw / PaelsTooth by deck conditions, then `list.AddRange(list)`
/// DOUBLES it and PaelsGrowth is appended; OptionPool3 is `[PaelsEye,
/// PaelsBlood]` plus PaelsLegion when the player has no event pet.
fn roll_pael(rng: &mut Rng, cond: &AncientConditions) -> Vec<RelicId> {
    use RelicId::{
        PaelsBlood, PaelsClaw, PaelsEye, PaelsFlesh, PaelsGrowth, PaelsHorn, PaelsLegion,
        PaelsTears, PaelsTooth, PaelsWing,
    };
    let pool1 = [PaelsFlesh, PaelsHorn, PaelsTears];
    let o1 = next_item(rng, &pool1);

    let mut list = vec![PaelsWing];
    if cond.goopy_eligible_count >= 3 {
        list.push(PaelsClaw);
    }
    if cond.removable_count >= 5 {
        list.push(PaelsTooth);
    }
    // C# `list.AddRange(list)` — doubles the current contents…
    let doubled = list.clone();
    list.extend(doubled);
    // …then appends PaelsGrowth.
    list.push(PaelsGrowth);
    let o2 = next_item(rng, &list);

    let mut pool3 = vec![PaelsEye, PaelsBlood];
    if !cond.has_event_pet {
        pool3.push(PaelsLegion);
    }
    let o3 = next_item(rng, &pool3);

    vec![o1, o2, o3]
}

// ── Act 3 ───────────────────────────────────────────────────────────────

/// `Vakuu.GenerateInitialOptions` — `UnstableShuffle` each of 3 pools, take
/// the head of each. DistinguishedCape's −9 MaxHP is a pickup side effect.
fn roll_vakuu(rng: &mut Rng) -> Vec<RelicId> {
    use RelicId::{
        BloodSoakedRose, ChoicesParadox, DistinguishedCape, Fiddle, JeweledMask, LordsParasol,
        MusicBox, PreservedFog, SereTalon, WhisperingEarring,
    };
    let mut pool1 = vec![BloodSoakedRose, WhisperingEarring, Fiddle];
    let mut pool2 = vec![PreservedFog, SereTalon, DistinguishedCape];
    let mut pool3 = vec![ChoicesParadox, MusicBox, LordsParasol, JeweledMask];
    rng.shuffle(&mut pool1);
    rng.shuffle(&mut pool2);
    rng.shuffle(&mut pool3);
    vec![pool1[0], pool2[0], pool3[0]]
}

/// `Tanx.GenerateInitialOptions` — base pool of 9 weapons (+TriBoomerang
/// when the deck has ≥3 Instinct-enchantable cards), `UnstableShuffle`,
/// take 3.
fn roll_tanx(rng: &mut Rng, cond: &AncientConditions) -> Vec<RelicId> {
    use RelicId::{
        Claws, Crossbow, IronClub, MeatCleaver, Sai, SpikedGauntlets, TanxsWhistle, ThrowingAxe,
        TriBoomerang, WarHammer,
    };
    let mut list = vec![
        Claws,
        Crossbow,
        IronClub,
        MeatCleaver,
        Sai,
        SpikedGauntlets,
        TanxsWhistle,
        ThrowingAxe,
        WarHammer,
    ];
    if cond.instinct_eligible_count >= 3 {
        list.push(TriBoomerang);
    }
    rng.shuffle(&mut list);
    list.into_iter().take(3).collect()
}

/// `Nonupeipe.GenerateInitialOptions` — base pool of 9 (+BeautifulBracelet
/// when the deck has ≥4 Swift-enchantable cards), `UnstableShuffle`, take 3.
fn roll_nonupeipe(rng: &mut Rng, cond: &AncientConditions) -> Vec<RelicId> {
    use RelicId::{
        BeautifulBracelet, BlessedAntler, BrilliantScarf, DelicateFrond, DiamondDiadem, FurCoat,
        Glitter, JewelryBox, LoomingFruit, SignetRing,
    };
    let mut list = vec![
        BlessedAntler,
        BrilliantScarf,
        DelicateFrond,
        DiamondDiadem,
        FurCoat,
        Glitter,
        JewelryBox,
        LoomingFruit,
        SignetRing,
    ];
    if cond.swift_eligible_count >= 4 {
        list.push(BeautifulBracelet);
    }
    rng.shuffle(&mut list);
    list.into_iter().take(3).collect()
}

// ── Shared ──────────────────────────────────────────────────────────────

/// `Darv.GenerateInitialOptions`. For each valid relic-set (declaration
/// order, act-gated), `NextItem(set)` picks one relic (single-element sets
/// still read); the picks are `UnstableShuffle`d; then `NextBool()` decides
/// between [take 2 + DustyTome] and [take 3].
fn roll_darv(rng: &mut Rng, cond: &AncientConditions) -> Vec<RelicId> {
    use RelicId::{
        Astrolabe, BlackStar, CallingBell, DustyTome, Ectoplasm, EmptyCage, PandorasBox,
        PhilosophersStone, RunicPyramid, SneckoEye, Sozu, VelvetChoker,
    };
    // `_validRelicSets` in C# declaration order, filtered by act / modifier.
    let mut sets: Vec<&[RelicId]> = vec![&[Astrolabe], &[BlackStar], &[CallingBell], &[EmptyCage]];
    if cond.pandoras_allowed {
        sets.push(&[PandorasBox]);
    }
    sets.push(&[RunicPyramid]);
    sets.push(&[SneckoEye]);
    if cond.act_index == 1 {
        sets.push(&[Ectoplasm, Sozu]);
    }
    if cond.act_index == 2 {
        sets.push(&[PhilosophersStone, VelvetChoker]);
    }

    let mut source: Vec<RelicId> = sets.iter().map(|s| next_item(rng, s)).collect();
    rng.shuffle(&mut source);

    if rng.next_bool() {
        let mut out: Vec<RelicId> = source.into_iter().take(2).collect();
        out.push(DustyTome);
        out
    } else {
        source.into_iter().take(3).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond() -> AncientConditions {
        AncientConditions {
            act_index: 1,
            goopy_eligible_count: 0,
            removable_count: 0,
            has_event_pet: false,
            instinct_eligible_count: 0,
            swift_eligible_count: 0,
            num_other_characters: 4,
            orobas_pool3: vec![RelicId::TouchOfOrobas, RelicId::ArchaicTooth],
            pandoras_allowed: true,
            tezcatara_basic_strike: false,
        }
    }

    /// Corpus-anchored RNG-faithfulness: for real scraped v0.103.2 Ironclad
    /// runs, our per-event roll reproduces the EXACT 3 relics the game
    /// offered at the act-2/3 Ancient room. This pins the whole pipeline —
    /// master RNG → `event_rng` seed (`with_name(master + 1, entry)`) → the
    /// per-event `NextItem` / `NextFloat` / `UnstableShuffle` call sequence.
    /// Anchors are the deck-INDEPENDENT ancients (Tezcatara / Orobas / Vakuu)
    /// so no deck snapshot is needed; offer sets extracted from
    /// `sts2_run_scraper/data/files/the-ironclad/**.run`
    /// (`map_point_history[*][*].player_stats[0].ancient_choice`).
    ///
    /// CAVEAT — these are the scraped seeds whose run-master derives correctly
    /// from the display string via `deterministic_hash_code(canonicalize_seed)`.
    /// ~20% of scraped string seeds do NOT round-trip through that
    /// canonicalization (verified: those same seeds also mis-derive the
    /// deterministic Neow act-1 offers — a per-seed master-derivation gap, not
    /// a per-ancient roll bug, since every ancient of a given seed passes or
    /// fails together). They are EXCLUDED here. This does not affect the
    /// engine/eval/training path, which seeds `rs.rng.seed` from an integer
    /// directly (no string round-trip) and is therefore byte-exact with the C#
    /// headless. Mirrors the Neow corpus anchor in [`crate::neow_offers`].
    // v0.107.1 RNG rework: these offers are anchored to SCRAPED v0.103.2 human
    // runs (System.Random). Under v0.107.1's MegaRandom the same seed yields
    // different offers, so a v0.103.2 corpus anchor cannot hold on this branch
    // (see the version-pin strategy: the run corpus is build_id==v0.103.2). No
    // v0.107.1 human corpus exists yet to re-anchor against.
    #[ignore = "v0.103.2 scraped-corpus anchor; invalid under v0.107.1 MegaRandom RNG (no v0.107.1 corpus yet)"]
    #[test]
    fn corpus_anchored_boss_relic_offers() {
        // (seed, ancient, expected SCREAMING_SNAKE offer set) — first-visit
        // act-2/3 Ancient rooms from real v0.103.2 standard Ironclad runs
        // (no run modifiers). Deck-independent ancients, master-correct seeds.
        use AncientId::{Orobas, Tezcatara, Vakuu};
        let cases: &[(&str, AncientId, [&str; 3])] = &[
            (
                "Y3X2YZSBQJ",
                Tezcatara,
                ["GOLDEN_COMPASS", "TOASTY_MITTENS", "VERY_HOT_COCOA"],
            ),
            (
                "YTWN1R1T54",
                Tezcatara,
                ["NUTRITIOUS_SOUP", "TOASTY_MITTENS", "TOY_BOX"],
            ),
            (
                "9NNCWTW7HW",
                Tezcatara,
                ["PUMPKIN_CANDLE", "SEAL_OF_GOLD", "YUMMY_COOKIE"],
            ),
            (
                "EZXNXCNC9B",
                Tezcatara,
                ["BIIIG_HUG", "PUMPKIN_CANDLE", "YUMMY_COOKIE"],
            ),
            (
                "12LXWDG96M",
                Tezcatara,
                ["GOLDEN_COMPASS", "STORYBOOK", "VERY_HOT_COCOA"],
            ),
            (
                "Y0RJF8MPJ5",
                Tezcatara,
                ["NUTRITIOUS_SOUP", "PUMPKIN_CANDLE", "TOASTY_MITTENS"],
            ),
            (
                "GLWW4BNJ1B",
                Tezcatara,
                ["TOASTY_MITTENS", "TOY_BOX", "YUMMY_COOKIE"],
            ),
            (
                "W97N80Y4FZ",
                Tezcatara,
                ["GOLDEN_COMPASS", "SEAL_OF_GOLD", "YUMMY_COOKIE"],
            ),
            (
                "SHXESP8WGK",
                Tezcatara,
                ["BIIIG_HUG", "TOY_BOX", "YUMMY_COOKIE"],
            ),
            (
                "SLDFQN6E0F",
                Orobas,
                ["ARCHAIC_TOOTH", "GLASS_EYE", "RADIANT_PEARL"],
            ),
            (
                "ZHM928V6DW",
                Orobas,
                ["ARCHAIC_TOOTH", "GLASS_EYE", "RADIANT_PEARL"],
            ),
            (
                "UGXGZBB7N3",
                Orobas,
                ["ALCHEMICAL_COFFER", "ELECTRIC_SHRYMP", "TOUCH_OF_OROBAS"],
            ),
            (
                "XHYJ5CQUWU",
                Orobas,
                ["ALCHEMICAL_COFFER", "GLASS_EYE", "TOUCH_OF_OROBAS"],
            ),
            (
                "F5SZ1860ZD",
                Orobas,
                ["GLASS_EYE", "RADIANT_PEARL", "TOUCH_OF_OROBAS"],
            ),
            (
                "59CJKPFN9S",
                Orobas,
                ["ARCHAIC_TOOTH", "GLASS_EYE", "RADIANT_PEARL"],
            ),
            (
                "2V0WQRWRBU",
                Orobas,
                ["ALCHEMICAL_COFFER", "PRISMATIC_GEM", "TOUCH_OF_OROBAS"],
            ),
            (
                "0C770NVXCP",
                Orobas,
                ["ARCHAIC_TOOTH", "RADIANT_PEARL", "SAND_CASTLE"],
            ),
            (
                "75YLV4Y5U2",
                Orobas,
                ["ARCHAIC_TOOTH", "DRIFTWOOD", "PRISMATIC_GEM"],
            ),
            (
                "CUK8K2S8M5",
                Orobas,
                ["DRIFTWOOD", "SAND_CASTLE", "TOUCH_OF_OROBAS"],
            ),
            (
                "3FYFKZ6MAZ",
                Orobas,
                ["ARCHAIC_TOOTH", "RADIANT_PEARL", "SAND_CASTLE"],
            ),
            (
                "XSK2NET8WQ",
                Vakuu,
                ["CHOICES_PARADOX", "DISTINGUISHED_CAPE", "FIDDLE"],
            ),
            (
                "Y0RJF8MPJ5",
                Vakuu,
                ["DISTINGUISHED_CAPE", "JEWELED_MASK", "WHISPERING_EARRING"],
            ),
            ("TNYLTG12KY", Vakuu, ["FIDDLE", "MUSIC_BOX", "SERE_TALON"]),
            (
                "CXTGFLZGH8",
                Vakuu,
                ["BLOOD_SOAKED_ROSE", "MUSIC_BOX", "PRESERVED_FOG"],
            ),
            (
                "SLDFQN6E0F",
                Vakuu,
                ["CHOICES_PARADOX", "SERE_TALON", "WHISPERING_EARRING"],
            ),
            (
                "ZHM928V6DW",
                Vakuu,
                ["BLOOD_SOAKED_ROSE", "MUSIC_BOX", "PRESERVED_FOG"],
            ),
            (
                "ZU90R93CE4",
                Vakuu,
                ["FIDDLE", "JEWELED_MASK", "PRESERVED_FOG"],
            ),
            (
                "10T016AJU3",
                Vakuu,
                ["BLOOD_SOAKED_ROSE", "CHOICES_PARADOX", "SERE_TALON"],
            ),
            (
                "K1L5RXTYAE",
                Vakuu,
                ["CHOICES_PARADOX", "FIDDLE", "PRESERVED_FOG"],
            ),
            (
                "0C770NVXCP",
                Vakuu,
                ["BLOOD_SOAKED_ROSE", "CHOICES_PARADOX", "SERE_TALON"],
            ),
            (
                "G0G8MWN54S",
                Vakuu,
                ["DISTINGUISHED_CAPE", "MUSIC_BOX", "WHISPERING_EARRING"],
            ),
            (
                "75YLV4Y5U2",
                Vakuu,
                [
                    "CHOICES_PARADOX",
                    "DISTINGUISHED_CAPE",
                    "WHISPERING_EARRING",
                ],
            ),
        ];
        let sset = |mut v: Vec<String>| {
            v.sort();
            v
        };
        let c = cond();
        let mut fails = 0;
        for (seed, ancient, expected) in cases {
            let master = crate::deterministic_hash_code(&crate::canonicalize_seed(seed));
            let mut rng = event_rng(master, *ancient);
            let got: Vec<String> = roll_offers(*ancient, &mut rng, &c)
                .iter()
                .map(|r| r.to_python_name().to_string())
                .collect();
            let exp: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
            if sset(got.clone()) != sset(exp.clone()) {
                fails += 1;
                println!("BAD seed {seed} {ancient:?}: got {got:?} expected {exp:?}");
            }
        }
        assert_eq!(
            fails,
            0,
            "{fails}/{} corpus boss-relic anchors mismatched",
            cases.len()
        );
    }

    /// Every non-Neow ancient yields exactly 3 offers, all distinct
    /// (each event draws one option from each of three disjoint pools, or
    /// shuffles+takes 3 from a single pool of distinct relics).
    #[test]
    fn every_ancient_yields_three_distinct_offers() {
        let ancients = [
            AncientId::Orobas,
            AncientId::Pael,
            AncientId::Tezcatara,
            AncientId::Nonupeipe,
            AncientId::Tanx,
            AncientId::Vakuu,
            AncientId::Darv,
        ];
        let c = cond();
        for a in ancients {
            for seed in 0..64i32 {
                let mut rng = event_rng(seed.wrapping_mul(2_654_435_761u32 as i32), a);
                let offers = roll_offers(a, &mut rng, &c);
                assert_eq!(offers.len(), 3, "{a:?} offered {} relics", offers.len());
                assert!(
                    offers[0] != offers[1] && offers[1] != offers[2] && offers[0] != offers[2],
                    "{a:?} offered duplicates: {offers:?}"
                );
            }
        }
    }

    /// The roll is deterministic for a fixed seed + conditions.
    #[test]
    fn roll_is_deterministic() {
        let c = cond();
        for a in [AncientId::Orobas, AncientId::Pael, AncientId::Darv] {
            let mut r1 = event_rng(12345, a);
            let mut r2 = event_rng(12345, a);
            assert_eq!(roll_offers(a, &mut r1, &c), roll_offers(a, &mut r2, &c));
        }
    }

    /// Tezcatara draws one relic from each fixed pool, in pool order
    /// (`Tezcatara.cs:188-198`). `NutritiousSoup` is in pool 1 only when the
    /// deck holds a Basic Strike; `SealOfGold` lives at the end of pool 3.
    #[test]
    fn tezcatara_offers_come_from_each_pool() {
        let pool2 = [
            RelicId::BiiigHug,
            RelicId::Storybook,
            RelicId::ToastyMittens,
        ];
        let pool3 = [
            RelicId::GoldenCompass,
            RelicId::PumpkinCandle,
            RelicId::ToyBox,
            RelicId::SealOfGold,
        ];
        // Without a Basic Strike: pool 1 = [VeryHotCocoa, YummyCookie].
        let mut c = cond();
        c.tezcatara_basic_strike = false;
        let pool1_no = [RelicId::VeryHotCocoa, RelicId::YummyCookie];
        // With a Basic Strike: NutritiousSoup is appended.
        let pool1_yes = [
            RelicId::VeryHotCocoa,
            RelicId::YummyCookie,
            RelicId::NutritiousSoup,
        ];
        for seed in 0..128i32 {
            let mut rng = event_rng(seed, AncientId::Tezcatara);
            let o = roll_tezcatara(&mut rng, &c);
            assert!(pool1_no.contains(&o[0]));
            assert!(pool2.contains(&o[1]));
            assert!(pool3.contains(&o[2]));
        }
        c.tezcatara_basic_strike = true;
        for seed in 0..128i32 {
            let mut rng = event_rng(seed, AncientId::Tezcatara);
            let o = roll_tezcatara(&mut rng, &c);
            assert!(pool1_yes.contains(&o[0]));
            assert!(pool2.contains(&o[1]));
            assert!(pool3.contains(&o[2]));
        }
    }

    /// Tanx's TriBoomerang appears only when the Instinct condition holds.
    #[test]
    fn tanx_triboomerang_gated_on_instinct() {
        let mut c = cond();
        c.instinct_eligible_count = 0;
        let mut saw_no = false;
        for seed in 0..256i32 {
            let mut rng = event_rng(seed, AncientId::Tanx);
            if roll_tanx(&mut rng, &c).contains(&RelicId::TriBoomerang) {
                saw_no = true;
            }
        }
        assert!(!saw_no, "TriBoomerang offered without Instinct≥3");

        c.instinct_eligible_count = 3;
        let mut saw_yes = false;
        for seed in 0..256i32 {
            let mut rng = event_rng(seed, AncientId::Tanx);
            if roll_tanx(&mut rng, &c).contains(&RelicId::TriBoomerang) {
                saw_yes = true;
                break;
            }
        }
        assert!(saw_yes, "TriBoomerang never offered with Instinct≥3");
    }

    /// Darv's act gate swaps PhilosophersStone/VelvetChoker (act 3) for
    /// Ectoplasm/Sozu (act 2) in the candidate universe.
    #[test]
    fn darv_act_gate_swaps_relic_sets() {
        let mut c2 = cond();
        c2.act_index = 1;
        let mut c3 = cond();
        c3.act_index = 2;
        let mut act2_seen = std::collections::HashSet::new();
        let mut act3_seen = std::collections::HashSet::new();
        for seed in 0..512i32 {
            let mut r2 = event_rng(seed, AncientId::Darv);
            act2_seen.extend(roll_darv(&mut r2, &c2));
            let mut r3 = event_rng(seed, AncientId::Darv);
            act3_seen.extend(roll_darv(&mut r3, &c3));
        }
        assert!(act2_seen.contains(&RelicId::Sozu) || act2_seen.contains(&RelicId::Ectoplasm));
        assert!(!act2_seen.contains(&RelicId::PhilosophersStone));
        assert!(!act2_seen.contains(&RelicId::VelvetChoker));
        assert!(
            act3_seen.contains(&RelicId::PhilosophersStone)
                || act3_seen.contains(&RelicId::VelvetChoker)
        );
    }
}
