//! Shop + Ancient (Boss) relic dispatch — bead `sts2-rs-yyw` (P7-5).
//!
//! Ported from `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py`,
//! restricted to the `RelicRarity.SHOP` (30 entries) and
//! `RelicRarity.ANCIENT` (92 entries) classes — 122 relics total. The
//! sibling bead `sts2-rs-7ez` ports the `RelicRarity.EVENT` subset of
//! the same Python file into `event_reward.rs`.
//!
//! ## Terminology note
//!
//! The bead description and the game UI both refer to these as
//! "Boss relics". In the Python source they live in the
//! `RelicRarity.ANCIENT` bucket (see
//! `sts2-rl-agent@01c094d:sts2_env/core/enums.py`). Throughout this
//! file `ANCIENT` is the Python rarity and "Boss" is the gameplay
//! label — they refer to the same set.
//!
//! ## Dead Branch caveat
//!
//! The bead spec called for deep pin-tests on `SneckoEye`,
//! `DeadBranch`, and `PandorasBox`. `DeadBranch` does **not** exist in
//! the `01c094d` fork of `sts2-rl-agent` (no `class DeadBranch` in
//! `shop_event.py`, no `RelicId::DEAD_BRANCH` in `base.py`). The deep
//! pin-tests below cover `SneckoEye`, `PandorasBox`, and a third
//! high-impact boss relic (`Brimstone`, which mutates per-turn
//! Strength on both sides — the closest combat-math analogue to Dead
//! Branch's "auto-generate cards" hook in this fork).
//!
//! ## Handler convention
//!
//! Each handler is a marker that returns `EffectResult::Resolved`
//! (matches the canonical pattern landed in `common.rs` /
//! `uncommon.rs` / `rare.rs` by sibling beads `evs`, `l22`, `ge1`).
//! Gameplay plumbing — potion-slot growth, energy modifications,
//! card-reward filters, rest-site additions, gold-gain hooks —
//! deferred to downstream integration beads. Each handler's doc
//! comment records the Python class name + rarity so the wiring
//! bead can locate the source listener in one grep.

use sts2_core::card_effects::{
    apply_power_to_creature, generate_free_attack_in_hand, set_card_var, spawn_card_into_hand,
    EffectResult,
};
use sts2_core::cards::{CardType, VarKey};
use sts2_core::hooks::CardPlayCountCtx;
use sts2_core::hooks::{self, BlockCtx, BlockGainedCtx, CreatureRef, ValueProps};
use sts2_core::powers::PowerKind;
use sts2_core::relic_effects::{
    register_after_block_gained, register_after_card_played, register_after_side_turn_start,
    register_before_combat_start, register_before_side_turn_start, register_before_turn_end,
    register_modify_block_multiplicative, register_modify_damage_additive,
    register_owned_after_card_exhausted, register_owned_after_potion_used,
    register_owned_after_shuffle_bus, register_owned_before_hand_draw,
    register_owned_modify_hand_draw, register_owned_modify_x_value,
    register_owned_relic_modify_card_play_count, AfterBlockGainedFn, AfterCardExhaustedFn,
    AfterCardPlayedFn, AfterPotionUsedFn, AfterShuffleFn, AfterSideTurnStartFn,
    BeforeCombatStartFn, BeforeHandDrawFn, BeforeSideTurnStartFn, BeforeTurnEndFn, CardEventCtx,
    ModifyBlockMultiplicativeFn, ModifyDamageAdditiveFn, ModifyDamageCtx, ModifyHandDrawFn,
    ModifyXValueFn, RelicModifyCardPlayCountFn, ShuffleCtx,
};
use sts2_core::relics::RelicId;
use sts2_core::run_effects::{
    register_after_obtained, register_after_rest_site_heal, register_after_room_entered,
    register_after_shop_inventory_generated, register_allow_card_reward_reroll,
    register_modify_card_reward_creation_options, register_modify_card_reward_options_late,
    register_modify_generated_map, register_modify_generated_map_late,
    register_modify_rest_site_options, register_modify_rewards,
    register_modify_unknown_map_point_room_types, register_on_gold_gained,
    register_should_disable_remaining_rest_site_options, register_should_gain_gold,
    register_should_generate_treasure, register_should_procure_potion, AfterObtainedRunFn,
    AfterRestSiteHealRunFn, AfterRoomEnteredRunFn, AfterShopInventoryGeneratedRunFn,
    AllowCardRewardRerollRunFn, ModifyCardRewardCreationOptionsRunFn,
    ModifyCardRewardOptionsLateRunFn, ModifyGeneratedMapLateRunFn, ModifyGeneratedMapRunFn,
    ModifyRestSiteOptionsRunFn, ModifyRewardsRunFn, ModifyUnknownMapPointRoomTypesRunFn,
    OnGoldGainedRunFn, ShouldDisableRemainingRestSiteOptionsRunFn, ShouldGainGoldRunFn,
    ShouldGenerateTreasureRunFn, ShouldProcurePotionRunFn,
};
use sts2_core::runstate::{
    CardReward, CardRewardGenerationOptions, PotionReward, RelicRarity, RelicReward,
    RestSiteOption, Reward, RoomType, RunState,
};
use sts2_core::shop::{auto_purchase_shop_inventory, ShopInventory};
use sts2_core::{CombatState, RelicKind, Side};

// ---------------------------------------------------------------------------
// Pool installer
// ---------------------------------------------------------------------------

/// All 122 shop + boss + ancient relic ids owned by this pool, in
/// Python `@register_relic` declaration order. Membership/identity data
/// only — live behaviour flows through the typed hook-channel listeners
/// registered by [`install_pool`], not through any dispatch marker.
/// Backs [`pool_relic_ids`] (the run-loop boss-relic source).
const SHOP_BOSS_RELIC_IDS: &[RelicId] = &[
    RelicId::BeltBuckle,
    RelicId::Bread,
    RelicId::Brimstone,
    RelicId::BurningSticks,
    RelicId::Cauldron,
    RelicId::ChemicalX,
    RelicId::DingyRug,
    RelicId::DollysMirror,
    RelicId::DragonFruit,
    RelicId::GhostSeed,
    RelicId::GnarledHammer,
    RelicId::Kifuda,
    RelicId::LavaLamp,
    RelicId::LeesWaffle,
    RelicId::MembershipCard,
    RelicId::MiniatureTent,
    RelicId::MysticLighter,
    RelicId::NinjaScroll,
    RelicId::Orrery,
    RelicId::PunchDagger,
    RelicId::RingingTriangle,
    RelicId::RoyalStamp,
    RelicId::RunicCapacitor,
    RelicId::ScreamingFlagon,
    RelicId::SlingOfCourage,
    RelicId::TheAbacus,
    RelicId::Toolbox,
    RelicId::UndyingSigil,
    RelicId::VitruvianMinion,
    RelicId::WingCharm,
    RelicId::AlchemicalCoffer,
    RelicId::ArcaneScroll,
    RelicId::ArchaicTooth,
    RelicId::Astrolabe,
    RelicId::BeautifulBracelet,
    RelicId::BiiigHug,
    RelicId::BlackStar,
    RelicId::BlessedAntler,
    RelicId::BloodSoakedRose,
    RelicId::BoomingConch,
    RelicId::BrilliantScarf,
    RelicId::CallingBell,
    RelicId::ChoicesParadox,
    RelicId::Claws,
    RelicId::Crossbow,
    RelicId::CursedPearl,
    RelicId::DelicateFrond,
    RelicId::DiamondDiadem,
    RelicId::DistinguishedCape,
    RelicId::Driftwood,
    RelicId::DustyTome,
    RelicId::Ectoplasm,
    RelicId::ElectricShrymp,
    RelicId::EmptyCage,
    RelicId::Fiddle,
    RelicId::FurCoat,
    RelicId::GlassEye,
    RelicId::Glitter,
    RelicId::GoldenCompass,
    RelicId::GoldenPearl,
    RelicId::IronClub,
    RelicId::JeweledMask,
    RelicId::JewelryBox,
    RelicId::LargeCapsule,
    RelicId::LavaRock,
    RelicId::LeadPaperweight,
    RelicId::LeafyPoultice,
    RelicId::LoomingFruit,
    RelicId::LordsParasol,
    RelicId::LostCoffer,
    RelicId::MassiveScroll,
    RelicId::MeatCleaver,
    RelicId::MusicBox,
    RelicId::NeowsTorment,
    RelicId::NewLeaf,
    RelicId::NutritiousOyster,
    RelicId::NutritiousSoup,
    RelicId::PaelsBlood,
    RelicId::PaelsClaw,
    RelicId::PaelsEye,
    RelicId::PaelsFlesh,
    RelicId::PaelsGrowth,
    RelicId::PaelsHorn,
    RelicId::PaelsLegion,
    RelicId::PaelsTears,
    RelicId::PaelsTooth,
    RelicId::PaelsWing,
    RelicId::PandorasBox,
    RelicId::PhilosophersStone,
    RelicId::Pomander,
    RelicId::PrecariousShears,
    RelicId::PreciseScissors,
    RelicId::PreservedFog,
    RelicId::PrismaticGem,
    RelicId::PumpkinCandle,
    RelicId::RadiantPearl,
    RelicId::RunicPyramid,
    RelicId::Sai,
    RelicId::SandCastle,
    RelicId::ScrollBoxes,
    RelicId::SeaGlass,
    RelicId::SealOfGold,
    RelicId::SereTalon,
    RelicId::SignetRing,
    RelicId::SilverCrucible,
    RelicId::SmallCapsule,
    RelicId::SneckoEye,
    RelicId::Sozu,
    RelicId::SpikedGauntlets,
    RelicId::StoneHumidifier,
    RelicId::Storybook,
    RelicId::TanxsWhistle,
    RelicId::ThrowingAxe,
    RelicId::ToastyMittens,
    RelicId::TouchOfOrobas,
    RelicId::ToyBox,
    RelicId::TriBoomerang,
    RelicId::VelvetChoker,
    RelicId::VeryHotCocoa,
    RelicId::WarHammer,
    RelicId::WhisperingEarring,
    RelicId::YummyCookie,
];

/// Install every shop/boss-relic handler. Called from
/// [`crate::relics::dispatch::install_default_registry`]. Idempotent
/// (re-registering overwrites the same function pointer).
/// Every [`RelicId`] registered in this module's shop+boss pool — the
/// droppable relics of this tier. Used to seed the run's relic grab
/// bag so combat/treasure/shop relic rewards are stocked (the run
/// loop's `populate_relic_grab_bag_by_rarity` source). Bead `sts2-rs-64mq2`.
#[must_use]
pub fn pool_relic_ids() -> Vec<RelicId> {
    SHOP_BOSS_RELIC_IDS.to_vec()
}

pub fn install_pool() {
    // Bead `sts2-rs-bmw3`: real `after_obtained` listener for
    // `LeesWaffle::MAX_HP = 7` + heal-to-full (Python
    // `shop_event.py::LeesWaffle.after_obtained`). Combat-side
    // `on_trigger_lees_waffle` stays a `resolved_marker`; this is the
    // run-state mutator that actually bumps max HP and tops the player up.
    register_after_obtained(
        RelicId::LeesWaffle,
        listener_lees_waffle as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-r4pr`: real `after_obtained` listeners for
    // `LoomingFruit::MAX_HP = 31` and `NutritiousOyster::MAX_HP = 11`
    // (Python `shop_event.py::LoomingFruit` / `::NutritiousOyster`).
    // Both ANCIENT-pool, pure `gain_max_hp` bodies — same gap shape as
    // Strawberry/Pear/Mango/OldCoin/LeesWaffle.
    register_after_obtained(
        RelicId::LoomingFruit,
        listener_looming_fruit as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::NutritiousOyster,
        listener_nutritious_oyster as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-yfi0`: real `after_obtained` listener for
    // `GoldenPearl::GOLD = 150` (Python `shop_event.py::GoldenPearl`).
    // Same gap shape as OldCoin (tyqo) — combat-side
    // `on_trigger_golden_pearl` stays a `resolved_marker`; this is the
    // run-state mutator that actually grants the gold.
    register_after_obtained(
        RelicId::GoldenPearl,
        listener_golden_pearl as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-2ciy`: real `after_obtained` listener for
    // `SignetRing::GOLD = 999` (Python `shop_event.py::SignetRing`).
    // Same gap shape as GoldenPearl (yfi0) — combat-side
    // `on_trigger_signet_ring` stays a `resolved_marker`; this is the
    // run-state mutator that actually grants the gold.
    register_after_obtained(
        RelicId::SignetRing,
        listener_signet_ring as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-run-rest-option`: PumpkinCandle ANCIENT (v0.107.1) —
    // `AfterObtained → Rekindle` sets `KindleCount` (stored in
    // `rs.relic_counters[PumpkinCandle]`) by adding `kindleAmount = 5`.
    // While > 0 the combat-side `modify_max_energy` arm grants +1 max
    // energy; the count decays 1/combat and the rest-site Kindle option
    // tops it back up.
    register_after_obtained(
        RelicId::PumpkinCandle,
        listener_pumpkin_candle as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-run-rest-option`: PumpkinCandle's
    // `TryModifyRestSiteOptions` appends the KINDLE option to every rest
    // site while owned (C# `PumpkinCandle.cs:77-85`). Mirrors the Shovel
    // `modify_rest_site_options` precedent in `rare.rs`.
    register_modify_rest_site_options(
        RelicId::PumpkinCandle,
        listener_pumpkin_candle_rest_site_options as ModifyRestSiteOptionsRunFn,
    );
    // Bead `sts2-rs-u5emw`: DingyRug SHOP — replace the implicit
    // character pool with an explicit `custom_card_ids` slate
    // (character + colorless) on the
    // `modify_card_reward_creation_options` channel (tu7g5
    // precedent — first consumer of the channel). The combat-side
    // dispatch in `on_trigger_dingy_rug` stays a `Resolved` marker.
    register_modify_card_reward_creation_options(
        RelicId::DingyRug,
        listener_dingy_rug as ModifyCardRewardCreationOptionsRunFn,
    );
    // Bead `sts2-rs-psf9`: real `after_obtained` listener for Orrery.
    // Python `shop_event.py::Orrery.after_obtained` loops `CARDS = 5`
    // calls to `owner.offer_card_reward()`. Rust's `RelicReward` /
    // `CardReward` POD layer is the sentinel surface for umbrella
    // sts2-rs-01o (random reward rolling); same convention as
    // BlackStar (83sa) / PrayerWheel (4ihz) / WhiteStar (b4tb) —
    // push `Reward::Card(CardReward::empty())` placeholders that the
    // 01o roller will materialize.
    register_after_obtained(RelicId::Orrery, listener_orrery as AfterObtainedRunFn);
    // Bead `sts2-rs-beda`: real `after_obtained` listener for
    // `LeafyPoultice::MAX_HP_LOSS = 10` (Python
    // `shop_event.py::LeafyPoultice.after_obtained`). Python also
    // transforms 1 Strike + 1 Defend from the basic deck; that clause
    // is deferred to umbrella sts2-rs-01o / run-side deck transform
    // surface — same gating as StoneCracker / PunchDagger / GnarledHammer.
    register_after_obtained(
        RelicId::LeafyPoultice,
        listener_leafy_poultice as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-u4wv`: real `after_obtained` listener for
    // CallingBell. Python `shop_event.py::CallingBell.after_obtained`
    // adds CurseOfTheBell to the deck and calls
    // `owner.offer_relic_rewards(3, rarities=...)`. The relic-rewards
    // portion uses `rs.offer_relic_rewards(RelicId::CallingBell, 3)`
    // sentinel — umbrella sts2-rs-01o resolves the rarity tiers at
    // flush time. The CurseOfTheBell card-add clause is deferred to
    // umbrella sts2-rs-01o pending run-side deck mutation surface
    // (same gating as NeowsTorment / PaelsHorn).
    register_after_obtained(
        RelicId::CallingBell,
        listener_calling_bell as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-gf5s`: real `after_obtained` listener for
    // SmallCapsule. Python `shop_event.py::SmallCapsule.after_obtained`:
    // `owner.offer_relic_rewards(1)`. Same shape as CallingBell (u4wv)
    // but 1 reward instead of 3.
    register_after_obtained(
        RelicId::SmallCapsule,
        listener_small_capsule as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-2nrk`: real `after_obtained` listener for Cauldron.
    // Python `shop_event.py::Cauldron.after_obtained` does
    // `owner.offer_potions(5)`. Push 5 `Reward::Potion(PotionReward::empty())`
    // sentinels — the dispatcher rolls concrete PotionKinds at flush
    // time (ib04 convention). Same shape as Orrery's CardReward push.
    register_after_obtained(RelicId::Cauldron, listener_cauldron as AfterObtainedRunFn);
    // Bead `sts2-rs-64vb`: real `after_obtained` listener for
    // MassiveScroll. Python `shop_event.py::MassiveScroll.after_obtained`
    // calls `owner.offer_multiplayer_cards(3)`. Coop/multiplayer is
    // out of scope per CLAUDE.md — collapses to 3 regular CardReward
    // offers, same convention as Orrery (psf9).
    register_after_obtained(
        RelicId::MassiveScroll,
        listener_massive_scroll as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.4` (enchant-on-obtain wave): real
    // `after_obtained` listeners for the five static-enchantment
    // shop/event relics (Python `shop_event.py::{GnarledHammer,
    // Kifuda, PunchDagger, BeautifulBracelet, RoyalStamp}`). All
    // share the same `enchant_selected_cards` direct-apply shape,
    // routed through the [`enchant_eligible_deck_cards`] helper.
    // The `defer_followup_rewards` reward-queue arm is gated on
    // umbrella `sts2-rs-01o` (future `Reward::Enchant` variant).
    register_after_obtained(
        RelicId::GnarledHammer,
        listener_gnarled_hammer as AfterObtainedRunFn,
    );
    register_after_obtained(RelicId::Kifuda, listener_kifuda as AfterObtainedRunFn);
    register_after_obtained(
        RelicId::PunchDagger,
        listener_punch_dagger as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::BeautifulBracelet,
        listener_beautiful_bracelet as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::RoyalStamp,
        listener_royal_stamp as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.4` — TriBoomerang: same enchant-on-obtain
    // pattern, Instinct(1) on up to 3 Attack cards. Uses the same
    // `enchant_eligible_deck_cards` helper as the previous wave; cost
    // reduction on non-X-cost attacks fires via
    // `apply_static_enchantment`.
    register_after_obtained(
        RelicId::TriBoomerang,
        listener_tri_boomerang as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.4` — Storybook: push one `BrightestFlame`
    // card to `rs.master_deck` on obtain. Same shape as NeowsTorment
    // (a49q3) / PaelsHorn (a49q3) — pure deck-add via `push_deck_card`.
    register_after_obtained(RelicId::Storybook, listener_storybook as AfterObtainedRunFn);
    // Bead `sts2-rs-70anj.4` — BiiigHug: two-leg listener. Run-side
    // `after_obtained` removes 4 cards from master_deck; combat-side
    // `after_shuffle_bus` pushes one Soot card to the draw pile on
    // every shuffle event while owned.
    register_after_obtained(
        RelicId::BiiigHug,
        listener_biiig_hug_remove as AfterObtainedRunFn,
    );
    register_owned_after_shuffle_bus(
        RelicId::BiiigHug,
        RelicKind::BiiigHug,
        listener_biiig_hug_shuffle as AfterShuffleFn,
    );
    // Bead `sts2-rs-70anj.4` — NinjaScroll: round-1 player side-turn
    // start spawns 3 Shivs into the player's hand. Uses
    // `before_side_turn_start` channel + standard hand-cap overflow
    // semantics via `spawn_card_into_hand`.
    register_before_side_turn_start(
        RelicId::NinjaScroll,
        listener_ninja_scroll as BeforeSideTurnStartFn,
    );
    // Bead `sts2-rs-70anj.4` — ElectricShrymp: enchant 1 Skill card
    // with Imbued(1) on obtain. Same helper as the prior wave.
    register_after_obtained(
        RelicId::ElectricShrymp,
        listener_electric_shrymp as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.4` — RadiantPearl: round-1 player side-turn
    // start spawns 1 Luminesce into hand. Same channel + idiom as
    // NinjaScroll, but 1 card and CardType::Skill (Luminesce).
    register_before_side_turn_start(
        RelicId::RadiantPearl,
        listener_radiant_pearl as BeforeSideTurnStartFn,
    );
    // Bead `sts2-rs-70anj.4` (Wave 4 body-fill, batch 5):
    // ToyBox / PaelsClaw / WingCharm.
    register_after_obtained(RelicId::ToyBox, listener_toy_box as AfterObtainedRunFn);
    register_after_obtained(
        RelicId::PaelsClaw,
        listener_paels_claw as AfterObtainedRunFn,
    );
    register_modify_card_reward_options_late(
        RelicId::WingCharm,
        listener_wing_charm_card_reward as ModifyCardRewardOptionsLateRunFn,
    );
    // Bead `sts2-rs-70anj.4` (Wave 4 body-fill, batch 6):
    // PaelsGrowth — enchant 1 eligible deck card with Clone(4).
    // The rest-site CloneOption leg is deferred (needs rest-site
    // options channel).
    register_after_obtained(
        RelicId::PaelsGrowth,
        listener_paels_growth as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.4` (Wave 4 body-fill, batch 7):
    // Ectoplasm — should_gain_gold returns False, vetoing every
    // positive gold gain. The +1 max_energy leg is already wired
    // via hooks::modify_max_energy (bead `sts2-rs-70anj.2`).
    register_should_gain_gold(
        RelicId::Ectoplasm,
        listener_ectoplasm_should_gain_gold as ShouldGainGoldRunFn,
    );
    // Bead `sts2-rs-70anj.3.2`: Sozu vetoes every potion procurement
    // via the `should_procure_potion` channel. The +1 max_energy leg
    // is already wired via hooks::modify_max_energy.
    register_should_procure_potion(
        RelicId::Sozu,
        listener_sozu_should_procure_potion as ShouldProcurePotionRunFn,
    );
    // Bead `sts2-rs-70anj.3.5`: Driftwood — unconditionally allow
    // card-reward reroll. The reroll UI consumer is downstream of this
    // channel; the primitive is the bool the consumer reads.
    register_allow_card_reward_reroll(
        RelicId::Driftwood,
        listener_driftwood_allow_reroll as AllowCardRewardRerollRunFn,
    );
    // Bead `sts2-rs-70anj.4` (Wave 4 body-fill, batch 8):
    // VelvetChoker — per-turn play cap of `VELVET_CHOKER_MAX_CARDS = 6`.
    // `after_card_played` bumps `relic.counter`; `before_side_turn_start`
    // resets it on the PLAYER side. The play gate is
    // hooks::relic_should_play_blocked (reads relic.counter), called by
    // combat_loop::player_should_play_blocked and legal_actions; the +1
    // max_energy leg is the VelvetChoker arm of hooks::modify_max_energy
    // (GH #742 wired both).
    register_after_card_played(
        RelicId::VelvetChoker,
        listener_velvet_choker_card_played as AfterCardPlayedFn,
    );
    register_before_side_turn_start(
        RelicId::VelvetChoker,
        listener_velvet_choker_turn_reset as BeforeSideTurnStartFn,
    );
    // Bead `sts2-rs-70anj.3.21`: DiamondDiadem — at PLAYER turn-end, if
    // <=2 cards were played that turn, apply +1 DiamondDiadem power to
    // the player. The power (PowerKind::DiamondDiadem) is single-stack,
    // halves powered-attack damage targeting owner, and self-removes at
    // ENEMY turn-end via the after_turn_end arm in hooks.rs.
    register_before_turn_end(
        RelicId::DiamondDiadem,
        listener_diamond_diadem_before_turn_end as BeforeTurnEndFn,
    );
    // Bead `sts2-rs-70anj.3.10`: DollysMirror — on obtain, duplicate
    // one random card from master_deck via RunState::duplicate_card_from_deck.
    register_after_obtained(
        RelicId::DollysMirror,
        listener_dollys_mirror as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.3.15`: ArcaneScroll — on obtain, add one
    // random Rare card to the master deck. Uses the pool helper
    // `cards::spec_index::random_card_from_rarity`. Character filter
    // is `None` at this pin (RunState has no character field yet;
    // documented pin-divergence vs Python's owner.character_id filter).
    register_after_obtained(
        RelicId::ArcaneScroll,
        listener_arcane_scroll as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.3.15` follow-on: DustyTome — on obtain, add
    // 1 random Ancient-rarity upgraded card. Pool subsets the
    // Transcendence cards present in our slices (Break/Quadcast/Suppress;
    // Protector + MeteorShower live in Necrobinder/Regent slices not
    // yet generated).
    register_after_obtained(
        RelicId::DustyTome,
        listener_dusty_tome as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.3.15` follow-on: SereTalon — on obtain, add
    // 2 random Curses to deck. The "+3 Wish cards" leg is deferred
    // (Wish card not ported).
    register_after_obtained(
        RelicId::SereTalon,
        listener_sere_talon as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.3.17`: StoneHumidifier — gain +5 max HP each
    // time a rest-site REST actually heals. Python
    // `shop_event.py::StoneHumidifier.after_rest_site_heal` at pin
    // `01c094d:sts2_env/relics/shop_event.py:3431`.
    register_after_rest_site_heal(
        RelicId::StoneHumidifier,
        listener_stone_humidifier as AfterRestSiteHealRunFn,
    );
    // Bead `sts2-rs-ekf4j`: MeatCleaver — append Cook option to the
    // rest-site option list if not already present. Python
    // `shop_event.py::MeatCleaver.modify_rest_site_options`.
    register_modify_rest_site_options(
        RelicId::MeatCleaver,
        listener_meat_cleaver_rest_site_options as ModifyRestSiteOptionsRunFn,
    );
    // Bead `sts2-rs-70anj.3.4`: MiniatureTent — vote `Some(false)` on
    // the disable-remaining channel so picking one rest-site option
    // doesn't grey out the rest. Python
    // `shop_event.py::MiniatureTent.should_disable_remaining_rest_site_options`.
    register_should_disable_remaining_rest_site_options(
        RelicId::MiniatureTent,
        listener_miniature_tent_disable_remaining as ShouldDisableRemainingRestSiteOptionsRunFn,
    );
    // Bead `sts2-rs-70anj.3.9`: Astrolabe — on its own pickup, replace
    // 3 random non-curse master-deck cards with random Rare upgrades.
    // Python `shop_event.py::Astrolabe.after_obtained` transforms then
    // upgrades 3 cards. The Rust port samples replacements from the
    // Rare bucket (vs Python's same-rarity-as-original) because there
    // is no `CardId -> CardSpec` lookup on the run side yet; documented
    // divergence — matches the ArcaneScroll/DustyTome pattern of
    // `random_card_from_rarity` + fresh `Card::pod_defaults()`.
    register_after_obtained(RelicId::Astrolabe, listener_astrolabe as AfterObtainedRunFn);
    // Bead `sts2-rs-p6dj`: real `after_side_turn_start` listener for
    // `PaelsFlesh::ENERGY = 1` (Python `shop_event.py::PaelsFlesh`).
    // From player round 3 onward, +1 energy each player turn.
    // Combat-side `on_trigger_paels_flesh` stays a `resolved_marker`;
    // this is the side-turn-start listener that fires the energy gain.
    register_after_side_turn_start(
        RelicId::PaelsFlesh,
        listener_paels_flesh as AfterSideTurnStartFn,
    );
    // Bead `sts2-rs-83sa`: real `modify_rewards` listener for BlackStar.
    // Python `shop_event.py::BlackStar.modify_rewards` appends a
    // `RelicReward` when `room.room_type == ELITE`. Same shape as
    // PrayerWheel (4ihz) / WhiteStar (b4tb) / AmethystAubergine (0463)
    // but appends `Reward::Relic` instead of CardReward/Gold. The
    // "random relic" semantics in Python are deferred — current
    // `RelicReward` POD carries a concrete `relic_id` field with no
    // "pick from N" variant; per the WongosMysteryTicket placeholder
    // convention we push the relic's own id as a sentinel.
    register_modify_rewards(
        RelicId::BlackStar,
        listener_black_star as ModifyRewardsRunFn,
    );
    // Bead `sts2-rs-70anj.1` (Wave 1 first batch): real
    // `after_side_turn_start` listener for `Sai::BLOCK = 7` (Python
    // `shop_event.py::Sai`). Every player turn, gain 7 block unpowered.
    // Combat-side `on_trigger_sai` stays a `resolved_marker`.
    register_after_side_turn_start(RelicId::Sai, listener_sai as AfterSideTurnStartFn);
    // Bead `sts2-rs-70anj.1`: real `after_side_turn_start` listener for
    // `VeryHotCocoa::ENERGY = 4` (Python `shop_event.py::VeryHotCocoa`).
    // Round 1 player turn only.
    register_after_side_turn_start(
        RelicId::VeryHotCocoa,
        listener_very_hot_cocoa as AfterSideTurnStartFn,
    );
    // Bead `sts2-rs-70anj.1` (Wave 1 body-fill batch 2): real
    // `after_card_played` listener for `IronClub::CARD_THRESHOLD = 4`
    // (Python `shop_event.py::IronClub`). Every 4 cards played
    // (persistent counter across turns within a combat), draw 1 card.
    // Per-instance counter tracked on `Relic.counter`.
    register_after_card_played(RelicId::IronClub, listener_iron_club as AfterCardPlayedFn);
    // Bead `sts2-rs-70anj.1`: `before_hand_draw` listener for
    // `ToastyMittens::STRENGTH = 1` (C# `ToastyMittens.BeforeHandDraw`). Each
    // player turn BEFORE the hand is drawn: exhaust the first non-Innate draw
    // card (round 1) / top card (later), then gain 1 Strength. Registered on
    // `before_hand_draw` — NOT `after_side_turn_start`, which fired after the
    // opening draw and exhausted the wrong card (see `listener_toasty_mittens`).
    register_owned_before_hand_draw(
        RelicId::ToastyMittens,
        RelicKind::ToastyMittens,
        listener_toasty_mittens as BeforeHandDrawFn,
    );
    // Bead `sts2-rs-70anj.1` (hook corrected 2026-07-17): `before_hand_draw`
    // listener for `JeweledMask` (C# `JeweledMask.BeforeHandDraw`). Turn-1
    // hand draw only, BEFORE the opening draw: pick a random Power card in
    // the draw pile (CombatCardSelection stream), set `FreeThisTurn` on it,
    // and move it to hand. Registered on `before_hand_draw` — NOT
    // `after_side_turn_start`, which fired after the opening draw (see
    // `listener_jeweled_mask` and the ToastyMittens note above).
    register_owned_before_hand_draw(
        RelicId::JeweledMask,
        RelicKind::JeweledMask,
        listener_jeweled_mask as BeforeHandDrawFn,
    );
    // Bead `sts2-rs-h450h` (70anj.5.runic_capacitor): real
    // `after_side_turn_start` listener for `RunicCapacitor` (SHOP,
    // DEFECT pool, `ORB_SLOTS = 3`). Round 1 player-side turn: add 3
    // orb slots via `state.player.orbs.add_slots(3)`. Mirrors the
    // VeryHotCocoa round-1 player-gate pattern.
    register_after_side_turn_start(
        RelicId::RunicCapacitor,
        listener_runic_capacitor as AfterSideTurnStartFn,
    );
    // Bead `sts2-rs-70anj.1`: real `after_card_exhausted` listener for
    // `BurningSticks` (Python `shop_event.py::BurningSticks`). First
    // Skill exhausted per combat: clone it into hand. Per-combat latch
    // via `Relic.counter` (`0` = available, `1` = consumed).
    register_owned_after_card_exhausted(
        RelicId::BurningSticks,
        RelicKind::BurningSticks,
        listener_burning_sticks as AfterCardExhaustedFn,
    );
    // Bead `sts2-rs-70anj.1`: real `modify_damage_additive` listener
    // for `MysticLighter::EXTRA_DAMAGE = 9` (Python
    // `shop_event.py::MysticLighter`). Enchanted Attack cards dealt by
    // the player get +9 damage on `MOVE` damage events.
    register_modify_damage_additive(
        RelicId::MysticLighter,
        listener_mystic_lighter as ModifyDamageAdditiveFn,
    );
    // Bead `sts2-rs-70anj.1` (Wave 1 body-fill batch 3): real
    // `before_combat_start` + `modify_hand_draw` legs for `SneckoEye`
    // (Python `shop_event.py::SneckoEye`). At combat start apply
    // `PowerKind::Confused` (stack 1) to the player; every turn add
    // `SNECKO_EYE_EXTRA_DRAW = 2` to the player's hand-draw count.
    // Mirrors `before_combat_start` (Confused application) and
    // `modify_hand_draw` (+EXTRA_DRAW = 2).
    register_before_combat_start(
        RelicId::SneckoEye,
        listener_snecko_eye_combat_start as BeforeCombatStartFn,
    );
    register_owned_modify_hand_draw(
        RelicId::SneckoEye,
        RelicKind::SneckoEye,
        listener_snecko_eye_hand_draw as ModifyHandDrawFn,
    );
    // Bead `sts2-rs-70anj.3.7`: ChemicalX adds +2 to every X-cost play's
    // effective `x` value. Fires from combat_loop::play_card just before
    // `card.energy_spent` is recorded.
    register_owned_modify_x_value(
        RelicId::ChemicalX,
        RelicKind::ChemicalX,
        listener_chemical_x_modify_x_value as ModifyXValueFn,
    );
    // Bead `sts2-rs-70anj.3.3`: ThrowingAxe — first card each combat is
    // played twice. Per-combat latch lives on `Relic.counter` (0 = unused,
    // 1 = fired). The companion `before_combat_start` listener resets the
    // latch so the next combat re-arms (Python's `before_combat_start` +
    // `after_combat_end` both clear `self._used_this_combat`).
    register_owned_relic_modify_card_play_count(
        RelicId::ThrowingAxe,
        RelicKind::ThrowingAxe,
        listener_throwing_axe_play_count as RelicModifyCardPlayCountFn,
    );
    register_before_combat_start(
        RelicId::ThrowingAxe,
        listener_throwing_axe_reset as BeforeCombatStartFn,
    );
    // Bead `sts2-rs-70anj.3.12`: DelicateFrond — fills empty potion
    // slots with random non-event, character-locked potions at combat
    // start. Mirrors `relics/shop_event.py::DelicateFrond.before_combat_start`
    // (`sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py:1476`).
    register_before_combat_start(
        RelicId::DelicateFrond,
        listener_delicate_frond_combat_start as BeforeCombatStartFn,
    );
    // Bead `sts2-rs-sei89`: GhostSeed — stamp `EtherealThisCombat` on
    // every basic Strike/Defend in any of the player's piles at combat
    // start. Mirrors Python `shop_event.py::GhostSeed.before_combat_
    // start`. The companion `after_card_entered_combat` arm (covers
    // mid-combat additions) is a follow-up — the bead scope is
    // explicitly the combat-start sweep only.
    register_before_combat_start(
        RelicId::GhostSeed,
        listener_ghost_seed_combat_start as BeforeCombatStartFn,
    );
    // Bead `sts2-rs-70anj.3.13.tb`: Toolbox — round-1 only, offer 3
    // colorless cards via AddGeneratedToHand choice. Per-combat latch
    // on Relic.counter resets at combat start. Mirrors Python
    // `shop_event.py::Toolbox.before_hand_draw` + `before_combat_start`.
    register_owned_before_hand_draw(
        RelicId::Toolbox,
        RelicKind::Toolbox,
        listener_toolbox_before_hand_draw as BeforeHandDrawFn,
    );
    register_before_combat_start(
        RelicId::Toolbox,
        listener_toolbox_reset as BeforeCombatStartFn,
    );
    // Bead `sts2-rs-70anj.3.13.cp`: ChoicesParadox — round-1 only,
    // offer 5 retain-tagged character cards via
    // AddGeneratedRetainedToHand choice. Routed through
    // `after_side_turn_start(Side::Player)` (Python's `after_player_turn_start`).
    // Per-combat latch on Relic.counter resets at combat start.
    register_after_side_turn_start(
        RelicId::ChoicesParadox,
        listener_choices_paradox as AfterSideTurnStartFn,
    );
    register_before_combat_start(
        RelicId::ChoicesParadox,
        listener_choices_paradox_reset as BeforeCombatStartFn,
    );
    // Bead `sts2-rs-70anj.1`: real `after_card_played` +
    // `before_side_turn_start` legs for `BrilliantScarf` (Python
    // `shop_event.py::BrilliantScarf`). The Python class is a
    // tracker-only mirror at pin `01c094d`. The counter is C#'s
    // `CardsPlayedThisTurn`: incremented per player card played, reset on
    // player side-turn-start. The "5th card costs 0" effect
    // (`TryModifyEnergyCostInCombatLate`) reads it in sts2-core
    // `hooks::brilliant_scarf_makes_free`. (C# skips auto-plays,
    // `cardPlay.IsAutoPlay`; this listener has no such flag and counts them.)
    register_after_card_played(
        RelicId::BrilliantScarf,
        listener_brilliant_scarf_card_played as AfterCardPlayedFn,
    );
    register_before_side_turn_start(
        RelicId::BrilliantScarf,
        listener_brilliant_scarf_turn_reset as BeforeSideTurnStartFn,
    );
    // `BlessedAntler`. C# v0.107.1 `BlessedAntler.BeforeHandDraw`
    // (`TurnNumber == 1`): add `BLESSED_ANTLER_DAZED_COUNT = 3` Dazed to the
    // owner's draw pile at CardPilePosition.Random BEFORE the turn-1 hand is
    // drawn, so a Dazed can land in the opening hand. Registered on
    // `before_hand_draw`, NOT `before_side_turn_start` (the prior port, citing
    // the Python pin): that channel fires earlier than C#'s, which reorders
    // this relic's three Shuffle-stream reads against the other
    // `before_hand_draw` generators (FuneraryMask's Souls). Same move as
    // FuneraryMask / ToastyMittens (`sts2-rs-70anj.1`). The companion
    // `modify_max_energy: +1` leg is deferred — no `modify_max_energy`
    // register channel exists yet (the noop_hook in `hooks.rs` would need a
    // RelicKind match-arm). Beads `sts2-rs-70anj.1`, `sts2-rs-lfdaa`.
    register_owned_before_hand_draw(
        RelicId::BlessedAntler,
        RelicKind::BlessedAntler,
        listener_blessed_antler as BeforeHandDrawFn,
    );
    // Bead `sts2-rs-vfzgs`: `PaelsEye` listeners. Python
    // `shop_event.py::PaelsEye` uses two per-instance flags
    // (`_used_this_combat`, `_any_cards_played`) packed into bit 0 and
    // bit 1 of `Relic.counter` respectively. The
    // `should_take_extra_turn` predicate + `_used_this_combat` latch
    // live in the engine surface (`hooks.rs` + `combat_loop.rs`); the
    // content-side legs wired here are:
    //   - `after_card_played` → set bit 1 (Python's `before_card_played`
    //     mutates `self._any_cards_played = True`; we use the
    //     functionally equivalent `after_card_played` because the
    //     relic-side bus exposes it as `&mut state` and there is no
    //     `before_card_played` relic channel at this pin).
    //   - `before_side_turn_start` (PLAYER) → clear bit 1.
    //   - `before_turn_end` (PLAYER) → exhaust the entire hand when
    //     extra-turn conditions hold.
    register_after_card_played(
        RelicId::PaelsEye,
        listener_paels_eye_card_played as AfterCardPlayedFn,
    );
    register_before_side_turn_start(
        RelicId::PaelsEye,
        listener_paels_eye_side_turn_start as BeforeSideTurnStartFn,
    );
    register_before_turn_end(
        RelicId::PaelsEye,
        listener_paels_eye_before_turn_end as BeforeTurnEndFn,
    );
    // Bead `sts2-rs-ad53n`: `MusicBox` listeners. Python
    // `shop_event.py::MusicBox` — first Attack played per player turn,
    // spawn an Ethereal clone into hand. Per-turn latch is bit 0 of
    // `Relic.counter`. The Ethereal-on-clone bit rides on the per-card
    // `VarKey::EtherealThisCombat` slot (honored by the end-of-turn
    // ethereal sweep in `combat_loop::end_player_turn_phase_two`).
    register_after_card_played(
        RelicId::MusicBox,
        listener_music_box_card_played as AfterCardPlayedFn,
    );
    register_before_side_turn_start(
        RelicId::MusicBox,
        listener_music_box_side_turn_start as BeforeSideTurnStartFn,
    );
    // Bead `sts2-rs-zh82z`: `Crossbow.after_side_turn_start` — every
    // player turn, generate one random 0-cost Attack from the player's
    // character pool into hand. Engine surface
    // (`card_effects::generate_free_attack_in_hand`) handles the
    // sampler + free-this-turn flag.
    register_after_side_turn_start(RelicId::Crossbow, listener_crossbow as AfterSideTurnStartFn);
    // Bead `sts2-rs-1e0ae`: `SealOfGold.after_side_turn_start` — every
    // player turn, if gold >= 5, spend 5 and gain 1 energy. Routes
    // through the closure-registry gold accessors
    // (`card_effects::read_player_gold` + `try_spend_player_gold`).
    // No-ops gracefully in core-only test builds where no accessor is
    // installed.
    register_after_side_turn_start(
        RelicId::SealOfGold,
        listener_seal_of_gold as AfterSideTurnStartFn,
    );
    // 2026-07-17 soft-drop census: `Bread.AfterSideTurnStart` — on the
    // FIRST player turn, lose `BREAD_LOSE_ENERGY = 2` energy (C#
    // `MegaCrit.Sts2.Core.Models.Relics/Bread.cs:AfterSideTurnStart`).
    // The +1 max-energy leg (turns 2+) already lives in the inline
    // `modify_max_energy` arm (`hooks.rs`, bead `sts2-rs-70anj.2`);
    // this listener closes the missing turn-1 penalty leg.
    register_after_side_turn_start(RelicId::Bread, listener_bread as AfterSideTurnStartFn);
    // Bead `sts2-rs-70anj.3` (Wave 3 first primitive — starter_relic_swap):
    // `TouchOfOrobas.setup_for_player`-style behavior wired as an
    // `after_obtained` listener. Python pin
    // `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::TouchOfOrobas`
    // maps the 5 starter relics to their upgraded variants:
    //   BURNING_BLOOD → BLACK_BLOOD (Ironclad)
    //   RING_OF_THE_SNAKE → RING_OF_THE_DRAKE (Silent)
    //   CRACKED_CORE → INFUSED_CORE (Defect)
    //   BOUND_PHYLACTERY → PHYLACTERY_UNBOUND (Necrobinder)
    //   DIVINE_RIGHT → DIVINE_DESTINY (Regent)
    // We mutate `rs.relics` in place at obtain time.
    register_after_obtained(
        RelicId::TouchOfOrobas,
        listener_touch_of_orobas as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.3`: `LargeCapsule::after_obtained` offers
    // 2 random relic rewards (mirror of `SmallCapsule`'s 1-reward
    // landing under `sts2-rs-gf5s`). Python pin
    // `shop_event.py::LargeCapsule.RELICS = 2` plus a deferred
    // Strike+Defend deck-add clause (requires shop_event-side
    // `add_card_to_master_deck` plumbing, separate primitive).
    register_after_obtained(
        RelicId::LargeCapsule,
        listener_large_capsule as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.4` (Wave 4 shop_event body-fill, batch 1):
    // JewelryBox + TanxsWhistle both port to single `master_deck.push`
    // calls at obtain time. Python pin
    // `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py`:
    // - JewelryBox.after_obtained: `owner.add_card_to_deck(
    //   CardInstance(CardId.APOTHEOSIS))` — adds 1 unupgraded
    //   Apotheosis (Colorless rare scaling card) to the run deck.
    // - TanxsWhistle.after_obtained: `owner.add_card_to_deck(
    //   CardInstance(CardId.WHISTLE))` — adds 1 unupgraded Whistle
    //   (Colorless common scaling card) to the run deck.
    // Both fall through the existing `RunState.master_deck`
    // Vec<Card> surface via a `Card::pod_defaults()` + `card_id` /
    // `upgraded` override (the same shape `mk_deck_card` uses
    // privately in `runstate.rs` tests). No `add_card_to_master_deck`
    // helper exists yet on RunState; that's deferred to a follow-up
    // refactor bead — these listeners push directly per the
    // CLAUDE.md "don't add features beyond what the task requires"
    // rule. The deck-add hook `on_card_added_to_deck_run` channel
    // would fire here in production but isn't drained from this
    // path at this pin (out of scope for the body-fill bead).
    register_after_obtained(
        RelicId::JewelryBox,
        listener_jewelry_box as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::TanxsWhistle,
        listener_tanxs_whistle as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-r2y4x` (70anj.4.5): two more run-side
    // `after_obtained` listeners.
    //
    // DistinguishedCape: lose 9 max HP via C#-faithful `lose_max_hp`
    // (partial — Apparition card-add deferred pending `add_card_to_deck`).
    // Same shape as LeafyPoultice (`sts2-rs-beda`).
    //
    // SandCastle: upgrade 6 random deck cards via `upgrade_random_cards`
    // (full implementation — the `defer_followup_rewards` reward-offer
    // path follows the LeesWaffle / SignetRing convention of taking
    // the non-defer Python fallback). Wires through the
    // `upgrade_random_cards` primitive landed by `sts2-rs-7krx0`.
    register_after_obtained(
        RelicId::DistinguishedCape,
        listener_distinguished_cape as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::SandCastle,
        listener_sand_castle as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-66l9f` (70anj.4.6): three more reward-sentinel
    // pushers, all following the Orrery / MassiveScroll / LostCoffer
    // convention of pushing empty `Reward::Card` / `Reward::Potion`
    // placeholders deferred to umbrella `sts2-rs-01o` for materialization.
    //
    // GlassEye: 5 Reward::Card sentinels (rarity-forcing deferred).
    // LostCoffer: 1 Reward::Card + 1 Reward::Potion sentinel.
    // ScrollBoxes (v0.107.1): generate 2 card bundles (no gold loss).
    register_after_obtained(RelicId::GlassEye, listener_glass_eye as AfterObtainedRunFn);
    register_after_obtained(
        RelicId::LostCoffer,
        listener_lost_coffer as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::ScrollBoxes,
        listener_scroll_boxes as AfterObtainedRunFn,
    );
    // v0.107.1 Ancient/Neow relic (bead `sts2-rs-utgn4`): 2 cross-character
    // card rewards generated on pickup (Niche-shuffled non-owner pools +
    // base-odds card creation on Rewards). See `listener_kaleidoscope`.
    register_after_obtained(
        RelicId::Kaleidoscope,
        listener_kaleidoscope as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-a49q3` (70anj.4.7): four more after_obtained
    // listeners using the direct-deck-push pattern established by
    // JewelryBox (k3y) / TanxsWhistle (k3y) — confirms add_card_to_deck
    // doesn't need a dedicated RunState primitive when `Card::pod_defaults()`
    // suffices for adding curses / status cards.
    //
    // NeowsTorment: +1 NeowsFury card.
    // PaelsHorn: +2 Relax cards.
    // CursedPearl: +1 Greed curse + 333 gold (single relic, both legs).
    // BloodSoakedRose: +1 Enthralled curse (modify_max_energy combat
    //   leg deferred to that channel).
    register_after_obtained(
        RelicId::NeowsTorment,
        listener_neows_torment as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::PaelsHorn,
        listener_paels_horn as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::CursedPearl,
        listener_cursed_pearl as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::BloodSoakedRose,
        listener_blood_soaked_rose as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-qw9oe` (70anj.4.8): three more after_obtained
    // listeners.
    //
    // LeadPaperweight: 2 Reward::Card sentinels (colorless filter
    //   deferred to 01o roller).
    // SeaGlass: 15 Reward::Card sentinels (character-pool / rarity
    //   filters deferred to 01o).
    // YummyCookie: upgrade_random_cards(4) — same SandCastle (r2y4x)
    //   convention of using random-upgrade for select-N Python paths.
    register_after_obtained(
        RelicId::LeadPaperweight,
        listener_lead_paperweight as AfterObtainedRunFn,
    );
    register_after_obtained(RelicId::SeaGlass, listener_sea_glass as AfterObtainedRunFn);
    register_after_obtained(
        RelicId::YummyCookie,
        listener_yummy_cookie as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-cvbxs` (70anj.4.9): three more after_obtained
    // listeners.
    //
    // Pomander: upgrade_random_cards(1) — SandCastle / YummyCookie
    //   convention.
    // PrecariousShears: remove 2 cards, then lose_hp(16). Remove-2-cards
    //   leg deferred pending remove_cards_from_deck primitive.
    // AlchemicalCoffer: gain_potion_slots(4), then fill the 4 new slots
    //   with 4 random out-of-combat potions (bead `sts2-rs-cvbxs` closed).
    register_after_obtained(RelicId::Pomander, listener_pomander as AfterObtainedRunFn);
    register_after_obtained(
        RelicId::PrecariousShears,
        listener_precarious_shears as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::AlchemicalCoffer,
        listener_alchemical_coffer as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-t762n`: deck-remove listeners enabled by the
    // newly-landed `RunState::remove_cards_from_deck` primitive.
    //
    // PreciseScissors: remove 1 card.
    // EmptyCage: remove 2 cards.
    // PreservedFog: remove 5 cards + add 1 Folly curse.
    // PrecariousShears: already registered above with the partial
    //   (lose_hp) leg; the listener body was upgraded in this bead
    //   to also call remove_cards_from_deck(2).
    register_after_obtained(
        RelicId::PreciseScissors,
        listener_precise_scissors as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::EmptyCage,
        listener_empty_cage as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::PreservedFog,
        listener_preserved_fog as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-rvwas`: ArchaicTooth — starter-card transform via
    // direct master_deck mutation (preserves upgrade + enchantment
    // state automatically since the card POD's other fields are
    // untouched).
    register_after_obtained(
        RelicId::ArchaicTooth,
        listener_archaic_tooth as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-w9vf8`: Claws + NutritiousSoup deck mutators.
    //
    // Claws: transform up to 6 deck cards into Maul via in-place
    //   card_id rewrites (ArchaicTooth pattern; preserves upgrade /
    //   enchantments).
    // NutritiousSoup: set TezcatarasEmber on every basic-Strike card
    //   (idempotent static enchantment).
    register_after_obtained(RelicId::Claws, listener_claws as AfterObtainedRunFn);
    register_after_obtained(
        RelicId::NutritiousSoup,
        listener_nutritious_soup as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-5xg1w`: Glitter — apply Glam(1) to every card-
    // reward option as it's generated. Same channel as FresnelLens.
    register_modify_card_reward_options_late(
        RelicId::Glitter,
        listener_glitter_card_reward as ModifyCardRewardOptionsLateRunFn,
    );
    // Bead `sts2-rs-70anj.4` (Wave 4 body-fill, batch 2):
    // DragonFruit ports to a single `on_gold_gained` listener that
    // tops `rs.max_hp` by 1 per positive gold gain. Python pin
    // `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::DragonFruit`:
    // ```
    // def on_gold_gained(self, owner, amount):
    //     if amount > 0:
    //         owner.gain_max_hp(self.MAX_HP)  # MAX_HP = 1
    // ```
    // The `on_gold_gained` run-side channel landed under bead
    // `sts2-rs-wag75`; this is the second consumer after BowlerHat.
    // ownership-gate via `rs.relics.contains` matches the canonical
    // run-side listener pattern (LeesWaffle, FragrantMushroom, …).
    register_on_gold_gained(
        RelicId::DragonFruit,
        listener_dragon_fruit_on_gold_gained as OnGoldGainedRunFn,
    );
    // Bead `sts2-rs-apy0a`: real shop-entry listener for `LordsParasol`.
    // Python `relics/shop_event.py:2343-2355`:
    //     def after_room_entered(self, owner, room_type):
    //         if not getattr(room_type, "is_merchant", False): return
    //         run_manager = getattr(room_type, "run_manager", None)
    //         if run_manager is not None:
    //             run_manager._auto_purchase_shop_inventory()
    // The Python flow gates on `room_type.is_merchant` and reaches into
    // the run-manager for the inventory; the Rust port flips the
    // direction — the dedicated `after_shop_inventory_generated`
    // channel only fires from inside the shop driver, so the gating is
    // implicit and the inventory is supplied directly. Owner-side gate
    // via `rs.relics.contains` mirrors the canonical pattern
    // (TouchOfOrobas, DragonFruit, …). Combat-side
    // `on_trigger_lords_parasol` stays a `resolved_marker`; this is the
    // run-state mutator.
    register_after_shop_inventory_generated(
        RelicId::LordsParasol,
        listener_lords_parasol as AfterShopInventoryGeneratedRunFn,
    );
    // Bead `sts2-rs-ytiok` (70anj.4.10): NewLeaf — open a 1-card
    // transform prompt on pickup. Prompt-opener branch only; the
    // in-place transform branch is blocked on a character-pool
    // sampler that requires `RunState.character_id` to land (same
    // gap as jya0y's `roll_card_reward_cards`).
    register_after_obtained(RelicId::NewLeaf, listener_new_leaf as AfterObtainedRunFn);
    // Bead `sts2-rs-70anj.4` (PandorasBox sub-pick): real
    // `after_obtained` listener for `PandorasBox` (Python
    // `relics/shop_event.py::PandorasBox.after_obtained` —
    // `owner.transform_specific_cards(owner.basic_strike_defend_cards())`).
    // Unblocked by ab8uo (`transform_specific_cards`), xbkhd
    // (`RunState.character_id`), and the existing
    // `basic_strike_defend_cards` lookup in spec_index.rs. Combat-side
    // `on_trigger_pandoras_box` stays a `resolved_marker`.
    register_after_obtained(
        RelicId::PandorasBox,
        listener_pandoras_box as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-535i7` (70anj.4 handoff): SilverCrucible —
    // multi-channel relic that (a) counts treasure-room entries via
    // `after_room_entered`, (b) vetoes the first treasure room via
    // `should_generate_treasure` (channel elcvh), and (c) pre-upgrades
    // the first 3 card rewards via `modify_card_reward_options_late`
    // (channel qotxh). The two per-relic state fields Python keeps
    // (`_times_used`, `_treasure_rooms_entered`) are packed into a
    // single `rs.relic_counters[SilverCrucible]` i32 slot — low 8 bits
    // for times_used, next 8 bits for treasure_rooms_entered (both
    // bounded < 256 over any real run). Helpers
    // `silver_crucible_{pack,times_used,treasure_rooms}` keep the
    // encoding cohesive. Combat-side `on_trigger_silver_crucible`
    // stays a `resolved_marker`.
    register_after_room_entered(
        RelicId::SilverCrucible,
        listener_silver_crucible_after_room_entered as AfterRoomEnteredRunFn,
    );
    register_should_generate_treasure(
        RelicId::SilverCrucible,
        listener_silver_crucible_should_generate_treasure as ShouldGenerateTreasureRunFn,
    );
    register_modify_card_reward_options_late(
        RelicId::SilverCrucible,
        listener_silver_crucible_modify_card_reward_options_late
            as ModifyCardRewardOptionsLateRunFn,
    );
    // Bead `sts2-rs-eboqq`: FurCoat — two-channel listener.
    // after_obtained latches the obtain act-index into RunState
    // (since modify_generated_map_late has no map access in this
    // dispatch signature). modify_generated_map_late picks
    // `FUR_COAT_COMBATS = 7` random Monster/Elite rooms and marks
    // them via the map-layer registry from mxbai. Combat-side
    // 1-HP clamp ships in a follow-up bead — needs a
    // CombatState↔ActMap snapshot bridge for the current room.
    register_after_obtained(
        RelicId::FurCoat,
        listener_fur_coat_after_obtained as AfterObtainedRunFn,
    );
    register_modify_generated_map_late(
        RelicId::FurCoat,
        listener_fur_coat_modify_generated_map_late as ModifyGeneratedMapLateRunFn,
    );
    // Bead `sts2-rs-k2nuu` (70anj.4 handoff): GoldenCompass — three
    // listeners. `after_obtained` captures `rs.act_index` into
    // `rs.relic_counters[GoldenCompass]`; `modify_generated_map`
    // overwrites the rolled act_map with `generate_golden_path_map(1)`
    // when the captured act matches; `modify_unknown_map_point_room_types`
    // narrows the `?`-node candidate set to {Event}. Python pin
    // `relics/shop_event.py::GoldenCompass` also calls
    // `run_state.generate_map()` from `after_obtained` to force
    // immediate regen; the Rust port lacks a `RunState::generate_map()`
    // analogue (RunState doesn't own an `ActMap` field at this Rust
    // pin), so the swap only activates on the next driver-side
    // regeneration. Documented divergence; combat-side
    // `on_trigger_golden_compass` stays a `resolved_marker`.
    register_after_obtained(
        RelicId::GoldenCompass,
        listener_golden_compass_after_obtained as AfterObtainedRunFn,
    );
    register_modify_generated_map(
        RelicId::GoldenCompass,
        listener_golden_compass_modify_generated_map as ModifyGeneratedMapRunFn,
    );
    register_modify_unknown_map_point_room_types(
        RelicId::GoldenCompass,
        listener_golden_compass_modify_unknown_map_point_room_types
            as ModifyUnknownMapPointRoomTypesRunFn,
    );
    // BeltBuckle — "while you have no potions, gain +BELT_BUCKLE_DEXTERITY
    // Dexterity." C# `BeltBuckle.cs` latches `_dexterityApplied` and
    // applies/removes `DexterityPower(2)` as the player's potion count
    // crosses 0. The combat-side achievable subset at this pin:
    //   * `before_combat_start` resets the latch and applies +2 Dex when
    //     the player starts combat with no potions;
    //   * `after_potion_used` applies +2 Dex when using the last potion
    //     leaves the player potionless (latch-guarded).
    // The `after_potion_procured` / `after_potion_discarded` remove-path
    // (-2 Dex when a potion is gained mid-combat) has no typed channel at
    // this pin and is deferred — see the listener doc-comment. The latch
    // lives in `relic.counter` (0 = not applied, 1 = applied). Bead
    // `sts2-rs-xu8d6`.
    register_before_combat_start(
        RelicId::BeltBuckle,
        listener_belt_buckle_before_combat_start as BeforeCombatStartFn,
    );
    register_owned_after_potion_used(
        RelicId::BeltBuckle,
        RelicKind::BeltBuckle,
        listener_belt_buckle_after_potion_used as AfterPotionUsedFn,
    );
    // PaelsLegion (sts2-rs-ppabi) — boss/pet relic: "the first time you
    // gain block from a card each [cooldown] turns, double it." C#
    // `PaelsLegion.cs` summons a (behaviourless) pet and runs a
    // block-doubling cooldown machine:
    //   * BeforeCombatStart    → summon pet + reset cooldown (armed);
    //   * ModifyBlockMultiplicative → 2× owner block from a card while
    //     off cooldown;
    //   * AfterModifyingBlockAmount + AfterCardPlayed → start the
    //     `Turns`-turn cooldown once the triggering card-play resolves;
    //   * AfterSideTurnStart    → decrement the cooldown each owner turn.
    // The cooldown lives in `relic.counter` with the sentinel scheme
    //   0  = armed (ready to double),
    //   -1 = doubled this card-play, cooldown pending its end,
    //   >0 = turns of cooldown remaining.
    // The pet is cosmetic (animation triggers only); the block-doubling is
    // the mechanic. See the listener doc-comments.
    register_before_combat_start(
        RelicId::PaelsLegion,
        listener_paels_legion_before_combat_start as BeforeCombatStartFn,
    );
    register_modify_block_multiplicative(
        RelicId::PaelsLegion,
        listener_paels_legion_modify_block as ModifyBlockMultiplicativeFn,
    );
    register_after_block_gained(
        RelicId::PaelsLegion,
        listener_paels_legion_after_block_gained as AfterBlockGainedFn,
    );
    register_after_card_played(
        RelicId::PaelsLegion,
        listener_paels_legion_after_card_played as AfterCardPlayedFn,
    );
    register_after_side_turn_start(
        RelicId::PaelsLegion,
        listener_paels_legion_after_side_turn_start as AfterSideTurnStartFn,
    );
}

/// `PaelsLegion::before_combat_start` — summon the behaviourless
/// PaelsLegion pet and reset the cooldown to armed (`relic.counter = 0`).
/// Mirrors C# `PaelsLegion.cs::BeforeCombatStart` (`SummonPet`) plus the
/// `AfterCombatEnd` reset (`Cooldown = 0`). Bead `sts2-rs-ppabi`.
fn listener_paels_legion_before_combat_start(state: &mut CombatState) -> EffectResult {
    let Some(idx) = state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::PaelsLegion)
    else {
        return EffectResult::NotImplemented;
    };
    state.relics[idx].counter = 0;
    let _ = state.summon_pet(sts2_core::MonsterKind::PaelsLegion, PAELS_LEGION_PET_HP);
    EffectResult::Resolved
}

/// `PaelsLegion::modify_block_multiplicative` — double (`2×`) block the
/// player gains from a card while off cooldown. Mirrors C#
/// `PaelsLegion.cs::ModifyBlockMultiplicative`: gates on
/// `props.IsCardOrMonsterMove()` + `cardSource != null` (here
/// `ctx.card_source.is_some()`), `target == Owner` (the player), and
/// `Cooldown <= 0` (sentinel `counter <= 0`, covering both armed `0` and
/// the pending `-1`). Returns identity otherwise. Bead `sts2-rs-ppabi`.
fn listener_paels_legion_modify_block(state: &CombatState, ctx: &BlockCtx) -> hooks::DamageMul {
    if ctx.card_source.is_none() {
        return hooks::DamageMul::ONE;
    }
    if !matches!(ctx.target, CreatureRef::Player) {
        return hooks::DamageMul::ONE;
    }
    let on_cooldown = state
        .relics
        .iter()
        .find(|r| !r.is_melted && r.kind == RelicKind::PaelsLegion)
        .is_some_and(|r| r.counter > 0);
    let owned = state.relics.owns(RelicKind::PaelsLegion);
    if owned && !on_cooldown {
        hooks::DamageMul::new(2, 1)
    } else {
        hooks::DamageMul::ONE
    }
}

/// `PaelsLegion::after_block_gained` — once a doubled card-block lands
/// (armed, card-sourced, owner-targeted), mark the cooldown pending with
/// the `-1` sentinel. Subsequent block gains in the SAME card-play stay
/// `<= 0`, so they are still doubled by
/// [`listener_paels_legion_modify_block`] — matching C#'s
/// `AfterModifyingBlockAmount` latching the whole `CardPlay` before
/// `AfterCardPlayed` starts the cooldown. Bead `sts2-rs-ppabi`.
fn listener_paels_legion_after_block_gained(
    state: &mut CombatState,
    ctx: &BlockGainedCtx,
) -> EffectResult {
    if ctx.card_source.is_none() || !matches!(ctx.target, CreatureRef::Player) {
        return EffectResult::NotImplemented;
    }
    let Some(idx) = state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::PaelsLegion)
    else {
        return EffectResult::NotImplemented;
    };
    if state.relics[idx].counter <= 0 {
        state.relics[idx].counter = -1;
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `PaelsLegion::after_card_played` — when the card-play that triggered
/// the doubling resolves (`counter == -1` pending), start the
/// `PAELS_LEGION_TURNS`-turn cooldown. Mirrors C#
/// `PaelsLegion.cs::AfterCardPlayed` (`Cooldown = DynamicVars["Turns"]`).
/// Bead `sts2-rs-ppabi`.
fn listener_paels_legion_after_card_played(
    state: &mut CombatState,
    _ctx: &CardEventCtx,
) -> EffectResult {
    let Some(idx) = state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::PaelsLegion)
    else {
        return EffectResult::NotImplemented;
    };
    if state.relics[idx].counter == -1 {
        state.relics[idx].counter = PAELS_LEGION_TURNS;
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `PaelsLegion::after_side_turn_start` — decrement the cooldown each
/// owner (player) turn start; at `0` the relic is armed again. Mirrors
/// C# `PaelsLegion.cs::AfterSideTurnStart` (`if side == Owner.Side:
/// Cooldown--`). A pending `-1` is left untouched (the cooldown has not
/// started yet). Bead `sts2-rs-ppabi`.
fn listener_paels_legion_after_side_turn_start(
    state: &mut CombatState,
    side: Side,
) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    let Some(idx) = state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::PaelsLegion)
    else {
        return EffectResult::NotImplemented;
    };
    if state.relics[idx].counter > 0 {
        state.relics[idx].counter -= 1;
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// Shared BeltBuckle apply helper: when the player owns BeltBuckle, holds
/// no potions, and the latch (`relic.counter`) is unset, fold
/// +`BELT_BUCKLE_DEXTERITY` Dexterity into the player and set the latch.
/// Mirrors C# `BeltBuckle.ApplyDexterity` (`if (!DexterityApplied) {
/// DexterityApplied = true; PowerCmd.Apply<DexterityPower>(+2); }`).
/// Returns `true` iff Dexterity was applied this call.
fn belt_buckle_apply_dexterity(state: &mut CombatState) -> bool {
    if !state.player.potions.is_empty() {
        return false;
    }
    let Some(idx) = state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::BeltBuckle)
    else {
        return false;
    };
    if state.relics[idx].counter != 0 {
        return false;
    }
    state.relics[idx].counter = 1;
    if let Some(uid) = state.player.creature_uid {
        apply_power_to_creature(state, uid, PowerKind::Dexterity, BELT_BUCKLE_DEXTERITY);
    }
    true
}

/// `BeltBuckle::before_combat_start` — reset the `_dexterityApplied`
/// latch (`relic.counter` → 0) and, if the player holds no potions,
/// apply +`BELT_BUCKLE_DEXTERITY` Dexterity. Mirrors C#
/// `BeltBuckle.BeforeCombatStart` (`DexterityApplied = false; if
/// (!Owner.Potions.Any()) ApplyDexterity();`). Bead `sts2-rs-xu8d6`.
fn listener_belt_buckle_before_combat_start(state: &mut CombatState) -> EffectResult {
    let Some(idx) = state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::BeltBuckle)
    else {
        return EffectResult::NotImplemented;
    };
    state.relics[idx].counter = 0;
    if belt_buckle_apply_dexterity(state) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `BeltBuckle::after_potion_used` — when using a potion leaves the
/// player with no potions, apply +`BELT_BUCKLE_DEXTERITY` Dexterity
/// (latch-guarded). Mirrors C# `BeltBuckle.AfterPotionUsed` (`if
/// (IsInProgress && !Owner.Potions.Any()) ApplyDexterity();`).
///
/// **Deferred (no channel at this pin):** C# also runs the
/// `RemoveDexterity` (-2 Dex) path from `AfterPotionProcured` /
/// `AfterPotionDiscarded` when the player *gains* a potion mid-combat
/// (potion count crosses 0→1). There is no `after_potion_procured` /
/// `after_potion_discarded` typed channel yet, so the only divergence is
/// a player who procures a potion mid-combat without using it (rare):
/// the +2 Dex is not retracted. Net is unaffected for the common
/// start-empty / use-last-potion flows. File a follow-up bead if a
/// procurement channel lands.
fn listener_belt_buckle_after_potion_used(
    state: &mut CombatState,
    _potion: sts2_core::potions::PotionKind,
    _target: Option<u8>,
) -> EffectResult {
    if belt_buckle_apply_dexterity(state) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `TouchOfOrobas::after_obtained` — starter-relic swap (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::TouchOfOrobas`).
/// At obtain time, iterate `rs.relics` and replace the first
/// occurrence of any starter-relic id with its character-specific
/// upgraded variant:
///   `BurningBlood` → `BlackBlood` (Ironclad)
///   `RingOfTheSnake` → `RingOfTheDrake` (Silent)
///   `CrackedCore` → `InfusedCore` (Defect)
///   `BoundPhylactery` → `PhylacteryUnbound` (Necrobinder)
///   `DivineRight` → `DivineDestiny` (Regent)
/// Idempotent: subsequent calls (shouldn't happen — `after_obtained`
/// fires once) leave already-upgraded starter relics alone since the
/// mapping keys on the un-upgraded variant only. Bead `sts2-rs-70anj.3`.
fn listener_touch_of_orobas(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::TouchOfOrobas) {
        return EffectResult::NotImplemented;
    }
    let mut upgraded = false;
    for slot in &mut rs.relics {
        let swap = match *slot {
            RelicId::BurningBlood => Some(RelicId::BlackBlood),
            RelicId::RingOfTheSnake => Some(RelicId::RingOfTheDrake),
            RelicId::CrackedCore => Some(RelicId::InfusedCore),
            RelicId::BoundPhylactery => Some(RelicId::PhylacteryUnbound),
            RelicId::DivineRight => Some(RelicId::DivineDestiny),
            _ => None,
        };
        if let Some(new_id) = swap {
            *slot = new_id;
            upgraded = true;
            break;
        }
    }
    if upgraded {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `LargeCapsule::after_obtained` — offers 2 random relic rewards
/// (Python pin `shop_event.py::LargeCapsule.RELICS = 2`, plus a
/// Strike+Defend deck-add clause deferred — that requires the
/// shop_event-side `add_card_to_master_deck` plumbing). Mirror of
/// [`listener_small_capsule`] using `RelicId::LargeCapsule` as the
/// random-sampling TBD sentinel. Bead `sts2-rs-70anj.3`.
fn listener_large_capsule(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::LargeCapsule) {
        return EffectResult::NotImplemented;
    }
    // C# `LargeCapsule.AfterObtained` (LargeCapsule.cs:23-35): OBTAIN 2 relics
    // directly (`await RelicCmd.Obtain` — no pick), THEN add the character's
    // basic Strike and Defend to the deck (Strike first). The obtain-BEFORE-add
    // order is load-bearing: an egg relic just obtained here (e.g. MoltenEgg)
    // upgrades the Strike as it is added via the `modify_card_being_added_to_deck`
    // hook. Offering the relics as pickable rewards (the old shape) left them
    // unowned at add-time, so MoltenEgg never saw the Strike (RP1 PARITY36:
    // C# adds an upgraded Strike, Rust an unupgraded one). The grab-bag pop
    // order is unchanged (same `roll_reward_relic_rarity` + `pop_relic_from_grab_bag`).
    for _ in 0..LARGE_CAPSULE_RELICS {
        let rarity = rs.roll_reward_relic_rarity();
        if let Some(id) = rs.pop_relic_from_grab_bag(rarity) {
            rs.obtain_relic(id);
        }
    }
    // Restore the pickup marker the inner obtains overwrote, so the rest of
    // this after_obtained walk (and the NeowsBones grant chain, when LargeCapsule
    // was granted by it) still sees LargeCapsule as the owner.
    rs.last_obtained_relic = Some(RelicId::LargeCapsule);
    let character_id = rs.character_id;
    if let Some([strike_id, defend_id]) =
        crate::cards::spec_index::basic_strike_defend_cards(character_id)
    {
        for id in [strike_id, defend_id] {
            if let Some(mut card) = crate::cards::spec_index::card_pod_from_id(id, false) {
                // C# `CardPileCmd.Add` runs every relic's
                // `TryModifyCardBeingAddedToDeck` (MoltenEgg upgrades Attacks,
                // FrozenEgg Powers, ToxicEgg Skills).
                let _ =
                    sts2_core::run_effects::dispatch_modify_card_being_added_to_deck(rs, &mut card);
                // C# `CardPileCmd.Add(.., PileType.Deck)` fires
                // `AbstractRelic.AfterCardChangedPiles` for every relic, which
                // is where `LuckyFysh.cs` grants its `GoldVar(15)` per card that
                // lands in the deck (LuckyFysh.cs:AfterCardChangedPiles). Capture
                // the card identity/type BEFORE the moving push, then dispatch the
                // run-side `on_card_added_to_deck` channel so LuckyFysh (and any
                // other deck-add listener) fires for each of the Strike and Defend
                // — without this the two adds were raw-pushed silently and
                // LuckyFysh never granted its 30 gold (parity map_gold_minus30).
                let ctx = sts2_core::run_effects::CardAddedRunCtx {
                    card_id: card.card_id,
                    card_type: card.card_type,
                    source: sts2_core::relic_effects::AddSource::RelicGain,
                };
                rs.master_deck.push(card);
                let _ = sts2_core::run_effects::dispatch_on_card_added_to_deck(rs, &ctx);
            }
        }
    }
    EffectResult::Resolved
}

/// `LargeCapsule.RELICS = 2` (Python pin `shop_event.py:LargeCapsule.RELICS`).
pub const LARGE_CAPSULE_RELICS: usize = 2;

/// `JewelryBox::after_obtained` — Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::JewelryBox`:
/// adds one unupgraded `Apotheosis` (Colorless rare scaling card,
/// `CardId::Apotheosis`) to the run-side master deck. Bead
/// `sts2-rs-70anj.4` (Wave 4 body-fill, batch 1). Helper-free direct
/// push to `rs.master_deck` — same shape as the `mk_deck_card`
/// pattern used in `runstate.rs` tests, kept inline per CLAUDE.md
/// "don't add features beyond what the task requires" until a
/// second consumer justifies hoisting a `RunState::add_card_to_deck`
/// helper.
fn listener_jewelry_box(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::JewelryBox) {
        return EffectResult::NotImplemented;
    }
    let mut card = sts2_core::cards::Card::pod_defaults();
    card.card_id = sts2_core::cards::CardId::Apotheosis;
    card.upgraded = false;
    rs.master_deck.push(card);
    EffectResult::Resolved
}

/// `TanxsWhistle::after_obtained` — Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::TanxsWhistle`:
/// adds one unupgraded `Whistle` (Colorless common scaling card,
/// `CardId::Whistle`) to the run-side master deck. Bead
/// `sts2-rs-70anj.4` (Wave 4 body-fill, batch 1). Same direct-push
/// shape as [`listener_jewelry_box`].
fn listener_tanxs_whistle(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::TanxsWhistle) {
        return EffectResult::NotImplemented;
    }
    let mut card = sts2_core::cards::Card::pod_defaults();
    card.card_id = sts2_core::cards::CardId::Whistle;
    card.upgraded = false;
    rs.master_deck.push(card);
    EffectResult::Resolved
}

/// `DragonFruit::on_gold_gained` — Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::DragonFruit`:
/// `if amount > 0: owner.gain_max_hp(self.MAX_HP)` with `MAX_HP = 1`.
/// Fires on every positive gold gain at run-state level. Bead
/// `sts2-rs-70anj.4` (Wave 4 body-fill, batch 2). Ownership-gated to
/// avoid mutating the run for non-owners (sibling channel listeners
/// dispatch on all registered relics, not just owned ones).
fn listener_dragon_fruit_on_gold_gained(rs: &mut RunState, amount: i32) -> EffectResult {
    if !rs.owns_active_relic(RelicId::DragonFruit) {
        return EffectResult::NotImplemented;
    }
    if amount <= 0 {
        return EffectResult::NotImplemented;
    }
    rs.gain_max_hp(DRAGON_FRUIT_MAX_HP);
    EffectResult::Resolved
}

/// `LordsParasol::after_shop_inventory_generated` — Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::LordsParasol`:
/// at merchant, auto-purchase every shop entry for free by routing
/// through the shared [`auto_purchase_shop_inventory`] primitive,
/// which:
///
/// - clones every available character + colorless card slot onto
///   `rs.master_deck` and flips its price to `SHOP_ENTRY_SOLD_OUT_PRICE`,
/// - pushes every available relic slot onto `rs.relics` and fires the
///   `after_obtained` channel,
/// - routes every available potion slot through `rs.procure_potion`
///   so the `should_procure_potion` veto channel still applies.
///
/// Owner-gated via `rs.relics.contains` (canonical run-side listener
/// pattern — sibling listeners DragonFruit / TouchOfOrobas / LeesWaffle
/// gate the same way). The interactive free-removal branch is
/// intentionally NOT plumbed here — Python guards it on
/// `_should_allow_merchant_card_removal` + an interactive
/// `RemoveCardReward` choice, both of which live in the run-loop
/// driver layer. The dedicated channel-level `auto_purchase_*` helper
/// already mirrors that omission. Bead `sts2-rs-apy0a` (Channel:
/// shop auto-purchase).
fn listener_lords_parasol(rs: &mut RunState, inv: &mut ShopInventory) -> EffectResult {
    if !rs.owns_active_relic(RelicId::LordsParasol) {
        return EffectResult::NotImplemented;
    }
    auto_purchase_shop_inventory(rs, inv);
    EffectResult::Resolved
}

/// `Sai::after_side_turn_start` — `if side == PLAYER:
/// owner.gain_block(7, unpowered=True)` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::Sai`,
/// `BLOCK = 7`). Fires every player turn for the entire combat.
/// Bead `sts2-rs-70anj.1`.
fn listener_sai(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::Sai) {
        return EffectResult::NotImplemented;
    }
    // C# `Sai.cs` declares `BlockVar(7m, ValueProp.Unpowered)` —
    // bypass Frail/Dexterity (bead `sts2-rs-dko39`).
    gain_block_player_unpowered(state, SAI_BLOCK);
    EffectResult::Resolved
}

/// Bead `sts2-rs-dko39`: file-local block grant that bypasses
/// Frail/Dexterity — mirrors C# `BlockVar(N, ValueProp.Unpowered)`.
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

/// `VeryHotCocoa::after_side_turn_start` — `if side == PLAYER and
/// combat.round_number == 1: combat.gain_energy(owner, 4)` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::VeryHotCocoa`,
/// `ENERGY = 4`). Round 1 only. Bead `sts2-rs-70anj.1`.
fn listener_very_hot_cocoa(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if state.round_no() != 1 {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::VeryHotCocoa) {
        return EffectResult::NotImplemented;
    }
    state.gain_energy(VERY_HOT_COCOA_ENERGY);
    EffectResult::Resolved
}

/// `PaelsFlesh::after_side_turn_start` — `if side == PLAYER and
/// combat.round_number >= 3: combat.gain_energy(owner, 1)` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::PaelsFlesh`,
/// `ENERGY = 1`). Single-channel, no per-instance latch (the
/// `round_no` gate alone is sufficient — fires every player turn
/// from round 3 to combat end). Bead `sts2-rs-p6dj`.
fn listener_paels_flesh(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::PaelsFlesh) {
        return EffectResult::NotImplemented;
    }
    if state.round_no() < PAELS_FLESH_ROUND_MIN {
        return EffectResult::NotImplemented;
    }
    state.gain_energy(PAELS_FLESH_ENERGY);
    EffectResult::Resolved
}

/// `IronClub::after_card_played` — increments a per-instance counter
/// on every player card play (persistent across turns within a combat)
/// and triggers `combat.draw_cards(owner, 1)` every
/// `IRON_CLUB_CARD_THRESHOLD = 4` plays (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::IronClub`).
/// Counter lives on `Relic.counter`. Bead `sts2-rs-70anj.1`.
fn listener_iron_club(state: &mut CombatState, _ctx: &CardEventCtx) -> EffectResult {
    let mut should_draw = false;
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::IronClub {
            relic.counter = relic.counter.saturating_add(1);
            if relic.counter % IRON_CLUB_CARD_THRESHOLD == 0 {
                should_draw = true;
            }
            break;
        }
    }
    if should_draw {
        let _ = state.draw_into_hand(1);
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `ToastyMittens::before_side_turn_start` — `if side == PLAYER:
/// combat.exhaust_top_of_draw_pile(owner); owner.apply_power(STRENGTH,
/// 1)` (Python `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::
/// ToastyMittens`, `STRENGTH = 1`). Fires every player turn — exhausts
/// the top of the draw pile (one card; Python's
/// `exhaust_top_of_draw_pile` pops `draw[0]`, matching Rust's
/// `exhaust_from_draw_pile(1)`) and applies +1 Strength to the player.
/// Bead `sts2-rs-70anj.1`.
fn listener_toasty_mittens(state: &mut CombatState) -> EffectResult {
    if !state.relics.owns(RelicKind::ToastyMittens) {
        return EffectResult::NotImplemented;
    }
    // C# `ToastyMittens.BeforeHandDraw` (`ToastyMittens.cs`): every player
    // turn, BEFORE the hand is drawn — `ShuffleIfNecessary`, then exhaust the
    // first NON-Innate draw card on round 1 (else the top card), then gain +1
    // Strength. This MUST fire on `before_hand_draw`, not `after_side_turn_start`
    // (CombatManager.cs:539 fires `Hook.BeforeHandDraw` before the :560
    // `CardPileCmd.Draw`). The old `after_side_turn_start` registration fired
    // AFTER the opening draw, so the post-shuffle top card had already moved to
    // hand → the relic exhausted the wrong card and the intended top card stayed
    // in hand (act-2 IC:1004: C# exhausts BLUDGEON pre-draw; Rust kept it in the
    // opening hand and exhausted the next card down). ToastyMittens is an Ancient
    // relic gained entering act 2, so the desync first surfaced at the first
    // act-2 combat. Bead `sts2-rs-70anj.1`.
    let skip_innate = state.round_no() == 1;
    state.exhaust_top_from_draw_skipping_innate(skip_innate);
    if let Some(player_uid) = state.player.creature_uid {
        apply_power_to_creature(
            state,
            player_uid,
            PowerKind::Strength,
            TOASTY_MITTENS_STRENGTH,
        );
    }
    EffectResult::Resolved
}

/// `JeweledMask::before_hand_draw` — C# v0.107.1 `JeweledMask.cs`
/// `BeforeHandDraw`: `if (TurnNumber <= 1) { list = Draw.Cards.Where(c =>
/// c.Type == Power); if non-empty: card = Rng.CombatCardSelection
/// .NextItem(list); card.SetToFreeThisTurn(); CardPileCmd.Add(card, Hand) }`.
/// Rust port uses `VarKey::FreeThisTurn = 1` as the "free for this
/// turn" mechanism (same pattern as `InfernalBlade` in
/// `cards/ironclad.rs`). Bead `sts2-rs-70anj.1`.
///
/// Timing (2026-07-17 fix, same shape as the ToastyMittens/FuneraryMask
/// corrections above): the C# hook is `BeforeHandDraw`, which fires
/// BEFORE the opening 5-card draw (`CombatManager.cs:539` vs the `:560`
/// `CardPileCmd.Draw`). The prior port registered on
/// `after_side_turn_start` (post-draw), so a Power that happened to be
/// drawn naturally was excluded from the selection pool — and when the
/// deck's ONLY Power got drawn, the relic silently did nothing.
///
/// RNG (same fix): C# reads `Rng.CombatCardSelection.NextItem(list)` =
/// exactly one `NextInt(0, count)` tick on the CombatCardSelection
/// stream (`Rng.cs:256/264`); the old port read the generic
/// `state.rng`, desyncing the `combat_card_selection` counter.
fn listener_jeweled_mask(state: &mut CombatState) -> EffectResult {
    if state.round != 1 {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::JeweledMask) {
        return EffectResult::NotImplemented;
    }
    // Pool = Power cards in the (pre-draw) draw pile, in pile order —
    // mirrors the C# `Draw.Cards.Where(...)` enumeration order.
    let pool: smallvec::SmallVec<[sts2_core::CardUid; 8]> = state
        .player
        .draw
        .iter()
        .copied()
        .filter(|uid| {
            let idx = uid.0.get() as usize - 1;
            state
                .cards
                .get(idx)
                .is_some_and(|c| c.card_type == CardType::Power)
        })
        .collect();
    if pool.is_empty() {
        // C# guards `list.Count != 0` BEFORE the Rng read — an empty
        // pool must not consume a CombatCardSelection tick.
        return EffectResult::NotImplemented;
    }
    let idx = state
        .run_rng
        .combat_card_selection
        .next_int_exclusive(0, pool.len() as i32) as usize;
    let uid = pool[idx];
    set_card_var(state, uid, VarKey::FreeThisTurn, 1);
    state.move_card_to_hand(uid);
    EffectResult::Resolved
}

/// `BurningSticks::after_card_exhausted` — `if not used_this_combat and
/// card.card_type == SKILL: used_this_combat = True;
/// combat.clone_card_to_hand(owner, card)` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::BurningSticks`).
/// Per-combat latch is stored on `Relic.counter` (`0` = available,
/// `1` = already used this combat — Python resets via
/// `before_combat_start`; the Rust counter is zeroed at the same
/// point by the combat-state reset path). Clone is built by cloning
/// the exhausted card's POD from the arena and routing it through
/// `spawn_card_into_hand`. Bead `sts2-rs-70anj.1`.
fn listener_burning_sticks(state: &mut CombatState, ctx: &CardEventCtx) -> EffectResult {
    let Some(card_uid) = ctx.card_uid else {
        return EffectResult::NotImplemented;
    };
    let idx = card_uid.0.get() as usize - 1;
    let Some(card) = state.cards.get(idx) else {
        return EffectResult::NotImplemented;
    };
    if card.card_type != CardType::Skill {
        return EffectResult::NotImplemented;
    }
    let mut consumed = false;
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::BurningSticks && relic.counter == 0 {
            relic.counter = 1;
            consumed = true;
            break;
        }
    }
    if !consumed {
        return EffectResult::NotImplemented;
    }
    // Re-read the exhausted card POD (Relic mutation above may have
    // invalidated any prior borrow) and clone it into hand.
    let cloned = state.cards[idx];
    spawn_card_into_hand(state, cloned);
    EffectResult::Resolved
}

/// `MysticLighter::modify_damage_additive` — `if dealer is owner and
/// card.card_type == ATTACK and card.is_enchanted and props & MOVE:
/// return EXTRA_DAMAGE = 9` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::MysticLighter`).
/// "Enchanted" means `!card.enchantments.is_empty()` on the Rust
/// `Card` POD (mirrors Python `CardInstance.enchantments: dict[str,
/// int]` non-empty). Bead `sts2-rs-70anj.1`.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn listener_mystic_lighter(state: &CombatState, ctx: &ModifyDamageCtx) -> i32 {
    if !state.relics.owns(RelicKind::MysticLighter) {
        return 0;
    }
    let Some(player_uid) = state.player.creature_uid else {
        return 0;
    };
    if ctx.dealer != Some(player_uid) {
        return 0;
    }
    if !ctx.props.contains(ValueProps::MOVE) {
        return 0;
    }
    let Some(card_id) = ctx.card else {
        return 0;
    };
    let is_enchanted_attack = state
        .cards
        .iter()
        .find(|c| c.card_id == card_id)
        .is_some_and(|c| c.card_type == CardType::Attack && !c.enchantments.is_empty());
    if is_enchanted_attack {
        MYSTIC_LIGHTER_EXTRA_DAMAGE
    } else {
        0
    }
}

/// `SneckoEye::before_combat_start` — `owner.apply_power(CONFUSED, 1)`
/// (Python `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::
/// SneckoEye`). Applies a single stack of `PowerKind::Confused` to
/// the player at combat start. Bead `sts2-rs-70anj.1`.
fn listener_snecko_eye_combat_start(state: &mut CombatState) -> EffectResult {
    if !state.relics.owns(RelicKind::SneckoEye) {
        return EffectResult::NotImplemented;
    }
    let Some(player_uid) = state.player.creature_uid else {
        return EffectResult::NotImplemented;
    };
    apply_power_to_creature(state, player_uid, PowerKind::Confused, 1);
    EffectResult::Resolved
}

/// `GhostSeed::before_combat_start` — stamp
/// [`VarKey::EtherealThisCombat`] = 1 on every basic Strike / Defend
/// in any of the player's piles (`draw`, `hand`, `discard`,
/// `exhaust`). Mirrors Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::GhostSeed.
/// before_combat_start` — Python iterates `state.all_piles` and gates
/// on `CardRarity.BASIC` + `"STRIKE" in card_id.name or "DEFEND" in
/// card_id.name` + `not card.is_ethereal`. Rust port matches on the
/// concrete basic Strike / Defend [`CardId`] variants (one Strike +
/// one Defend per character), which is the closed set of cards
/// satisfying Python's rarity-and-name filter. The companion
/// `after_card_entered_combat` arm (mid-combat additions) is out of
/// scope per the bead body — file a follow-up if the residual
/// becomes load-bearing for parity. The existing end-of-turn
/// ethereal sweep in [`crate::sts2_core::combat_loop`] honors the
/// flag (bead `sts2-rs-ad53n`). Bead `sts2-rs-sei89`.
fn listener_ghost_seed_combat_start(state: &mut CombatState) -> EffectResult {
    use sts2_core::cards::CardId;
    if !state.relics.owns(RelicKind::GhostSeed) {
        return EffectResult::NotImplemented;
    }
    // Closed set: every Python `CardRarity.BASIC` card whose name
    // contains "STRIKE" or "DEFEND" at pin `01c094d`. Derived cards
    // (`SetupStrikeCard`, `FocusedStrikeCard`) are non-basic and must
    // NOT match — Python's rarity gate excludes them, and this
    // explicit allowlist preserves that behavior.
    const GHOST_SEED_TARGETS: &[CardId] = &[
        CardId::StrikeIronclad,
        CardId::DefendIronclad,
        CardId::StrikeSilent,
        CardId::DefendSilent,
        CardId::StrikeDefect,
        CardId::DefendDefect,
        CardId::StrikeRegent,
        CardId::DefendRegent,
        CardId::StrikeNecrobinder,
        CardId::DefendNecrobinder,
    ];
    let mut uids: Vec<sts2_core::CardUid> = Vec::new();
    uids.extend(state.player.draw.iter().copied());
    uids.extend(state.player.hand.iter().copied());
    uids.extend(state.player.discard.iter().copied());
    uids.extend(state.player.exhaust.iter().copied());
    for uid in uids {
        let card_id = match state.cards.get(uid.0.get() as usize - 1) {
            Some(c) => c.card_id,
            None => continue,
        };
        if GHOST_SEED_TARGETS.contains(&card_id) {
            set_card_var(state, uid, VarKey::EtherealThisCombat, 1);
        }
    }
    EffectResult::Resolved
}

/// `SneckoEye::modify_hand_draw` — `return draw + EXTRA_DRAW = 2`
/// (Python `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::
/// SneckoEye`). No round gate — fires every turn while owned. Bead
/// `sts2-rs-70anj.1`.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn listener_snecko_eye_hand_draw(state: &CombatState, current_draw: i32) -> i32 {
    if !state.relics.owns(RelicKind::SneckoEye) {
        return current_draw;
    }
    current_draw + SNECKO_EYE_EXTRA_DRAW
}

/// `ThrowingAxe::modify_card_play_count` — first card each combat is
/// played twice. Mirrors Python `sts2-rl-agent@01c094d:
/// sts2_env/relics/shop_event.py::ThrowingAxe`:
///
/// ```py
/// def modify_card_play_count(self, owner, count, card):
///     if not self._used_this_combat:
///         self._used_this_combat = True
///         return count + 1
///     return count
/// ```
///
/// Per-combat latch lives on the relic's `counter` field
/// (`0` = unused, `1` = fired) — same 3nis pattern used by
/// `Permafrost`/`EmberTea`. We only fire on the player's own play
/// (`ctx.owner == CreatureRef::Player`). Returns the *delta* (Rust
/// convention) not the new total. Bead `sts2-rs-70anj.3.3`.
fn listener_throwing_axe_play_count(state: &mut CombatState, ctx: &CardPlayCountCtx) -> i32 {
    if ctx.owner != sts2_core::hooks::CreatureRef::Player {
        return 0;
    }
    let Some(relic) = state
        .relics
        .iter_mut()
        .find(|r| !r.is_melted && r.kind == RelicKind::ThrowingAxe)
    else {
        return 0;
    };
    if relic.counter != 0 {
        return 0;
    }
    relic.counter = 1;
    1
}

/// `ThrowingAxe::before_combat_start` — clear the once-per-combat latch.
/// Mirrors Python's `self._used_this_combat = False` reset (Python hooks
/// both `before_combat_start` and `after_combat_end`; the Rust port hooks
/// the symmetric pre-combat channel so the reset is observable on the
/// very first dispatch of the next combat). Bead `sts2-rs-70anj.3.3`.
fn listener_throwing_axe_reset(state: &mut CombatState) -> EffectResult {
    let Some(relic) = state
        .relics
        .iter_mut()
        .find(|r| !r.is_melted && r.kind == RelicKind::ThrowingAxe)
    else {
        return EffectResult::NotImplemented;
    };
    relic.counter = 0;
    EffectResult::Resolved
}

/// `DelicateFrond::before_combat_start` — fills empty potion slots
/// with random potions. Mirrors Python's
/// `combat.fill_empty_potion_slots(owner, in_combat=False)` at
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py:1476`. The
/// core-side helper [`CombatState::fill_empty_potion_slots`] caps at
/// [`MAX_POTIONS`](sts2_core::potions::MAX_POTIONS); see that method's
/// doc-comment for the soft-cap-vs-hard-cap divergence note.
/// Bead `sts2-rs-70anj.3.12`.
fn listener_delicate_frond_combat_start(state: &mut CombatState) -> EffectResult {
    // Ownership gate: the `before_combat_start` channel fires every
    // registered listener unconditionally, so without this guard the belt
    // would be filled in *every* combat (even for players who don't carry
    // DelicateFrond) — surfacing phantom drinkable potions. Mirrors the
    // implicit ownership scoping every other per-relic listener applies.
    if !state.relics.owns(RelicKind::DelicateFrond) {
        return EffectResult::NotImplemented;
    }
    let _ = state.fill_empty_potion_slots();
    EffectResult::Resolved
}

/// `ChemicalX::modify_x_value` — `return x + INCREASE = 2`
/// (Python `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::
/// ChemicalX`). Adds +2 to every X-cost play's effective `x` value
/// while owned. Bead `sts2-rs-70anj.3.7`.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn listener_chemical_x_modify_x_value(state: &CombatState, current_x: i32) -> i32 {
    if !state.relics.owns(RelicKind::ChemicalX) {
        return current_x;
    }
    current_x + CHEMICAL_X_INCREASE
}

/// `BrilliantScarf::after_card_played` — `self._cards_this_turn += 1`
/// (Python `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::
/// BrilliantScarf`). Tracker-only mirror — Python's documented "5th
/// card costs 0" effect is not implemented at this pin (see
/// `feedback_plan_doc_lag_check_code_first`). We track the per-turn
/// play count on `Relic.counter` so downstream beads can wire the
/// cost-0 hook once Python adds it. Bead `sts2-rs-70anj.1`.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn listener_brilliant_scarf_card_played(
    state: &mut CombatState,
    _ctx: &CardEventCtx,
) -> EffectResult {
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::BrilliantScarf {
            relic.counter = relic.counter.saturating_add(1);
            return EffectResult::Resolved;
        }
    }
    EffectResult::NotImplemented
}

/// `BrilliantScarf::before_side_turn_start` — `if side == PLAYER:
/// self._cards_this_turn = 0` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::
/// BrilliantScarf`). Resets the per-turn play counter on every
/// player turn start. Bead `sts2-rs-70anj.1`.
fn listener_brilliant_scarf_turn_reset(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::BrilliantScarf {
            relic.counter = 0;
            return EffectResult::Resolved;
        }
    }
    EffectResult::NotImplemented
}

/// `BlessedAntler::before_hand_draw` — on the turn-1 hand draw, add
/// `BLESSED_ANTLER_DAZED_COUNT = 3` Dazed to the owner's draw pile at RANDOM
/// depths. C# `BlessedAntler.BeforeHandDraw` (`BlessedAntler.cs:46-58`), gated
/// on `Owner.PlayerCombatState.TurnNumber == 1`:
/// [decompiled C# omitted — see the cited path]
/// `AddGeneratedCardsToCombat` loops the singular add per card
/// (`CardPileCmd.cs:1043`), each resolving `CardPilePosition.Random` to
/// `AddInternal(card, Rng.Shuffle.NextInt(Cards.Count + 1))`
/// (`CardPileCmd.cs:511-515`) — so three fresh random depths against a pile
/// that grows by one each iteration, and three Shuffle-stream reads. The prior
/// port followed the Python pin's top-insert
/// (`combat.add_card_to_draw_pile`), which diverged on DRAW_ORDER and consumed
/// ZERO Shuffle draws, desyncing every later draw/reshuffle in the combat.
/// Same shape and same fix as `FuneraryMask` / `TeaOfDiscourtesy` (bead
/// `sts2-rs-n4sa4`).
///
/// Channel: registered on `before_hand_draw`, matching C#'s `BeforeHandDraw`
/// — the Dazed must be in the pile before the opening draw so one can land in
/// the turn-1 hand (the same correction `FuneraryMask` took). The `+1`
/// max-energy `ModifyMaxEnergy` leg is still deferred — no `modify_max_energy`
/// register channel exists at this pin. Beads `sts2-rs-70anj.1`,
/// `sts2-rs-lfdaa`.
fn listener_blessed_antler(state: &mut CombatState) -> EffectResult {
    if state.round_no() != 1 {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::BlessedAntler) {
        return EffectResult::NotImplemented;
    }
    #[allow(clippy::cast_sign_loss)]
    for _ in 0..(BLESSED_ANTLER_DAZED_COUNT as usize) {
        let dazed =
            crate::cards::status::card_from_spec(&crate::cards::status_specs_generated::dazed());
        let _ = sts2_core::card_effects::spawn_card_into_draw_pile_random_position(state, dazed);
    }
    EffectResult::Resolved
}

/// `PaelsEye::after_card_played` — `self._any_cards_played = True`
/// (Python `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::
/// PaelsEye`). Sets bit 1 of `Relic.counter` so the
/// `should_take_extra_turn` predicate fails until the next side-turn
/// reset. Python's hook is `before_card_played`; we use the relic-bus
/// `after_card_played` channel because (a) it has a `&mut state`
/// listener signature, (b) no relic-side `before_card_played`
/// channel exists at this pin, and (c) the flag is only read at
/// turn-end (after both fire), so the swap is semantically lossless.
/// Bead `sts2-rs-vfzgs`.
fn listener_paels_eye_card_played(state: &mut CombatState, _ctx: &CardEventCtx) -> EffectResult {
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::PaelsEye {
            relic.counter |= 0b10;
            return EffectResult::Resolved;
        }
    }
    EffectResult::NotImplemented
}

/// `PaelsEye::before_side_turn_start` — `if side == PLAYER:
/// self._any_cards_played = False` (Python pin same class). Clears
/// bit 1 of `Relic.counter` on every player side-turn-start. Bead
/// `sts2-rs-vfzgs`.
fn listener_paels_eye_side_turn_start(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::PaelsEye {
            relic.counter &= !0b10;
            return EffectResult::Resolved;
        }
    }
    EffectResult::NotImplemented
}

/// `PaelsEye::before_turn_end` — `if not _used_this_combat and not
/// _any_cards_played and side == PLAYER: exhaust entire hand` (Python
/// pin same class). The extra-turn re-entry + `_used_this_combat`
/// latch live in the engine surface (`combat_loop::end_player_turn`
/// short-circuit); the relic listener only handles the exhaust-hand
/// body. Gated on bit 0 (used) == 0 AND bit 1 (any_cards) == 0 so
/// the exhaust matches the would-be `should_take_extra_turn` outcome.
/// Bead `sts2-rs-vfzgs`.
fn listener_paels_eye_before_turn_end(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    let should_exhaust = state
        .relics
        .iter()
        .any(|r| !r.is_melted && r.kind == RelicKind::PaelsEye && r.counter.trailing_zeros() >= 2);
    if !should_exhaust {
        return EffectResult::NotImplemented;
    }
    state.exhaust_hand();
    EffectResult::Resolved
}

/// `MusicBox::after_card_played` — `if not _used_this_turn and
/// card.card_type == ATTACK: combat.create_ethereal_clone_in_hand(owner,
/// card); _used_this_turn = True` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::MusicBox`).
/// Per-turn latch is bit 0 of `Relic.counter`; the clone is built by
/// cloning the played card's POD from the arena, setting
/// `VarKey::EtherealThisCombat = 1` on the new instance, and routing
/// through `spawn_card_into_hand`. The end-of-turn ethereal sweep in
/// `combat_loop::end_player_turn_phase_two` honors the per-instance
/// flag so the clone exhausts at turn end even when its `CardId` is
/// not in the static ethereal registry. Bead `sts2-rs-ad53n`.
fn listener_music_box_card_played(state: &mut CombatState, ctx: &CardEventCtx) -> EffectResult {
    let Some(played_uid) = ctx.card_uid else {
        return EffectResult::NotImplemented;
    };
    let idx = played_uid.0.get() as usize - 1;
    let Some(card) = state.cards.get(idx) else {
        return EffectResult::NotImplemented;
    };
    if card.card_type != CardType::Attack {
        return EffectResult::NotImplemented;
    }
    let mut consumed = false;
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::MusicBox && (relic.counter & 0b01) == 0 {
            relic.counter |= 0b01;
            consumed = true;
            break;
        }
    }
    if !consumed {
        return EffectResult::NotImplemented;
    }
    let cloned = state.cards[idx];
    let new_uid = spawn_card_into_hand(state, cloned);
    set_card_var(state, new_uid, VarKey::EtherealThisCombat, 1);
    EffectResult::Resolved
}

/// `MusicBox::before_side_turn_start` — `if side == PLAYER:
/// self._used_this_turn = False` (Python pin same class). Clears bit 0
/// of `Relic.counter` so the next player turn re-arms the
/// first-Attack-clone trigger. Bead `sts2-rs-ad53n`.
fn listener_music_box_side_turn_start(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::MusicBox {
            relic.counter &= !0b01;
            return EffectResult::Resolved;
        }
    }
    EffectResult::NotImplemented
}

/// `Crossbow::after_side_turn_start` — `if side == PLAYER:
/// combat.generate_free_attack_in_hand(owner)` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::Crossbow`).
/// Engine surface (`card_effects::generate_free_attack_in_hand`) samples
/// a random Attack from the player's character pool, sets
/// `VarKey::FreeThisTurn = 1`, and pushes into hand. Bead
/// `sts2-rs-zh82z`.
fn listener_crossbow(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::Crossbow) {
        return EffectResult::NotImplemented;
    }
    generate_free_attack_in_hand(state);
    EffectResult::Resolved
}

const SEAL_OF_GOLD_COST: i32 = 5;
const SEAL_OF_GOLD_ENERGY: i32 = 1;

/// `SealOfGold::after_side_turn_start` — `if side == PLAYER and
/// owner.gold >= GOLD_COST = 5: owner.lose_gold(5);
/// combat.gain_energy(owner, ENERGY = 1)` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::SealOfGold`).
/// Reads gold via `card_effects::read_player_gold` (closure registry
/// installed by the run-loop driver at combat start) and spends via
/// `try_spend_player_gold`. Returns `NotImplemented` when:
/// - No gold accessor is installed (core-only test build).
/// - `gold < 5` (insufficient funds).
/// - The atomic spend fails (the run-loop's gold mutator returns
///   false — e.g. another listener already drained gold mid-walk).
/// Bead `sts2-rs-1e0ae`.
fn listener_seal_of_gold(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::SealOfGold) {
        return EffectResult::NotImplemented;
    }
    // C# `SealOfGold.AfterSideTurnStart`: while gold ≥ 5, spend 5 for +1 energy.
    // Reads/spends the combat-time gold mirror (`enter_combat` snapshots run
    // gold in, `exit_combat` writes it back). The old `read_player_gold` /
    // `try_spend_player_gold` global-accessor path was never installed by any
    // driver, so this relic silently no-opped (IC:1002 @377). Bead
    // `sts2-rs-RP17`.
    if state.gold < SEAL_OF_GOLD_COST {
        return EffectResult::NotImplemented;
    }
    state.gold -= SEAL_OF_GOLD_COST;
    state.gain_energy(SEAL_OF_GOLD_ENERGY);
    EffectResult::Resolved
}

/// `Bread::after_side_turn_start` — on the FIRST player turn only,
/// `PlayerCmd.LoseEnergy(LoseEnergy = 2)` (C# v0.107.1
/// `MegaCrit.Sts2.Core.Models.Relics/Bread.cs:AfterSideTurnStart`:
/// `if participants.Contains(Owner.Creature) && TurnNumber == 1:
/// LoseEnergy(2)`). C# `PlayerCombatState.LoseEnergy` clamps at 0
/// (`PlayerCombatState.cs:178-185`), so the subtraction saturates.
/// The complementary `ModifyMaxEnergy` leg (+1 from turn 2 onward,
/// nothing on turn 1) is the inline `RelicKind::Bread` arm in
/// `sts2_core::hooks::modify_max_energy`. 2026-07-17 soft-drop
/// census fix.
fn listener_bread(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if state.round != 1 {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::Bread) {
        return EffectResult::NotImplemented;
    }
    let new_energy = (i32::from(state.player.energy) - BREAD_LOSE_ENERGY).max(0);
    state.player.energy = i8::try_from(new_energy).unwrap_or(0);
    EffectResult::Resolved
}

/// `LeesWaffle::after_obtained` — `owner.gain_max_hp(7)` + `owner.heal(max_hp)`
/// (Python `shop_event.py::LeesWaffle`). Bead `sts2-rs-bmw3`.
fn listener_lees_waffle(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::LeesWaffle) {
        return EffectResult::NotImplemented;
    }
    rs.gain_max_hp(LEES_WAFFLE_MAX_HP);
    rs.heal(rs.max_hp);
    EffectResult::Resolved
}

/// `LoomingFruit::after_obtained` — `owner.gain_max_hp(31)` (Python
/// `shop_event.py::LoomingFruit`, `MAX_HP=31`). Bead `sts2-rs-r4pr`.
fn listener_looming_fruit(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::LoomingFruit) {
        return EffectResult::NotImplemented;
    }
    rs.gain_max_hp(LOOMING_FRUIT_MAX_HP);
    EffectResult::Resolved
}

/// `NutritiousOyster::after_obtained` — `owner.gain_max_hp(11)` (Python
/// `shop_event.py::NutritiousOyster`, `MAX_HP=11`). Bead `sts2-rs-r4pr`.
fn listener_nutritious_oyster(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::NutritiousOyster) {
        return EffectResult::NotImplemented;
    }
    rs.gain_max_hp(NUTRITIOUS_OYSTER_MAX_HP);
    EffectResult::Resolved
}

/// `GoldenPearl::after_obtained` — `owner.gain_gold(150)` (Python
/// `shop_event.py::GoldenPearl`, `GOLD=150`). Bead `sts2-rs-yfi0`.
fn listener_golden_pearl(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::GoldenPearl) {
        return EffectResult::NotImplemented;
    }
    rs.gain_gold(GOLDEN_PEARL_GOLD);
    EffectResult::Resolved
}

/// `SignetRing::after_obtained` — `owner.gain_gold(999)` (Python
/// `shop_event.py::SignetRing`, `GOLD=999`). Bead `sts2-rs-2ciy`.
fn listener_signet_ring(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::SignetRing) {
        return EffectResult::NotImplemented;
    }
    rs.gain_gold(SIGNET_RING_GOLD);
    EffectResult::Resolved
}

/// `PumpkinCandle::AfterObtained → Rekindle` (v0.107.1) — add
/// `kindleAmount = 5` to the relic's `KindleCount`, stored in
/// `rs.relic_counters[PumpkinCandle]`. Mirrors C#
/// `MegaCrit.Sts2.Core.Models.Relics/PumpkinCandle.cs:52-56`
/// (`AfterObtained → Rekindle`) + `:87-91` (`Rekindle → KindleCount += 5`).
///
/// While `KindleCount > 0` the combat-side `modify_max_energy` arm in
/// `core/hooks.rs` grants +1 max energy; the count decays by 1 per
/// combat end (`exit_combat` bridge) and the rest-site `Kindle` option
/// tops it back up by another 5. The v0.107.1 redesign replaced the
/// old v0.103.2 act-snapshot capture this listener used to perform.
///
/// `rs.relic_counters[PumpkinCandle]` → combat-side `Relic.counter`
/// thread-through lives on the `enter_combat(rs, encounter)` bridge
/// (`sts2-rs-tjn0v`). Bead `sts2-rs-run-rest-option`.
fn listener_pumpkin_candle(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::PumpkinCandle) {
        return EffectResult::NotImplemented;
    }
    *rs.relic_counters.entry(RelicId::PumpkinCandle).or_insert(0) +=
        sts2_core::runstate::PUMPKIN_CANDLE_KINDLE_AMOUNT;
    EffectResult::Resolved
}

/// `PumpkinCandle::TryModifyRestSiteOptions` — append
/// [`RestSiteOption::Kindle`] to the rest-site option list while the
/// relic is owned. Mirrors C#
/// `MegaCrit.Sts2.Core.Models.Relics/PumpkinCandle.cs:77-85`
/// (`options.Add(new KindleRestSiteOption(player))`, owner-gated) and
/// the Shovel `modify_rest_site_options` precedent
/// (`rare.rs::listener_shovel_rest_site_options`). Appended after the
/// base `[Heal, Smith]` list, matching C# `RestSiteOption.Generate`'s
/// `Hook.ModifyRestSiteOptions` walk. Bead `sts2-rs-run-rest-option`.
fn listener_pumpkin_candle_rest_site_options(
    rs: &RunState,
    options: &mut Vec<RestSiteOption>,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::PumpkinCandle) {
        return EffectResult::NotImplemented;
    }
    if options.iter().any(|opt| opt.option_id() == "KINDLE") {
        return EffectResult::NotImplemented;
    }
    options.push(RestSiteOption::Kindle);
    EffectResult::Resolved
}

/// `DingyRug::modify_card_reward_creation_options` — replace the
/// implicit character pool with an explicit `custom_card_ids` slate
/// that unions the character pool with the colorless pool, then
/// flip `has_custom_card_pool = true` so the rarity-roll generator
/// downstream consumes the explicit slate. Mirrors Python
/// `shop_event.py::DingyRug.modify_card_reward_creation_options`
/// (`sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py:262-319`).
///
/// Python flow:
/// 1. Bail when `options.allow_card_pool_modifications` is false
///    (the per-relic skip handshake — Snecko-style `modify_card_pool`
///    suppression flips this off for its own roll).
/// 2. Collect character-pool ids via `card_reward_candidate_ids`
///    (already obeys `card_type` / `generation_context` filters).
/// 3. Collect colorless ids via `eligible_registered_cards(card_pool
///    = COLORLESS)`. When `allow_rarity_modifications` is false,
///    constrain the colorless ids to the rarities already present
///    in the character-pool slate.
/// 4. Return options with `has_custom_card_pool=True`,
///    `character_ids=()`, `use_default_character_pool=False`, and
///    `custom_card_ids = dedupe(character + colorless)`.
///
/// Rust port: same shape, but the filter args (`card_type`,
/// `generation_context`, multiplayer) collapse at this pin —
/// `card_type` defaults to `None` in `CardRewardGenerationOptions`
/// at every documented call site, the spec-side per-card
/// `generation_context` filter isn't ported yet, and multiplayer
/// is out of scope per `CLAUDE.md`. So the walk is "every spec in
/// the per-character + colorless slices". The
/// `allow_rarity_modifications=false` branch IS honored: when the
/// flag is clear, the colorless slate is filtered to the rarities
/// seen in the character pool (matches Python's `allowed_rarities`
/// guard for SilverCrucible-style chains).
///
/// Bead `sts2-rs-u5emw` (P9.3 / 70anj.4 body-fill).
fn listener_dingy_rug(rs: &mut RunState, opts: &mut CardRewardGenerationOptions) -> EffectResult {
    if !rs.owns_active_relic(RelicId::DingyRug) {
        return EffectResult::NotImplemented;
    }
    if !opts.allow_card_pool_modifications {
        return EffectResult::NotImplemented;
    }

    use std::collections::HashSet;

    use crate::cards::spec_index::{specs_for_character, COLORLESS_SPECS};

    let mut seen: HashSet<sts2_core::cards::CardId> = HashSet::new();
    let mut combined: Vec<sts2_core::cards::CardId> = Vec::new();
    let mut character_rarities: HashSet<crate::cards::CardRarity> = HashSet::new();

    for &factory in specs_for_character(rs.character_id) {
        let spec = factory();
        if seen.insert(spec.id) {
            combined.push(spec.id);
            character_rarities.insert(spec.rarity);
        }
    }

    let rarity_gated = !opts.allow_rarity_modifications;
    for &factory in COLORLESS_SPECS {
        let spec = factory();
        if rarity_gated && !character_rarities.contains(&spec.rarity) {
            continue;
        }
        if seen.insert(spec.id) {
            combined.push(spec.id);
        }
    }

    opts.has_custom_card_pool = true;
    opts.custom_card_ids = combined;
    opts.character_ids.clear();
    opts.use_default_character_pool = false;
    EffectResult::Resolved
}

/// `MassiveScroll::after_obtained` — push `MASSIVE_SCROLL_CARDS = 3`
/// `Reward::Card(CardReward::empty())` sentinels (Python
/// `shop_event.py::MassiveScroll`: `owner.offer_multiplayer_cards(3)`).
/// Coop/multiplayer is out of scope per CLAUDE.md, so this collapses
/// to regular card-reward offers — same convention as Orrery (psf9).
/// Bead `sts2-rs-64vb`.
fn listener_massive_scroll(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::MassiveScroll) {
        return EffectResult::NotImplemented;
    }
    for _ in 0..MASSIVE_SCROLL_CARDS {
        rs.pending_rewards.push(Reward::Card(CardReward::empty()));
    }
    EffectResult::Resolved
}

/// `Cauldron::after_obtained` — push `CAULDRON_POTIONS = 5`
/// `Reward::Potion(PotionReward::empty())` sentinels to
/// `rs.pending_rewards` (Python `shop_event.py::Cauldron`:
/// `owner.offer_potions(5)`). The empty PotionReward is the
/// random-potion placeholder pending dispatcher-side roll at flush
/// time (ib04 convention). Same shape as Orrery's CardReward push.
/// Bead `sts2-rs-2nrk`.
fn listener_cauldron(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::Cauldron) {
        return EffectResult::NotImplemented;
    }
    for _ in 0..CAULDRON_POTIONS {
        rs.pending_rewards
            .push(Reward::Potion(PotionReward::empty()));
    }
    EffectResult::Resolved
}

/// `SmallCapsule::after_obtained` — push `SMALL_CAPSULE_RELICS = 1`
/// RelicReward sentinel via `rs.offer_relic_rewards` (Python
/// `shop_event.py::SmallCapsule.after_obtained`:
/// `owner.offer_relic_rewards(1)`). Same shape as CallingBell (u4wv)
/// but 1 reward instead of 3. Bead `sts2-rs-gf5s`.
fn listener_small_capsule(rs: &mut RunState) -> EffectResult {
    // One-shot on own pickup: gate on `last_obtained_relic`, not on
    // `relics.contains` — the dispatcher walks every relic acquisition,
    // so an `is-owned` check would re-fire on every later pickup and
    // flood the relic-reward queue.
    if rs.last_obtained_relic != Some(RelicId::SmallCapsule) {
        return EffectResult::NotImplemented;
    }
    // Python `offer_relic_rewards(1)` — rolls one random rarity per slot.
    rs.offer_random_relic_rewards(SMALL_CAPSULE_RELICS, &[]);
    EffectResult::Resolved
}

/// `CallingBell::after_obtained` — push `CALLING_BELL_RELICS = 3`
/// RelicReward sentinels via `rs.offer_relic_rewards` (Python
/// `shop_event.py::CallingBell.after_obtained`: `owner.offer_relic_rewards(
/// 3, rarities=(COMMON, UNCOMMON, RARE))`). The RelicId acts as the
/// sentinel placeholder; umbrella sts2-rs-01o resolves rarity tiers
/// at flush time. The CurseOfTheBell card-add clause is deferred —
/// same gating as NeowsTorment / PaelsHorn pending run-side deck
/// mutation surface. Bead `sts2-rs-u4wv`.
fn listener_calling_bell(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::CallingBell) {
        return EffectResult::NotImplemented;
    }
    // Python pin: `offer_relic_rewards(3, rarities=(COMMON, UNCOMMON, RARE))`.
    rs.offer_random_relic_rewards(
        CALLING_BELL_RELICS,
        &[
            Some(RelicRarity::Common),
            Some(RelicRarity::Uncommon),
            Some(RelicRarity::Rare),
        ],
    );
    EffectResult::Resolved
}

/// `LeafyPoultice::after_obtained` — C# `LeafyPoultice.cs`:
/// [decompiled C# omitted — see the cited path]
/// Loses `LEAFY_POULTICE_MAX_HP_LOSS = 12` max HP (C# `MaxHpVar(12m)`),
/// then transforms the **first** Basic Strike and **first** Basic Defend
/// in the deck into two random `{Common, Uncommon, Rare}` cards from the
/// character pool, each rolled off the `transformations` RNG stream.
///
/// **RNG order** matches C# `CardCmd.Transform`, which calls
/// `GetReplacement(rng)` for each transformation in the *input list*
/// order — Strike first, Defend second — **before** the pile-index sort
/// (the sort only reorders insertion, not RNG reads). So we roll the
/// Strike replacement, then the Defend replacement, in that order. The
/// per-card pool / pick is `CardFactory.CreateRandomCardForTransform`
/// out of combat — see [`crate::cards::spec_index::roll_transform_replacement_basic`]
/// for the pool derivation. Slots are deterministic (first Strike, first
/// Defend), so they consume no RNG; the in-place swap goes through
/// [`sts2_core::runstate::RunState::transform_specific_cards`] (same
/// position-preserving primitive PandorasBox uses).
///
/// A character returning `None` from `basic_strike_defend_cards` would no-op
/// the transform leg while the HP loss still applies; every character now has
/// a pair, so that branch is unreachable. Bead `sts2-rs-beda` (transform leg under
/// umbrella `sts2-rs-01o`).
fn listener_leafy_poultice(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::LeafyPoultice) {
        return EffectResult::NotImplemented;
    }
    rs.lose_max_hp(LEAFY_POULTICE_MAX_HP_LOSS);
    let character_id = rs.character_id;
    let Some([strike_id, defend_id]) =
        crate::cards::spec_index::basic_strike_defend_cards(character_id)
    else {
        return EffectResult::Resolved;
    };
    // C# collects the first Basic Strike, then the first Basic Defend,
    // and rolls each replacement in that order off `Transformations`.
    let mut indices: Vec<usize> = Vec::with_capacity(2);
    let mut replacements: Vec<sts2_core::Card> = Vec::with_capacity(2);
    for original_id in [strike_id, defend_id] {
        let Some(deck_idx) = rs
            .master_deck
            .iter()
            .position(|card| card.card_id == original_id)
        else {
            continue;
        };
        let Some(new_id) = crate::cards::spec_index::roll_transform_replacement_basic(
            rs,
            character_id,
            original_id,
        ) else {
            continue;
        };
        // Build the replacement from its spec, not a blank `pod_defaults()`
        // with only `card_id` set: a transformed card carries its full
        // stats into combat (the master-deck POD is the combat card's
        // source), so a stat-less POD plays for zero damage / block. C#
        // `CardCmd.Transform` inserts a fully-constructed `CardModel`
        // (`GetReplacement` → `CardFactory.CreateRandomCardForTransform`).
        // Transform replacements are never upgraded out of combat. GH#31:
        // this is why a Leafy-Poultice-rolled Collision Course dealt 0 in
        // Rust vs 11 in C#.
        let Some(card) = crate::cards::spec_index::card_pod_from_id(new_id, false) else {
            continue;
        };
        indices.push(deck_idx);
        replacements.push(card);
    }
    if !indices.is_empty() {
        let _ = rs.transform_specific_cards(&indices, replacements);
    }
    EffectResult::Resolved
}

/// `DistinguishedCape::after_obtained` — `owner.lose_max_hp(9)` plus
/// push 3 Apparition cards onto `rs.master_deck` (Python
/// `shop_event.py::DistinguishedCape.after_obtained`, `HP_LOSS = 9`,
/// `CARDS = 3`). HP-loss leg landed in bead `sts2-rs-r2y4x`; the
/// Apparition deck-add leg landed in bead `sts2-rs-a49q3` once the
/// direct-deck-push pattern (JewelryBox / TanxsWhistle) was confirmed
/// sufficient. Uses the C#-faithful `lose_max_hp` (only clamps current HP
/// down when the new max falls below it) per the LeafyPoultice precedent.
fn listener_distinguished_cape(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::DistinguishedCape) {
        return EffectResult::NotImplemented;
    }
    rs.lose_max_hp(DISTINGUISHED_CAPE_HP_LOSS);
    for _ in 0..DISTINGUISHED_CAPE_CARDS {
        push_deck_card(rs, sts2_core::cards::CardId::Apparition);
    }
    EffectResult::Resolved
}

/// `SandCastle::after_obtained` — C# `SandCastle.cs:22-32`:
/// `Deck.Cards.Where(IsUpgradable).StableShuffle(Rng.Niche).Take(6)` →
/// `CardCmd.Upgrade`. This upgrades 6 RANDOM cards with no player choice
/// (the grid is a preview animation, not a chooser), so it routes through
/// [`RunState::upgrade_random_cards_stable`], NOT the interactive-aware
/// `upgrade_random_cards`: the latter, under the workbench / lockstep
/// `interactive_card_upgrade` flag, would wrongly present a "pick 6 cards"
/// selection screen. Bead `sts2-rs-r2y4x`.
fn listener_sand_castle(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::SandCastle) {
        return EffectResult::NotImplemented;
    }
    rs.upgrade_random_cards_stable(SAND_CASTLE_CARDS);
    EffectResult::Resolved
}

/// `GlassEye::after_obtained` — push `GLASS_EYE_CARDS = 5`
/// `Reward::Card(CardReward::empty())` sentinels (Python
/// `shop_event.py::GlassEye.after_obtained`: 5-iteration loop over
/// `(COMMON, COMMON, UNCOMMON, UNCOMMON, RARE)` rarities, each
/// pushing a `CardReward` with `option_count=3` + forced rarity).
/// The rarity-forcing + `option_count=3` clauses are deferred —
/// the current Rust `CardReward::empty()` placeholder has no rarity
/// or option-count fields; umbrella `sts2-rs-01o` (random reward
/// rolling) materializes both at flush time. Same convention as
/// Orrery (`sts2-rs-psf9`) / MassiveScroll (`sts2-rs-64vb`). Bead
/// `sts2-rs-66l9f`.
fn listener_glass_eye(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::GlassEye) {
        return EffectResult::NotImplemented;
    }
    for _ in 0..GLASS_EYE_CARDS {
        rs.pending_rewards.push(Reward::Card(CardReward::empty()));
    }
    EffectResult::Resolved
}

/// `LostCoffer::after_obtained` — push one `Reward::Card` + one
/// `Reward::Potion` sentinel (Python
/// `shop_event.py::LostCoffer.after_obtained`: `owner.offer_card_reward()`
/// then `owner.offer_potion_reward()`). The Rust port uses
/// `CardReward::empty()` / `PotionReward::empty()` placeholders
/// pending umbrella `sts2-rs-01o` random rolling — same convention
/// as Orrery / MassiveScroll. Counts hard-coded to 1 in Python (no
/// class-level constant). Bead `sts2-rs-66l9f`.
fn listener_lost_coffer(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::LostCoffer) {
        return EffectResult::NotImplemented;
    }
    rs.pending_rewards.push(Reward::Card(CardReward::empty()));
    rs.pending_rewards
        .push(Reward::Potion(PotionReward::empty()));
    EffectResult::Resolved
}

/// `Kaleidoscope::after_obtained` — push 2 pre-resolved `Reward::Card`
/// sentinels, one per card reward Kaleidoscope offers on pickup
/// (`Kaleidoscope.cs:33-50`, `DynamicVars.Cards == 2`). Each reward's 3
/// cross-character cards are rolled NOW (at obtain time, as C# does in
/// `AfterObtained`) via [`crate::cards::spec_index::roll_kaleidoscope_card_rewards`]
/// — Niche-shuffled non-owner pools + base-odds card creation on the Rewards
/// stream — so the offered slate is materialized verbatim (the non-empty
/// `Reward::Card` surfaces directly through `drain_pending_rewards` instead
/// of re-rolling the owner pool). The two rewards surface sequentially as
/// `card_reward` picks (C# `RewardsCmd.OfferCustom([reward1, reward2])`).
/// Fires once on Kaleidoscope's own pickup (`last_obtained_relic`), like
/// HeftyTablet / ScrollBoxes — gating on `relics.contains` would re-roll the
/// streams on every later relic obtain. Bead `sts2-rs-utgn4`.
fn listener_kaleidoscope(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::Kaleidoscope) {
        return EffectResult::NotImplemented;
    }
    let owner = rs.character_id;
    let rewards = crate::cards::spec_index::roll_kaleidoscope_card_rewards(rs, owner);
    for reward in rewards {
        rs.pending_rewards
            .push(Reward::Card(CardReward::from_options(reward)));
    }
    EffectResult::Resolved
}

/// `ScrollBoxes::after_obtained` — generate 2 card bundles and queue the
/// pick-1-of-N-packs choice. v0.107.1 delta
/// (`sts2-rs-v0107-content-port-wb8lk.9` §7): the relic **no longer loses
/// all gold** on pickup (`ScrollBoxes.cs:30-40` — the prior v0.103.2
/// `PlayerCmd.LoseGold(Owner.Gold)` line was removed; the AfterObtained
/// body now only generates + offers the bundles). The Neow-pool
/// `IsAllowedAtNeow` gate (≥4 unlocked commons / ≥2 unlocked uncommons via
/// `CanGenerateBundles`) is a no-op for the shipped character pools (all
/// satisfy it) and there is no Neow-offer filtering surface in the Rust
/// run loop to hook it into, so it is not modeled.
///
/// The card-bundle mechanic (each bundle is a multi-card "all-or-nothing"
/// pick) surfaces via `rs.pending_bundles` /
/// `RunManager::surface_pending_bundle`. Bead `sts2-rs-66l9f` (initial) /
/// `…wb8lk.9` (v0.107.1: drop the gold loss).
fn listener_scroll_boxes(rs: &mut RunState) -> EffectResult {
    // Fire exactly once, on ScrollBoxes's own pickup (mirrors HeftyTablet).
    // Gating on `relics.contains` would re-fire on every subsequent relic
    // obtain — RE-ROLLING the Rewards RNG and clobbering `pending_bundles`,
    // which would both desync downstream RNG and resurface a phantom bundle
    // pick. `last_obtained_relic` fires once.
    if rs.last_obtained_relic != Some(RelicId::ScrollBoxes) {
        return EffectResult::NotImplemented;
    }
    // v0.107.1: NO gold loss (the v0.103.2 `LoseGold(Owner.Gold)` was removed).
    // Generate the 2 packs (2 common + 1 uncommon each) and queue the
    // pick-1-of-N-packs choice (`ScrollBoxes.cs:25-34`
    // `GenerateRandomBundles` → `FromChooseABundleScreen` → add the whole
    // chosen pack to the Deck). The run loop surfaces `pending_bundles` as a
    // no-skip CardReward fan-out (`RunManager::surface_pending_bundle`).
    let character = rs.character_id;
    let bundles = crate::cards::spec_index::generate_scroll_box_bundles(rs, character);
    debug_assert_eq!(
        bundles.len(),
        SCROLL_BOXES_BUNDLES,
        "ScrollBoxes makes 2 packs"
    );
    if !bundles.is_empty() {
        rs.pending_bundles = bundles;
    }
    EffectResult::Resolved
}

/// Helper: push a single fresh `CardId` POD onto `rs.master_deck`.
/// Mirrors the direct-deck-push pattern used by JewelryBox /
/// TanxsWhistle, with a [`CardSpec`] lookup so `card_type`, `cost`,
/// `base_damage`, `base_block`, and `target_type` come from the per-
/// card factory rather than the generic `Card::pod_defaults` defaults
/// (which would tag every card as a 1-cost Attack regardless of
/// `card_id`). Falls back to `pod_defaults` when the spec isn't
/// registered. Bead `sts2-rs-a49q3` (initial form) / `sts2-rs-jzu61`
/// (spec lookup added so curses correctly receive `CardType::Curse`
/// and downstream gates like RoyalStamp's `card_type in {Attack,
/// Skill}` filter work).
fn push_deck_card(rs: &mut RunState, card_id: sts2_core::cards::CardId) {
    let mut card = sts2_core::cards::Card::pod_defaults();
    card.card_id = card_id;
    if let Some(spec) = crate::names::deck_card_spec_pool()
        .into_iter()
        .find(|s| s.id == card_id)
    {
        card.card_type = spec.card_type;
        card.cost = spec.cost;
        card.base_damage = spec.base_damage;
        card.base_block = spec.base_block;
        card.target_type = spec.target_type;
    }
    rs.master_deck.push(card);
}

/// Enchant up to `max_count` eligible cards in `rs.master_deck` with
/// `ench` at the given `amount`. Mirrors the direct-apply arm of
/// `owner.enchant_selected_cards(...)` from
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py` —
/// i.e. the path taken when `run_state.defer_followup_rewards` is
/// `False`. Eligibility uses [`crate::enchantments::can_enchant_card`]
/// against the matching [`crate::cards::CardSpec`]; cards lacking a
/// spec entry are skipped. Picks the first `max_count` eligible
/// cards in deck order (deterministic) — Python's random selection
/// is gated on the deferred-reward arm, which lands later via
/// umbrella `sts2-rs-01o` (`Reward::Enchant` variant). Returns the
/// number actually enchanted (≤ `max_count`).
/// Collect the `CardId`s of every master-deck card eligible to receive
/// `ench`. Mirrors Python's `[card for card in owner.deck if
/// can_enchant_card(card, ench)]` list comprehension used by the
/// deferred-reward arm of every shop_event enchant-on-obtain relic.
///
/// Returns one entry per deck-position, so duplicates are preserved
/// (matches Python's list semantics; the reward UI dedupes display-
/// side, not data-side). Bead `sts2-rs-2v8y2`.
fn enchantable_candidate_card_ids(
    rs: &RunState,
    ench: sts2_core::cards::EnchantmentId,
) -> Vec<sts2_core::cards::CardId> {
    let pool = crate::names::deck_card_spec_pool();
    rs.master_deck
        .iter()
        .filter_map(|card| {
            let spec = pool.iter().find(|s| s.id == card.card_id)?;
            crate::enchantments::can_enchant_card(spec, card, ench).then_some(card.card_id)
        })
        .collect()
}

fn enchant_eligible_deck_cards(
    rs: &mut RunState,
    ench: sts2_core::cards::EnchantmentId,
    amount: i32,
    max_count: usize,
) -> usize {
    if max_count == 0 || rs.master_deck.is_empty() {
        return 0;
    }
    let pool = crate::names::deck_card_spec_pool();
    let mut applied = 0;
    for card in &mut rs.master_deck {
        if applied >= max_count {
            break;
        }
        let Some(spec) = pool.iter().find(|s| s.id == card.card_id) else {
            continue;
        };
        if !crate::enchantments::can_enchant_card(spec, card, ench) {
            continue;
        }
        let _ = crate::enchantments::apply_static_enchantment(card, ench, amount);
        applied += 1;
    }
    applied
}

/// The `CardType` restriction(s) for an enchant-on-obtain relic's
/// interactive deck card-select, mirroring the per-enchantment
/// `card_type` arm of [`crate::enchantments::can_enchant_card`]. Returns
/// `(primary, secondary)`: the player may pick any deck card whose type
/// matches either. `(None, None)` = unrestricted (any enchantable card).
///
/// Kept in sync with `can_enchant_card` by hand (core's
/// `enchant_eligible_deck_indices` can't reach the content-layer gate);
/// covers only the enchantments the shop enchant-on-obtain relics use.
fn enchant_card_type_restriction(
    ench: sts2_core::cards::EnchantmentId,
) -> (
    Option<sts2_core::cards::CardType>,
    Option<sts2_core::cards::CardType>,
) {
    use sts2_core::cards::{CardType, EnchantmentId};
    match ench {
        // Attack-only (Sharp/Momentum/… group in `can_enchant_card`).
        EnchantmentId::Sharp | EnchantmentId::Momentum => (Some(CardType::Attack), None),
        // Skill-only.
        EnchantmentId::Imbued => (Some(CardType::Skill), None),
        // Attack OR Skill — RoyalStamp's RoyallyApproved.
        EnchantmentId::RoyallyApproved => (Some(CardType::Attack), Some(CardType::Skill)),
        // Unrestricted (Adroit, Swift, …).
        _ => (None, None),
    }
}

/// Interactive arm shared by every shop enchant-on-obtain relic: when the
/// run surfaces deck card-selects (`interactive_card_transform`) AND the
/// enchant offers a real choice (more eligible candidates than picks),
/// queue `count` interactive [`PendingDeckEnchant`] picks and return
/// `true`. Mirrors C# `CardSelectCmd.FromDeckForEnchantment(count)`.
///
/// Returns `false` (caller falls through to its defer/auto arms) when not
/// interactive, or when `candidates <= count` so every eligible card is
/// enchanted regardless of the pick — matching the SelfHelpBook/Wood
/// Carvings `eligible > 1` convention generalized to `count`.
fn try_queue_interactive_enchant(
    rs: &mut RunState,
    ench: sts2_core::cards::EnchantmentId,
    amount: i32,
    count: usize,
) -> bool {
    if !rs.interactive_card_transform {
        return false;
    }
    let candidates = enchantable_candidate_card_ids(rs, ench);
    if candidates.len() <= count {
        return false;
    }
    let (type_restriction, secondary_type) = enchant_card_type_restriction(ench);
    for _ in 0..count {
        rs.pending_deck_enchants
            .push(sts2_core::PendingDeckEnchant {
                enchantment: ench,
                amount,
                type_restriction,
                secondary_type,
            });
    }
    true
}

/// `GnarledHammer::after_obtained` — enchant up to `GNARLED_HAMMER_CARDS`
/// eligible deck cards with `Sharp(GNARLED_HAMMER_SHARP)` (Python
/// `shop_event.py::GnarledHammer.after_obtained`,
/// `enchant_selected_cards("Sharp", 3, 3, cards=candidates)`).
/// Interactive arm (`interactive_card_transform`) surfaces a deck
/// card-select (C# `CardSelectCmd.FromDeckForEnchantment`); the defer
/// reward-queue and direct-apply arms cover the other cases.
/// Bead `sts2-rs-70anj.4`.
fn listener_gnarled_hammer(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::GnarledHammer) {
        return EffectResult::NotImplemented;
    }
    // C# `GnarledHammer.AfterObtained` awaits an interactive
    // `CardSelectCmd.FromDeckForEnchantment` — surface the deck
    // card-select when the run is interactive.
    if try_queue_interactive_enchant(
        rs,
        sts2_core::cards::EnchantmentId::Sharp,
        GNARLED_HAMMER_SHARP,
        GNARLED_HAMMER_CARDS,
    ) {
        return EffectResult::Resolved;
    }
    // Bead `sts2-rs-2v8y2`: deferred-reward arm. Mirrors Python
    // `relics/shop_event.py:410` (GnarledHammer.after_obtained):
    // when redeemed as part of a reward cascade, queue a player-pick
    // EnchantCards reward (`min_count=0` makes it skippable) instead
    // of auto-enchanting the first N eligible deck cards.
    if rs.defer_followup_rewards {
        let candidates = enchantable_candidate_card_ids(rs, sts2_core::cards::EnchantmentId::Sharp);
        rs.offer_enchant_cards_reward(
            sts2_core::cards::EnchantmentId::Sharp,
            GNARLED_HAMMER_SHARP,
            GNARLED_HAMMER_CARDS,
            candidates,
            Some(0),
        );
        return EffectResult::Resolved;
    }
    let _ = enchant_eligible_deck_cards(
        rs,
        sts2_core::cards::EnchantmentId::Sharp,
        GNARLED_HAMMER_SHARP,
        GNARLED_HAMMER_CARDS,
    );
    EffectResult::Resolved
}

/// `Kifuda::after_obtained` — enchant up to `KIFUDA_CARDS` eligible
/// deck cards with `Adroit(KIFUDA_ADROIT)` (Python
/// `shop_event.py::Kifuda.after_obtained`,
/// `enchant_selected_cards("Adroit", 3, 3, cards=candidates)`).
/// Interactive arm (`interactive_card_transform`) surfaces a deck
/// card-select (C# `CardSelectCmd.FromDeckForEnchantment`); the defer
/// and direct-apply arms cover the reward-queue / auto cases.
/// Bead `sts2-rs-70anj.4`.
fn listener_kifuda(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::Kifuda) {
        return EffectResult::NotImplemented;
    }
    // C# `Kifuda.AfterObtained` awaits an interactive
    // `CardSelectCmd.FromDeckForEnchantment` — surface the deck
    // card-select when the run is interactive.
    if try_queue_interactive_enchant(
        rs,
        sts2_core::cards::EnchantmentId::Adroit,
        KIFUDA_ADROIT,
        KIFUDA_CARDS,
    ) {
        return EffectResult::Resolved;
    }
    // Bead `sts2-rs-g5ksv`: Python `relics/shop_event.py:428`
    // (Kifuda.after_obtained) — `min_count=0` (skippable).
    if rs.defer_followup_rewards {
        let candidates =
            enchantable_candidate_card_ids(rs, sts2_core::cards::EnchantmentId::Adroit);
        rs.offer_enchant_cards_reward(
            sts2_core::cards::EnchantmentId::Adroit,
            KIFUDA_ADROIT,
            KIFUDA_CARDS,
            candidates,
            Some(0),
        );
        return EffectResult::Resolved;
    }
    let _ = enchant_eligible_deck_cards(
        rs,
        sts2_core::cards::EnchantmentId::Adroit,
        KIFUDA_ADROIT,
        KIFUDA_CARDS,
    );
    EffectResult::Resolved
}

/// `PunchDagger::after_obtained` — enchant 1 eligible deck card with
/// `Momentum(PUNCH_DAGGER_MOMENTUM)` (Python
/// `shop_event.py::PunchDagger.after_obtained`,
/// `enchant_selected_cards("Momentum", 5, 1, cards=candidates)`).
/// Count hard-coded to 1 in Python (no separate const).
/// Interactive arm (`interactive_card_transform`) surfaces a deck
/// card-select (C# `CardSelectCmd.FromDeckForEnchantment`); the defer
/// and direct-apply arms cover the reward-queue / auto cases.
/// Bead `sts2-rs-70anj.4`.
fn listener_punch_dagger(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::PunchDagger) {
        return EffectResult::NotImplemented;
    }
    // C# `PunchDagger.AfterObtained` awaits an interactive
    // `CardSelectCmd.FromDeckForEnchantment` — surface the deck
    // card-select when the run is interactive.
    if try_queue_interactive_enchant(
        rs,
        sts2_core::cards::EnchantmentId::Momentum,
        PUNCH_DAGGER_MOMENTUM,
        1,
    ) {
        return EffectResult::Resolved;
    }
    // Bead `sts2-rs-g5ksv`: Python `relics/shop_event.py:587`
    // (PunchDagger.after_obtained) — no `min_count` (exact-count
    // semantics per `offer_enchant_cards_reward` default).
    if rs.defer_followup_rewards {
        let candidates =
            enchantable_candidate_card_ids(rs, sts2_core::cards::EnchantmentId::Momentum);
        rs.offer_enchant_cards_reward(
            sts2_core::cards::EnchantmentId::Momentum,
            PUNCH_DAGGER_MOMENTUM,
            1,
            candidates,
            None,
        );
        return EffectResult::Resolved;
    }
    let _ = enchant_eligible_deck_cards(
        rs,
        sts2_core::cards::EnchantmentId::Momentum,
        PUNCH_DAGGER_MOMENTUM,
        1,
    );
    EffectResult::Resolved
}

/// `BeautifulBracelet::after_obtained` — enchant up to
/// `BEAUTIFUL_BRACELET_CARDS` eligible deck cards with
/// `Swift(BEAUTIFUL_BRACELET_SWIFT)` (Python
/// `shop_event.py::BeautifulBracelet.after_obtained`,
/// `enchant_selected_cards("Swift", 3, 3, cards=candidates)`).
/// Interactive arm (`interactive_card_transform`) surfaces a deck
/// card-select (C# `CardSelectCmd.FromDeckForEnchantment`); the defer
/// and direct-apply arms cover the reward-queue / auto cases.
/// Bead `sts2-rs-70anj.4`.
fn listener_beautiful_bracelet(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::BeautifulBracelet) {
        return EffectResult::NotImplemented;
    }
    // C# `BeautifulBracelet.AfterObtained` awaits an interactive
    // `CardSelectCmd.FromDeckForEnchantment` — surface the deck
    // card-select when the run is interactive.
    if try_queue_interactive_enchant(
        rs,
        sts2_core::cards::EnchantmentId::Swift,
        BEAUTIFUL_BRACELET_SWIFT,
        BEAUTIFUL_BRACELET_CARDS,
    ) {
        return EffectResult::Resolved;
    }
    // Bead `sts2-rs-g5ksv`: Python `relics/shop_event.py:1030`
    // (BeautifulBracelet.after_obtained) — no `min_count` (exact).
    if rs.defer_followup_rewards {
        let candidates = enchantable_candidate_card_ids(rs, sts2_core::cards::EnchantmentId::Swift);
        rs.offer_enchant_cards_reward(
            sts2_core::cards::EnchantmentId::Swift,
            BEAUTIFUL_BRACELET_SWIFT,
            BEAUTIFUL_BRACELET_CARDS,
            candidates,
            None,
        );
        return EffectResult::Resolved;
    }
    let _ = enchant_eligible_deck_cards(
        rs,
        sts2_core::cards::EnchantmentId::Swift,
        BEAUTIFUL_BRACELET_SWIFT,
        BEAUTIFUL_BRACELET_CARDS,
    );
    EffectResult::Resolved
}

/// `RoyalStamp::after_obtained` — enchant 1 eligible deck card
/// (Attack or Skill, per `can_enchant_card` gate) with
/// `RoyallyApproved(1)` (Python `shop_event.py::RoyalStamp.after_obtained`,
/// `enchant_selected_cards("RoyallyApproved", 1, 1, cards=candidates)`).
/// Count hard-coded to 1 in Python (no separate const). Adds INNATE +
/// RETAIN to the card's effective keywords via
/// [`crate::enchantments::effective_keywords`]. Interactive arm
/// (`interactive_card_transform`) surfaces a deck card-select (C#
/// `CardSelectCmd.FromDeckForEnchantment`); defer + direct-apply
/// arms cover the reward-queue / auto cases. Bead `sts2-rs-70anj.4`.
fn listener_royal_stamp(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::RoyalStamp) {
        return EffectResult::NotImplemented;
    }
    // C# `RoyalStamp.AfterObtained` awaits an interactive
    // `CardSelectCmd.FromDeckForEnchantment` (pick 1 of the enchantable
    // cards) — surface that deck card-select when the run is interactive.
    if try_queue_interactive_enchant(rs, sts2_core::cards::EnchantmentId::RoyallyApproved, 1, 1) {
        return EffectResult::Resolved;
    }
    // Bead `sts2-rs-g5ksv`: Python `relics/shop_event.py:619`
    // (RoyalStamp.after_obtained) — no `min_count` (exact).
    if rs.defer_followup_rewards {
        let candidates =
            enchantable_candidate_card_ids(rs, sts2_core::cards::EnchantmentId::RoyallyApproved);
        rs.offer_enchant_cards_reward(
            sts2_core::cards::EnchantmentId::RoyallyApproved,
            1,
            1,
            candidates,
            None,
        );
        return EffectResult::Resolved;
    }
    let _ = enchant_eligible_deck_cards(rs, sts2_core::cards::EnchantmentId::RoyallyApproved, 1, 1);
    EffectResult::Resolved
}

/// `ElectricShrymp::after_obtained` — enchant 1 eligible Skill card
/// in `rs.master_deck` with `Imbued(1)` (Python
/// `shop_event.py::ElectricShrymp.after_obtained`,
/// `enchant_selected_cards("Imbued", 1, 1, cards=candidates)`).
/// Imbued is Skill-only per
/// [`crate::enchantments::can_enchant_card`]. Count + amount both
/// hard-coded to 1 in the Python call (no class const).
/// Interactive arm (`interactive_card_transform`) surfaces a deck
/// card-select (C# `CardSelectCmd.FromDeckForEnchantment`); the defer
/// and direct-apply arms cover the reward-queue / auto cases.
/// Bead `sts2-rs-70anj.4`.
fn listener_electric_shrymp(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::ElectricShrymp) {
        return EffectResult::NotImplemented;
    }
    // C# `ElectricShrymp.AfterObtained` awaits an interactive
    // `CardSelectCmd.FromDeckForEnchantment` — surface the deck
    // card-select when the run is interactive.
    if try_queue_interactive_enchant(rs, sts2_core::cards::EnchantmentId::Imbued, 1, 1) {
        return EffectResult::Resolved;
    }
    let _ = enchant_eligible_deck_cards(rs, sts2_core::cards::EnchantmentId::Imbued, 1, 1);
    EffectResult::Resolved
}

/// `RadiantPearl::before_side_turn_start` — on the round-1 player
/// side-turn start, generate 1 Luminesce card into the player's hand
/// (Python `shop_event.py::RadiantPearl.before_side_turn_start`:
/// `combat.create_card_in_hand(owner, "Luminesce")`). Luminesce is
/// `CardType::Skill`, cost 0, target Self_, EXHAUST+RETAIN — pinned
/// from `status_specs_generated::luminesce`. Bead `sts2-rs-70anj.4`.
fn listener_radiant_pearl(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if state.round_no() != 1 {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::RadiantPearl) {
        return EffectResult::NotImplemented;
    }
    let mut card = sts2_core::cards::Card::pod_defaults();
    card.card_id = sts2_core::cards::CardId::Luminesce;
    card.card_type = sts2_core::cards::CardType::Skill;
    card.cost = 0;
    card.base_damage = 0;
    card.base_block = 0;
    card.target_type = sts2_core::cards::TargetType::Self_;
    let _ = sts2_core::card_effects::spawn_card_into_hand(state, card);
    EffectResult::Resolved
}

/// `ToyBox::AfterObtained` — offer `TOY_BOX_RELICS = 4` relic rewards, each
/// stamped WAX. C# `MegaCrit.Sts2.Core.Models.Relics/ToyBox.cs:87-97`:
///
/// [decompiled C# omitted — see the cited path]
///
/// The wax flag rides the OFFER, not the pool: a skipped reward never
/// becomes wax. `RunState::accept_relic_reward` records the taken ones into
/// `rs.wax_relics`, and `run_bridge::exit_combat` melts one every
/// [`TOY_BOX_COMBATS`] combats. Bead `sts2-rs-70anj.4`.
///
/// Pool nuance: C# `PullNextRelicFromFront` takes the next relic off the
/// front of the shared bag; the Rust port rolls a rarity then pops from the
/// matching grab bag (the same surface every other multi-relic offer uses).
/// That is a pre-existing offer-composition difference, not a melt one.
fn listener_toy_box(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::ToyBox) {
        return EffectResult::NotImplemented;
    }
    rs.offer_random_relic_rewards_wax(TOY_BOX_RELICS, &[], true);
    EffectResult::Resolved
}

/// `PaelsClaw::after_obtained` — enchant every deck card with
/// `Goopy(1)` (Python `shop_event.py::PaelsClaw.after_obtained`:
/// `owner.enchant_all_cards("Goopy")`). Python's `enchant_all_cards`
/// blindly calls `card.add_enchantment` on every deck card with no
/// eligibility check, so the Rust port mirrors that — direct
/// `add_enchantment` per card, no `can_enchant_card` gate. Bead
/// `sts2-rs-70anj.4`.
fn listener_paels_claw(rs: &mut RunState) -> EffectResult {
    use sts2_core::cards::EnchantmentId;
    if rs.last_obtained_relic != Some(RelicId::PaelsClaw) {
        return EffectResult::NotImplemented;
    }
    for card in &mut rs.master_deck {
        card.add_enchantment(EnchantmentId::Goopy, 1);
    }
    EffectResult::Resolved
}

/// `VelvetChoker::after_card_played` — bump `relic.counter` once per
/// card played (Python `shop_event.py::VelvetChoker.after_card_played`:
/// `self._cards_this_turn += 1`). The per-turn cap is enforced by
/// `hooks::relic_should_play_blocked` (reads the counter and rejects play
/// when >= `VELVET_CHOKER_MAX_CARDS`). Bead `sts2-rs-70anj.4`.
fn listener_velvet_choker_card_played(
    state: &mut CombatState,
    _ctx: &CardEventCtx,
) -> EffectResult {
    let mut bumped = false;
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::VelvetChoker {
            relic.counter = relic.counter.saturating_add(1);
            bumped = true;
            break;
        }
    }
    if bumped {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `DiamondDiadem::before_turn_end` — at PLAYER turn end, if the player
/// played <=2 cards this turn, apply +1 DiamondDiadem power to the
/// player. Mirrors Python `shop_event.py::DiamondDiadem.before_turn_end`
/// (sts2-rl-agent@01c094d, line ~1165): `if side == PLAYER and
/// self._cards_this_turn <= CARD_THRESHOLD: owner.apply_power(
/// DIAMOND_DIADEM, 1)`. We use the engine's global
/// `count_player_cards_played_this_turn(None)` instead of a per-relic
/// counter; both have identical semantics under the
/// before_turn_end fire schedule (counter is reset on PLAYER
/// before_side_turn_start before the next-turn count begins).
/// Bead `sts2-rs-70anj.3.21`.
fn listener_diamond_diadem_before_turn_end(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::DiamondDiadem) {
        return EffectResult::NotImplemented;
    }
    const CARD_THRESHOLD: u32 = 2;
    if state.count_player_cards_played_this_turn(None) > CARD_THRESHOLD {
        return EffectResult::NotImplemented;
    }
    let Some(player_uid) = state.player.creature_uid else {
        return EffectResult::NotImplemented;
    };
    sts2_core::card_effects::apply_power_to_creature(
        state,
        player_uid,
        sts2_core::PowerKind::DiamondDiadem,
        1,
    );
    EffectResult::Resolved
}

/// `VelvetChoker::before_side_turn_start` — reset `relic.counter` on
/// PLAYER side (Python `shop_event.py::VelvetChoker.before_side_turn_start`:
/// `if side == PLAYER: self._cards_this_turn = 0`). Bead `sts2-rs-70anj.4`.
fn listener_velvet_choker_turn_reset(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    let mut reset = false;
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::VelvetChoker {
            relic.counter = 0;
            reset = true;
            break;
        }
    }
    if reset {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `Ectoplasm::should_gain_gold` — veto every positive gold gain
/// while the relic is owned (Python
/// `shop_event.py::Ectoplasm.should_gain_gold` returns `False`
/// unconditionally). Sets the `cancel` out-parameter to short-circuit
/// the dispatch loop. The +1 max-energy leg is already wired via
/// `hooks::modify_max_energy`. Bead `sts2-rs-70anj.4`.
fn listener_ectoplasm_should_gain_gold(
    rs: &mut RunState,
    _amount: i32,
    cancel: &mut bool,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::Ectoplasm) {
        return EffectResult::NotImplemented;
    }
    *cancel = true;
    EffectResult::Resolved
}

/// `Sozu::should_procure_potion` — veto every potion procurement
/// while the relic is owned (Python
/// `shop_event.py::Sozu.should_procure_potion` returns `False`
/// unconditionally). Bead `sts2-rs-70anj.3.2`.
fn listener_sozu_should_procure_potion(rs: &mut RunState, cancel: &mut bool) -> EffectResult {
    if !rs.owns_active_relic(RelicId::Sozu) {
        return EffectResult::NotImplemented;
    }
    *cancel = true;
    EffectResult::Resolved
}

/// `Driftwood::allow_card_reward_reroll` — return `True` unconditionally
/// (Python `shop_event.py::Driftwood.allow_card_reward_reroll`). Setting
/// `*allow = true` short-circuits the dispatcher; the reroll UI consumer
/// reads the bool to expose the action. Bead `sts2-rs-70anj.3.5`.
fn listener_driftwood_allow_reroll(rs: &mut RunState, allow: &mut bool) -> EffectResult {
    if !rs.owns_active_relic(RelicId::Driftwood) {
        return EffectResult::NotImplemented;
    }
    *allow = true;
    EffectResult::Resolved
}

/// `DollysMirror::after_obtained` — duplicate one random card from
/// the master deck (Python `shop_event.py::DollysMirror.after_obtained`
/// at line 222). The agent-path fallback Python takes is
/// `candidates[0].clone(...)`; the Rust port uses
/// `RunState::duplicate_card_from_deck` which samples a random card
/// via `rewards_rng` — see the helper docs for the divergence rationale.
/// Bead `sts2-rs-70anj.3.10`.
fn listener_dollys_mirror(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::DollysMirror) {
        return EffectResult::NotImplemented;
    }
    let _ = rs.duplicate_card_from_deck();
    EffectResult::Resolved
}

/// `ArcaneScroll::after_obtained` — add one random Rare card to the
/// master deck. C# `ArcaneScroll.AfterObtained` builds
/// `CardCreationOptions` over `Owner.Character.CardPool` (the **current
/// character's** pool, uniform odds, `c.Rarity == Rare`) — so the roll
/// must be filtered to `rs.character_id`, not the all-character union.
/// Passing `None` previously let a Silent run roll a Defect Rare (GH#30:
/// RP6 added `SUPERCRITICAL` where C# added `MALAISE`).
/// Bead `sts2-rs-70anj.3.15`; character-pool fix for GH#30.
fn listener_arcane_scroll(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::ArcaneScroll) {
        return EffectResult::NotImplemented;
    }
    let character_id = rs.character_id;
    let Some(card_id) = crate::cards::spec_index::random_card_from_rarity(
        rs,
        Some(character_id),
        crate::cards::CardRarity::Rare,
    ) else {
        return EffectResult::NotImplemented;
    };
    // Build through the registered card factory so spec stats are
    // populated (a bare `pod_defaults()` leaves base_damage/cost at the
    // POD defaults — see the transform-leg fix in this file, GH#30).
    let card = sts2_core::lookup_card_factory()
        .and_then(|f| f(card_id, false))
        .unwrap_or_else(|| {
            let mut c = sts2_core::Card::pod_defaults();
            c.card_id = card_id;
            c
        });
    rs.master_deck.push(card);
    EffectResult::Resolved
}

/// `StoneHumidifier::AfterRestSiteHeal` — `owner.gain_max_hp(5)` every time
/// a rest-site REST fires the heal hook, REGARDLESS of how much (if any) HP
/// was restored. C# `HealRestSiteOption.ExecuteRestSiteHeal`
/// (`HealRestSiteOption.cs:102-109`) heals then UNCONDITIONALLY awaits
/// `Hook.AfterRestSiteHeal`, whose signature (`Hook.cs:772-776`) carries NO
/// heal amount; `StoneHumidifier.AfterRestSiteHeal` (`StoneHumidifier.cs:18-25`)
/// then grants +5 MaxHp (which itself heals +5). So a campfire rest at full
/// HP still grants the MaxHp. The old `healed <= 0` guard came from the Python
/// pin (`shop_event.py:3431` passes `healed=amount`), which drifts from the
/// amount-agnostic C# hook — the C# decomp wins. Bead `sts2-rs-RP21`
/// (was `sts2-rs-70anj.3.17`). The dispatch site `rest_site_heal`
/// (`runstate.rs:2415-2417`) fires the channel even at amount==0, so the
/// listener is reached at full HP.
fn listener_stone_humidifier(rs: &mut RunState, _healed: i32) -> EffectResult {
    if !rs.owns_active_relic(RelicId::StoneHumidifier) {
        return EffectResult::NotImplemented;
    }
    rs.gain_max_hp(STONE_HUMIDIFIER_MAX_HP);
    EffectResult::Resolved
}

/// `MeatCleaver::modify_rest_site_options` — append a `Cook` option if
/// the player owns MeatCleaver and Cook isn't already in the list.
/// Python `shop_event.py::MeatCleaver.modify_rest_site_options` at pin
/// `01c094d:sts2_env/relics/shop_event.py:2442`.
///
/// `enabled` is `true` iff the deck has at least
/// [`COOK_OPTION_CARD_COUNT`] removable cards (matches Python's
/// `has_enough_removable=removable_count >= CARD_COUNT`). Bead
/// `sts2-rs-ekf4j`.
fn listener_meat_cleaver_rest_site_options(
    rs: &RunState,
    options: &mut Vec<RestSiteOption>,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::MeatCleaver) {
        return EffectResult::NotImplemented;
    }
    if options.iter().any(|opt| opt.option_id() == "COOK") {
        return EffectResult::NotImplemented;
    }
    let enabled = rs.removable_deck_card_count() >= COOK_OPTION_CARD_COUNT;
    options.push(RestSiteOption::Cook { enabled });
    EffectResult::Resolved
}

/// `MiniatureTent::should_disable_remaining_rest_site_options` —
/// returns `Some(false)` to override the default disable-after-pick UX
/// so the player can take multiple rest-site actions per visit.
/// Python `shop_event.py::MiniatureTent.should_disable_remaining_rest_site_options`
/// at pin `01c094d:sts2_env/relics/shop_event.py:513`. Bead
/// `sts2-rs-70anj.3.4`.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn listener_miniature_tent_disable_remaining(
    rs: &RunState,
    _chosen: RestSiteOption,
) -> Option<bool> {
    if !rs.owns_active_relic(RelicId::MiniatureTent) {
        return None;
    }
    Some(false)
}

/// `Astrolabe::after_obtained` — transform 3 random non-curse cards
/// from the master deck into random Rare cards, then upgrade them.
/// Python `shop_event.py::Astrolabe.after_obtained` at pin
/// `01c094d:sts2_env/relics/shop_event.py:1002` calls
/// `owner.transform_and_upgrade_cards(self.CARDS, cards=candidates)`
/// (CARDS = 3); without the headless flag the path delegates to
/// `_transform_and_upgrade_selected` over the candidate list head.
///
/// Self-gated on [`RunState::last_obtained_relic`] so the listener
/// fires exactly once — on Astrolabe's own pickup — rather than on
/// every subsequent relic acquisition (the `after_obtained` channel
/// dispatcher walks ALL listeners on every pickup; see
/// [`crate::run_effects::dispatch_after_obtained`] doc-comment).
///
/// Divergence from Python: the replacement pool collapses to a
/// uniform Rare-rarity sample (`character = None`) for every chosen
/// slot, because Rust has no `CardId -> CardSpec` lookup on the run
/// side yet — Python preserves the original rarity bucket. Same
/// shortcut used by `listener_arcane_scroll` / `listener_dusty_tome`.
/// Bead `sts2-rs-70anj.3.9`.
fn listener_astrolabe(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::Astrolabe) {
        return EffectResult::NotImplemented;
    }
    if rs.master_deck.is_empty() {
        return EffectResult::Resolved;
    }
    // Candidate set: deck indices whose cards are transformable.
    // Python skips curses/statuses; the Rust master_deck does not
    // currently hold curses, so the filter degrades to "all indices"
    // — but keep the structure so adding a real filter later is local.
    let mut candidates: Vec<usize> = (0..rs.master_deck.len()).collect();
    let take = ASTROLABE_CARDS.min(candidates.len());
    // Partial Fisher-Yates draws `take` distinct deck indices using
    // the rewards-RNG stream (same convention as `upgrade_random_cards`).
    {
        // Bead `sts2-rs-hm2eb` (943w7 Stage 3): dotnet `RunRngSet` reward
        // stream, not the legacy Xoshiro `rewards_rng()` shim.
        let rng = &mut rs.rng.rewards;
        for i in 0..take {
            let len_remaining = (candidates.len() - i) as i32;
            let j = i + rng.next_int_exclusive(0, len_remaining) as usize;
            candidates.swap(i, j);
        }
    }
    let chosen: Vec<usize> = candidates.iter().take(take).copied().collect();
    for deck_idx in chosen {
        let Some(new_id) = crate::cards::spec_index::random_card_from_rarity(
            rs,
            None,
            crate::cards::CardRarity::Rare,
        ) else {
            continue;
        };
        let mut fresh = sts2_core::Card::pod_defaults();
        fresh.card_id = new_id;
        fresh.upgrade();
        rs.master_deck[deck_idx] = fresh;
    }
    EffectResult::Resolved
}

/// `Toolbox::before_hand_draw` — once per combat on round 1, open a
/// 3-option colorless `AddGeneratedToHand` choice. Mirrors Python
/// `shop_event.py::Toolbox.before_hand_draw` at
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py:702`.
///
/// Per-combat latch lives on the Toolbox `Relic.counter` (0 = unused,
/// 1 = fired). Reset to 0 by [`listener_toolbox_reset`] on
/// `before_combat_start`. Sampling uses `state.rng` (combat RNG)
/// since `rs.rewards_rng` is run-side and the listener has only
/// `&mut CombatState`. Bead `sts2-rs-70anj.3.13.tb`.
fn listener_toolbox_before_hand_draw(state: &mut CombatState) -> EffectResult {
    use sts2_core::cards::CardId;
    use sts2_core::{ChoiceKind, ChoiceOption};

    if state.round != 1 {
        return EffectResult::NotImplemented;
    }
    // Locate the Toolbox relic + check the once-per-combat latch.
    let Some(relic_idx) = state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::Toolbox)
    else {
        return EffectResult::NotImplemented;
    };
    if state.relics[relic_idx].counter != 0 {
        return EffectResult::NotImplemented;
    }
    // Sample TOOLBOX_CARDS distinct colorless cards using the combat
    // RNG. We don't have a `RunState` ref here, so we inline the
    // partial Fisher-Yates over a fresh pool snapshot.
    let pool: Vec<CardId> = crate::cards::spec_index::COLORLESS_SPECS
        .iter()
        .map(|f| f().id)
        .collect();
    if pool.is_empty() {
        return EffectResult::NotImplemented;
    }
    let mut candidates = pool;
    let take = TOOLBOX_CARDS.min(candidates.len());
    for i in 0..take {
        let len_remaining = candidates.len() - i;
        let j = i + (state.rng.next_int(0, i32::MAX) as usize) % len_remaining;
        candidates.swap(i, j);
    }
    let options: Vec<ChoiceOption> = candidates
        .into_iter()
        .take(take)
        .map(|card_id| ChoiceOption {
            card_id,
            upgraded: false,
            card_uid: None,
            source_pile: None,
        })
        .collect();
    state.request_card_choice(ChoiceKind::AddGeneratedToHand, options, 1, 1, false);
    state.relics[relic_idx].counter = 1;
    EffectResult::Resolved
}

/// `Toolbox::before_combat_start` / `after_combat_end` — reset the
/// per-combat latch on `Relic.counter` so the round-1 offer fires
/// once per combat.
fn listener_toolbox_reset(state: &mut CombatState) -> EffectResult {
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::Toolbox {
            relic.counter = 0;
            return EffectResult::Resolved;
        }
    }
    EffectResult::NotImplemented
}

/// `ChoicesParadox::after_player_turn_start` — once per combat on
/// round 1, open a 5-option retain-tagged character `AddGeneratedRetainedToHand`
/// choice. Mirrors Python `shop_event.py::ChoicesParadox.after_player_turn_start`
/// at `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py:1340`.
///
/// Routed through the Rust `after_side_turn_start(Side::Player)` channel
/// — Python's `after_player_turn_start` is the player-side specialization
/// of the general after-turn-start fire. Per-combat latch on Relic.counter.
///
/// Divergence: Python applies the `retain` keyword (persistent across
/// turns); the Rust port pushes the resolved uid into
/// `player.retain_this_turn` (one-turn flag) at resolver time. If the
/// player doesn't play the card on round 1, it discards at end-of-turn.
/// Bead `sts2-rs-70anj.3.13.cp`.
fn listener_choices_paradox(state: &mut CombatState, side: Side) -> EffectResult {
    use sts2_core::cards::CardId;
    use sts2_core::{ChoiceKind, ChoiceOption};

    if side != Side::Player || state.round != 1 {
        return EffectResult::NotImplemented;
    }
    let Some(relic_idx) = state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::ChoicesParadox)
    else {
        return EffectResult::NotImplemented;
    };
    if state.relics[relic_idx].counter != 0 {
        return EffectResult::NotImplemented;
    }
    let Some(character) = state.player.character else {
        return EffectResult::NotImplemented;
    };
    let specs = crate::cards::spec_index::specs_for_character(character);
    let pool: Vec<CardId> = specs.iter().map(|f| f().id).collect();
    if pool.is_empty() {
        return EffectResult::NotImplemented;
    }
    let mut candidates = pool;
    let take = CHOICES_PARADOX_CARDS.min(candidates.len());
    for i in 0..take {
        let len_remaining = candidates.len() - i;
        let j = i + (state.rng.next_int(0, i32::MAX) as usize) % len_remaining;
        candidates.swap(i, j);
    }
    let options: Vec<ChoiceOption> = candidates
        .into_iter()
        .take(take)
        .map(|card_id| ChoiceOption {
            card_id,
            upgraded: false,
            card_uid: None,
            source_pile: None,
        })
        .collect();
    state.request_card_choice(ChoiceKind::AddGeneratedRetainedToHand, options, 1, 1, false);
    state.relics[relic_idx].counter = 1;
    EffectResult::Resolved
}

/// `ChoicesParadox::before_combat_start` — reset per-combat latch.
fn listener_choices_paradox_reset(state: &mut CombatState) -> EffectResult {
    for relic in &mut state.relics {
        if relic.is_melted {
            continue;
        }
        if relic.kind == RelicKind::ChoicesParadox {
            relic.counter = 0;
            return EffectResult::Resolved;
        }
    }
    EffectResult::NotImplemented
}

/// `DustyTome::after_obtained` — add 1 random Ancient-rarity upgraded
/// card to the master deck (Python `shop_event.py::DustyTome` at line
/// 1230: "Add 1 random Ancient-rarity upgraded card to deck"). The
/// Python pool restricts to a Transcendence-cards subset
/// (Break/Suppress/Protector/MeteorShower/Quadcast); our pool helper
/// returns the Ancient-rarity intersection of the Ironclad/Silent/
/// Defect slices (3 of 5: Break, Suppress, Quadcast — Protector and
/// MeteorShower live in the Necrobinder/Regent slices not yet
/// generated). Bead `sts2-rs-70anj.3.15`.
fn listener_dusty_tome(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::DustyTome) {
        return EffectResult::NotImplemented;
    }
    // C# `DustyTome.AfterObtained` (`DustyTome.cs:58`) adds the card chosen by
    // `SetupForPlayer` at OPTION-GENERATION time — it does NOT roll again here.
    // When DustyTome was offered through the normal Darv path, the
    // `setup_boss_relic_ancient` callback already consumed the `Rewards` read
    // and stashed the card; consume it (no second read). Fall back to a fresh
    // roll only when no setup card is present (e.g. a direct grant path that
    // never ran option setup, or a unit test exercising the listener alone).
    let card_id = if let Some(id) = rs.dusty_tome_setup_card.take() {
        id
    } else {
        let Some(id) = crate::cards::spec_index::random_card_from_rarity(
            rs,
            None,
            crate::cards::CardRarity::Ancient,
        ) else {
            return EffectResult::NotImplemented;
        };
        id
    };
    let mut card = sts2_core::Card::pod_defaults();
    card.card_id = card_id;
    card.upgraded = true;
    rs.master_deck.push(card);
    EffectResult::Resolved
}

const SERE_TALON_CURSES: usize = 2;

/// `SereTalon::after_obtained` — add 2 random Curse cards to the deck
/// (Python `shop_event.py::SereTalon` at line 2544: "Add 2 random
/// curses and 3 Wish cards"). The 3 Wish cards leg is deferred —
/// Wish is a special card not yet ported. Bead `sts2-rs-70anj.3.15`.
fn listener_sere_talon(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::SereTalon) {
        return EffectResult::NotImplemented;
    }
    for _ in 0..SERE_TALON_CURSES {
        let Some(card_id) = crate::cards::spec_index::random_curse_card(rs) else {
            return EffectResult::NotImplemented;
        };
        let mut card = sts2_core::Card::pod_defaults();
        card.card_id = card_id;
        rs.master_deck.push(card);
    }
    EffectResult::Resolved
}

/// `PaelsGrowth::after_obtained` — enchant one eligible deck card
/// with `Clone(4)` (Python `shop_event.py::PaelsGrowth.after_obtained`:
/// `owner.enchant_selected_cards("Clone", 4, 1)`). The rest-site
/// `CloneOption` leg is deferred pending a rest-site options channel.
/// Bead `sts2-rs-70anj.4`.
fn listener_paels_growth(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::PaelsGrowth) {
        return EffectResult::NotImplemented;
    }
    let _ = enchant_eligible_deck_cards(rs, sts2_core::cards::EnchantmentId::Clone, 4, 1);
    EffectResult::Resolved
}

/// `WingCharm::modify_card_reward_options_late` — pick one random
/// option from the generated card-reward list and add `Swift(1)`
/// (Python `shop_event.py::WingCharm.modify_card_reward_options_late`,
/// `SWIFT = 1`). Index draw uses the dotnet `rs.rng.rewards` stream
/// (bead `sts2-rs-hm2eb`; was the legacy Xoshiro `rewards_rng()` shim).
/// Same channel as
/// Glitter / FresnelLens. Bead `sts2-rs-70anj.4`.
fn listener_wing_charm_card_reward(
    rs: &mut RunState,
    cards: &mut Vec<sts2_core::cards::Card>,
) -> EffectResult {
    use sts2_core::cards::EnchantmentId;
    if !rs.owns_active_relic(RelicId::WingCharm) {
        return EffectResult::NotImplemented;
    }
    if cards.is_empty() {
        return EffectResult::NotImplemented;
    }
    // Bead `sts2-rs-hm2eb` (943w7 Stage 3): dotnet `RunRngSet` reward
    // stream, not the legacy Xoshiro `rewards_rng()` shim.
    let idx = rs.rng.rewards.next_int_exclusive(0, cards.len() as i32) as usize;
    cards[idx].add_enchantment(EnchantmentId::Swift, WING_CHARM_SWIFT);
    EffectResult::Resolved
}

/// `BiiigHug::after_obtained` — remove `BIIIG_HUG_CARDS = 4` cards
/// from `rs.master_deck` (Python `shop_event.py::BiiigHug.after_obtained`:
/// `owner.remove_cards_from_deck(4)`). Routes through
/// [`RunState::remove_cards_from_deck`], which mirrors Python's
/// head-N drain (no per-card removable filter at this pin — no Quest
/// type exists yet, so the candidate set is the deck verbatim).
/// Direct-apply arm only — the `enable_deck_choice_requests`
/// interactive arm lands later with umbrella `sts2-rs-01o`.
/// Bead `sts2-rs-70anj.4`.
fn listener_biiig_hug_remove(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::BiiigHug) {
        return EffectResult::NotImplemented;
    }
    let _ = rs.remove_cards_from_deck(BIIIG_HUG_CARDS);
    EffectResult::Resolved
}

/// `BiiigHug::after_shuffle` — every draw-pile shuffle while the
/// relic is owned adds one `Soot` card to the draw pile at a RANDOM
/// depth. C# `BiiigHug.AfterShuffle` (`BiiigHug.cs:30-40`):
/// [decompiled C# omitted — see the cited path]
/// `CardPilePosition.Random` resolves to
/// `AddInternal(card, Rng.Shuffle.NextInt(Cards.Count + 1))`
/// (`CardPileCmd.cs:511-515`), so the insert lands at a uniform depth AND
/// consumes exactly one Shuffle-stream draw. The prior top-insert
/// (`add_card_to_draw_pile`) diverged on DRAW_ORDER, consumed zero Shuffle
/// draws (desyncing every later draw/reshuffle in the combat), skipped the
/// `AfterCardGeneratedForCombat` cascade that `AddGeneratedCardToCombat`
/// fires, and hand-rolled a Soot POD that dropped the spec's tags and the
/// `GeneratedInCombat` marker. Same shape and same fix as
/// `TeaOfDiscourtesy` (bead `sts2-rs-n4sa4`). `Soot` is pinned from
/// `sts2-content::cards::status_specs_generated::soot`. Beads
/// `sts2-rs-70anj.4`, `sts2-rs-lfdaa`.
fn listener_biiig_hug_shuffle(state: &mut CombatState, _ctx: &ShuffleCtx) -> EffectResult {
    if !state.relics.owns(RelicKind::BiiigHug) {
        return EffectResult::NotImplemented;
    }
    let soot = crate::cards::status::card_from_spec(&crate::cards::status_specs_generated::soot());
    let _ = sts2_core::card_effects::spawn_card_into_draw_pile_random_position(state, soot);
    EffectResult::Resolved
}

/// `NinjaScroll::before_side_turn_start` — on the round-1 player
/// side-turn start, generate `NINJA_SCROLL_SHIVS = 3` Shivs into the
/// player's hand (Python `shop_event.py::NinjaScroll.before_side_turn_start`:
/// `combat.create_shivs_in_hand(owner, SHIVS = 3)` /
/// `add_shivs_to_hand` fallback). Each Shiv is `Attack`, cost 0,
/// damage 4, Enemy target, EXHAUST — pinned from
/// `sts2-content::cards::status_specs_generated::shiv`. Routes through
/// [`crate::card_effects::spawn_card_into_hand`] so hand-cap overflow
/// (>10 cards) sends Shivs to discard instead of crashing. Bead
/// `sts2-rs-70anj.4`.
fn listener_ninja_scroll(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if state.round_no() != 1 {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::NinjaScroll) {
        return EffectResult::NotImplemented;
    }
    for _ in 0..NINJA_SCROLL_SHIVS {
        let mut card = sts2_core::cards::Card::pod_defaults();
        card.card_id = sts2_core::cards::CardId::Shiv;
        card.card_type = sts2_core::cards::CardType::Attack;
        card.cost = 0;
        card.base_damage = 4;
        card.base_block = 0;
        card.target_type = sts2_core::cards::TargetType::Enemy;
        let _ = sts2_core::card_effects::spawn_card_into_hand(state, card);
    }
    EffectResult::Resolved
}

/// `TriBoomerang::after_obtained` — enchant up to `TRI_BOOMERANG_CARDS`
/// eligible Attack cards with `Instinct(1)` (Python
/// `shop_event.py::TriBoomerang.after_obtained`,
/// `enchant_selected_cards("Instinct", 1, 3, cards=candidates)`).
/// Instinct (C# `Instinct.cs`) is a 2× powered-attack damage multiplier with
/// no cost behavior. Direct-apply arm
/// only — the `defer_followup_rewards` reward-queue arm lands later
/// with umbrella `sts2-rs-01o`. Bead `sts2-rs-70anj.4`.
fn listener_tri_boomerang(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::TriBoomerang) {
        return EffectResult::NotImplemented;
    }
    let _ = enchant_eligible_deck_cards(
        rs,
        sts2_core::cards::EnchantmentId::Instinct,
        1,
        TRI_BOOMERANG_CARDS,
    );
    EffectResult::Resolved
}

/// `Storybook::after_obtained` — push one `BrightestFlame` card onto
/// `rs.master_deck` (Python `shop_event.py::Storybook.after_obtained`:
/// `owner.add_card_to_deck("BrightestFlame")`). Count hard-coded to 1
/// in the Python call (no class const). Same shape as NeowsTorment
/// (a49q3). Direct-apply arm only — the `defer_followup_rewards`
/// reward-queue arm lands later with umbrella `sts2-rs-01o`.
/// Bead `sts2-rs-70anj.4`.
fn listener_storybook(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::Storybook) {
        return EffectResult::NotImplemented;
    }
    push_deck_card(rs, sts2_core::cards::CardId::BrightestFlame);
    EffectResult::Resolved
}

/// `NeowsTorment::after_obtained` — push one `NeowsFury` card onto
/// `rs.master_deck` (Python `shop_event.py::NeowsTorment.after_obtained`:
/// `owner.add_card_to_deck("NeowsFury")`). Count hard-coded to 1 in
/// the Python call (no class const). Bead `sts2-rs-a49q3`.
fn listener_neows_torment(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::NeowsTorment) {
        return EffectResult::NotImplemented;
    }
    push_deck_card(rs, sts2_core::cards::CardId::NeowsFury);
    EffectResult::Resolved
}

/// `PaelsHorn::after_obtained` — push `PAELS_HORN_CARDS = 2` Relax
/// cards onto `rs.master_deck` (Python `shop_event.py::PaelsHorn.after_obtained`:
/// `for _ in range(2): owner.add_card_to_deck("Relax")`). Bead
/// `sts2-rs-a49q3`.
fn listener_paels_horn(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::PaelsHorn) {
        return EffectResult::NotImplemented;
    }
    for _ in 0..PAELS_HORN_CARDS {
        push_deck_card(rs, sts2_core::cards::CardId::Relax);
    }
    EffectResult::Resolved
}

/// `CursedPearl::after_obtained` — push one Greed curse onto
/// `rs.master_deck` AND grant `CURSED_PEARL_GOLD = 333` gold (Python
/// `shop_event.py::CursedPearl.after_obtained`: `owner.add_card_to_deck("Greed")`
/// then `owner.gold += self.GOLD`). The Python `should_gain_gold`
/// veto-chain is omitted here — at obtain time `rs.relics` contains
/// only CursedPearl, and CursedPearl itself does not implement
/// `should_gain_gold`, so the chain unconditionally accepts. Bead
/// `sts2-rs-a49q3`.
fn listener_cursed_pearl(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::CursedPearl) {
        return EffectResult::NotImplemented;
    }
    push_deck_card(rs, sts2_core::cards::CardId::Greed);
    rs.gain_gold(CURSED_PEARL_GOLD);
    EffectResult::Resolved
}

/// `BloodSoakedRose::after_obtained` — push one Enthralled curse
/// onto `rs.master_deck` (Python `shop_event.py::BloodSoakedRose.after_obtained`:
/// `owner.add_card_to_deck("Enthralled")`). The Python class also
/// implements `modify_max_energy` (+1 max energy) — that combat-side
/// modifier is deferred pending the `modify_max_energy` hook channel
/// (same gap as Bread / Ectoplasm / VelvetChoker / PhilosophersStone
/// energy legs). Bead `sts2-rs-a49q3`.
fn listener_blood_soaked_rose(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::BloodSoakedRose) {
        return EffectResult::NotImplemented;
    }
    push_deck_card(rs, sts2_core::cards::CardId::Enthralled);
    EffectResult::Resolved
}

/// `LeadPaperweight::after_obtained` — roll `LEAD_PAPERWEIGHT_CARDS = 2`
/// concrete Colorless options off `PlayerRng.Rewards` (per card:
/// base-odds rarity → `NextItem` → upgrade float; see
/// [`crate::cards::spec_index::roll_lead_paperweight_colorless_options`])
/// and queue a skippable choose-1 screen, mirroring C#
/// `LeadPaperweight.AfterObtained` (`LeadPaperweight.cs:19-37`):
/// `CardFactory.CreateForReward(owner, 2, ColorlessCardPool/Other/
/// RegularEncounter)` → `CardSelectCmd.FromChooseACardScreen(canSkip:
/// true)` → chosen card appended to the Deck tail. The run loop surfaces
/// the queue as an interactive `card_select` (min 0 / max 1).
///
/// Gate: `last_obtained_relic` (NOT `relics.contains`) — the
/// after-obtained channel dispatches EVERY registered listener on EVERY
/// relic obtain, so a contains-gate would re-roll the Rewards stream and
/// re-queue a phantom screen on each later pickup while owned (the old
/// sentinel-pusher had exactly that bug). Bead 6ynt.4.deck-add / GH #34.
fn listener_lead_paperweight(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::LeadPaperweight) {
        return EffectResult::NotImplemented;
    }
    let options = crate::cards::spec_index::roll_lead_paperweight_colorless_options(
        rs,
        LEAD_PAPERWEIGHT_CARDS,
    );
    if !options.is_empty() {
        rs.pending_card_selects
            .push(sts2_core::runstate::PendingCardSelect::from_options(
                options,
            ));
    }
    EffectResult::Resolved
}

/// `SeaGlass::after_obtained` — push `SEA_GLASS_CARDS = 15`
/// `Reward::Card(CardReward::empty())` sentinels (Python
/// `shop_event.py::SeaGlass.after_obtained`:
/// `owner.offer_custom_card_reward(option_count=15, character_ids=...)`).
/// The character-pool pinning + forced-rarity mix (5 COMMON, 5
/// UNCOMMON, 5 RARE) are deferred — `CardReward::empty()` has no
/// character-filter or rarity fields. Umbrella `sts2-rs-01o` will
/// materialize both at flush time. Bead `sts2-rs-qw9oe`.
fn listener_sea_glass(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::SeaGlass) {
        return EffectResult::NotImplemented;
    }
    for _ in 0..SEA_GLASS_CARDS {
        rs.pending_rewards.push(Reward::Card(CardReward::empty()));
    }
    EffectResult::Resolved
}

/// `YummyCookie::after_obtained` — C# `YummyCookie.cs`:
/// `CardSelectCmd.FromDeckForUpgrade(count 4)` → `CardCmd.Upgrade`. This is
/// a genuine PLAYER PICK (choose 4 cards to upgrade), so it correctly routes
/// through the interactive-aware [`RunState::upgrade_random_cards`], which
/// under `interactive_card_upgrade` queues a `pending_deck_upgrades`
/// card_select and otherwise auto-upgrades. (Contrast SandCastle, which is a
/// no-pick random `StableShuffle` upgrade.) Bead `sts2-rs-qw9oe`.
fn listener_yummy_cookie(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::YummyCookie) {
        return EffectResult::NotImplemented;
    }
    rs.upgrade_random_cards(YUMMY_COOKIE_CARDS);
    EffectResult::Resolved
}

/// `Pomander::after_obtained` — C# `Pomander.cs`:
/// `CardSelectCmd.FromDeckForUpgrade(count 1)` → `CardCmd.Upgrade`. A genuine
/// PLAYER PICK (choose 1 card to upgrade), so it correctly routes through the
/// interactive-aware [`RunState::upgrade_random_cards`] (queues a
/// `pending_deck_upgrades` card_select under `interactive_card_upgrade`).
/// Bead `sts2-rs-cvbxs`.
fn listener_pomander(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::Pomander) {
        return EffectResult::NotImplemented;
    }
    rs.upgrade_random_cards(1);
    EffectResult::Resolved
}

/// `PrecariousShears::after_obtained` — remove `PRECARIOUS_SHEARS_CARDS
/// = 2` cards from the deck, THEN take `PRECARIOUS_SHEARS_DAMAGE = 16`
/// HP damage. HP-loss leg landed in bead `sts2-rs-cvbxs`; the remove-
/// cards leg landed in bead `sts2-rs-t762n` once
/// `RunState::remove_cards_from_deck` shipped.
///
/// C# `PrecariousShears.AfterObtained` (`PrecariousShears.cs:24-31`)
/// orders the legs `await FromDeckForRemoval(2)` → `RemoveFromDeck` →
/// `Damage(16)` — the damage applies only AFTER the removal pick
/// resolves. In interactive mode the removal surfaces a `card_select`,
/// so the damage must be deferred to after the pick drains (else HP
/// drops a step early vs C#); the non-interactive auto-removal path keeps
/// the eager order since it removes synchronously. Bead `sts2-rs-1hiht`.
fn listener_precarious_shears(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::PrecariousShears) {
        return EffectResult::NotImplemented;
    }
    rs.remove_cards_from_deck(PRECARIOUS_SHEARS_CARDS);
    if rs.interactive_card_removal && rs.pending_deck_removals > 0 {
        // Removal is now a pending interactive `card_select`; defer the damage
        // until the batch resolves (drained in `do_remove_deck_card_at`).
        rs.pending_post_removal_hp_loss += PRECARIOUS_SHEARS_DAMAGE;
    } else {
        let _ = rs.lose_hp(PRECARIOUS_SHEARS_DAMAGE);
    }
    EffectResult::Resolved
}

/// `PreciseScissors::after_obtained` — remove 1 card from the deck
/// (Python `shop_event.py::PreciseScissors.after_obtained`: count
/// hard-coded to 1). Bead `sts2-rs-t762n`.
fn listener_precise_scissors(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::PreciseScissors) {
        return EffectResult::NotImplemented;
    }
    rs.remove_cards_from_deck(1);
    EffectResult::Resolved
}

/// `EmptyCage::after_obtained` — remove `EMPTY_CAGE_CARDS = 2` cards
/// from the deck (Python `shop_event.py::EmptyCage.after_obtained`).
/// Bead `sts2-rs-t762n`.
fn listener_empty_cage(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::EmptyCage) {
        return EffectResult::NotImplemented;
    }
    rs.remove_cards_from_deck(EMPTY_CAGE_CARDS);
    EffectResult::Resolved
}

/// `PreservedFog::after_obtained` — remove `PRESERVED_FOG_CARDS = 3`
/// cards from the deck AND push one `Folly` curse onto the deck
/// (C# `PreservedFog.cs:20-27` AfterObtained = CardsVar(3) deck
/// removals + AddCurseToDeck<Folly>). Bead `sts2-rs-t762n`
/// (initial port at 5 from Python pin); bead `sts2-rs-rn76k`
/// rebased to 3 per C#.
fn listener_preserved_fog(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::PreservedFog) {
        return EffectResult::NotImplemented;
    }
    rs.remove_cards_from_deck(PRESERVED_FOG_CARDS);
    push_deck_card(rs, sts2_core::cards::CardId::Folly);
    EffectResult::Resolved
}

/// `ArchaicTooth::after_obtained` — find the first card in
/// `rs.master_deck` whose `card_id` matches a starter→ancient mapping
/// entry and rewrite its `card_id` to the ancient version, preserving
/// upgrade + enchantment state (Python
/// `shop_event.py::ArchaicTooth.after_obtained`, lines 682-746 at pin
/// `01c094d`).
///
/// Mapping:
/// * `Bash` → `Break`
/// * `Neutralize` → `Suppress`
/// * `Unleash` → `Protector`
/// * `FallingStar` → `MeteorShower`
/// * `Dualcast` → `Quadcast`
///
/// Python's two-phase design (`setup_for_player` picks the starter
/// type at character creation; `after_obtained` consults that
/// recorded type first) is collapsed to a single pass here. The
/// recorded-starter-id path only differs from the fallback if the
/// deck has been mutated to hold multiple starter types between
/// character creation and relic obtain — that's not a path the
/// engine takes at the current pin. The fallback's "find first
/// matching card in deck order" gives the same result for normal
/// runs.
///
/// `preserve_upgrades=True` + `preserve_enchantments=True` in Python
/// are honored automatically by mutating `card.card_id` in place
/// without touching `card.upgraded` or `card.enchantments`.
/// Bead `sts2-rs-rvwas`.
fn listener_archaic_tooth(rs: &mut RunState) -> EffectResult {
    use sts2_core::cards::CardId;
    if rs.last_obtained_relic != Some(RelicId::ArchaicTooth) {
        return EffectResult::NotImplemented;
    }
    let mapping = |id: CardId| -> Option<CardId> {
        match id {
            CardId::Bash => Some(CardId::Break),
            CardId::Neutralize => Some(CardId::Suppress),
            CardId::Unleash => Some(CardId::Protector),
            CardId::FallingStar => Some(CardId::MeteorShower),
            CardId::Dualcast => Some(CardId::Quadcast),
            _ => None,
        }
    };
    for card in &mut rs.master_deck {
        if let Some(target) = mapping(card.card_id) {
            card.card_id = target;
            break;
        }
    }
    EffectResult::Resolved
}

/// `Claws::after_obtained` — rewrite the `card_id` of up to
/// `CLAWS_CARDS = 6` deck cards to `CardId::Maul`, preserving upgrade
/// + enchantment state (Python `shop_event.py::Claws.after_obtained`,
/// lines 1057-1072 at pin `01c094d`). Uses the in-place direct-
/// mutation pattern from ArchaicTooth so other POD fields stay
/// untouched. Walks the deck in order; only mutates cards that
/// aren't already Maul. Bead `sts2-rs-w9vf8`.
fn listener_claws(rs: &mut RunState) -> EffectResult {
    use sts2_core::cards::CardId;
    if rs.last_obtained_relic != Some(RelicId::Claws) {
        return EffectResult::NotImplemented;
    }
    let mut transformed = 0_usize;
    for card in &mut rs.master_deck {
        if transformed >= CLAWS_CARDS {
            break;
        }
        if card.card_id != CardId::Maul {
            // Rebuild the instance from Maul's spec instead of only flipping
            // `card_id`. A `Card` stores its combat fields per-instance
            // (`target_type`, `base_damage`, `cost`, `card_type`), so a bare
            // `card_id = Maul` left a malformed card carrying the ORIGINAL
            // card's stats: a non-Enemy source (e.g. a Defend, target_type
            // None) became a Maul that `play_card` dispatched untargeted →
            // `play_maul`'s `target.expect` panicked (bead sts2-rs-scds8), and
            // a 0-damage source dealt 0 even when played correctly. Preserve
            // only upgrade + enchantment state, matching the Python pin
            // (`shop_event.py::Claws.after_obtained`).
            if let Some(mut maul) =
                crate::cards::spec_index::card_pod_from_id(CardId::Maul, card.upgraded)
            {
                maul.uid = card.uid;
                // Move (not clone) — `*card` is overwritten immediately below.
                maul.enchantments = std::mem::take(&mut card.enchantments);
                *card = maul;
                transformed += 1;
            }
        }
    }
    EffectResult::Resolved
}

/// `NutritiousSoup::after_obtained` — apply the `TezcatarasEmber`
/// static enchantment to every basic-Strike card in the deck (Python
/// `shop_event.py::NutritiousSoup.after_obtained`:
/// `owner.enchant_basic_strikes("TezcatarasEmber")`). Static
/// enchantments use `set_enchantment` (not `add`) so the call is
/// idempotent. Basic-Strike CardIds across all five characters are
/// enumerated explicitly — same convention as the deck-builder /
/// damage tests. Bead `sts2-rs-w9vf8`.
fn listener_nutritious_soup(rs: &mut RunState) -> EffectResult {
    use sts2_core::cards::{CardId, EnchantmentId};
    if rs.last_obtained_relic != Some(RelicId::NutritiousSoup) {
        return EffectResult::NotImplemented;
    }
    let is_basic_strike = |id: CardId| {
        matches!(
            id,
            CardId::StrikeIronclad
                | CardId::StrikeSilent
                | CardId::StrikeDefect
                | CardId::StrikeRegent
                | CardId::StrikeNecrobinder
        )
    };
    for card in &mut rs.master_deck {
        if is_basic_strike(card.card_id) {
            card.set_enchantment(EnchantmentId::TezcatarasEmber, 1);
        }
    }
    EffectResult::Resolved
}

/// `Glitter::modify_card_reward_options_late` — add `Glam = 1` to
/// every option in the card-reward list as it's generated (Python
/// `shop_event.py::Glitter.modify_card_reward_options_late`, lines
/// 1605-1621 at pin `01c094d`). Same channel + idiom as FresnelLens
/// (`Nimble`) and the rare-tier eggs (rarity-specific upgrades).
/// Listener iterates `cards: &mut Vec<Card>` and calls
/// `add_enchantment(Glam, 1)` per option. Bead `sts2-rs-5xg1w`.
fn listener_glitter_card_reward(
    rs: &mut RunState,
    cards: &mut Vec<sts2_core::cards::Card>,
) -> EffectResult {
    use sts2_core::cards::EnchantmentId;
    if !rs.owns_active_relic(RelicId::Glitter) {
        return EffectResult::NotImplemented;
    }
    if cards.is_empty() {
        return EffectResult::NotImplemented;
    }
    for card in cards {
        card.add_enchantment(EnchantmentId::Glam, 1);
    }
    EffectResult::Resolved
}

/// `AlchemicalCoffer::after_obtained` — gain 4 potion slots, then fill
/// them with 4 random out-of-combat potions. Mirrors C#
/// `MegaCrit.Sts2.Core.Models.Relics/AlchemicalCoffer.cs:20-29`
/// (`AfterObtained`): `GainMaxPotionCount(PotionSlots=4)` THEN
/// `PotionFactory.CreateRandomPotionsOutOfCombat(owner, 4,
/// RunState.Rng.CombatPotionGeneration)` + a `PotionCmd.TryToProcure`
/// loop into the new slots. The roll uses the `combat_potion_generation`
/// stream to match C#'s `Rng.CombatPotionGeneration` so the cursor stays
/// faithful. Closes the previously-deferred fill leg (bead
/// `sts2-rs-cvbxs`): the run-side procure primitives
/// (`create_random_potions_out_of_combat` + `add_potion_to_belt`) now
/// exist.
fn listener_alchemical_coffer(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::AlchemicalCoffer) {
        return EffectResult::NotImplemented;
    }
    rs.gain_potion_slots(ALCHEMICAL_COFFER_POTION_SLOTS);
    let kinds = crate::potions::all::create_random_potions_out_of_combat(
        &mut rs.rng.combat_potion_generation,
        rs.character_id,
        ALCHEMICAL_COFFER_POTION_SLOTS as usize,
    );
    for kind in kinds {
        // Direct `PotionCmd.TryToProcure` loop, NOT a reward screen: C#
        // drops on TooFull with no discard-to-claim offer, so count the
        // overflow but never queue a swap (GH #252).
        if !rs.add_potion_to_belt(kind) {
            rs.potions_overflow_dropped += 1;
        }
    }
    EffectResult::Resolved
}

/// `NewLeaf::after_obtained` — open a transform-1-card prompt on
/// the run-side master deck. Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::NewLeaf`:
/// ```text
/// candidates = owner.transformable_deck_cards()
/// if owner.run_state.enable_deck_choice_requests:
///     owner.offer_transform_cards_reward(1, cards=candidates)
///     return
/// owner.transform_cards(1, cards=candidates)
/// ```
///
/// **Port stance (bead `sts2-rs-ytiok`, 70anj.4.10; interactive surface
/// 6ynt.4.deck-add / GH #34):** calls
/// [`RunState::offer_transform_cards_reward`] with `count = 1`. Under
/// `interactive_card_transform` (workbench / run_parity lockstep) that
/// queues [`RunState::pending_deck_transforms`] picks the run loop
/// surfaces as a deck `card_select`, with the replacement rolled at
/// resolve time off `Rng.Niche` (C# `NewLeaf.AfterObtained` →
/// `CardSelectCmd.FromDeckForTransformation` →
/// `CardCmd.TransformToRandom(item, RunState.Rng.Niche)`) via the
/// `roll_transform_replacement` content callback. The legacy default
/// opens the run-level `pending_choice` prompt (bead `sts2-rs-aw73x`).
///
/// **Parity drift:** the Rust `offer_transform_cards_reward` opens a
/// prompt over the **entire** `master_deck`, whereas Python filters
/// to `transformable_deck_cards()` (excludes Quest cards + non-
/// removable cards). At this pin Quest/curse handling isn't deeply
/// modelled in the POD Card, so the over-offer is a temporary
/// divergence pinned by [`tests::new_leaf_opens_transform_prompt`].
/// Ownership-gated via `rs.relics.contains` per the canonical
/// after_obtained pattern (LeesWaffle, GoldenPearl, …).
fn listener_new_leaf(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::NewLeaf) {
        return EffectResult::NotImplemented;
    }
    rs.offer_transform_cards_reward(1);
    EffectResult::Resolved
}

/// `FurCoat::after_obtained` — latch the obtain act-index into
/// [`RunState::fur_coat_act_index`]. Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::FurCoat.after_obtained`
/// also marks the current `run_state.map`; the Rust port can't
/// reach the act_map from an `after_obtained` listener (no
/// `&mut ActMap` in this dispatch signature), so the room-marking
/// runs through [`listener_fur_coat_modify_generated_map_late`]
/// instead. The listener idempotently keeps the first latched act
/// — once `Some`, never reassigns — so the act-gate on the late
/// listener stays stable even if the player drops & re-picks
/// FurCoat across acts in some future game-mode. Bead
/// `sts2-rs-eboqq`.
fn listener_fur_coat_after_obtained(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::FurCoat) {
        return EffectResult::NotImplemented;
    }
    if rs.fur_coat_act_index.is_none() {
        rs.fur_coat_act_index = Some(rs.act_index);
    }
    EffectResult::Resolved
}

/// `FurCoat::modify_generated_map_late` — pick `FUR_COAT_COMBATS = 7`
/// Monster/Elite rooms uniformly at random from the freshly-generated
/// act map and mark them via the run-side
/// [`sts2_core::map::ActMap::mark_room`] registry (bead
/// `sts2-rs-mxbai`). Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::FurCoat`,
/// lines 1971-2013.
///
/// Gates (matching Python's `_add_marked_rooms`):
/// 1. Owner-gate via `rs.owns_active_relic(RelicId::FurCoat)`.
/// 2. Act-gate: the late hook no-ops unless `act_index ==
///    rs.fur_coat_act_index`. Python's relic only marks rooms on
///    the obtain act, leaving subsequent acts unmodified.
/// 3. Idempotency-gate: if the act-map already carries 7
///    FurCoat-owned marks, the listener is a no-op (Python checks
///    `valid_existing` — that every prior coord still points at a
///    MONSTER/ELITE node; the Rust port treats "we already have 7"
///    as the closest equivalent until a `modify_generated_map`
///    rewrite changes node types, in which case the listener
///    re-rolls).
///
/// RNG: the Python pin derives a per-relic stream
/// (`Rng(seed + player_id + hash("FurCoat"))`). The Rust port uses
/// [`sts2_core::dotnet_rng::Rng::with_name`] with the same
/// `"FurCoat"` name so the named-stream seed-mixing matches; the
/// `player_id` term is omitted because the Rust port is
/// single-player at this pin. Sampling: shuffle the candidate
/// `room_points` and take the first 7 (Fisher-Yates via the dotnet
/// Rng's `shuffle` method — same shape Python uses).
///
/// Combat-side 1-HP clamp is deferred to a follow-up bead — it
/// needs a `CombatState` ↔ `ActMap` bridge that snapshots
/// `is_marked_by(current_coord, FurCoat)` at combat init. Bead
/// `sts2-rs-eboqq`.
fn listener_fur_coat_modify_generated_map_late(
    rs: &mut RunState,
    act_map: &mut sts2_core::map::ActMap,
    act_index: u8,
) -> EffectResult {
    use sts2_core::map::{MapCoord, MapPointType};

    if !rs.owns_active_relic(RelicId::FurCoat) {
        return EffectResult::NotImplemented;
    }
    if rs.fur_coat_act_index != Some(act_index) {
        return EffectResult::NotImplemented;
    }
    // Idempotency: already marked the full quota on this map?
    let existing_marks = act_map.marked_coords_for(RelicId::FurCoat).len();
    if existing_marks >= FUR_COAT_COMBATS {
        return EffectResult::Resolved;
    }
    let mut candidates: Vec<MapCoord> = act_map
        .room_points()
        .iter()
        .filter(|p| matches!(p.point_type, MapPointType::Monster | MapPointType::Elite))
        .map(|p| MapCoord::new(p.col(), p.row()))
        .collect();
    if candidates.is_empty() {
        return EffectResult::NotImplemented;
    }
    let mut rng = sts2_core::dotnet_rng::Rng::with_name(rs.rng.seed, "FurCoat");
    rng.shuffle(&mut candidates);
    let needed = FUR_COAT_COMBATS.saturating_sub(existing_marks);
    let to_mark: Vec<MapCoord> = candidates.into_iter().take(needed).collect();
    act_map.mark_rooms(to_mark, RelicId::FurCoat);
    EffectResult::Resolved
}

/// `PandorasBox::after_obtained` — Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::PandorasBox`:
/// ```python
/// def after_obtained(self, owner):
///     cards = owner.basic_strike_defend_cards()
///     ...
///     owner.transform_specific_cards(cards)
/// ```
/// Walks `rs.master_deck`, collects every index whose card matches
/// the character's basic Strike or Defend (via
/// [`crate::cards::spec_index::basic_strike_defend_cards`]), rolls one
/// replacement card per match from the character pool, and applies
/// the in-place swap through
/// [`sts2_core::runstate::RunState::transform_specific_cards`] (bead
/// `sts2-rs-ab8uo`).
///
/// **Parity approximation:** Python's `create_transform_card` samples
/// from an "original-card-specific" eligibility pool that varies per
/// transform source. The Rust port doesn't have that depth yet; we
/// approximate by sampling Common-rarity cards from the character
/// pool via [`crate::cards::spec_index::random_card_from_rarity`].
/// This matches the established sibling-relic convention (Astrolabe
/// at line 2498 uses the same `random_card_from_rarity(rs, _, Rare)`
/// shape). A full rarity-rolled / original-keyed sampler is a
/// follow-up beyond this body-fill bead.
///
/// The listener no-ops for any character returning `None` from
/// `basic_strike_defend_cards`; every character now has a pair, so that
/// branch is unreachable.
/// Pickup behaviour stays observable for Ironclad / Silent / Defect.
///
/// Owner-gated via `rs.relics.contains` per the canonical
/// after-obtained pattern (TouchOfOrobas / LeesWaffle / NewLeaf).
fn listener_pandoras_box(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::PandorasBox) {
        return EffectResult::NotImplemented;
    }
    let character_id = rs.character_id;
    let Some([strike_id, defend_id]) =
        crate::cards::spec_index::basic_strike_defend_cards(character_id)
    else {
        return EffectResult::NotImplemented;
    };
    let indices: Vec<usize> = rs
        .master_deck
        .iter()
        .enumerate()
        .filter(|(_, card)| card.card_id == strike_id || card.card_id == defend_id)
        .map(|(idx, _)| idx)
        .collect();
    if indices.is_empty() {
        return EffectResult::Resolved;
    }
    let mut replacements: Vec<sts2_core::Card> = Vec::with_capacity(indices.len());
    for _ in 0..indices.len() {
        let Some(new_id) = crate::cards::spec_index::random_card_from_rarity(
            rs,
            Some(character_id),
            crate::cards::CardRarity::Common,
        ) else {
            return EffectResult::NotImplemented;
        };
        // Spec-populated POD, not a blank `pod_defaults()` carrying only
        // `card_id`: the master-deck POD is the combat card's source, so a
        // stat-less transform replacement plays for zero damage / block
        // (GH#31, same defect fixed for LeafyPoultice). Transform
        // replacements are never upgraded.
        let Some(card) = crate::cards::spec_index::card_pod_from_id(new_id, false) else {
            return EffectResult::NotImplemented;
        };
        replacements.push(card);
    }
    let _ = rs.transform_specific_cards(&indices, replacements);
    EffectResult::Resolved
}

// ── SilverCrucible — bead `sts2-rs-535i7` (70anj.4 handoff) ────────────

/// `SilverCrucible::CARD_REWARDS` from Python pin
/// `shop_event.py::SilverCrucible`.
const SILVER_CRUCIBLE_CARD_REWARDS: i32 = 3;

/// Pack `(times_used, treasure_rooms_entered)` into a single i32 for
/// storage in `rs.relic_counters[SilverCrucible]`. Both counters are
/// bounded (CARD_REWARDS=3, treasure rooms per run ≤ a small
/// constant), so 8 bits per field is more than enough. Negative
/// inputs clamp to 0; overflow saturates at 0xFF (defensive).
const fn silver_crucible_pack(times_used: i32, treasure_rooms_entered: i32) -> i32 {
    let t = if times_used < 0 {
        0
    } else if times_used > 0xFF {
        0xFF
    } else {
        times_used
    };
    let r = if treasure_rooms_entered < 0 {
        0
    } else if treasure_rooms_entered > 0xFF {
        0xFF
    } else {
        treasure_rooms_entered
    };
    (r << 8) | t
}

const fn silver_crucible_times_used(packed: i32) -> i32 {
    packed & 0xFF
}

const fn silver_crucible_treasure_rooms_entered(packed: i32) -> i32 {
    (packed >> 8) & 0xFF
}

/// `SilverCrucible::after_room_entered` — Python pin: increment the
/// `_treasure_rooms_entered` counter on every `RoomType::Treasure`.
/// Owner-gated via `rs.relics.contains`.
fn listener_silver_crucible_after_room_entered(
    rs: &mut RunState,
    room_type: RoomType,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::SilverCrucible) {
        return EffectResult::NotImplemented;
    }
    if room_type != RoomType::Treasure {
        return EffectResult::NotImplemented;
    }
    let packed = rs
        .relic_counters
        .get(&RelicId::SilverCrucible)
        .copied()
        .unwrap_or(0);
    let new_packed = silver_crucible_pack(
        silver_crucible_times_used(packed),
        silver_crucible_treasure_rooms_entered(packed) + 1,
    );
    rs.relic_counters
        .insert(RelicId::SilverCrucible, new_packed);
    EffectResult::Resolved
}

/// `SilverCrucible::should_generate_treasure` — Python pin returns
/// `self._treasure_rooms_entered > 1`. Skips the first treasure
/// room. The `after_room_entered` listener increments the counter
/// BEFORE this check fires (Python's `_fire_modifiers_after_room_entered`
/// runs before the should-generate-treasure poll in `_enter_room`),
/// so:
/// - First treasure: counter becomes 1, `1 > 1` is false → SKIP.
/// - Second+: counter ≥ 2, `2 > 1` is true → don't skip.
///
/// Rust channel encoding (bead `sts2-rs-elcvh`): `Resolved` = SKIP,
/// `NotImplemented` = abstain.
fn listener_silver_crucible_should_generate_treasure(rs: &mut RunState) -> EffectResult {
    if !rs.owns_active_relic(RelicId::SilverCrucible) {
        return EffectResult::NotImplemented;
    }
    let packed = rs
        .relic_counters
        .get(&RelicId::SilverCrucible)
        .copied()
        .unwrap_or(0);
    if silver_crucible_treasure_rooms_entered(packed) > 1 {
        EffectResult::NotImplemented
    } else {
        EffectResult::Resolved
    }
}

/// `SilverCrucible::modify_card_reward_options_late` — pre-upgrade
/// every card option until `_times_used >= CARD_REWARDS`. Python
/// uses a per-reward `_silver_crucible_upgraded` latch to avoid
/// double-counting across multiple listener fires for the same
/// reward; the Rust channel has no per-reward identifier so the
/// listener increments `_times_used` once per dispatch (the call
/// convention is one dispatch per reward generation, so semantics
/// line up under that invariant).
fn listener_silver_crucible_modify_card_reward_options_late(
    rs: &mut RunState,
    cards: &mut Vec<sts2_core::Card>,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::SilverCrucible) {
        return EffectResult::NotImplemented;
    }
    let packed = rs
        .relic_counters
        .get(&RelicId::SilverCrucible)
        .copied()
        .unwrap_or(0);
    let times_used = silver_crucible_times_used(packed);
    if times_used >= SILVER_CRUCIBLE_CARD_REWARDS {
        return EffectResult::NotImplemented;
    }
    for card in cards.iter_mut() {
        let _ = card.upgrade();
    }
    let new_packed = silver_crucible_pack(
        times_used + 1,
        silver_crucible_treasure_rooms_entered(packed),
    );
    rs.relic_counters
        .insert(RelicId::SilverCrucible, new_packed);
    EffectResult::Resolved
}

// ── GoldenCompass — bead `sts2-rs-k2nuu` (70anj.4 handoff) ─────────────
//
// Captured-act sentinel in `rs.relic_counters[GoldenCompass]`:
// - 0 = not picked up yet (default for unset entries)
// - 1/2/3 = the `rs.act_index` at obtain time
//
// All three listeners gate on `rs.relics.contains(&GoldenCompass)`
// AND the captured-act match.

/// `GoldenCompass::after_obtained` — capture the current
/// `rs.act_index` into `rs.relic_counters[GoldenCompass]`. Python
/// pin also calls `run_state.generate_map()` to force immediate
/// re-roll of the current act's map; the Rust port doesn't have
/// that primitive yet (no `RunState::generate_map` — RunState
/// doesn't own an `ActMap` field at this pin) so the golden-path
/// swap activates on the next driver-side regeneration. Documented
/// divergence.
fn listener_golden_compass_after_obtained(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::GoldenCompass) {
        return EffectResult::NotImplemented;
    }
    rs.relic_counters
        .insert(RelicId::GoldenCompass, i32::from(rs.act_index));
    // Bead `sts2-rs-4aunm` closes the in-act regen gap that k2nuu
    // documented: pickup-time, force the current act's map to be
    // re-rolled so the `modify_generated_map` listener below can
    // swap in the golden-path layout immediately rather than
    // waiting for the next driver-side regen at act-advance.
    rs.generate_map();
    EffectResult::Resolved
}

/// `GoldenCompass::modify_generated_map` — when the freshly-rolled
/// map's act matches the captured act, overwrite the contents with
/// `generate_golden_path_map(1)`. Python pin returns a fresh map
/// object; the Rust channel signature is `&mut ActMap` so we
/// overwrite in place via `*act_map = ...`. Mirrors:
///
/// ```python
/// def modify_generated_map(self, owner, run_state, act_map, act_index):
///     if act_index != self._golden_path_act:
///         return act_map
///     from sts2_env.map.generator import generate_golden_path_map
///     return generate_golden_path_map(player_count=len(getattr(run_state, "players", (owner,))))
/// ```
///
/// `player_count` always passes 1 in the single-player Rust port.
fn listener_golden_compass_modify_generated_map(
    rs: &mut RunState,
    act_map: &mut sts2_core::map::ActMap,
    act_index: u8,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::GoldenCompass) {
        return EffectResult::NotImplemented;
    }
    let captured = rs
        .relic_counters
        .get(&RelicId::GoldenCompass)
        .copied()
        .unwrap_or(0);
    if captured == 0 || i32::from(act_index) != captured {
        return EffectResult::NotImplemented;
    }
    *act_map = sts2_core::map_generator::generate_golden_path_map(1);
    EffectResult::Resolved
}

/// `GoldenCompass::modify_unknown_map_point_room_types` — on the
/// captured golden-path act, force every `?` resolution to
/// `RoomType::Event`. Python pin:
///
/// ```python
/// def modify_unknown_map_point_room_types(self, owner, room_types):
///     run_state = getattr(owner, "run_state", None)
///     if run_state is None or run_state.current_act_index != self._golden_path_act:
///         return room_types
///     return {RoomType.EVENT}
/// ```
///
/// Rust channel mutates the set in place — clear it and insert the
/// single `Event` variant.
fn listener_golden_compass_modify_unknown_map_point_room_types(
    rs: &mut RunState,
    room_types: &mut std::collections::BTreeSet<RoomType>,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::GoldenCompass) {
        return EffectResult::NotImplemented;
    }
    let captured = rs
        .relic_counters
        .get(&RelicId::GoldenCompass)
        .copied()
        .unwrap_or(0);
    if captured == 0 || i32::from(rs.act_index) != captured {
        return EffectResult::NotImplemented;
    }
    room_types.clear();
    room_types.insert(RoomType::Event);
    EffectResult::Resolved
}

/// `Orrery::after_obtained` — push `CARDS = 5` `Reward::Card`
/// sentinels to `rs.pending_rewards` (Python
/// `shop_event.py::Orrery.after_obtained`: `for _ in range(self.CARDS):
/// owner.offer_card_reward()`). Each entry is an empty `CardReward`
/// placeholder pending umbrella sts2-rs-01o random-card rolling;
/// same convention as BlackStar / PrayerWheel / WhiteStar. Bead
/// `sts2-rs-psf9`.
fn listener_orrery(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::Orrery) {
        return EffectResult::NotImplemented;
    }
    for _ in 0..ORRERY_CARDS {
        rs.pending_rewards.push(Reward::Card(CardReward::empty()));
    }
    EffectResult::Resolved
}

/// `BlackStar::modify_rewards` — append a `Reward::Relic` to the
/// rewards vec when the player just cleared an Elite room (Python
/// `shop_event.py::BlackStar.modify_rewards`, room-type gate
/// `RoomType.ELITE`). Python's `RelicReward(player_id)` with no
/// concrete `relic_id` argument rolls a random relic at flush time;
/// the current Rust `RelicReward` POD only carries a concrete
/// `relic_id`, so this listener follows the WongosMysteryTicket
/// placeholder convention and pushes the relic's own id as a sentinel.
/// The random-roll will land once umbrella `sts2-rs-01o` extends
/// `RelicReward` with a "pick from N" variant. Ownership-gated.
/// Bead `sts2-rs-83sa`.
fn listener_black_star(
    rs: &mut RunState,
    rewards: &mut Vec<Reward>,
    room_type: RoomType,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::BlackStar) {
        return EffectResult::NotImplemented;
    }
    if room_type != RoomType::Elite {
        return EffectResult::NotImplemented;
    }
    rewards.push(Reward::Relic(RelicReward::new(RelicId::BlackStar)));
    EffectResult::Resolved
}

// ---------------------------------------------------------------------------
// Public per-relic constants (pinned from Python)
// ---------------------------------------------------------------------------
//
// Exposed as `pub const` so the downstream integration bead can wire
// real hook bodies without re-typing the numbers from Python.

/// `VeryHotCocoa::ENERGY` — energy gained on player round 1
/// (`shop_event.py::VeryHotCocoa.ENERGY`). Bead `sts2-rs-70anj.1`.
pub const VERY_HOT_COCOA_ENERGY: i32 = 4;

/// `PaelsFlesh::ENERGY` — energy gained each player turn from round
/// 3 onward (`shop_event.py::PaelsFlesh.ENERGY`). Bead `sts2-rs-p6dj`.
pub const PAELS_FLESH_ENERGY: i32 = 1;
/// `PaelsFlesh` — first player round in which the energy gain fires
/// (`if combat.round_number >= 3`,
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::PaelsFlesh`).
/// `u16` matches the engine's `round_no()` type.
pub const PAELS_FLESH_ROUND_MIN: u16 = 3;

/// `BeltBuckle::DEXTERITY` — +2 / -2 Dex on potion-procurement state
/// (`shop_event.py::BeltBuckle.DEXTERITY`).
pub const BELT_BUCKLE_DEXTERITY: i32 = 2;

/// `PaelsLegion::Turns` — cooldown length (turns) after the block-double
/// fires, before it re-arms (C# `PaelsLegion.cs` `DynamicVar("Turns", 2)`).
/// Bead `sts2-rs-ppabi`.
pub const PAELS_LEGION_TURNS: i32 = 2;
/// `PaelsLegion` pet HP — behaviourless 9999-HP companion (C#
/// `Monsters/PaelsLegion.cs` `MinInitialHp == MaxInitialHp == 9999`).
/// Bead `sts2-rs-ppabi`.
const PAELS_LEGION_PET_HP: i32 = 9999;

/// `Bread::GAIN_ENERGY` / `LOSE_ENERGY` — +1 max energy, -2 round-1
/// (`shop_event.py::Bread`).
pub const BREAD_GAIN_ENERGY: i32 = 1;
pub const BREAD_LOSE_ENERGY: i32 = 2;

/// `Brimstone::SELF_STRENGTH` / `ENEMY_STRENGTH` — per-turn +2 Str /
/// +1 Str to all enemies (`shop_event.py::Brimstone`).
pub const BRIMSTONE_SELF_STRENGTH: i32 = 2;
pub const BRIMSTONE_ENEMY_STRENGTH: i32 = 1;

/// `PhilosophersStone::ENEMY_STRENGTH` / `ENERGY` — at combat start
/// every alive enemy gains STRENGTH+1; +1 max energy
/// (`shop_event.py::PhilosophersStone`). Only the `ENEMY_STRENGTH`
/// leg is wired by bead `sts2-rs-w0wl`; the `modify_max_energy: +1`
/// leg ships in a follow-up bead.
pub const PHILOSOPHERS_STONE_ENEMY_STRENGTH: i32 = 1;
pub const PHILOSOPHERS_STONE_ENERGY: i32 = 1;

/// `BoomingConch::EXTRA_DRAW` — round-1 elite-only +2 hand draw
/// (`shop_event.py::BoomingConch`). The listener body in
/// `sts2_core::relic_effects::on_modify_hand_draw_booming_conch`
/// inlines the literal `2`; a parity test in this file keeps the
/// constant and the inlined literal in lockstep.
pub const BOOMING_CONCH_EXTRA_DRAW: i32 = 2;

/// `MassiveScroll::CARDS` — multiplayer-card offers (coop is OOS,
/// so this is the count of CardReward sentinels pushed on obtain;
/// `shop_event.py::MassiveScroll` hard-codes 3).
pub const MASSIVE_SCROLL_CARDS: usize = 3;

/// `Cauldron::POTIONS` — random potions offered on obtain
/// (`shop_event.py::Cauldron.POTIONS`).
pub const CAULDRON_POTIONS: i32 = 5;

/// `ChemicalX::INCREASE` — bonus added to X-cost card X-value
/// (`shop_event.py::ChemicalX.INCREASE`).
pub const CHEMICAL_X_INCREASE: i32 = 2;

/// `DragonFruit::MAX_HP` — max-HP gained per gold-gain event
/// (`shop_event.py::DragonFruit.MAX_HP`).
pub const DRAGON_FRUIT_MAX_HP: i32 = 1;

/// `LeesWaffle::MAX_HP` — max-HP gained (and full heal) on obtain
/// (`shop_event.py::LeesWaffle.MAX_HP`).
pub const LEES_WAFFLE_MAX_HP: i32 = 7;

/// `LoomingFruit::MAX_HP` — max-HP gained on obtain
/// (`shop_event.py::LoomingFruit.MAX_HP`).
pub const LOOMING_FRUIT_MAX_HP: i32 = 31;

/// `GoldenPearl::GOLD` — gold granted on obtain
/// (`shop_event.py::GoldenPearl.GOLD`).
pub const GOLDEN_PEARL_GOLD: i32 = 150;

/// `SignetRing::GOLD` — gold granted on obtain
/// (`shop_event.py::SignetRing.GOLD`).
pub const SIGNET_RING_GOLD: i32 = 999;

/// `Orrery::CARDS` — number of card-reward sentinels pushed on obtain
/// (`shop_event.py::Orrery.CARDS`).
pub const ORRERY_CARDS: usize = 5;

/// `GnarledHammer::CARDS` — count of deck cards enchanted on obtain
/// (`shop_event.py::GnarledHammer.CARDS`). Bead `sts2-rs-qsxiy`.
pub const GNARLED_HAMMER_CARDS: usize = 3;

/// `GnarledHammer::SHARP` — Sharp keyword amount applied to each
/// enchanted card (`shop_event.py::GnarledHammer.SHARP`). Bead
/// `sts2-rs-qsxiy`.
pub const GNARLED_HAMMER_SHARP: i32 = 3;

/// `Kifuda::CARDS` — count of deck cards enchanted with Adroit on
/// obtain (`shop_event.py::Kifuda.CARDS`). Bead `sts2-rs-4nyt7`.
pub const KIFUDA_CARDS: usize = 3;

/// `Kifuda::ADROIT` — Adroit keyword amount applied to each
/// enchanted card (`shop_event.py::Kifuda` hard-codes `3` in the
/// `enchant_selected_cards` call). Bead `sts2-rs-4nyt7`.
pub const KIFUDA_ADROIT: i32 = 3;

/// `PunchDagger::MOMENTUM` — Momentum keyword amount applied to the
/// one card enchanted on obtain (`shop_event.py::PunchDagger.MOMENTUM`).
/// Count is hard-coded to 1 in the Python call; no separate constant.
/// Bead `sts2-rs-4nyt7`.
pub const PUNCH_DAGGER_MOMENTUM: i32 = 5;

/// `BeautifulBracelet::CARDS` — count of deck cards enchanted with
/// Swift on obtain (`shop_event.py::BeautifulBracelet.CARDS`). Bead
/// `sts2-rs-4nyt7`.
pub const BEAUTIFUL_BRACELET_CARDS: usize = 3;

/// `BeautifulBracelet::SWIFT` — Swift keyword amount applied to each
/// enchanted card (`shop_event.py::BeautifulBracelet.SWIFT`). Bead
/// `sts2-rs-4nyt7`.
pub const BEAUTIFUL_BRACELET_SWIFT: i32 = 3;

/// `TriBoomerang::CARDS` — count of deck cards enchanted with
/// Instinct on obtain (`shop_event.py::TriBoomerang.CARDS`).
/// Instinct amount is hard-coded to 1 in the Python call (no separate
/// class const). Bead `sts2-rs-70anj.4`.
pub const TRI_BOOMERANG_CARDS: usize = 3;

/// `BiiigHug::CARDS` — count of cards removed from master deck on
/// obtain (`shop_event.py::BiiigHug.CARDS`). Bead `sts2-rs-70anj.4`.
pub const BIIIG_HUG_CARDS: usize = 4;

/// `ToyBox::RELICS` — count of wax-relic RelicReward sentinels pushed
/// on obtain (`shop_event.py::ToyBox.RELICS`). Bead `sts2-rs-70anj.4`.
pub const TOY_BOX_RELICS: usize = 4;

/// `ToyBox::DynamicVars["Combats"]` — combats between melts
/// (`MegaCrit.Sts2.Core.Models.Relics/ToyBox.cs:84`, `new DynamicVar("Combats", 3m)`).
/// `AfterCombatEnd` increments `CombatsSeen` FIRST, then melts when
/// `CombatsSeen % Combats == 0`, so the first melt lands on combat 3.
pub const TOY_BOX_COMBATS: i32 = 3;

/// `WingCharm::SWIFT` — Swift enchant amount applied to one random
/// card-reward option (`shop_event.py::WingCharm.SWIFT`). Bead
/// `sts2-rs-70anj.4`.
pub const WING_CHARM_SWIFT: i32 = 1;

/// `VelvetChoker::MAX_CARDS` — per-turn play cap (Python
/// `shop_event.py::VelvetChoker.MAX_CARDS`). Bead `sts2-rs-70anj.4`.
pub const VELVET_CHOKER_MAX_CARDS: i32 = 6;

/// `Astrolabe::CARDS` — count of deck cards selected for transform +
/// upgrade on obtain (`shop_event.py::Astrolabe.CARDS`). Bead
/// `sts2-rs-kluld`.
pub const ASTROLABE_CARDS: usize = 3;

/// `StoneHumidifier::MAX_HP` — max-HP gained per rest-site heal
/// (`shop_event.py::StoneHumidifier.MAX_HP`). Bead `sts2-rs-70anj.3.17`.
pub const STONE_HUMIDIFIER_MAX_HP: i32 = 5;

/// `CookOption::CARD_COUNT` — number of deck cards removed by Cook
/// (`run/rest_site.py::CookOption.CARD_COUNT`). Bead `sts2-rs-ekf4j`.
pub const COOK_OPTION_CARD_COUNT: usize = 2;

/// `CookOption::MAX_HP` — max HP gained on Cook
/// (`run/rest_site.py::CookOption.MAX_HP`). Bead `sts2-rs-ekf4j`.
pub const COOK_OPTION_MAX_HP: i32 = 9;

/// `Toolbox::CARDS` — number of colorless choices offered on round 1
/// (`shop_event.py::Toolbox.CARDS`).
pub const TOOLBOX_CARDS: usize = 3;

/// `ChoicesParadox::CARDS` — number of retain-tagged character choices
/// offered on round 1 (`shop_event.py::ChoicesParadox.CARDS`).
pub const CHOICES_PARADOX_CARDS: usize = 5;

/// `EmptyCage::CARDS` — count of deck cards removed on obtain
/// (`shop_event.py::EmptyCage.CARDS`). Bead `sts2-rs-kluld`.
pub const EMPTY_CAGE_CARDS: usize = 2;

/// `DistinguishedCape::HP_LOSS` — max-HP lost on obtain
/// (`shop_event.py::DistinguishedCape.HP_LOSS`). Bead `sts2-rs-r2y4x`.
pub const DISTINGUISHED_CAPE_HP_LOSS: i32 = 9;

/// `DistinguishedCape::CARDS` — count of Apparition cards added on
/// obtain (`shop_event.py::DistinguishedCape.CARDS`). Bead
/// `sts2-rs-r2y4x`. Currently documentation-only — the Apparition
/// deck-add clause is deferred pending the run-side
/// `add_card_to_deck` primitive (umbrella `sts2-rs-01o`).
pub const DISTINGUISHED_CAPE_CARDS: usize = 3;

/// `SandCastle::CARDS` — count of random deck cards upgraded on
/// obtain (`shop_event.py::SandCastle.CARDS`). Bead `sts2-rs-r2y4x`.
pub const SAND_CASTLE_CARDS: usize = 6;

/// `GlassEye` — count of card-reward sentinels pushed on obtain
/// (Python `shop_event.py::GlassEye.after_obtained` hard-codes a
/// 5-iteration loop over `(COMMON, COMMON, UNCOMMON, UNCOMMON, RARE)`;
/// no class-level constant). Bead `sts2-rs-66l9f`.
pub const GLASS_EYE_CARDS: usize = 5;

/// `ScrollBoxes` — count of card-bundle sentinels pushed on obtain
/// (Python `shop_event.py::ScrollBoxes.offer_card_bundles` produces
/// 2 bundles; no class-level constant). Bead `sts2-rs-66l9f`.
pub const SCROLL_BOXES_BUNDLES: usize = 2;

/// `PaelsHorn` — count of Relax cards added on obtain. Python hard-
/// codes a `range(2)` loop (`shop_event.py::PaelsHorn`); no class-
/// level constant. Bead `sts2-rs-a49q3`.
pub const PAELS_HORN_CARDS: usize = 2;

/// `LeadPaperweight` — count of colorless-card-reward sentinels
/// pushed on obtain (Python `shop_event.py::LeadPaperweight.after_obtained`:
/// `owner.offer_colorless_cards(2)`; no class const). Bead
/// `sts2-rs-qw9oe`.
pub const LEAD_PAPERWEIGHT_CARDS: usize = 2;

/// `SeaGlass::CARDS` — count of character-pool card-reward sentinels
/// pushed on obtain (`shop_event.py::SeaGlass.CARDS`). Bead
/// `sts2-rs-qw9oe`.
pub const SEA_GLASS_CARDS: usize = 15;

/// `YummyCookie::CARDS` — count of deck cards upgraded on obtain
/// (`shop_event.py::YummyCookie.CARDS`). Bead `sts2-rs-qw9oe`.
pub const YUMMY_COOKIE_CARDS: usize = 4;

/// `PrecariousShears::CARDS` — count of deck cards removed on obtain
/// (`shop_event.py::PrecariousShears.CARDS`). Bead `sts2-rs-cvbxs`.
/// Pinned for parity; the remove-cards leg is deferred pending a
/// run-side `remove_cards_from_deck` primitive.
pub const PRECARIOUS_SHEARS_CARDS: usize = 2;

/// `PrecariousShears::DAMAGE` — HP damage taken on obtain. C#
/// `PrecariousShears.cs` `CanonicalVars` is `[CardsVar(2),
/// DamageVar(16m, Unpowered)]`; the Python pin's
/// `shop_event.py::PrecariousShears.DAMAGE = 13` drifted from the
/// shipped game. Bead `sts2-rs-3bigm`.
pub const PRECARIOUS_SHEARS_DAMAGE: i32 = 16;

/// `PreservedFog::CardsVar = 3` per C# v0.103.2
/// `MegaCrit.Sts2.Core.Models.Relics/PreservedFog.cs:18` — count of
/// deck cards removed on obtain (then 1 Folly curse is added).
/// Bead `sts2-rs-t762n` introduced the value as 5 (Python pin
/// `shop_event.py::PreservedFog.CARDS = 5`); bead `sts2-rs-rn76k`
/// rebases to 3 per the C# decomp (CLAUDE.md hard rule).
pub const PRESERVED_FOG_CARDS: usize = 3;

/// `Claws::CARDS` — count of deck cards transformed into Maul on
/// obtain (`shop_event.py::Claws.CARDS`). Bead `sts2-rs-w9vf8`.
pub const CLAWS_CARDS: usize = 6;

// `AlchemicalCoffer::POTION_SLOTS` constant is already defined
// elsewhere in this file (`ALCHEMICAL_COFFER_POTION_SLOTS`); the
// `sts2-rs-cvbxs` listener wires the `gain_potion_slots(N)` leg
// using that existing constant. Random procure-potion clauses are
// deferred pending a run-side `procure_potion` primitive.

// `CursedPearl::GOLD` — gold granted on obtain — pinned upstream by
// `sts2-rs-n06dm` as [`CURSED_PEARL_GOLD`]; `sts2-rs-a49q3` reuses
// it inside `listener_cursed_pearl` alongside the Greed card-add.

/// `LeafyPoultice::MAX_HP_LOSS` — max-HP lost on obtain. C#
/// `LeafyPoultice.cs` `CanonicalVars` is `MaxHpVar(12m)`; the
/// Python pin's `shop_event.py::LeafyPoultice.MAX_HP_LOSS = 10`
/// drifted from the shipped game. Bead `sts2-rs-3bigm`.
pub const LEAFY_POULTICE_MAX_HP_LOSS: i32 = 12;

/// `CallingBell::RELICS` — count of RelicReward sentinels pushed on
/// obtain (`shop_event.py::CallingBell.RELICS`).
pub const CALLING_BELL_RELICS: usize = 3;

/// `SmallCapsule::RELICS` — RelicReward sentinels offered on obtain
/// (`shop_event.py::SmallCapsule.after_obtained`: hard-coded `1`).
pub const SMALL_CAPSULE_RELICS: usize = 1;

/// `NutritiousOyster::MAX_HP` — max-HP gained on obtain
/// (`shop_event.py::NutritiousOyster.MAX_HP`).
pub const NUTRITIOUS_OYSTER_MAX_HP: i32 = 11;

/// `MembershipCard::DISCOUNT` — merchant discount %
/// (`shop_event.py::MembershipCard.DISCOUNT`).
pub const MEMBERSHIP_CARD_DISCOUNT: i32 = 50;

/// `MysticLighter::EXTRA_DAMAGE` — bonus damage on enchanted attacks
/// (`shop_event.py::MysticLighter.EXTRA_DAMAGE`).
pub const MYSTIC_LIGHTER_EXTRA_DAMAGE: i32 = 9;

/// `NinjaScroll::SHIVS` — round-1 Shivs generated
/// (`shop_event.py::NinjaScroll.SHIVS`).
pub const NINJA_SCROLL_SHIVS: i32 = 3;

/// `ScreamingFlagon::DAMAGE` — round-end self-damage
/// (`shop_event.py::ScreamingFlagon.DAMAGE`).
pub const SCREAMING_FLAGON_DAMAGE: i32 = 20;

/// `SlingOfCourage::STRENGTH` — elite-only round-1 Strength
/// (`shop_event.py::SlingOfCourage.STRENGTH`).
pub const SLING_OF_COURAGE_STRENGTH: i32 = 2;

/// `TheAbacus::BLOCK` — block gained on shuffle
/// (`shop_event.py::TheAbacus.BLOCK`).
pub const THE_ABACUS_BLOCK: i32 = 6;

/// `RunicCapacitor::ORB_SLOTS` — extra Defect orb slots
/// (`shop_event.py::RunicCapacitor.ORB_SLOTS`).
pub const RUNIC_CAPACITOR_ORB_SLOTS: i32 = 3;

/// `AlchemicalCoffer::POTION_SLOTS` — bonus potion slots on obtain
/// (`shop_event.py::AlchemicalCoffer.POTION_SLOTS`).
pub const ALCHEMICAL_COFFER_POTION_SLOTS: i32 = 4;

/// `FurCoat::COMBATS` — number of combat rooms FurCoat marks on the
/// obtain act's map (`shop_event.py::FurCoat.COMBATS`). Bead
/// `sts2-rs-eboqq`.
pub const FUR_COAT_COMBATS: usize = 7;

/// `BlessedAntler::ENERGY` / `DAZED_COUNT` — +1 max energy with 3 Dazed
/// shuffled into draw round 1 (`shop_event.py::BlessedAntler`).
pub const BLESSED_ANTLER_ENERGY: i32 = 1;
pub const BLESSED_ANTLER_DAZED_COUNT: i32 = 3;

/// `BloodSoakedRose::ENERGY` — +1 max energy
/// (`shop_event.py::BloodSoakedRose.ENERGY`).
pub const BLOOD_SOAKED_ROSE_ENERGY: i32 = 1;

/// `CursedPearl::GOLD` — gold gained when the Greed curse is added
/// (`shop_event.py::CursedPearl.GOLD`).
pub const CURSED_PEARL_GOLD: i32 = 333;

/// `Ectoplasm::ENERGY` — +1 max energy, prevents all gold gain
/// (`shop_event.py::Ectoplasm.ENERGY`).
pub const ECTOPLASM_ENERGY: i32 = 1;

/// `Sai::BLOCK` — per-turn block gained
/// (`shop_event.py::Sai.BLOCK`).
pub const SAI_BLOCK: i32 = 7;

/// `SneckoEye::EXTRA_DRAW` — extra cards drawn per turn (Confused on
/// combat start) (`shop_event.py::SneckoEye.EXTRA_DRAW`).
pub const SNECKO_EYE_EXTRA_DRAW: i32 = 2;

/// `Sozu::ENERGY` — +1 max energy, blocks potion procurement
/// (`shop_event.py::Sozu.ENERGY`).
pub const SOZU_ENERGY: i32 = 1;

/// `SpikedGauntlets::ENERGY` — +1 max energy, Power cards cost +1
/// (`shop_event.py::SpikedGauntlets.ENERGY`).
pub const SPIKED_GAUNTLETS_ENERGY: i32 = 1;

/// `IronClub::CARD_THRESHOLD` — every Nth player card play triggers a
/// `draw_cards(1)` (`shop_event.py::IronClub.CARD_THRESHOLD`). Bead
/// `sts2-rs-70anj.1`.
pub const IRON_CLUB_CARD_THRESHOLD: i32 = 4;

/// `ToastyMittens::STRENGTH` — per-player-turn Strength gain
/// (`shop_event.py::ToastyMittens.STRENGTH`). Bead `sts2-rs-70anj.1`.
pub const TOASTY_MITTENS_STRENGTH: i32 = 1;

// ---------------------------------------------------------------------------
// Typed-channel listener bodies
// ---------------------------------------------------------------------------

/// `RunicCapacitor::after_side_turn_start` — `if side == PLAYER and
/// combat.round_number == 1: combat.add_orb_slots(owner, 3)` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::RunicCapacitor`,
/// `ORB_SLOTS = 3`, pool=DEFECT). Round 1 only. Mirrors the
/// VeryHotCocoa / SymbioticVirus round-1 player-gate pattern; the
/// underlying engine call routes to
/// [`sts2_core::orbs::OrbSlots::add_slots`].
fn listener_runic_capacitor(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if state.round_no() != 1 {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::RunicCapacitor) {
        return EffectResult::NotImplemented;
    }
    let slots = usize::try_from(RUNIC_CAPACITOR_ORB_SLOTS).unwrap_or(0);
    state.player.orbs.add_slots(slots);
    EffectResult::Resolved
}
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    /// The id list covers exactly 122 entries — 30 SHOP + 92 ANCIENT in
    /// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py`.
    #[test]
    fn pool_has_one_hundred_twenty_two_entries() {
        assert_eq!(SHOP_BOSS_RELIC_IDS.len(), 122);
    }

    /// Every `RelicId` in the list is distinct. Catches accidental
    /// duplicate paste-edits inside [`SHOP_BOSS_RELIC_IDS`].
    #[test]
    fn pool_entries_are_unique_by_relic_id() {
        let mut ids: Vec<RelicId> = SHOP_BOSS_RELIC_IDS.to_vec();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(
            before,
            ids.len(),
            "duplicate RelicId in SHOP_BOSS_RELIC_IDS"
        );
    }

    /// `LargeCapsule::after_obtained` obtains `LARGE_CAPSULE_RELICS = 2`
    /// relics DIRECTLY (C# `RelicCmd.Obtain` — no pick) and then adds the
    /// character's basic Strike + Defend to the deck. Obtain-before-add is
    /// load-bearing for the egg-relic upgrade-on-add hook (PARITY36).
    #[test]
    fn listener_large_capsule_obtains_two_relics_and_adds_strike_defend() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        let (by_rarity, fallback, boss) = crate::run_callbacks::default_relic_grab_bag();
        rs.populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
        rs.relics.push(RelicId::LargeCapsule);
        rs.last_obtained_relic = Some(RelicId::LargeCapsule);
        let relics_before = rs.relics.len();
        let deck_before = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        // 2 relics obtained directly (not offered as pickable rewards).
        assert_eq!(rs.relic_rewards.len(), 0, "obtained directly, not offered");
        assert_eq!(rs.relics.len(), relics_before + LARGE_CAPSULE_RELICS);
        // Strike + Defend added to the deck.
        assert_eq!(rs.master_deck.len(), deck_before + 2);
    }

    /// Bead `sts2-rs-70anj.4` (Wave 4 body-fill, batch 1):
    /// `JewelryBox::after_obtained` adds 1 unupgraded `Apotheosis` to
    /// the run-side master deck. Pins the deck-push path AND the
    /// ownership-guard no-op shape.
    #[test]
    fn listener_jewelry_box_adds_apotheosis_to_master_deck_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        // Without ownership: dispatcher fires the registered listener
        // which ownership-gates to no-op; master_deck stays untouched.
        let mut rs_noop = RunState::new();
        let pre_len = rs_noop.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs_noop);
        assert_eq!(rs_noop.master_deck.len(), pre_len);

        // With ownership: master_deck grows by 1 with the Apotheosis
        // POD (unupgraded). Default RunState has an empty master_deck.
        let mut rs = RunState::new();
        rs.relics.push(RelicId::JewelryBox);
        rs.last_obtained_relic = Some(RelicId::JewelryBox);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), 1);
        let added = &rs.master_deck[0];
        assert_eq!(added.card_id, sts2_core::cards::CardId::Apotheosis);
        assert!(!added.upgraded, "Python pin adds the un-upgraded card");
    }

    /// Bead `sts2-rs-70anj.4` (Wave 4 body-fill, batch 1):
    /// `TanxsWhistle::after_obtained` adds 1 unupgraded `Whistle` to
    /// the run-side master deck.
    #[test]
    fn listener_tanxs_whistle_adds_whistle_to_master_deck_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::TanxsWhistle);
        rs.last_obtained_relic = Some(RelicId::TanxsWhistle);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), 1);
        let added = &rs.master_deck[0];
        assert_eq!(added.card_id, sts2_core::cards::CardId::Whistle);
        assert!(!added.upgraded);
    }

    /// Bead `sts2-rs-70anj.4` (Wave 4 body-fill, batch 2):
    /// `DragonFruit::on_gold_gained` bumps `rs.max_hp` by 1 per
    /// positive gold gain when owned. Negative / zero gold amounts
    /// are no-ops (Python `if amount > 0` guard). Non-ownership is
    /// also no-op (the listener fires on the dispatch walk over all
    /// registered relics — only the owned one mutates run state).
    #[test]
    fn listener_dragon_fruit_max_hp_grows_per_positive_gold_gain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        // Owned + positive gain → +1 max_hp per gain.
        let mut rs = RunState::new();
        rs.relics.push(RelicId::DragonFruit);
        let pre_max_hp = rs.max_hp;
        let _ = sts2_core::run_effects::dispatch_on_gold_gained(&mut rs, 5);
        assert_eq!(rs.max_hp, pre_max_hp + DRAGON_FRUIT_MAX_HP);
        // Second gain bumps again.
        let _ = sts2_core::run_effects::dispatch_on_gold_gained(&mut rs, 12);
        assert_eq!(rs.max_hp, pre_max_hp + 2 * DRAGON_FRUIT_MAX_HP);

        // Owned + zero or negative gain → no-op (Python pin guards).
        let mut rs0 = RunState::new();
        rs0.relics.push(RelicId::DragonFruit);
        let pre = rs0.max_hp;
        let _ = sts2_core::run_effects::dispatch_on_gold_gained(&mut rs0, 0);
        let _ = sts2_core::run_effects::dispatch_on_gold_gained(&mut rs0, -3);
        assert_eq!(rs0.max_hp, pre);

        // Not owned → no-op.
        let mut rs_noown = RunState::new();
        let pre_noown = rs_noown.max_hp;
        let _ = sts2_core::run_effects::dispatch_on_gold_gained(&mut rs_noown, 100);
        assert_eq!(rs_noown.max_hp, pre_noown);
    }

    /// Bead `sts2-rs-70anj.3` (Wave 3 starter_relic_swap primitive):
    /// `TouchOfOrobas::after_obtained` swaps each of the 5 starter
    /// relics in `rs.relics` to its upgraded variant when obtained.
    /// Mirrors Python pin
    /// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::TouchOfOrobas`.
    #[test]
    fn listener_touch_of_orobas_upgrades_each_starter_relic() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let cases = [
            (RelicId::BurningBlood, RelicId::BlackBlood),
            (RelicId::RingOfTheSnake, RelicId::RingOfTheDrake),
            (RelicId::CrackedCore, RelicId::InfusedCore),
            (RelicId::BoundPhylactery, RelicId::PhylacteryUnbound),
            (RelicId::DivineRight, RelicId::DivineDestiny),
        ];

        for (starter, upgraded) in cases {
            let mut rs = RunState::new();
            rs.relics.push(starter);
            rs.relics.push(RelicId::TouchOfOrobas);
            rs.last_obtained_relic = Some(RelicId::TouchOfOrobas);
            let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
            assert!(
                rs.relics.contains(&upgraded),
                "TouchOfOrobas must upgrade {starter:?} → {upgraded:?}"
            );
            assert!(
                !rs.relics.contains(&starter),
                "Original starter {starter:?} must be replaced"
            );
        }
    }

    /// No-op when TouchOfOrobas is owned but no starter relic is
    /// present (degenerate case — Python skips the swap silently).
    #[test]
    fn listener_touch_of_orobas_is_noop_without_starter() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::TouchOfOrobas);
        let before = rs.relics.clone();
        rs.last_obtained_relic = Some(RelicId::TouchOfOrobas);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.relics, before);
    }

    /// Every shop/boss `RelicId` carries an upper-snake-case Python
    /// name. Locks the parity-output naming for the future exporter.
    #[test]
    fn every_shop_boss_relic_has_python_name() {
        for &id in SHOP_BOSS_RELIC_IDS {
            let name = id.to_python_name();
            assert!(!name.is_empty(), "empty python_name for {id:?}");
            assert!(
                name.bytes()
                    .all(|b| b == b'_' || b.is_ascii_uppercase() || b.is_ascii_digit()),
                "non upper-snake-case python_name for {id:?}: {name:?}"
            );
        }
    }

    // --- Deep pin-tests for the headline boss/shop relics ----------------
    //
    // The bead spec asked for deep tests on Snecko Eye / Dead Branch /
    // Pandora's Box. `DeadBranch` is absent from the `01c094d` fork
    // (see module-level note), so the third deep test covers
    // `Brimstone` — the closest combat-math analogue (per-turn
    // Strength on both sides). Additional pin-tests cover Sozu, Sai,
    // BlackStar, MembershipCard, MysticLighter, RunicPyramid,
    // Ectoplasm, Bread, AlchemicalCoffer, CursedPearl, NinjaScroll,
    // SpikedGauntlets, BloodSoakedRose, ChemicalX, LeesWaffle,
    // DragonFruit, ScreamingFlagon — pinning the load-bearing Python
    // constants so the integration bead can drop in real logic
    // without retyping numbers.

    /// `SneckoEye` — pins `EXTRA_DRAW = 2` and locks the Confused
    /// pre-combat hook. This is the headline boss relic from the bead
    /// spec (`shop_event.py::SneckoEye`).
    #[test]
    fn snecko_eye_deep_pin() {
        assert_eq!(SNECKO_EYE_EXTRA_DRAW, 2);
        assert_eq!(RelicId::SneckoEye.to_python_name(), "SNECKO_EYE");
    }

    /// `PandorasBox` — pins the transform-all-basic-Strikes/Defends
    /// hook on `after_obtained`. The other headline boss relic from
    /// the bead spec (`shop_event.py::PandorasBox`).
    #[test]
    fn pandoras_box_deep_pin() {
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::PandorasBox.to_python_name(), "PANDORAS_BOX");
    }

    /// `Brimstone` — pins `SELF_STRENGTH = 2` and `ENEMY_STRENGTH = 1`
    /// (per-turn Strength on both sides). The closest combat-math
    /// analogue to Dead Branch in this fork — directly mutates damage
    /// pipeline every turn (`shop_event.py::Brimstone`).
    #[test]
    fn brimstone_deep_pin() {
        assert_eq!(BRIMSTONE_SELF_STRENGTH, 2);
        assert_eq!(BRIMSTONE_ENEMY_STRENGTH, 1);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::Brimstone.to_python_name(), "BRIMSTONE");
    }

    /// `Sozu` — pins `ENERGY = 1` and the `should_procure_potion`
    /// override semantics (one-shot block at the registry layer).
    #[test]
    fn sozu_deep_pin() {
        assert_eq!(SOZU_ENERGY, 1);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::Sozu.to_python_name(), "SOZU");
    }

    /// `Sai` — pins `BLOCK = 7` (per-turn block) for the
    /// `after_side_turn_start` hook.
    #[test]
    fn sai_deep_pin() {
        assert_eq!(SAI_BLOCK, 7);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::Sai.to_python_name(), "SAI");
    }

    /// `RunicPyramid` — locks the `should_flush` hook (retain all
    /// cards every turn — Pyramid's signature effect).
    #[test]
    fn runic_pyramid_deep_pin() {
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::RunicPyramid.to_python_name(), "RUNIC_PYRAMID");
    }

    /// `Ectoplasm` — pins `ENERGY = 1` and the gold-gain override
    /// (always returns `False` to block all gold gain).
    #[test]
    fn ectoplasm_deep_pin() {
        assert_eq!(ECTOPLASM_ENERGY, 1);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::Ectoplasm.to_python_name(), "ECTOPLASM");
    }

    /// `Bread` — pins +1 max energy / -2 round-1 energy
    /// (`shop_event.py::Bread`).
    #[test]
    fn bread_deep_pin() {
        assert_eq!(BREAD_GAIN_ENERGY, 1);
        assert_eq!(BREAD_LOSE_ENERGY, 2);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::Bread.to_python_name(), "BREAD");
    }

    /// `MembershipCard` — pins `DISCOUNT = 50` for the
    /// `modify_merchant_price` hook.
    #[test]
    fn membership_card_deep_pin() {
        assert_eq!(MEMBERSHIP_CARD_DISCOUNT, 50);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::MembershipCard.to_python_name(), "MEMBERSHIP_CARD");
    }

    /// `MysticLighter` — pins `EXTRA_DAMAGE = 9` for the
    /// `modify_damage_additive` hook.
    #[test]
    fn mystic_lighter_deep_pin() {
        assert_eq!(MYSTIC_LIGHTER_EXTRA_DAMAGE, 9);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::MysticLighter.to_python_name(), "MYSTIC_LIGHTER");
    }

    /// `NinjaScroll` — pins `SHIVS = 3` for the round-1
    /// `before_side_turn_start` hook (Silent pool).
    #[test]
    fn ninja_scroll_deep_pin() {
        assert_eq!(NINJA_SCROLL_SHIVS, 3);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::NinjaScroll.to_python_name(), "NINJA_SCROLL");
    }

    /// `AlchemicalCoffer` — pins `POTION_SLOTS = 4` for the
    /// `after_obtained` hook.
    #[test]
    fn alchemical_coffer_deep_pin() {
        assert_eq!(ALCHEMICAL_COFFER_POTION_SLOTS, 4);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(
            RelicId::AlchemicalCoffer.to_python_name(),
            "ALCHEMICAL_COFFER"
        );
    }

    /// `CursedPearl` — pins `GOLD = 333` (and the Greed-curse
    /// `after_obtained` semantics).
    #[test]
    fn cursed_pearl_deep_pin() {
        assert_eq!(CURSED_PEARL_GOLD, 333);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::CursedPearl.to_python_name(), "CURSED_PEARL");
    }

    /// `LeesWaffle` — pins `MAX_HP = 7` for the obtain-time max-HP +
    /// heal-to-full hook.
    #[test]
    fn lees_waffle_deep_pin() {
        assert_eq!(LEES_WAFFLE_MAX_HP, 7);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::LeesWaffle.to_python_name(), "LEES_WAFFLE");
    }

    /// `ChemicalX` — pins `INCREASE = 2` for the X-cost card hook.
    #[test]
    fn chemical_x_deep_pin() {
        assert_eq!(CHEMICAL_X_INCREASE, 2);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::ChemicalX.to_python_name(), "CHEMICAL_X");
    }

    /// `ScreamingFlagon` — pins `DAMAGE = 20` for the
    /// `before_turn_end` self-damage hook.
    #[test]
    fn screaming_flagon_deep_pin() {
        assert_eq!(SCREAMING_FLAGON_DAMAGE, 20);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(
            RelicId::ScreamingFlagon.to_python_name(),
            "SCREAMING_FLAGON"
        );
    }

    /// `BlackStar` — locks the extra-relic-after-elite hook
    /// (`modify_rewards`). A high-impact reward-stream relic.
    #[test]
    fn black_star_deep_pin() {
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::BlackStar.to_python_name(), "BLACK_STAR");
    }

    /// `BeltBuckle` — pins `DEXTERITY = 2` (the +2/-2 dex on
    /// potion-procurement state — load-bearing combat-math toggle).
    #[test]
    fn belt_buckle_deep_pin() {
        assert_eq!(BELT_BUCKLE_DEXTERITY, 2);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::BeltBuckle.to_python_name(), "BELT_BUCKLE");
    }

    // -----------------------------------------------------------------
    // BeltBuckle listener tests — bead `sts2-rs-xu8d6`
    // -----------------------------------------------------------------

    #[cfg(test)]
    fn belt_buckle_setup(
        state: &mut CombatState,
        equip: bool,
        potions: &[sts2_core::potions::PotionKind],
    ) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        state.player.potions.clear();
        for &p in potions {
            state.player.potions.push(p);
        }
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::BeltBuckle,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    #[cfg(test)]
    fn belt_buckle_player_dexterity(state: &CombatState) -> i32 {
        let Some(uid) = state.player.creature_uid else {
            return 0;
        };
        state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .map_or(0, |c| {
                c.powers
                    .iter()
                    .filter_map(|p_uid| state.powers.get_by_id(p_uid.0))
                    .filter(|p| p.kind == PowerKind::Dexterity)
                    .map(|p| p.amount)
                    .sum()
            })
    }

    /// **BeltBuckle #1** — combat start with no potions grants
    /// +`BELT_BUCKLE_DEXTERITY` Dexterity and latches.
    #[test]
    fn belt_buckle_grants_dexterity_when_potionless_at_combat_start() {
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        belt_buckle_setup(&mut state, true, &[]);
        let _ = dispatch_before_combat_start(&mut state);
        assert_eq!(
            belt_buckle_player_dexterity(&state),
            BELT_BUCKLE_DEXTERITY,
            "potionless combat start grants +BELT_BUCKLE_DEXTERITY Dex",
        );
        // Latch set — a second apply attempt must not double the bonus.
        let _ = belt_buckle_apply_dexterity(&mut state);
        assert_eq!(
            belt_buckle_player_dexterity(&state),
            BELT_BUCKLE_DEXTERITY,
            "latch prevents double-apply",
        );
    }

    /// **BeltBuckle #2** — combat start while holding a potion is inert.
    #[test]
    fn belt_buckle_inert_when_holding_potion_at_combat_start() {
        use sts2_core::potions::PotionKind;
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        belt_buckle_setup(&mut state, true, &[PotionKind::BlockPotion]);
        let _ = dispatch_before_combat_start(&mut state);
        assert_eq!(
            belt_buckle_player_dexterity(&state),
            0,
            "holding a potion suppresses the Dexterity grant",
        );
    }

    /// **BeltBuckle #3** — using the last potion (leaving the player
    /// potionless) grants the Dexterity mid-combat.
    #[test]
    fn belt_buckle_grants_dexterity_after_using_last_potion() {
        use sts2_core::potions::PotionKind;
        use sts2_core::relic_effects::{dispatch_after_potion_used, dispatch_before_combat_start};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        belt_buckle_setup(&mut state, true, &[PotionKind::BlockPotion]);
        // Start with a potion: combat-start grant is suppressed.
        let _ = dispatch_before_combat_start(&mut state);
        assert_eq!(belt_buckle_player_dexterity(&state), 0);
        // Simulate using the potion (inventory now empty), then fire the hook.
        state.player.potions.clear();
        let _ = dispatch_after_potion_used(&mut state, PotionKind::BlockPotion, None);
        assert_eq!(
            belt_buckle_player_dexterity(&state),
            BELT_BUCKLE_DEXTERITY,
            "using the last potion grants the Dexterity",
        );
    }

    /// **BeltBuckle #4** — no-op without ownership.
    #[test]
    fn belt_buckle_inert_without_ownership() {
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        belt_buckle_setup(&mut state, false, &[]);
        let _ = dispatch_before_combat_start(&mut state);
        assert_eq!(
            belt_buckle_player_dexterity(&state),
            0,
            "no relic, no Dexterity",
        );
    }

    // -----------------------------------------------------------------
    // PaelsLegion listener tests — bead `sts2-rs-ppabi`
    // -----------------------------------------------------------------

    #[cfg(test)]
    fn paels_legion_counter(state: &CombatState) -> i32 {
        state
            .relics
            .iter()
            .find(|r| !r.is_melted && r.kind == RelicKind::PaelsLegion)
            .expect("PaelsLegion present")
            .counter
    }

    #[cfg(test)]
    fn paels_card_event_ctx() -> CardEventCtx {
        CardEventCtx {
            card_id: sts2_core::cards::CardId::DefendIronclad,
            card_uid: None,
            target: None,
            card_tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
        }
    }

    /// `PaelsLegion` doubles the first card-granted block, then enters a
    /// `PAELS_LEGION_TURNS`-turn cooldown, then re-arms — full cycle.
    #[test]
    fn paels_legion_doubles_card_block_then_cooldowns_then_rearms() {
        use std::num::NonZeroU32;
        use sts2_core::card_effects::gain_block_player;
        use sts2_core::relic_effects::{
            dispatch_after_card_played, dispatch_after_side_turn_start,
        };
        crate::relics::dispatch::ensure_installed();

        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        state.relics.push(sts2_core::Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::PaelsLegion,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        // A card "in play" so block grants are card-sourced.
        state
            .player
            .play
            .push(sts2_core::CardUid(NonZeroU32::new(1).expect("non-zero")));

        // 1) Armed: 5 base block doubled to 10; cooldown pending (-1).
        gain_block_player(&mut state, 5);
        assert_eq!(state.player.block, 10, "card block doubled while armed");
        assert_eq!(paels_legion_counter(&state), -1, "cooldown pending");

        // 2) Card-play resolves → cooldown starts.
        let _ = dispatch_after_card_played(&mut state, &paels_card_event_ctx());
        assert_eq!(
            paels_legion_counter(&state),
            PAELS_LEGION_TURNS,
            "cooldown armed to Turns after card resolves"
        );

        // 3) On cooldown: next card block is NOT doubled (10 + 5 = 15).
        gain_block_player(&mut state, 5);
        assert_eq!(state.player.block, 15, "no doubling while on cooldown");
        assert_eq!(paels_legion_counter(&state), PAELS_LEGION_TURNS);

        // 4) Two player turn-starts decrement the cooldown to 0 (re-armed).
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(paels_legion_counter(&state), 0, "re-armed after 2 turns");

        // 5) Armed again: doubled (15 + 10 = 25).
        gain_block_player(&mut state, 5);
        assert_eq!(state.player.block, 25, "doubled again after re-arm");
    }

    /// `PaelsLegion` does not double non-card (system) block — `card_source`
    /// is `None` for turn-start / relic block grants.
    #[test]
    fn paels_legion_ignores_non_card_block() {
        use std::num::NonZeroU32;
        use sts2_core::card_effects::gain_block_player;
        crate::relics::dispatch::ensure_installed();

        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        state.relics.push(sts2_core::Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::PaelsLegion,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        // Empty play pile → block grant is non-card (card_source = None).
        gain_block_player(&mut state, 5);
        assert_eq!(state.player.block, 5, "non-card block is not doubled");
        assert_eq!(
            paels_legion_counter(&state),
            0,
            "non-card block does not arm the cooldown"
        );
    }

    /// `PaelsLegion::before_combat_start` summons the pet and resets the
    /// cooldown to armed.
    #[test]
    fn paels_legion_summons_pet_and_resets_cooldown() {
        use std::num::NonZeroU32;
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();

        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        state.relics.push(sts2_core::Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::PaelsLegion,
            counter: 9, // stale cooldown from a prior combat
            is_wax: false,
            is_melted: false,
        });
        let _ = dispatch_before_combat_start(&mut state);
        assert_eq!(paels_legion_counter(&state), 0, "cooldown reset to armed");
        let pets = state
            .creatures
            .iter()
            .filter(|c| c.species == Some(sts2_core::MonsterKind::PaelsLegion) && c.current_hp > 0)
            .count();
        assert_eq!(pets, 1, "PaelsLegion pet summoned");
    }

    /// `DragonFruit` — pins `MAX_HP = 1` per gold-gain (slow-roll
    /// run-scoped HP growth — pins the `on_gold_gained` hook name).
    #[test]
    fn dragon_fruit_deep_pin() {
        assert_eq!(DRAGON_FRUIT_MAX_HP, 1);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::DragonFruit.to_python_name(), "DRAGON_FRUIT");
    }

    /// Spot-check the load-bearing public constants exist with the
    /// values Python's `shop_event.py` pins. Catches accidental edits
    /// to the `pub const` block above.
    #[test]
    fn public_constants_match_python_pins() {
        assert_eq!(BELT_BUCKLE_DEXTERITY, 2);
        assert_eq!(BREAD_GAIN_ENERGY, 1);
        assert_eq!(BREAD_LOSE_ENERGY, 2);
        assert_eq!(BRIMSTONE_SELF_STRENGTH, 2);
        assert_eq!(BRIMSTONE_ENEMY_STRENGTH, 1);
        assert_eq!(CAULDRON_POTIONS, 5);
        assert_eq!(CHEMICAL_X_INCREASE, 2);
        assert_eq!(DRAGON_FRUIT_MAX_HP, 1);
        assert_eq!(LEES_WAFFLE_MAX_HP, 7);
        assert_eq!(MEMBERSHIP_CARD_DISCOUNT, 50);
        assert_eq!(MYSTIC_LIGHTER_EXTRA_DAMAGE, 9);
        assert_eq!(NINJA_SCROLL_SHIVS, 3);
        assert_eq!(SCREAMING_FLAGON_DAMAGE, 20);
        assert_eq!(SLING_OF_COURAGE_STRENGTH, 2);
        assert_eq!(THE_ABACUS_BLOCK, 6);
        assert_eq!(RUNIC_CAPACITOR_ORB_SLOTS, 3);
        assert_eq!(ALCHEMICAL_COFFER_POTION_SLOTS, 4);
        assert_eq!(BLESSED_ANTLER_ENERGY, 1);
        assert_eq!(BLESSED_ANTLER_DAZED_COUNT, 3);
        assert_eq!(BLOOD_SOAKED_ROSE_ENERGY, 1);
        assert_eq!(CURSED_PEARL_GOLD, 333);
        assert_eq!(ECTOPLASM_ENERGY, 1);
        assert_eq!(SAI_BLOCK, 7);
        assert_eq!(SNECKO_EYE_EXTRA_DRAW, 2);
        assert_eq!(SOZU_ENERGY, 1);
        assert_eq!(SPIKED_GAUNTLETS_ENERGY, 1);
    }

    /// `Brimstone` behavioural pin (bead `sts2-rs-o39p` wave 0):
    /// the `before_side_turn_start` listener applies STRENGTH(+2) to
    /// the player and STRENGTH(+1) to every alive enemy on each
    /// player turn — no round gate. Mirrors Python
    /// `shop_event.py::Brimstone.before_side_turn_start`.
    #[test]
    fn brimstone_applies_strength_to_player_and_every_alive_enemy() {
        use smallvec::SmallVec;
        use std::num::NonZeroU32;
        use sts2_core::card_effects::EffectResult;
        use sts2_core::relic_effects::dispatch_before_side_turn_start;
        use sts2_core::{
            Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, PowerKind, Relic,
            RelicKind, Side,
        };

        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();

        // Player creature row + uid (Brimstone listener needs
        // `state.player.creature_uid` to be Some).
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let player_uid = state.init_player_creature();
        state.sync_player_creature();

        // Two alive enemies + one dead one (the listener must skip
        // the corpse so STR is only applied to the alive set).
        let mut push_enemy = |raw: u32, hp: i32| -> CreatureUid {
            let uid = CreatureUid::new(raw).expect("non-zero");
            state.creatures.push(Creature {
                id: uid.0,
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
            state.enemy_uids.push(uid);
            uid
        };
        let enemy_a = push_enemy(10, 30);
        let enemy_b = push_enemy(11, 30);
        let _corpse = push_enemy(12, 0);

        // Equip Brimstone.
        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::Brimstone,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        // Fire the listener on the player side-turn-start.
        assert_eq!(
            dispatch_before_side_turn_start(&mut state, Side::Player),
            EffectResult::Resolved
        );

        let power_amount_on = |state: &CombatState, uid: CreatureUid, kind: PowerKind| -> i32 {
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .map_or(0, |c| {
                    c.powers
                        .iter()
                        .filter_map(|p_uid| state.powers.get_by_id(p_uid.0))
                        .find(|p| p.kind == kind)
                        .map_or(0, |p| p.amount)
                })
        };

        assert_eq!(
            power_amount_on(&state, player_uid, PowerKind::Strength),
            BRIMSTONE_SELF_STRENGTH,
            "Brimstone must apply SELF_STRENGTH(+2) to the player"
        );
        assert_eq!(
            power_amount_on(&state, enemy_a, PowerKind::Strength),
            BRIMSTONE_ENEMY_STRENGTH,
            "Brimstone must apply ENEMY_STRENGTH(+1) to alive enemy A"
        );
        assert_eq!(
            power_amount_on(&state, enemy_b, PowerKind::Strength),
            BRIMSTONE_ENEMY_STRENGTH,
            "Brimstone must apply ENEMY_STRENGTH(+1) to alive enemy B"
        );

        // Enemy side-turn-start must be inert (mirrors the Python
        // `if side == PLAYER:` guard).
        let mut fresh = CombatState::default();
        fresh.player.current_hp = 80;
        fresh.player.max_hp = 80;
        let _ = fresh.init_player_creature();
        fresh.sync_player_creature();
        fresh.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::Brimstone,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = dispatch_before_side_turn_start(&mut fresh, Side::Enemy);
        assert_eq!(
            power_amount_on(
                &fresh,
                fresh.player.creature_uid.unwrap(),
                PowerKind::Strength
            ),
            0,
            "Brimstone must NOT fire on enemy side-turn-start"
        );
    }

    /// `PhilosophersStone` behavioural pin (bead `sts2-rs-w0wl`):
    /// the `before_combat_start` listener applies STRENGTH(+1) to every
    /// alive enemy. The `modify_max_energy: +1` leg is deferred to a
    /// separate bead and is NOT exercised here. Mirrors Python
    /// `shop_event.py::PhilosophersStone.before_combat_start`.
    #[test]
    fn philosophers_stone_applies_strength_to_every_alive_enemy_at_combat_start() {
        use smallvec::SmallVec;
        use std::num::NonZeroU32;
        use sts2_core::card_effects::EffectResult;
        use sts2_core::relic_effects::dispatch_before_combat_start;
        use sts2_core::{
            Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, PowerKind, Relic,
            RelicKind,
        };

        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let player_uid = state.init_player_creature();
        state.sync_player_creature();

        let mut push_enemy = |raw: u32, hp: i32| -> CreatureUid {
            let uid = CreatureUid::new(raw).expect("non-zero");
            state.creatures.push(Creature {
                id: uid.0,
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
            state.enemy_uids.push(uid);
            uid
        };
        let enemy_a = push_enemy(10, 30);
        let enemy_b = push_enemy(11, 30);
        let _corpse = push_enemy(12, 0);

        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::PhilosophersStone,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        assert_eq!(
            dispatch_before_combat_start(&mut state),
            EffectResult::Resolved
        );

        let power_amount_on = |state: &CombatState, uid: CreatureUid, kind: PowerKind| -> i32 {
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .map_or(0, |c| {
                    c.powers
                        .iter()
                        .filter_map(|p_uid| state.powers.get_by_id(p_uid.0))
                        .find(|p| p.kind == kind)
                        .map_or(0, |p| p.amount)
                })
        };

        assert_eq!(
            power_amount_on(&state, enemy_a, PowerKind::Strength),
            PHILOSOPHERS_STONE_ENEMY_STRENGTH,
            "PhilosophersStone must apply ENEMY_STRENGTH(+1) to alive enemy A"
        );
        assert_eq!(
            power_amount_on(&state, enemy_b, PowerKind::Strength),
            PHILOSOPHERS_STONE_ENEMY_STRENGTH,
            "PhilosophersStone must apply ENEMY_STRENGTH(+1) to alive enemy B"
        );
        // Dead enemy must not receive Strength.
        assert_eq!(
            power_amount_on(&state, CreatureUid::new(12).unwrap(), PowerKind::Strength),
            0,
            "PhilosophersStone must skip corpses (current_hp == 0)"
        );
        // Player must NOT receive Strength (PhilosophersStone is enemy-only).
        assert_eq!(
            power_amount_on(&state, player_uid, PowerKind::Strength),
            0,
            "PhilosophersStone must NOT apply Strength to the player"
        );
    }

    /// `Brimstone` is inert when the player does not own the relic
    /// (the listener's `player_owns_relic` gate must short-circuit
    /// before applying any Strength).
    #[test]
    fn brimstone_inert_when_not_owned() {
        use sts2_core::card_effects::EffectResult;
        use sts2_core::relic_effects::dispatch_before_side_turn_start;
        use sts2_core::{PowerKind, Side};

        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let player_uid = state.init_player_creature();
        state.sync_player_creature();

        // No relic equipped.
        let res = dispatch_before_side_turn_start(&mut state, Side::Player);
        // No listeners registered for this kind → channel returns
        // Resolved (no-op) regardless.
        assert!(matches!(
            res,
            EffectResult::Resolved | EffectResult::NotImplemented
        ));

        let strength = state
            .creatures
            .iter()
            .find(|c| c.id == player_uid.0)
            .map_or(0, |c| {
                c.powers
                    .iter()
                    .filter_map(|p_uid| state.powers.get_by_id(p_uid.0))
                    .find(|p| p.kind == PowerKind::Strength)
                    .map_or(0, |p| p.amount)
            });
        assert_eq!(strength, 0, "Brimstone listener must be owner-gated");
    }

    /// `BoomingConch` behavioural pin (bead `sts2-rs-o39p` wave 1):
    /// the `modify_hand_draw` listener returns `base + EXTRA_DRAW=2`
    /// when the player owns the relic, the round is 1, and the
    /// combat is an elite. All three gates are tested independently
    /// (round != 1, !is_elite, not owned) — each must short-circuit
    /// to `base` (no bonus draw). Mirrors Python
    /// `shop_event.py::BoomingConch.modify_hand_draw`.
    #[test]
    fn booming_conch_modify_hand_draw_round_1_elite_only() {
        use std::num::NonZeroU32;
        use sts2_core::relic_effects::dispatch_modify_hand_draw;
        use sts2_core::{Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();

        let push_conch = |state: &mut CombatState| {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::BoomingConch,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        };

        // (a) Round 1 + is_elite + owned → +2 draw.
        let mut s = CombatState {
            round: 1,
            is_elite: true,
            is_boss: true,
            act_index: 1,
            ..CombatState::default()
        };
        push_conch(&mut s);
        assert_eq!(
            dispatch_modify_hand_draw(&s, 5),
            5 + BOOMING_CONCH_EXTRA_DRAW,
            "round 1 + elite + owned must add EXTRA_DRAW"
        );

        // (b) Round 2 + is_elite + owned → no bonus (round gate).
        let mut s = CombatState {
            round: 2,
            is_elite: true,
            is_boss: true,
            act_index: 1,
            ..CombatState::default()
        };
        push_conch(&mut s);
        assert_eq!(
            dispatch_modify_hand_draw(&s, 5),
            5,
            "round > 1 must short-circuit to base"
        );

        // (c) Round 1 + !is_elite + owned → no bonus (elite gate).
        let mut s = CombatState {
            round: 1,
            is_elite: false,
            is_boss: false,
            act_index: 1,
            ..CombatState::default()
        };
        push_conch(&mut s);
        assert_eq!(
            dispatch_modify_hand_draw(&s, 5),
            5,
            "non-elite combat must short-circuit to base"
        );

        // (d) Round 1 + is_elite + NOT owned → no bonus (owner gate).
        let s = CombatState {
            round: 1,
            is_elite: true,
            is_boss: true,
            act_index: 1,
            ..CombatState::default()
        };
        // No push_conch call — relic not owned.
        assert_eq!(
            dispatch_modify_hand_draw(&s, 5),
            5,
            "BoomingConch listener must be owner-gated"
        );
    }

    /// `TheAbacus` behavioural pin (bead `sts2-rs-k8vc` shop-tier
    /// wave 0): the `after_shuffle_bus` listener grants
    /// `THE_ABACUS_BLOCK=6` to the player on every shuffle event.
    /// Routed through `gain_block_player` so Frail/Dexterity folds
    /// in (this fixture has neither, so the raw +6 is observable).
    /// Mirrors Python `shop_event.py::TheAbacus.after_shuffle`.
    #[test]
    fn the_abacus_gains_six_block_on_shuffle() {
        use std::num::NonZeroU32;
        use sts2_core::relic_effects::{dispatch_after_shuffle_bus, ShuffleCtx};
        use sts2_core::{Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();

        // Player creature row + uid so `gain_block_player` lands
        // on the mirrored creature record.
        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();

        // Equip TheAbacus.
        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::TheAbacus,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        // Fire the listener (auto-reshuffle path).
        let _ = dispatch_after_shuffle_bus(
            &mut state,
            &ShuffleCtx {
                from_reshuffle: true,
            },
        );
        assert_eq!(
            state.player.block, THE_ABACUS_BLOCK,
            "TheAbacus must grant BLOCK=6 on shuffle"
        );

        // Second shuffle stacks (no per-combat cap in the pin).
        let _ = dispatch_after_shuffle_bus(
            &mut state,
            &ShuffleCtx {
                from_reshuffle: false,
            },
        );
        assert_eq!(
            state.player.block,
            THE_ABACUS_BLOCK * 2,
            "TheAbacus stacks across multiple shuffle events"
        );
    }

    /// `SlingOfCourage` behavioural pin (bead `sts2-rs-k8vc` shop-tier
    /// wave 1): in elite combat, the `before_combat_start` listener
    /// grants the player `STRENGTH(+SLING_OF_COURAGE_STRENGTH=2)`.
    /// Mirrors Python `shop_event.py::SlingOfCourage.before_combat_start`.
    #[test]
    fn sling_of_courage_grants_strength_in_elite() {
        use std::num::NonZeroU32;
        use sts2_core::powers::PowerKind;
        use sts2_core::relic_effects::dispatch_before_combat_start;
        use sts2_core::{Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();

        let mut state = CombatState {
            is_elite: true,
            is_boss: true,
            act_index: 1,
            ..CombatState::default()
        };
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();

        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::SlingOfCourage,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        let _ = dispatch_before_combat_start(&mut state);

        let player_uid = state.player.creature_uid.expect("player uid");
        let strength = state
            .creatures
            .iter()
            .find(|c| c.id == player_uid.0)
            .map_or(0, |c| {
                c.powers
                    .iter()
                    .filter_map(|p_uid| state.powers.get_by_id(p_uid.0))
                    .find(|p| p.kind == PowerKind::Strength)
                    .map_or(0, |p| p.amount)
            });
        assert_eq!(
            strength, SLING_OF_COURAGE_STRENGTH,
            "SlingOfCourage must grant STRENGTH=2 at elite combat start"
        );
    }

    /// `SlingOfCourage` is inert in non-elite combats.
    #[test]
    fn sling_of_courage_inert_in_non_elite() {
        use std::num::NonZeroU32;
        use sts2_core::powers::PowerKind;
        use sts2_core::relic_effects::dispatch_before_combat_start;
        use sts2_core::{Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();

        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();

        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::SlingOfCourage,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        let _ = dispatch_before_combat_start(&mut state);

        let player_uid = state.player.creature_uid.expect("player uid");
        let has_strength = state
            .creatures
            .iter()
            .find(|c| c.id == player_uid.0)
            .is_some_and(|c| {
                c.powers
                    .iter()
                    .filter_map(|p_uid| state.powers.get_by_id(p_uid.0))
                    .any(|p| p.kind == PowerKind::Strength)
            });
        assert!(
            !has_strength,
            "SlingOfCourage must not fire outside elite rooms"
        );
    }

    /// `SlingOfCourage` is inert when the player does not own the relic.
    #[test]
    fn sling_of_courage_inert_when_not_owned() {
        use sts2_core::powers::PowerKind;
        use sts2_core::relic_effects::dispatch_before_combat_start;

        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState {
            is_elite: true,
            is_boss: true,
            act_index: 1,
            ..CombatState::default()
        };
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();

        let _ = dispatch_before_combat_start(&mut state);

        let player_uid = state.player.creature_uid.expect("player uid");
        let has_strength = state
            .creatures
            .iter()
            .find(|c| c.id == player_uid.0)
            .is_some_and(|c| {
                c.powers
                    .iter()
                    .filter_map(|p_uid| state.powers.get_by_id(p_uid.0))
                    .any(|p| p.kind == PowerKind::Strength)
            });
        assert!(!has_strength, "SlingOfCourage listener must be owner-gated");
    }

    /// Bead `sts2-rs-bmw3`: LeesWaffle's `after_obtained` listener must
    /// bump max HP by `LEES_WAFFLE_MAX_HP = 7` and then heal the player to
    /// full. Pins the Python `gain_max_hp(7)` + `heal(max_hp)` sequence.
    #[test]
    fn listener_lees_waffle_gains_seven_max_hp_and_heals_to_full() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // Set up an injured player so the heal leg is observable.
        rs.current_hp = 50;
        let before_max = rs.max_hp;
        rs.relics.push(RelicId::LeesWaffle);
        rs.last_obtained_relic = Some(RelicId::LeesWaffle);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(
            rs.max_hp,
            before_max + LEES_WAFFLE_MAX_HP,
            "LeesWaffle must bump max HP by 7"
        );
        assert_eq!(
            rs.current_hp, rs.max_hp,
            "LeesWaffle must heal the player to full after the max-HP bump"
        );
    }

    /// Bead `sts2-rs-run-rest-option`: PumpkinCandle's (v0.107.1)
    /// `AfterObtained → Rekindle` listener adds `kindleAmount = 5` to
    /// `KindleCount` (`rs.relic_counters[PumpkinCandle]`), independent
    /// of act. The combat-side `modify_max_energy` arm then grants +1
    /// max energy while that count is `> 0`.
    #[test]
    fn listener_pumpkin_candle_rekindles_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        for act in [1_u8, 2, 3] {
            let mut rs = RunState::new();
            rs.act_index = act;
            rs.relics.push(RelicId::PumpkinCandle);
            rs.last_obtained_relic = Some(RelicId::PumpkinCandle);
            let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
            assert_eq!(
                rs.relic_counters.get(&RelicId::PumpkinCandle).copied(),
                Some(5),
                "PumpkinCandle Rekindle must set KindleCount=5 on obtain (act={act})",
            );
        }
    }

    /// PumpkinCandle's rest-site listener appends the KINDLE option
    /// while owned, and `do_rest_site_action(Kindle)` Rekindles the
    /// count by another 5 (C# `KindleRestSiteOption.OnSelect →
    /// Rekindle`). Together with `after_obtained` this gives the full
    /// KindleCount lifecycle.
    #[test]
    fn listener_pumpkin_candle_rest_site_kindle_option_and_rekindle() {
        use sts2_core::runstate::{RestSiteActionOutcome, RestSiteOption, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::PumpkinCandle);
        // Simulate pickup Rekindle (KindleCount = 5).
        rs.last_obtained_relic = Some(RelicId::PumpkinCandle);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);

        // The rest-site option list now offers KINDLE alongside the base
        // Heal/Smith pair.
        let opts = rs.rest_site_options();
        assert!(
            opts.iter().any(|o| matches!(o, RestSiteOption::Kindle)),
            "PumpkinCandle owner must be offered the Kindle rest-site option",
        );

        // Selecting Kindle Rekindles: 5 -> 10.
        let outcome = rs.do_rest_site_action(RestSiteOption::Kindle);
        assert_eq!(outcome, RestSiteActionOutcome::Kindled);
        assert_eq!(
            rs.relic_counters.get(&RelicId::PumpkinCandle).copied(),
            Some(10),
            "Kindle rest-site pick must add 5 to KindleCount",
        );
    }

    /// PumpkinCandle's listener is a no-op when the relic isn't
    /// owned — guards against a stray write into `relic_counters`
    /// from a dispatch fired for an unrelated relic.
    #[test]
    fn listener_pumpkin_candle_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.act_index = 2;
        // Do NOT push PumpkinCandle into rs.relics.
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(
            !rs.relic_counters.contains_key(&RelicId::PumpkinCandle),
            "PumpkinCandle counter must not be written when the relic is unowned",
        );
    }

    // ── DingyRug — bead sts2-rs-u5emw ─────────────────────────────

    /// DingyRug's `modify_card_reward_creation_options` listener
    /// must flip `has_custom_card_pool = true`, clear
    /// `character_ids`, clear `use_default_character_pool`, and
    /// populate `custom_card_ids` with the union of the active
    /// character's pool + the colorless pool. Mirrors Python
    /// `DingyRug.modify_card_reward_creation_options`'s return
    /// shape.
    #[test]
    fn listener_dingy_rug_populates_custom_pool_with_colorless_union() {
        use sts2_core::runstate::{CardRewardGenerationOptions, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // Ironclad pool is the canonical non-empty per-character
        // slice at the current pin.
        rs.character_id = sts2_core::potions::CharacterId::Ironclad;
        rs.relics.push(RelicId::DingyRug);
        let mut opts = CardRewardGenerationOptions::default();

        let res = sts2_core::run_effects::dispatch_modify_card_reward_creation_options(
            &mut rs, &mut opts,
        );
        assert_eq!(res, sts2_core::card_effects::EffectResult::Resolved);

        assert!(opts.has_custom_card_pool, "must flip has_custom_card_pool");
        assert!(
            !opts.use_default_character_pool,
            "must clear use_default_character_pool",
        );
        assert!(opts.character_ids.is_empty(), "must clear character_ids");
        assert!(
            !opts.custom_card_ids.is_empty(),
            "must populate custom_card_ids",
        );
        // No duplicates in the final slate.
        let unique: std::collections::HashSet<_> = opts.custom_card_ids.iter().copied().collect();
        assert_eq!(
            unique.len(),
            opts.custom_card_ids.len(),
            "custom_card_ids must be deduped",
        );
        // At least one colorless card is present (the colorless slice
        // is non-empty at the pin — 25+ entries). Pick a stable
        // canonical colorless card to assert on so the test doesn't
        // brittle-pin on the full slice length.
        assert!(
            opts.custom_card_ids
                .contains(&sts2_core::cards::CardId::Discovery),
            "Discovery (colorless) must be unioned into the slate",
        );
    }

    /// DingyRug's listener is a no-op when the relic isn't owned —
    /// the channel walks every registered listener, so an unowned
    /// relic must self-gate.
    #[test]
    fn listener_dingy_rug_is_noop_without_ownership() {
        use sts2_core::runstate::{CardRewardGenerationOptions, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut opts = CardRewardGenerationOptions::default();
        // Do NOT push DingyRug into rs.relics.
        let _ = sts2_core::run_effects::dispatch_modify_card_reward_creation_options(
            &mut rs, &mut opts,
        );
        assert!(
            !opts.has_custom_card_pool,
            "has_custom_card_pool must stay false when DingyRug is unowned",
        );
        assert!(
            opts.custom_card_ids.is_empty(),
            "custom_card_ids must stay empty when DingyRug is unowned",
        );
    }

    /// DingyRug honors `allow_card_pool_modifications=false` (the
    /// per-relic skip handshake): when the flag is clear, the
    /// listener returns NotImplemented and leaves `opts` untouched.
    #[test]
    fn listener_dingy_rug_skips_when_pool_modifications_disabled() {
        use sts2_core::runstate::{CardRewardGenerationOptions, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.character_id = sts2_core::potions::CharacterId::Ironclad;
        rs.relics.push(RelicId::DingyRug);
        let mut opts = CardRewardGenerationOptions {
            allow_card_pool_modifications: false,
            ..CardRewardGenerationOptions::default()
        };

        let _ = sts2_core::run_effects::dispatch_modify_card_reward_creation_options(
            &mut rs, &mut opts,
        );

        assert!(
            !opts.has_custom_card_pool,
            "has_custom_card_pool must stay false with allow_card_pool_modifications=false",
        );
        assert!(
            opts.custom_card_ids.is_empty(),
            "custom_card_ids must stay empty with allow_card_pool_modifications=false",
        );
    }

    // ── FurCoat — bead eboqq ──────────────────────────────────────

    /// Bead `sts2-rs-eboqq`: FurCoat's `after_obtained` listener
    /// must latch `rs.act_index` into `rs.fur_coat_act_index` on
    /// pickup so the late map-mark listener can act-gate. Latch is
    /// idempotent — once `Some`, never reassigned.
    #[test]
    fn listener_fur_coat_latches_obtain_act_on_pickup() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.act_index = 2;
        rs.relics.push(RelicId::FurCoat);
        assert!(rs.fur_coat_act_index.is_none(), "fresh run has no latch");
        rs.last_obtained_relic = Some(RelicId::FurCoat);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(
            rs.fur_coat_act_index,
            Some(2),
            "after_obtained must latch act_index into fur_coat_act_index"
        );
        rs.act_index = 3;
        rs.last_obtained_relic = Some(RelicId::FurCoat);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(
            rs.fur_coat_act_index,
            Some(2),
            "latch is sticky — re-dispatch in act 3 must not overwrite"
        );
    }

    /// Bead `sts2-rs-eboqq`: FurCoat's `after_obtained` listener is
    /// a no-op when the relic is not owned.
    #[test]
    fn listener_fur_coat_after_obtained_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.act_index = 2;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(
            rs.fur_coat_act_index.is_none(),
            "no FurCoat in rs.relics → no latch"
        );
    }

    /// Bead `sts2-rs-eboqq`: FurCoat's late map-mark listener picks
    /// exactly `FUR_COAT_COMBATS = 7` Monster/Elite rooms and marks
    /// them via the map-layer registry from mxbai. Idempotent on
    /// re-fire.
    #[test]
    fn listener_fur_coat_marks_seven_monster_or_elite_rooms() {
        use sts2_core::map::{ActMap, MapCoord, MapPointType};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.act_index = 1;
        rs.fur_coat_act_index = Some(1);
        rs.relics.push(RelicId::FurCoat);
        let mut map = ActMap::new(15);
        for row in 1..=4 {
            for col in 0..3 {
                let idx = map.get_or_create(col, row);
                map.point_mut(idx).point_type = if (col + row) % 2 == 0 {
                    MapPointType::Monster
                } else {
                    MapPointType::Elite
                };
            }
        }
        let result =
            sts2_core::run_effects::dispatch_modify_generated_map_late(&mut rs, &mut map, 1);
        assert_eq!(result, sts2_core::card_effects::EffectResult::Resolved);
        let marked: Vec<MapCoord> = map.marked_coords_for(RelicId::FurCoat);
        assert_eq!(
            marked.len(),
            FUR_COAT_COMBATS,
            "FurCoat must mark exactly 7 rooms"
        );
        let snapshot: std::collections::BTreeSet<MapCoord> = marked.iter().copied().collect();
        let _ = sts2_core::run_effects::dispatch_modify_generated_map_late(&mut rs, &mut map, 1);
        let after: std::collections::BTreeSet<MapCoord> = map
            .marked_coords_for(RelicId::FurCoat)
            .into_iter()
            .collect();
        assert_eq!(snapshot, after, "re-dispatch must be idempotent");
    }

    /// Bead `sts2-rs-eboqq`: FurCoat's late map-mark listener
    /// no-ops in any act other than the obtain act, matching
    /// Python's `current_act_index != _fur_coat_act_index` guard.
    #[test]
    fn listener_fur_coat_late_noops_on_wrong_act() {
        use sts2_core::map::{ActMap, MapPointType};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.act_index = 2;
        rs.fur_coat_act_index = Some(1);
        rs.relics.push(RelicId::FurCoat);
        let mut map = ActMap::new(15);
        for col in 0..5 {
            let idx = map.get_or_create(col, 1);
            map.point_mut(idx).point_type = MapPointType::Monster;
        }
        let _ = sts2_core::run_effects::dispatch_modify_generated_map_late(&mut rs, &mut map, 2);
        assert!(
            map.marked_coords_for(RelicId::FurCoat).is_empty(),
            "FurCoat must not mark rooms in a non-obtain act"
        );
    }

    /// Bead `sts2-rs-r4pr`: LoomingFruit's `after_obtained` listener must
    /// bump max HP by `LOOMING_FRUIT_MAX_HP = 31` when the run owns the relic.
    #[test]
    fn listener_looming_fruit_gains_thirty_one_max_hp_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_max = rs.max_hp;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::LoomingFruit);
        rs.last_obtained_relic = Some(RelicId::LoomingFruit);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max + LOOMING_FRUIT_MAX_HP);
        assert_eq!(rs.current_hp, before_hp + LOOMING_FRUIT_MAX_HP);
    }

    /// Bead `sts2-rs-r4pr`: NutritiousOyster's `after_obtained` listener
    /// must bump max HP by `NUTRITIOUS_OYSTER_MAX_HP = 11` when owned.
    #[test]
    fn listener_nutritious_oyster_gains_eleven_max_hp_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_max = rs.max_hp;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::NutritiousOyster);
        rs.last_obtained_relic = Some(RelicId::NutritiousOyster);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max + NUTRITIOUS_OYSTER_MAX_HP);
        assert_eq!(rs.current_hp, before_hp + NUTRITIOUS_OYSTER_MAX_HP);
    }

    /// Bead `sts2-rs-ytiok` (70anj.4.10): NewLeaf's `after_obtained`
    /// listener must open a 1-card transform prompt
    /// ([`sts2_core::ChoiceKind::CardSelect`]) on the master deck
    /// when the run owns the relic. Pins the prompt-opener branch
    /// (the in-place transform branch is deferred — see
    /// [`listener_new_leaf`]).
    #[test]
    fn new_leaf_opens_transform_prompt() {
        use sts2_core::cards::{Card, CardId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // Master deck needs at least 1 card or
        // `offer_transform_cards_reward` no-ops.
        rs.master_deck.push(Card {
            card_id: CardId::StrikeIronclad,
            upgraded: false,
            ..Card::pod_defaults()
        });
        rs.relics.push(RelicId::NewLeaf);
        assert!(
            rs.pending_choice.is_none(),
            "precondition: no prompt before fire"
        );
        rs.last_obtained_relic = Some(RelicId::NewLeaf);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(
            rs.pending_choice.is_some(),
            "NewLeaf must open a transform-1 prompt on pickup"
        );
    }

    /// Bead `sts2-rs-ytiok` (70anj.4.10): NewLeaf listener is a no-op
    /// when the run does not own the relic. Mirrors the canonical
    /// ownership-gate pattern (GoldenPearl, LeesWaffle, …).
    #[test]
    fn new_leaf_listener_is_noop_without_ownership() {
        use sts2_core::cards::{Card, CardId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck.push(Card {
            card_id: CardId::StrikeIronclad,
            upgraded: false,
            ..Card::pod_defaults()
        });
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(
            rs.pending_choice.is_none(),
            "no NewLeaf in rs.relics → no prompt opens"
        );
    }

    /// Bead `sts2-rs-yfi0`: GoldenPearl's `after_obtained` listener must
    /// grant `GOLDEN_PEARL_GOLD = 150` to `rs.gold` when owned.
    #[test]
    fn listener_golden_pearl_gains_one_hundred_fifty_gold_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_gold = rs.gold;
        rs.relics.push(RelicId::GoldenPearl);
        rs.last_obtained_relic = Some(RelicId::GoldenPearl);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.gold, before_gold + GOLDEN_PEARL_GOLD);
    }

    /// Bead `sts2-rs-yfi0`: GoldenPearl listener is noop without ownership.
    #[test]
    fn listener_golden_pearl_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_gold = rs.gold;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.gold, before_gold);
    }

    /// Bead `sts2-rs-2ciy`: SignetRing's `after_obtained` listener must
    /// grant `SIGNET_RING_GOLD = 999` to `rs.gold` when owned.
    #[test]
    fn listener_signet_ring_gains_nine_hundred_ninety_nine_gold_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_gold = rs.gold;
        rs.relics.push(RelicId::SignetRing);
        rs.last_obtained_relic = Some(RelicId::SignetRing);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.gold, before_gold + SIGNET_RING_GOLD);
    }

    /// Bead `sts2-rs-2ciy`: SignetRing listener is noop without ownership.
    #[test]
    fn listener_signet_ring_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_gold = rs.gold;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.gold, before_gold);
    }

    /// Bead `sts2-rs-psf9`: Orrery's `after_obtained` listener must push
    /// exactly `ORRERY_CARDS = 5` `Reward::Card` sentinels to
    /// `rs.pending_rewards` when owned.
    #[test]
    fn listener_orrery_pushes_five_card_reward_sentinels_on_obtain() {
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        rs.relics.push(RelicId::Orrery);
        rs.last_obtained_relic = Some(RelicId::Orrery);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before_len + ORRERY_CARDS);
        let pushed = &rs.pending_rewards[before_len..];
        assert_eq!(pushed.len(), 5);
        for reward in pushed {
            assert!(
                matches!(reward, Reward::Card(_)),
                "Orrery must push Reward::Card sentinels, got {reward:?}"
            );
        }
    }

    /// Bead `sts2-rs-beda` (corrected by `sts2-rs-3bigm`):
    /// LeafyPoultice's `after_obtained` listener must reduce
    /// `rs.max_hp` and `rs.current_hp` by
    /// `LEAFY_POULTICE_MAX_HP_LOSS = 12` (C# `MaxHpVar(12m)`).
    #[test]
    fn listener_leafy_poultice_loses_twelve_max_hp_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_max = rs.max_hp;
        let before_cur = rs.current_hp;
        rs.relics.push(RelicId::LeafyPoultice);
        rs.last_obtained_relic = Some(RelicId::LeafyPoultice);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max - LEAFY_POULTICE_MAX_HP_LOSS);
        assert_eq!(rs.current_hp, before_cur - LEAFY_POULTICE_MAX_HP_LOSS);
    }

    /// Bead `sts2-rs-beda` (transform leg): LeafyPoultice transforms the
    /// **first** Basic Strike and **first** Basic Defend in the deck —
    /// NOT every basic (that is PandorasBox). With 5 Strikes + 4 Defends
    /// seeded, exactly one Strike (index 0) and one Defend (first Defend
    /// slot) are swapped for non-basic character-pool cards, leaving
    /// 4 Strikes + 3 Defends; the deck length is preserved (in-place
    /// transform).
    #[test]
    fn listener_leafy_poultice_transforms_first_strike_and_first_defend() {
        use sts2_core::cards::CardId as InnerCardId;
        use sts2_core::potions::CharacterId;
        use sts2_core::run_effects::dispatch_after_obtained;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.character_id = CharacterId::Ironclad;
        rs.relics.push(RelicId::LeafyPoultice);
        let deck_len = pandoras_box_seed_ironclad_deck(&mut rs);

        rs.last_obtained_relic = Some(RelicId::LeafyPoultice);
        let _ = dispatch_after_obtained(&mut rs);

        assert_eq!(
            rs.master_deck.len(),
            deck_len,
            "transform_specific_cards removes+appends — deck length preserved"
        );
        let strikes = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == InnerCardId::StrikeIronclad)
            .count();
        let defends = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == InnerCardId::DefendIronclad)
            .count();
        assert_eq!(strikes, 4, "exactly the FIRST basic Strike is transformed");
        assert_eq!(defends, 3, "exactly the FIRST basic Defend is transformed");
        // C# deck transform removes the original and appends the replacement
        // to the tail, in ascending original-index order. The first Strike
        // (idx 0) and first Defend (idx 5) are removed; their replacements
        // append last, Strike's before Defend's.
        assert_ne!(
            rs.master_deck[deck_len - 2].card_id,
            InnerCardId::StrikeIronclad,
            "first Strike's replacement appended second-to-last"
        );
        assert_ne!(
            rs.master_deck[deck_len - 1].card_id,
            InnerCardId::DefendIronclad,
            "first Defend's replacement appended last"
        );
        // The anchor non-basic shifts up two slots (two cards removed before
        // it, two appended after it).
        assert_eq!(rs.master_deck[deck_len - 3].card_id, InnerCardId::Anger);
        // The surviving basics keep their relative head order.
        assert_eq!(rs.master_deck[0].card_id, InnerCardId::StrikeIronclad);
        assert_eq!(rs.master_deck[4].card_id, InnerCardId::DefendIronclad);
        // GH#31: each appended replacement must be spec-populated (not a
        // blank `pod_defaults()`), so its full stats reach combat. Compare
        // against the canonical POD the spec index builds for that id.
        for slot in [deck_len - 2, deck_len - 1] {
            let got = &rs.master_deck[slot];
            let want = crate::cards::spec_index::card_pod_from_id(got.card_id, false)
                .expect("rolled replacement id resolves to a spec");
            assert_eq!(
                got.base_damage, want.base_damage,
                "replacement carries its spec base_damage, not pod_defaults"
            );
            assert_eq!(
                got.base_block, want.base_block,
                "replacement carries its spec base_block, not pod_defaults"
            );
            assert_eq!(got.cost, want.cost, "replacement carries its spec cost");
        }
    }

    /// LeafyPoultice finds the Strike / Defend to transform by deck POSITION:
    /// `master_deck.position(|card| card.card_id == original_id)` (line 2526).
    /// The `==` → `!=` mutant matches the first card whose id DIFFERS from the
    /// target, transforming the wrong slot.
    ///
    /// Fixture: deck `[Anger, Strike, Defend]` — the basics are NOT at the
    /// head. Correct logic finds Strike at index 1 and Defend at index 2 and
    /// transforms both, leaving ZERO basic Strike/Defend. The `!=` mutant finds
    /// index 0 (Anger ≠ Strike, and Anger ≠ Defend) for BOTH, so it never
    /// touches the real Strike/Defend — they survive.
    #[test]
    fn listener_leafy_poultice_finds_basics_by_position() {
        use sts2_core::cards::CardId as InnerCardId;
        use sts2_core::potions::CharacterId;
        use sts2_core::run_effects::dispatch_after_obtained;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.character_id = CharacterId::Ironclad;
        rs.relics.push(RelicId::LeafyPoultice);
        rs.master_deck.clear();
        for id in [
            InnerCardId::Anger,
            InnerCardId::StrikeIronclad,
            InnerCardId::DefendIronclad,
        ] {
            let mut c = sts2_core::Card::pod_defaults();
            c.card_id = id;
            rs.master_deck.push(c);
        }

        rs.last_obtained_relic = Some(RelicId::LeafyPoultice);
        let _ = dispatch_after_obtained(&mut rs);

        let strikes = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == InnerCardId::StrikeIronclad)
            .count();
        let defends = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == InnerCardId::DefendIronclad)
            .count();
        assert_eq!(
            strikes, 0,
            "the Strike at index 1 must be transformed (a `!=` match would skip it)"
        );
        assert_eq!(
            defends, 0,
            "the Defend at index 2 must be transformed (a `!=` match would skip it)"
        );
    }

    /// Bead `sts2-rs-beda`: the transform leg is deterministic under a
    /// fixed seed — two identical runs produce the same replacement
    /// cards (the `transformations` stream is seeded from the run seed).
    #[test]
    fn listener_leafy_poultice_transform_is_deterministic() {
        use sts2_core::potions::CharacterId;
        use sts2_core::run_effects::dispatch_after_obtained;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let run = || {
            let mut rs = RunState::new();
            rs.character_id = CharacterId::Ironclad;
            rs.relics.push(RelicId::LeafyPoultice);
            pandoras_box_seed_ironclad_deck(&mut rs);
            rs.last_obtained_relic = Some(RelicId::LeafyPoultice);
            let _ = dispatch_after_obtained(&mut rs);
            rs.master_deck.iter().map(|c| c.card_id).collect::<Vec<_>>()
        };
        assert_eq!(run(), run(), "same seed → same transformed deck");
    }

    /// Bead `sts2-rs-64vb`: MassiveScroll's `after_obtained` listener
    /// must push `MASSIVE_SCROLL_CARDS = 3` `Reward::Card` sentinels
    /// to `rs.pending_rewards` when owned.
    #[test]
    fn listener_massive_scroll_pushes_three_card_reward_sentinels_on_obtain() {
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        rs.relics.push(RelicId::MassiveScroll);
        rs.last_obtained_relic = Some(RelicId::MassiveScroll);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before_len + MASSIVE_SCROLL_CARDS);
        let pushed = &rs.pending_rewards[before_len..];
        for reward in pushed {
            assert!(
                matches!(reward, Reward::Card(_)),
                "MassiveScroll must push Reward::Card sentinels, got {reward:?}"
            );
        }
    }

    /// Bead `sts2-rs-64vb`: MassiveScroll listener is noop without ownership.
    #[test]
    fn listener_massive_scroll_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before_len);
    }

    /// Bead `sts2-rs-2nrk`: Cauldron's `after_obtained` listener must
    /// push `CAULDRON_POTIONS = 5` `Reward::Potion` sentinels to
    /// `rs.pending_rewards` when owned.
    #[test]
    fn listener_cauldron_pushes_five_potion_reward_sentinels_on_obtain() {
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        rs.relics.push(RelicId::Cauldron);
        rs.last_obtained_relic = Some(RelicId::Cauldron);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(
            rs.pending_rewards.len(),
            before_len + CAULDRON_POTIONS as usize
        );
        let pushed = &rs.pending_rewards[before_len..];
        assert_eq!(pushed.len(), CAULDRON_POTIONS as usize);
        for reward in pushed {
            assert!(
                matches!(reward, Reward::Potion(_)),
                "Cauldron must push Reward::Potion sentinels, got {reward:?}"
            );
        }
    }

    /// Bead `sts2-rs-2nrk`: Cauldron listener is noop without ownership.
    #[test]
    fn listener_cauldron_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before_len);
    }

    /// Bead `sts2-rs-gf5s`: SmallCapsule's `after_obtained` listener
    /// must push `SMALL_CAPSULE_RELICS = 1` RelicReward sentinel.
    #[test]
    fn listener_small_capsule_pushes_one_relic_reward_sentinel_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // Post-#69: listener gates on `last_obtained_relic` and rolls a
        // real id from the grab bag — set both and drop the SmallCapsule-
        // identity assertion (the rolled id is random, by design).
        let (by_rarity, fallback, boss) = crate::run_callbacks::default_relic_grab_bag();
        rs.populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
        let before_len = rs.relic_rewards.len();
        rs.relics.push(RelicId::SmallCapsule);
        rs.last_obtained_relic = Some(RelicId::SmallCapsule);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.relic_rewards.len(), before_len + SMALL_CAPSULE_RELICS);
    }

    /// Bead `sts2-rs-gf5s`: SmallCapsule listener is noop without ownership.
    #[test]
    fn listener_small_capsule_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.relic_rewards.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.relic_rewards.len(), before_len);
    }

    /// Bead `sts2-rs-u4wv`: CallingBell's `after_obtained` listener
    /// must push `CALLING_BELL_RELICS = 3` RelicReward sentinels to
    /// `rs.relic_rewards` when owned.
    #[test]
    fn listener_calling_bell_pushes_three_relic_reward_sentinels_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // Post-#69: gate on `last_obtained_relic`, roll real (C/U/R) ids
        // from per-rarity buckets. Identity assertion dropped — pushed
        // ids are the random rolls, not CallingBell itself.
        let (by_rarity, fallback, boss) = crate::run_callbacks::default_relic_grab_bag();
        rs.populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
        let before_len = rs.relic_rewards.len();
        rs.relics.push(RelicId::CallingBell);
        rs.last_obtained_relic = Some(RelicId::CallingBell);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.relic_rewards.len(), before_len + CALLING_BELL_RELICS);
    }

    /// Bead `sts2-rs-u4wv`: CallingBell listener is noop without ownership.
    #[test]
    fn listener_calling_bell_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.relic_rewards.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.relic_rewards.len(), before_len);
    }

    /// Bead `sts2-rs-beda`: LeafyPoultice listener is noop without ownership.
    #[test]
    fn listener_leafy_poultice_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_max = rs.max_hp;
        let before_cur = rs.current_hp;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max);
        assert_eq!(rs.current_hp, before_cur);
    }

    /// Bead `sts2-rs-r2y4x` + `sts2-rs-a49q3`: DistinguishedCape's
    /// `after_obtained` listener must reduce `rs.max_hp` and
    /// `rs.current_hp` by `DISTINGUISHED_CAPE_HP_LOSS = 9` AND push
    /// `DISTINGUISHED_CAPE_CARDS = 3` Apparition cards to
    /// `rs.master_deck` when owned. HP-loss leg landed in r2y4x; the
    /// Apparition deck-add leg landed in a49q3.
    #[test]
    fn listener_distinguished_cape_loses_nine_max_hp_and_adds_three_apparitions_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_max = rs.max_hp;
        let before_cur = rs.current_hp;
        let before_deck_len = rs.master_deck.len();
        rs.relics.push(RelicId::DistinguishedCape);
        rs.last_obtained_relic = Some(RelicId::DistinguishedCape);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max - DISTINGUISHED_CAPE_HP_LOSS);
        assert_eq!(rs.current_hp, before_cur - DISTINGUISHED_CAPE_HP_LOSS);
        assert_eq!(
            rs.master_deck.len(),
            before_deck_len + DISTINGUISHED_CAPE_CARDS
        );
        let added = &rs.master_deck[before_deck_len..];
        assert!(added.iter().all(|c| c.card_id == CardId::Apparition));
    }

    /// Bead `sts2-rs-r2y4x`: DistinguishedCape listener is noop without
    /// ownership.
    #[test]
    fn listener_distinguished_cape_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_max = rs.max_hp;
        let before_cur = rs.current_hp;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max);
        assert_eq!(rs.current_hp, before_cur);
    }

    /// Bead `sts2-rs-r2y4x`: SandCastle's `after_obtained` listener
    /// must upgrade up to `SAND_CASTLE_CARDS = 6` non-upgraded master-
    /// deck cards when owned. We seed the deck with 10 non-upgraded
    /// cards and assert at most `SAND_CASTLE_CARDS` get upgraded.
    #[test]
    fn listener_sand_castle_upgrades_six_random_cards_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..10 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        rs.relics.push(RelicId::SandCastle);
        rs.last_obtained_relic = Some(RelicId::SandCastle);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let n_upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(n_upgraded, SAND_CASTLE_CARDS);
    }

    /// SandCastle is a RANDOM upgrade (C# `StableShuffle(Rng.Niche).Take(6)`),
    /// NOT a player pick — so it must upgrade immediately even when the host
    /// sets `interactive_card_upgrade` (the workbench / lockstep flag), and
    /// must NEVER queue a `pending_deck_upgrades` card_select. Regression for
    /// the workbench bug where SandCastle wrongly offered "select 6 cards".
    #[test]
    fn listener_sand_castle_ignores_interactive_card_upgrade_flag() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..10 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        // Workbench / run_parity lockstep host opts into interactive upgrades
        // (for the genuine FromDeckForUpgrade relics like Pomander) — SandCastle
        // must not be caught by it.
        rs.interactive_card_upgrade = true;
        rs.relics.push(RelicId::SandCastle);
        rs.last_obtained_relic = Some(RelicId::SandCastle);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(
            rs.pending_deck_upgrades, 0,
            "SandCastle must not queue an interactive card_select"
        );
        let n_upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(
            n_upgraded, SAND_CASTLE_CARDS,
            "SandCastle must upgrade 6 random cards immediately"
        );
    }

    /// Bead `sts2-rs-r2y4x`: SandCastle listener is noop without
    /// ownership — no deck card should be upgraded.
    #[test]
    fn listener_sand_castle_is_noop_without_ownership() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..10 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let n_upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(n_upgraded, 0);
    }

    /// Bead `sts2-rs-66l9f`: GlassEye's `after_obtained` listener
    /// must push `GLASS_EYE_CARDS = 5` `Reward::Card` sentinels when
    /// owned. Rarity-forcing is deferred to umbrella 01o.
    #[test]
    fn listener_glass_eye_pushes_five_card_reward_sentinels_on_obtain() {
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        rs.relics.push(RelicId::GlassEye);
        rs.last_obtained_relic = Some(RelicId::GlassEye);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before_len + GLASS_EYE_CARDS);
        assert!(rs
            .pending_rewards
            .iter()
            .rev()
            .take(GLASS_EYE_CARDS)
            .all(|r| matches!(r, Reward::Card(_))));
    }

    /// Bead `sts2-rs-66l9f`: GlassEye listener is noop without ownership.
    #[test]
    fn listener_glass_eye_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before_len);
    }

    /// Bead `sts2-rs-66l9f`: LostCoffer's `after_obtained` listener
    /// must push 1 `Reward::Card` + 1 `Reward::Potion` sentinel when
    /// owned (counts hard-coded to 1 in Python).
    #[test]
    fn listener_lost_coffer_pushes_one_card_and_one_potion_sentinel_on_obtain() {
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        rs.relics.push(RelicId::LostCoffer);
        rs.last_obtained_relic = Some(RelicId::LostCoffer);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before_len + 2);
        let last_two = &rs.pending_rewards[rs.pending_rewards.len() - 2..];
        assert!(matches!(last_two[0], Reward::Card(_)));
        assert!(matches!(last_two[1], Reward::Potion(_)));
    }

    /// Bead `sts2-rs-66l9f`: LostCoffer listener is noop without ownership.
    #[test]
    fn listener_lost_coffer_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before_len);
    }

    /// ScrollBoxes's `after_obtained` listener (v0.107.1) **preserves** gold
    /// and queues the real two-pack bundle choice on `pending_bundles` (each
    /// pack = 2 common + 1 uncommon, no card id repeated across packs),
    /// mirroring C# `ScrollBoxes.GenerateRandomBundles`. The v0.103.2 gold
    /// loss was removed (bead `…wb8lk.9` §7). Supersedes the deferred
    /// 2-card-sentinel stub (bead `sts2-rs-66l9f`).
    #[test]
    fn listener_scroll_boxes_preserves_gold_and_queues_two_card_bundles_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new(); // Ironclad: ample commons/uncommons
        rs.gain_gold(250);
        let gold_before = rs.gold;
        rs.relics.push(RelicId::ScrollBoxes);
        rs.last_obtained_relic = Some(RelicId::ScrollBoxes);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.gold, gold_before, "v0.107.1: ScrollBoxes keeps gold");
        assert_eq!(rs.pending_bundles.len(), SCROLL_BOXES_BUNDLES);
        for pack in &rs.pending_bundles {
            assert_eq!(pack.len(), 3, "each pack is 2 common + 1 uncommon");
        }
        // No duplicate card id across the two packs (shared `usedCardIds`).
        let mut all: Vec<_> = rs.pending_bundles.iter().flatten().copied().collect();
        let total = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), total, "packs must not share a card id");
    }

    /// ScrollBoxes listener is a noop unless it was the just-obtained relic
    /// (gating on `last_obtained_relic`, not mere ownership) — gold preserved,
    /// no bundle queued. Guards against re-firing / re-rolling the Rewards RNG
    /// on later relic obtains.
    #[test]
    fn listener_scroll_boxes_is_noop_without_being_just_obtained() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.gain_gold(250);
        rs.relics.push(RelicId::ScrollBoxes); // owned, but not just-obtained
        let before_gold = rs.gold;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.gold, before_gold);
        assert!(rs.pending_bundles.is_empty());
    }

    /// Bead `sts2-rs-utgn4`: Kaleidoscope's `after_obtained` queues 2
    /// pre-resolved `Reward::Card` sentinels (C# `OfferCustom([r1, r2])`),
    /// each carrying 3 cross-character cards from OTHER-character pools (never
    /// the owner's). Fires only on its own pickup.
    #[test]
    fn listener_kaleidoscope_queues_two_three_card_rewards_from_non_owner_pools_on_obtain() {
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new(); // Ironclad owner
        let owner = rs.character_id;
        rs.relics.push(RelicId::Kaleidoscope);
        rs.last_obtained_relic = Some(RelicId::Kaleidoscope);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let cards: Vec<_> = rs
            .pending_rewards
            .iter()
            .filter_map(|r| match r {
                Reward::Card(c) => Some(c.options.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(cards.len(), 2, "Kaleidoscope offers Cards==2 rewards");
        for reward in &cards {
            assert_eq!(reward.len(), 3, "each reward draws 3 cross-pool cards");
            for &id in reward {
                // Every offered card belongs to a DIFFERENT character (C#
                // `CharacterCardPools.Where(p != owner.CardPool)`).
                assert_ne!(
                    crate::cards::spec_index::character_for_card_id(id),
                    Some(owner),
                    "Kaleidoscope never offers the owner's own pool"
                );
            }
        }
    }

    /// Kaleidoscope listener is a noop unless it was the just-obtained relic
    /// (gating on `last_obtained_relic`, not mere ownership) — re-firing would
    /// re-roll the Niche + Rewards streams on every later relic obtain.
    #[test]
    fn listener_kaleidoscope_is_noop_without_being_just_obtained() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::Kaleidoscope); // owned, but not just-obtained
        let before = rs.pending_rewards.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before);
    }

    /// Bead `sts2-rs-a49q3`: NeowsTorment's `after_obtained` listener
    /// must push one `NeowsFury` card to `rs.master_deck` when owned.
    #[test]
    fn listener_neows_torment_adds_one_neows_fury_to_master_deck_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.master_deck.len();
        rs.relics.push(RelicId::NeowsTorment);
        rs.last_obtained_relic = Some(RelicId::NeowsTorment);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len + 1);
        assert_eq!(rs.master_deck.last().unwrap().card_id, CardId::NeowsFury);
    }

    /// Bead `sts2-rs-a49q3`: NeowsTorment listener is noop without
    /// ownership.
    #[test]
    fn listener_neows_torment_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    /// Bead `sts2-rs-a49q3`: PaelsHorn's `after_obtained` listener
    /// must push `PAELS_HORN_CARDS = 2` Relax cards to `rs.master_deck`
    /// when owned.
    #[test]
    fn listener_paels_horn_adds_two_relax_cards_to_master_deck_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.master_deck.len();
        rs.relics.push(RelicId::PaelsHorn);
        rs.last_obtained_relic = Some(RelicId::PaelsHorn);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len + PAELS_HORN_CARDS);
        let added = &rs.master_deck[before_len..];
        assert!(added.iter().all(|c| c.card_id == CardId::Relax));
    }

    /// Bead `sts2-rs-a49q3`: PaelsHorn listener is noop without
    /// ownership.
    #[test]
    fn listener_paels_horn_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    /// Bead `sts2-rs-a49q3`: CursedPearl's `after_obtained` listener
    /// must push one `Greed` curse to `rs.master_deck` AND grant
    /// `CURSED_PEARL_GOLD = 333` gold when owned.
    #[test]
    fn listener_cursed_pearl_adds_greed_and_grants_gold_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_gold = rs.gold;
        let before_len = rs.master_deck.len();
        rs.relics.push(RelicId::CursedPearl);
        rs.last_obtained_relic = Some(RelicId::CursedPearl);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.gold, before_gold + CURSED_PEARL_GOLD);
        assert_eq!(rs.master_deck.len(), before_len + 1);
        assert_eq!(rs.master_deck.last().unwrap().card_id, CardId::Greed);
    }

    /// Bead `sts2-rs-a49q3`: CursedPearl listener is noop without
    /// ownership.
    #[test]
    fn listener_cursed_pearl_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_gold = rs.gold;
        let before_len = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.gold, before_gold);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    /// Bead `sts2-rs-a49q3`: BloodSoakedRose's `after_obtained`
    /// listener must push one `Enthralled` curse to `rs.master_deck`
    /// when owned. modify_max_energy combat leg deferred.
    #[test]
    fn listener_blood_soaked_rose_adds_enthralled_to_master_deck_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.master_deck.len();
        rs.relics.push(RelicId::BloodSoakedRose);
        rs.last_obtained_relic = Some(RelicId::BloodSoakedRose);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len + 1);
        assert_eq!(rs.master_deck.last().unwrap().card_id, CardId::Enthralled);
    }

    /// Bead `sts2-rs-a49q3`: BloodSoakedRose listener is noop without
    /// ownership.
    #[test]
    fn listener_blood_soaked_rose_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    /// Bead `sts2-rs-qw9oe`: LeadPaperweight's `after_obtained`
    /// listener must push `LEAD_PAPERWEIGHT_CARDS = 2` `Reward::Card`
    /// sentinels when owned. Colorless filter deferred to 01o.
    #[test]
    fn listener_lead_paperweight_queues_two_distinct_colorless_choose_screen() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::LeadPaperweight);
        rs.last_obtained_relic = Some(RelicId::LeadPaperweight);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        // No reward sentinels — a concrete skippable choose-1 screen
        // (C# FromChooseACardScreen) is queued instead.
        assert!(rs.pending_rewards.is_empty());
        assert_eq!(rs.pending_card_selects.len(), 1);
        let opts = &rs.pending_card_selects[0].options;
        assert_eq!(opts.len(), LEAD_PAPERWEIGHT_CARDS, "got {opts:?}");
        assert_ne!(opts[0], opts[1], "options distinct, got {opts:?}");
        for id in opts {
            assert!(
                crate::cards::spec_index::COLORLESS_SPECS
                    .iter()
                    .any(|f| f().id == *id),
                "option {id:?} must come from the colorless pool"
            );
        }
        assert!(rs.pending_card_selects[0].post_add.is_empty());
    }

    /// LeadPaperweight gates on `last_obtained_relic` — owning it is not
    /// enough (the after-obtained channel fires every listener on every
    /// relic obtain; a contains-gate re-rolled the Rewards stream and
    /// queued a phantom screen on each later pickup).
    #[test]
    fn listener_lead_paperweight_is_noop_when_not_the_obtained_relic() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::LeadPaperweight);
        rs.last_obtained_relic = Some(RelicId::BurningBlood);
        let rewards_cursor_before = rs.rng.rewards.counter();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(rs.pending_card_selects.is_empty());
        assert_eq!(
            rs.rng.rewards.counter(),
            rewards_cursor_before,
            "no Rewards reads without a fresh LeadPaperweight obtain"
        );
    }

    /// Bead `sts2-rs-qw9oe`: SeaGlass's `after_obtained` listener
    /// must push `SEA_GLASS_CARDS = 15` `Reward::Card` sentinels when
    /// owned. Character pool + rarity filters deferred to 01o.
    #[test]
    fn listener_sea_glass_pushes_fifteen_card_reward_sentinels_on_obtain() {
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        rs.relics.push(RelicId::SeaGlass);
        rs.last_obtained_relic = Some(RelicId::SeaGlass);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before_len + SEA_GLASS_CARDS);
        assert!(rs
            .pending_rewards
            .iter()
            .rev()
            .take(SEA_GLASS_CARDS)
            .all(|r| matches!(r, Reward::Card(_))));
    }

    /// Bead `sts2-rs-qw9oe`: SeaGlass listener is noop without
    /// ownership.
    #[test]
    fn listener_sea_glass_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before_len);
    }

    /// Bead `sts2-rs-qw9oe`: YummyCookie's `after_obtained` listener
    /// must upgrade up to `YUMMY_COOKIE_CARDS = 4` non-upgraded
    /// master-deck cards when owned. Same convention as SandCastle.
    #[test]
    fn listener_yummy_cookie_upgrades_four_random_cards_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..10 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        rs.relics.push(RelicId::YummyCookie);
        rs.last_obtained_relic = Some(RelicId::YummyCookie);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let n_upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(n_upgraded, YUMMY_COOKIE_CARDS);
    }

    /// Bead `sts2-rs-qw9oe`: YummyCookie listener is noop without
    /// ownership — no deck card should be upgraded.
    #[test]
    fn listener_yummy_cookie_is_noop_without_ownership() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..10 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let n_upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(n_upgraded, 0);
    }

    /// Bead `sts2-rs-cvbxs`: Pomander's `after_obtained` listener
    /// must upgrade one non-upgraded master-deck card when owned.
    #[test]
    fn listener_pomander_upgrades_one_random_card_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        rs.relics.push(RelicId::Pomander);
        rs.last_obtained_relic = Some(RelicId::Pomander);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let n_upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(n_upgraded, 1);
    }

    /// Bead `sts2-rs-cvbxs`: Pomander listener is noop without
    /// ownership.
    #[test]
    fn listener_pomander_is_noop_without_ownership() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let n_upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(n_upgraded, 0);
    }

    /// Bead `sts2-rs-cvbxs` + `sts2-rs-t762n`: PrecariousShears's
    /// `after_obtained` listener must reduce `rs.current_hp` by
    /// `PRECARIOUS_SHEARS_DAMAGE = 16` AND remove
    /// `PRECARIOUS_SHEARS_CARDS = 2` cards from the deck when owned
    /// (non-interactive auto-removal path — damage applies eagerly).
    /// HP-loss leg landed in cvbxs; remove-cards leg landed in t762n.
    #[test]
    fn listener_precarious_shears_loses_hp_and_removes_two_cards_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let before_cur = rs.current_hp;
        let before_deck_len = rs.master_deck.len();
        rs.relics.push(RelicId::PrecariousShears);
        rs.last_obtained_relic = Some(RelicId::PrecariousShears);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.current_hp, before_cur - PRECARIOUS_SHEARS_DAMAGE);
        assert_eq!(
            rs.master_deck.len(),
            before_deck_len - PRECARIOUS_SHEARS_CARDS
        );
    }

    /// Bead `sts2-rs-1hiht`: in interactive-removal mode PrecariousShears
    /// must NOT lower HP at obtain — it queues the removal `card_select`
    /// (`pending_deck_removals`) and DEFERS the damage
    /// (`pending_post_removal_hp_loss`) until the pick resolves, matching
    /// C#'s `FromDeckForRemoval` → `RemoveFromDeck` → `Damage` order.
    #[test]
    fn listener_precarious_shears_defers_hp_loss_in_interactive_mode() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.interactive_card_removal = true;
        for _ in 0..5 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let before_cur = rs.current_hp;
        let before_deck_len = rs.master_deck.len();
        rs.relics.push(RelicId::PrecariousShears);
        rs.last_obtained_relic = Some(RelicId::PrecariousShears);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        // HP unchanged; deck unchanged (removal is now a pending pick);
        // damage parked for after the pick.
        assert_eq!(rs.current_hp, before_cur);
        assert_eq!(rs.master_deck.len(), before_deck_len);
        assert_eq!(rs.pending_deck_removals, PRECARIOUS_SHEARS_CARDS as u32);
        assert_eq!(rs.pending_post_removal_hp_loss, PRECARIOUS_SHEARS_DAMAGE);
    }

    /// Bead `sts2-rs-cvbxs`: PrecariousShears listener is noop without
    /// ownership.
    #[test]
    fn listener_precarious_shears_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_cur = rs.current_hp;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.current_hp, before_cur);
    }

    /// Bead `sts2-rs-cvbxs`: AlchemicalCoffer's `after_obtained`
    /// listener must increase `rs.max_potion_slots` by
    /// `ALCHEMICAL_COFFER_POTION_SLOTS = 4` AND fill the new slots with
    /// 4 random out-of-combat potions, mirroring C#
    /// `AlchemicalCoffer.cs:20-29` (`GainMaxPotionCount` +
    /// `CreateRandomPotionsOutOfCombat` + `TryToProcure` loop).
    #[test]
    fn listener_alchemical_coffer_gains_four_potion_slots_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_slots = rs.max_potion_slots;
        let before_potions = rs.potions.len();
        rs.relics.push(RelicId::AlchemicalCoffer);
        rs.last_obtained_relic = Some(RelicId::AlchemicalCoffer);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(
            rs.max_potion_slots,
            before_slots + ALCHEMICAL_COFFER_POTION_SLOTS
        );
        // The 4 new slots are filled with 4 random potions (belt started
        // empty in a fresh RunState; cap rose to >=4 above).
        assert_eq!(
            rs.potions.len(),
            before_potions + ALCHEMICAL_COFFER_POTION_SLOTS as usize
        );
    }

    /// Bead `sts2-rs-cvbxs`: AlchemicalCoffer listener is noop without
    /// ownership.
    #[test]
    fn listener_alchemical_coffer_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_slots = rs.max_potion_slots;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_potion_slots, before_slots);
    }

    /// Bead `sts2-rs-t762n`: PreciseScissors's `after_obtained`
    /// listener must remove one card from `rs.master_deck` when owned.
    #[test]
    fn listener_precise_scissors_removes_one_card_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let before_len = rs.master_deck.len();
        rs.relics.push(RelicId::PreciseScissors);
        rs.last_obtained_relic = Some(RelicId::PreciseScissors);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len - 1);
    }

    /// Bead `sts2-rs-t762n`: PreciseScissors listener is noop without
    /// ownership.
    #[test]
    fn listener_precise_scissors_is_noop_without_ownership() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let before_len = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    /// Bead `sts2-rs-t762n`: EmptyCage's `after_obtained` listener
    /// must remove `EMPTY_CAGE_CARDS = 2` cards from `rs.master_deck`
    /// when owned.
    #[test]
    fn listener_empty_cage_removes_two_cards_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let before_len = rs.master_deck.len();
        rs.relics.push(RelicId::EmptyCage);
        rs.last_obtained_relic = Some(RelicId::EmptyCage);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len - EMPTY_CAGE_CARDS);
    }

    /// Bead `sts2-rs-t762n`: EmptyCage listener is noop without
    /// ownership.
    #[test]
    fn listener_empty_cage_is_noop_without_ownership() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let before_len = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    /// Bead `sts2-rs-t762n` / `sts2-rs-rn76k`: PreservedFog's
    /// `after_obtained` listener must remove `PRESERVED_FOG_CARDS = 3`
    /// cards (per C# `PreservedFog.cs:18` `CardsVar(3)`) then push 1
    /// Folly curse, net deck delta = -2 when owned.
    #[test]
    fn listener_preserved_fog_removes_three_cards_and_adds_one_folly_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..10 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let before_len = rs.master_deck.len();
        rs.relics.push(RelicId::PreservedFog);
        rs.last_obtained_relic = Some(RelicId::PreservedFog);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        // -3 strikes removed, +1 Folly = net -2
        assert_eq!(rs.master_deck.len(), before_len - PRESERVED_FOG_CARDS + 1);
        assert_eq!(rs.master_deck.last().unwrap().card_id, CardId::Folly);
    }

    /// Bead `sts2-rs-t762n`: PreservedFog listener is noop without
    /// ownership.
    #[test]
    fn listener_preserved_fog_is_noop_without_ownership() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..10 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let before_len = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    /// Bead `sts2-rs-rvwas`: ArchaicTooth's `after_obtained` listener
    /// must rewrite the first eligible starter card's `card_id` to its
    /// ancient counterpart when owned, preserving upgrade flag.
    #[test]
    fn listener_archaic_tooth_transforms_bash_to_break_preserving_upgrade() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // Three Strikes (not in mapping), one upgraded Bash, one Defend.
        for _ in 0..3 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let mut bash = sts2_core::cards::Card::pod_defaults();
        bash.card_id = CardId::Bash;
        bash.upgraded = true;
        rs.master_deck.push(bash);
        let mut defend = sts2_core::cards::Card::pod_defaults();
        defend.card_id = CardId::DefendIronclad;
        rs.master_deck.push(defend);
        let before_len = rs.master_deck.len();
        rs.relics.push(RelicId::ArchaicTooth);
        rs.last_obtained_relic = Some(RelicId::ArchaicTooth);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len);
        // Bash should be transformed to Break, upgrade preserved.
        let break_idx = rs
            .master_deck
            .iter()
            .position(|c| c.card_id == CardId::Break)
            .expect("Bash transformed to Break");
        assert!(rs.master_deck[break_idx].upgraded);
        assert!(!rs.master_deck.iter().any(|c| c.card_id == CardId::Bash));
    }

    /// Bead `sts2-rs-rvwas`: ArchaicTooth transforms only the first
    /// matching starter and leaves the rest of the deck alone.
    #[test]
    fn listener_archaic_tooth_transforms_only_first_match() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // Two Bash cards.
        for _ in 0..2 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::Bash;
            rs.master_deck.push(c);
        }
        rs.relics.push(RelicId::ArchaicTooth);
        rs.last_obtained_relic = Some(RelicId::ArchaicTooth);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let n_break = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == CardId::Break)
            .count();
        let n_bash = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == CardId::Bash)
            .count();
        assert_eq!(n_break, 1);
        assert_eq!(n_bash, 1);
    }

    /// Bead `sts2-rs-rvwas`: ArchaicTooth listener is noop without
    /// ownership — no deck card transformed.
    #[test]
    fn listener_archaic_tooth_is_noop_without_ownership() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut bash = sts2_core::cards::Card::pod_defaults();
        bash.card_id = CardId::Bash;
        rs.master_deck.push(bash);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck[0].card_id, CardId::Bash);
    }

    /// Bead `sts2-rs-w9vf8`: Claws's `after_obtained` listener must
    /// rewrite up to `CLAWS_CARDS = 6` deck cards to `CardId::Maul`,
    /// preserving upgrade state.
    #[test]
    fn listener_claws_transforms_six_cards_to_maul_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..10 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        rs.relics.push(RelicId::Claws);
        rs.last_obtained_relic = Some(RelicId::Claws);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let n_maul = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == CardId::Maul)
            .count();
        assert_eq!(n_maul, CLAWS_CARDS);
    }

    /// Bead `sts2-rs-scds8`: a Claws transform must REBUILD the instance from
    /// Maul's spec, not just flip `card_id` — otherwise a non-Enemy source
    /// (target_type None, base_damage 0) becomes a malformed Maul that
    /// `play_card` dispatches untargeted, panicking `play_maul`'s
    /// `target.expect`. Assert the transformed card carries Maul's combat
    /// fields, while uid + upgrade + enchantment state survive.
    #[test]
    fn listener_claws_rebuilds_instance_from_maul_spec() {
        use sts2_core::cards::{CardId, EnchantmentId, TargetType};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // A Defend-like source: non-Enemy target, zero damage, upgraded, with
        // an enchantment — every field that a bare `card_id =` would leave stale.
        let mut src = sts2_core::cards::Card::pod_defaults();
        src.card_id = CardId::DefendIronclad;
        src.target_type = TargetType::None;
        src.card_type = sts2_core::cards::CardType::Skill;
        src.base_damage = 0;
        src.upgraded = true;
        src.enchantments.push((EnchantmentId::Glam, 1));
        let src_uid = src.uid;
        rs.master_deck.push(src);
        rs.relics.push(RelicId::Claws);
        rs.last_obtained_relic = Some(RelicId::Claws);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);

        let maul_ref = crate::cards::spec_index::card_pod_from_id(CardId::Maul, true)
            .expect("Maul spec registered");
        let c = &rs.master_deck[0];
        assert_eq!(c.card_id, CardId::Maul);
        assert_eq!(c.target_type, TargetType::Enemy, "must take Maul's target");
        assert_eq!(c.card_type, sts2_core::cards::CardType::Attack);
        assert_eq!(c.base_damage, maul_ref.base_damage, "upgraded Maul damage");
        assert_eq!(c.cost, maul_ref.cost);
        assert_eq!(c.uid, src_uid, "uid preserved");
        assert!(c.upgraded, "upgrade state preserved");
        assert_eq!(
            c.enchantments.as_slice(),
            &[(EnchantmentId::Glam, 1)],
            "enchantments preserved"
        );
    }

    /// Bead `sts2-rs-w9vf8`: Claws clamps to deck size when the deck
    /// has fewer than `CLAWS_CARDS` cards.
    #[test]
    fn listener_claws_clamps_to_deck_size() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..3 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        rs.relics.push(RelicId::Claws);
        rs.last_obtained_relic = Some(RelicId::Claws);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(rs.master_deck.iter().all(|c| c.card_id == CardId::Maul));
        assert_eq!(rs.master_deck.len(), 3);
    }

    /// Bead `sts2-rs-w9vf8`: Claws listener is noop without
    /// ownership — no card transformed.
    #[test]
    fn listener_claws_is_noop_without_ownership() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..3 {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(rs
            .master_deck
            .iter()
            .all(|c| c.card_id == CardId::StrikeIronclad));
    }

    /// Bead `sts2-rs-w9vf8`: NutritiousSoup's `after_obtained` listener
    /// must set `TezcatarasEmber = 1` on every basic-Strike card in
    /// the deck, leaving non-Strike cards untouched.
    #[test]
    fn listener_nutritious_soup_enchants_all_basic_strikes_with_tezcataras_ember() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for id in [
            CardId::StrikeIronclad,
            CardId::StrikeSilent,
            CardId::DefendIronclad,
        ] {
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_id = id;
            rs.master_deck.push(c);
        }
        rs.relics.push(RelicId::NutritiousSoup);
        rs.last_obtained_relic = Some(RelicId::NutritiousSoup);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        // Strikes got the enchantment.
        for card in &rs.master_deck {
            let val = card.enchantment(EnchantmentId::TezcatarasEmber);
            match card.card_id {
                CardId::StrikeIronclad | CardId::StrikeSilent => {
                    assert_eq!(val, 1, "{:?} should be enchanted", card.card_id);
                }
                CardId::DefendIronclad => {
                    assert_eq!(val, 0, "Defend should NOT be enchanted");
                }
                _ => {}
            }
        }
    }

    /// Bead `sts2-rs-w9vf8`: NutritiousSoup listener is noop without
    /// ownership.
    #[test]
    fn listener_nutritious_soup_is_noop_without_ownership() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut c = sts2_core::cards::Card::pod_defaults();
        c.card_id = CardId::StrikeIronclad;
        rs.master_deck.push(c);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(
            rs.master_deck[0].enchantment(EnchantmentId::TezcatarasEmber),
            0
        );
    }

    /// Bead `sts2-rs-70anj.4`: GnarledHammer's `after_obtained`
    /// listener must enchant up to `GNARLED_HAMMER_CARDS = 3` eligible
    /// Attack cards with `Sharp(GNARLED_HAMMER_SHARP = 3)`.
    #[test]
    fn listener_gnarled_hammer_enchants_up_to_three_attacks_with_sharp_three() {
        use sts2_core::cards::EnchantmentId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // Five attacks: only the first three should be enchanted.
        for _ in 0..5 {
            push_deck_card(&mut rs, sts2_core::cards::CardId::StrikeIronclad);
        }
        rs.relics.push(RelicId::GnarledHammer);
        rs.last_obtained_relic = Some(RelicId::GnarledHammer);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let enchanted = rs
            .master_deck
            .iter()
            .filter(|c| c.enchantment(EnchantmentId::Sharp) == GNARLED_HAMMER_SHARP)
            .count();
        assert_eq!(
            enchanted, GNARLED_HAMMER_CARDS,
            "first 3 attacks must be Sharp"
        );
        assert_eq!(rs.master_deck.len(), 5, "no cards added/removed");
    }

    /// Bead `sts2-rs-70anj.4`: GnarledHammer listener is no-op without
    /// ownership; deck enchantments untouched.
    #[test]
    fn listener_gnarled_hammer_is_noop_without_ownership() {
        use sts2_core::cards::EnchantmentId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        push_deck_card(&mut rs, sts2_core::cards::CardId::StrikeIronclad);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck[0].enchantment(EnchantmentId::Sharp), 0);
    }

    /// Bead `sts2-rs-70anj.4`: Sharp gate must skip non-Attack cards
    /// (per `can_enchant_card` rules). With 1 attack + 5 skills,
    /// only the attack is enchanted.
    #[test]
    fn listener_gnarled_hammer_skips_non_attack_cards() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad);
        for _ in 0..5 {
            push_deck_card(&mut rs, CardId::DefendIronclad);
        }
        rs.relics.push(RelicId::GnarledHammer);
        rs.last_obtained_relic = Some(RelicId::GnarledHammer);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(
            rs.master_deck[0].enchantment(EnchantmentId::Sharp),
            GNARLED_HAMMER_SHARP,
            "the lone Attack must be enchanted"
        );
        let defends_enchanted = rs.master_deck[1..]
            .iter()
            .filter(|c| c.enchantment(EnchantmentId::Sharp) != 0)
            .count();
        assert_eq!(defends_enchanted, 0, "Skills must not receive Sharp");
    }

    /// Build a single-card deck whose card carries `AscendersBane`'s id (whose
    /// spec is `UNPLAYABLE`) but an Attack `card_type`, so the Sharp gate's
    /// `card.card_type == Attack` check passes and only the SPEC lookup decides
    /// eligibility. Used to pin the `pool.iter().find(|s| s.id == card.card_id)`
    /// spec match in `enchantable_candidate_card_ids` / `enchant_eligible_deck_cards`.
    fn rs_with_unplayable_attack_card() -> sts2_core::runstate::RunState {
        use sts2_core::cards::{Card, CardId, CardType};
        use sts2_core::runstate::RunState;
        let mut rs = RunState::new();
        rs.master_deck.clear();
        let mut card = Card::pod_defaults();
        // AscendersBane's real spec is UNPLAYABLE (pool index 3); a `!=` spec
        // match instead latches onto the FIRST non-matching spec (StrikeIronclad,
        // playable), flipping eligibility.
        card.card_id = CardId::AscendersBane;
        card.card_type = CardType::Attack;
        rs.master_deck.push(card);
        rs
    }

    /// `enchantable_candidate_card_ids` looks up each deck card's spec via
    /// `pool.iter().find(|s| s.id == card.card_id)` (line 2724). The card's
    /// REAL spec (`AscendersBane`, UNPLAYABLE) makes `can_enchant_card` return
    /// false, so the card is NOT a Sharp candidate. The `==` → `!=` mutant
    /// matches the first OTHER spec (StrikeIronclad, playable) instead, which
    /// WOULD pass the UNPLAYABLE check and wrongly admit the card.
    #[test]
    fn enchantable_candidate_uses_matching_spec() {
        let rs = rs_with_unplayable_attack_card();
        let candidates =
            enchantable_candidate_card_ids(&rs, sts2_core::cards::EnchantmentId::Sharp);
        assert!(
            candidates.is_empty(),
            "the card's own spec is UNPLAYABLE → not a Sharp candidate; a \
             mismatched spec would wrongly admit it, got {candidates:?}"
        );
    }

    /// `enchant_eligible_deck_cards` looks up each deck card's spec via
    /// `pool.iter().find(|s| s.id == card.card_id)` (line 2745) before the
    /// `can_enchant_card` gate. With the card's REAL spec (`AscendersBane`,
    /// UNPLAYABLE) the gate rejects it → nothing enchanted. The `==` → `!=`
    /// mutant matches StrikeIronclad's (playable) spec → the card is wrongly
    /// enchanted with Sharp.
    #[test]
    fn enchant_eligible_uses_matching_spec() {
        use sts2_core::cards::EnchantmentId;
        let mut rs = rs_with_unplayable_attack_card();
        let applied = enchant_eligible_deck_cards(&mut rs, EnchantmentId::Sharp, 3, 3);
        assert_eq!(
            applied, 0,
            "the card's own spec is UNPLAYABLE → not enchantable; a mismatched \
             spec would enchant it"
        );
        assert_eq!(
            rs.master_deck[0].enchantment(EnchantmentId::Sharp),
            0,
            "no Sharp should land on the UNPLAYABLE-spec card"
        );
    }

    /// `gain_block_player_unpowered` dispatches the `after_block_gained` buses
    /// ONLY when `final_block > 0` (line 1594). `JuggernautPower` reacts to that
    /// bus by dealing its amount to a random alive enemy, so a positive block
    /// grant with Juggernaut installed MUST damage the enemy — proving the
    /// guard fired the dispatch. Kills the `> → ==` and `> → <` mutants (both
    /// suppress the dispatch for a positive grant). The `> → >=` mutant is
    /// EQUIVALENT: `base = SAI_BLOCK = 7 > 0`, so `final_block` is always > 0
    /// here and `>= 0` never diverges from `> 0`.
    #[test]
    fn gain_block_player_unpowered_fires_after_block_gained() {
        use smallvec::SmallVec;
        use sts2_core::{
            Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, PowerKind, Side,
        };
        crate::relics::dispatch::ensure_installed();

        let mut state = CombatState::default();
        state.side = Side::Player;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let player_uid = state.init_player_creature();
        state.sync_player_creature();

        let enemy_uid = CreatureUid::new(10).expect("nz");
        state.creatures.push(Creature {
            id: enemy_uid.0,
            current_hp: 40,
            max_hp: 40,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            monster_id: MonsterId(""),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        state.enemy_uids.push(enemy_uid);

        // Juggernaut(5) on the player: reacts to after_block_gained by dealing
        // 5 unpowered damage to a random alive enemy.
        sts2_core::card_effects::apply_power_to_creature(
            &mut state,
            player_uid,
            PowerKind::Juggernaut,
            5,
        );

        let enemy_hp = |state: &CombatState| {
            state
                .creatures
                .iter()
                .find(|c| c.id == enemy_uid.0)
                .map_or(0, |c| c.current_hp)
        };
        let before = enemy_hp(&state);

        gain_block_player_unpowered(&mut state, SAI_BLOCK);

        assert_eq!(
            state.player.block, SAI_BLOCK,
            "7 block granted unconditionally"
        );
        assert_eq!(
            before - enemy_hp(&state),
            5,
            "final_block (7) > 0 → after_block_gained dispatched → Juggernaut dealt 5"
        );
    }

    /// Bead `sts2-rs-2v8y2`: when `defer_followup_rewards` is set,
    /// GnarledHammer queues a player-pick `Reward::EnchantCards`
    /// instead of auto-enchanting the deck. Pool carries only the
    /// Sharp-eligible Attacks; `min_count = Some(0)` matches Python
    /// `relics/shop_event.py:410` (skippable).
    #[test]
    fn listener_gnarled_hammer_deferred_path_queues_enchant_reward() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..3 {
            push_deck_card(&mut rs, CardId::StrikeIronclad);
        }
        for _ in 0..2 {
            push_deck_card(&mut rs, CardId::DefendIronclad);
        }
        rs.relics.push(RelicId::GnarledHammer);
        rs.defer_followup_rewards = true;
        rs.last_obtained_relic = Some(RelicId::GnarledHammer);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        // No card enchanted in the master deck.
        assert!(
            rs.master_deck
                .iter()
                .all(|c| c.enchantment(EnchantmentId::Sharp) == 0),
            "deferred path must not enchant the deck directly"
        );
        // Exactly one EnchantCards reward queued.
        let queued = rs
            .pending_rewards
            .iter()
            .filter(|r| matches!(r, Reward::EnchantCards(_)))
            .count();
        assert_eq!(queued, 1, "exactly one EnchantCards reward queued");
        let Reward::EnchantCards(r) = rs
            .pending_rewards
            .iter()
            .find(|r| matches!(r, Reward::EnchantCards(_)))
            .expect("reward queued")
        else {
            unreachable!()
        };
        assert_eq!(r.enchantment, EnchantmentId::Sharp);
        assert_eq!(r.amount, GNARLED_HAMMER_SHARP);
        assert_eq!(r.count, GNARLED_HAMMER_CARDS);
        assert_eq!(r.min_count, Some(0));
        // Pool: 3 Sharp-eligible Strikes; 2 Defends excluded by
        // `can_enchant_card(Sharp)`.
        assert_eq!(
            r.options.len(),
            3,
            "pool carries the 3 Sharp-eligible Attacks only"
        );
        assert!(r.options.iter().all(|cid| *cid == CardId::StrikeIronclad));
    }

    /// Bead `sts2-rs-g5ksv`: Kifuda deferred path queues an
    /// EnchantCards reward (Adroit, min_count=Some(0)) instead of
    /// auto-enchanting. Python pin `relics/shop_event.py:428`.
    #[test]
    fn listener_kifuda_deferred_path_queues_enchant_reward() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            push_deck_card(&mut rs, CardId::StrikeIronclad);
        }
        rs.relics.push(RelicId::Kifuda);
        rs.defer_followup_rewards = true;
        rs.last_obtained_relic = Some(RelicId::Kifuda);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(
            rs.master_deck
                .iter()
                .all(|c| c.enchantment(EnchantmentId::Adroit) == 0),
            "deferred path leaves the deck untouched"
        );
        let Reward::EnchantCards(r) = rs
            .pending_rewards
            .iter()
            .find(|r| matches!(r, Reward::EnchantCards(_)))
            .expect("EnchantCards reward queued")
        else {
            unreachable!()
        };
        assert_eq!(r.enchantment, EnchantmentId::Adroit);
        assert_eq!(r.amount, KIFUDA_ADROIT);
        assert_eq!(r.count, KIFUDA_CARDS);
        assert_eq!(r.min_count, Some(0));
    }

    /// Bead `sts2-rs-g5ksv`: PunchDagger deferred path queues an
    /// EnchantCards reward (Momentum, no min_count). Python pin
    /// `relics/shop_event.py:587`.
    #[test]
    fn listener_punch_dagger_deferred_path_queues_enchant_reward() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..3 {
            push_deck_card(&mut rs, CardId::StrikeIronclad);
        }
        rs.relics.push(RelicId::PunchDagger);
        rs.defer_followup_rewards = true;
        rs.last_obtained_relic = Some(RelicId::PunchDagger);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(
            rs.master_deck
                .iter()
                .all(|c| c.enchantment(EnchantmentId::Momentum) == 0),
            "deferred path leaves the deck untouched"
        );
        let Reward::EnchantCards(r) = rs
            .pending_rewards
            .iter()
            .find(|r| matches!(r, Reward::EnchantCards(_)))
            .expect("EnchantCards reward queued")
        else {
            unreachable!()
        };
        assert_eq!(r.enchantment, EnchantmentId::Momentum);
        assert_eq!(r.amount, PUNCH_DAGGER_MOMENTUM);
        assert_eq!(r.count, 1, "count hard-coded to 1 per Python pin");
        assert_eq!(r.min_count, None);
    }

    /// Bead `sts2-rs-g5ksv`: BeautifulBracelet deferred path queues
    /// an EnchantCards reward (Swift, no min_count). Python pin
    /// `relics/shop_event.py:1030`.
    #[test]
    fn listener_beautiful_bracelet_deferred_path_queues_enchant_reward() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            push_deck_card(&mut rs, CardId::StrikeIronclad);
        }
        rs.relics.push(RelicId::BeautifulBracelet);
        rs.defer_followup_rewards = true;
        rs.last_obtained_relic = Some(RelicId::BeautifulBracelet);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(
            rs.master_deck
                .iter()
                .all(|c| c.enchantment(EnchantmentId::Swift) == 0),
            "deferred path leaves the deck untouched"
        );
        let Reward::EnchantCards(r) = rs
            .pending_rewards
            .iter()
            .find(|r| matches!(r, Reward::EnchantCards(_)))
            .expect("EnchantCards reward queued")
        else {
            unreachable!()
        };
        assert_eq!(r.enchantment, EnchantmentId::Swift);
        assert_eq!(r.amount, BEAUTIFUL_BRACELET_SWIFT);
        assert_eq!(r.count, BEAUTIFUL_BRACELET_CARDS);
        assert_eq!(r.min_count, None);
    }

    /// Bead `sts2-rs-g5ksv`: RoyalStamp deferred path queues an
    /// EnchantCards reward (RoyallyApproved, no min_count). Python
    /// pin `relics/shop_event.py:619`.
    #[test]
    fn listener_royal_stamp_deferred_path_queues_enchant_reward() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::{Reward, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad);
        rs.relics.push(RelicId::RoyalStamp);
        rs.defer_followup_rewards = true;
        rs.last_obtained_relic = Some(RelicId::RoyalStamp);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(
            rs.master_deck
                .iter()
                .all(|c| c.enchantment(EnchantmentId::RoyallyApproved) == 0),
            "deferred path leaves the deck untouched"
        );
        let Reward::EnchantCards(r) = rs
            .pending_rewards
            .iter()
            .find(|r| matches!(r, Reward::EnchantCards(_)))
            .expect("EnchantCards reward queued")
        else {
            unreachable!()
        };
        assert_eq!(r.enchantment, EnchantmentId::RoyallyApproved);
        assert_eq!(r.amount, 1);
        assert_eq!(r.count, 1);
        assert_eq!(r.min_count, None);
    }

    /// Bead `sts2-rs-70anj.4`: Kifuda's `after_obtained` listener must
    /// enchant up to `KIFUDA_CARDS = 3` eligible cards with
    /// `Adroit(KIFUDA_ADROIT = 3)`. Adroit is unrestricted (any card),
    /// so 3 of 5 cards get the enchantment.
    #[test]
    fn listener_kifuda_enchants_up_to_three_cards_with_adroit_three() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            push_deck_card(&mut rs, CardId::StrikeIronclad);
        }
        rs.relics.push(RelicId::Kifuda);
        rs.last_obtained_relic = Some(RelicId::Kifuda);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let enchanted = rs
            .master_deck
            .iter()
            .filter(|c| c.enchantment(EnchantmentId::Adroit) == KIFUDA_ADROIT)
            .count();
        assert_eq!(enchanted, KIFUDA_CARDS);
    }

    /// Bead `sts2-rs-70anj.4`: PunchDagger enchants exactly 1 Attack
    /// card with `Momentum(PUNCH_DAGGER_MOMENTUM = 5)`.
    #[test]
    fn listener_punch_dagger_enchants_one_attack_with_momentum_five() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..3 {
            push_deck_card(&mut rs, CardId::StrikeIronclad);
        }
        rs.relics.push(RelicId::PunchDagger);
        rs.last_obtained_relic = Some(RelicId::PunchDagger);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(
            rs.master_deck[0].enchantment(EnchantmentId::Momentum),
            PUNCH_DAGGER_MOMENTUM
        );
        assert_eq!(rs.master_deck[1].enchantment(EnchantmentId::Momentum), 0);
        assert_eq!(rs.master_deck[2].enchantment(EnchantmentId::Momentum), 0);
    }

    /// Bead `sts2-rs-70anj.4`: BeautifulBracelet enchants up to
    /// `BEAUTIFUL_BRACELET_CARDS = 3` cards with
    /// `Swift(BEAUTIFUL_BRACELET_SWIFT = 3)`.
    #[test]
    fn listener_beautiful_bracelet_enchants_up_to_three_with_swift_three() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..4 {
            push_deck_card(&mut rs, CardId::StrikeIronclad);
        }
        rs.relics.push(RelicId::BeautifulBracelet);
        rs.last_obtained_relic = Some(RelicId::BeautifulBracelet);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let enchanted = rs
            .master_deck
            .iter()
            .filter(|c| c.enchantment(EnchantmentId::Swift) == BEAUTIFUL_BRACELET_SWIFT)
            .count();
        assert_eq!(enchanted, BEAUTIFUL_BRACELET_CARDS);
    }

    /// Bead `sts2-rs-70anj.4`: RoyalStamp enchants exactly 1 card
    /// (Attack or Skill, per `can_enchant_card`) with
    /// `RoyallyApproved(1)`.
    #[test]
    fn listener_royal_stamp_enchants_one_card_with_royally_approved_one() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad);
        push_deck_card(&mut rs, CardId::DefendIronclad);
        rs.relics.push(RelicId::RoyalStamp);
        rs.last_obtained_relic = Some(RelicId::RoyalStamp);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(
            rs.master_deck[0].enchantment(EnchantmentId::RoyallyApproved),
            1
        );
        assert_eq!(
            rs.master_deck[1].enchantment(EnchantmentId::RoyallyApproved),
            0
        );
    }

    /// RoyalStamp in INTERACTIVE mode surfaces a deck card-select
    /// (`CardSelectCmd.FromDeckForEnchantment`, `RoyalStamp.cs:37`)
    /// instead of auto-enchanting the first eligible card: it queues a
    /// single `PendingDeckEnchant` (RoyallyApproved) and leaves the deck
    /// untouched until the player picks.
    #[test]
    fn listener_royal_stamp_interactive_queues_deck_enchant() {
        use sts2_core::cards::{CardId, CardType, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad); // Attack
        push_deck_card(&mut rs, CardId::DefendIronclad); // Skill
        rs.interactive_card_transform = true;
        rs.relics.push(RelicId::RoyalStamp);
        rs.last_obtained_relic = Some(RelicId::RoyalStamp);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        // Nothing auto-applied — the pick is deferred to the player.
        assert!(
            rs.master_deck
                .iter()
                .all(|c| c.enchantment(EnchantmentId::RoyallyApproved) == 0),
            "interactive path must not auto-enchant any card"
        );
        assert_eq!(rs.pending_deck_enchants.len(), 1, "one interactive pick");
        let entry = &rs.pending_deck_enchants[0];
        assert_eq!(entry.enchantment, EnchantmentId::RoyallyApproved);
        assert_eq!(entry.amount, 1);
        // RoyallyApproved is Attack|Skill (`can_enchant_card`).
        assert_eq!(entry.type_restriction, Some(CardType::Attack));
        assert_eq!(entry.secondary_type, Some(CardType::Skill));
    }

    /// A multi-card enchant relic (GnarledHammer, `GNARLED_HAMMER_CARDS`
    /// picks) queues one interactive pick per card when interactive and
    /// the candidate pool exceeds the pick count.
    #[test]
    fn listener_gnarled_hammer_interactive_queues_count_picks() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..(GNARLED_HAMMER_CARDS + 2) {
            push_deck_card(&mut rs, CardId::StrikeIronclad); // Attack — Sharp-eligible
        }
        rs.interactive_card_transform = true;
        rs.relics.push(RelicId::GnarledHammer);
        rs.last_obtained_relic = Some(RelicId::GnarledHammer);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(
            rs.master_deck
                .iter()
                .all(|c| c.enchantment(EnchantmentId::Sharp) == 0),
            "interactive path must not auto-enchant"
        );
        assert_eq!(rs.pending_deck_enchants.len(), GNARLED_HAMMER_CARDS);
        assert!(rs
            .pending_deck_enchants
            .iter()
            .all(|e| e.enchantment == EnchantmentId::Sharp));
    }

    /// Degenerate interactive case: when the candidate pool is no larger
    /// than the pick count there is no real choice, so the relic
    /// auto-applies (matching the SelfHelpBook `eligible > 1` convention).
    #[test]
    fn listener_royal_stamp_interactive_single_candidate_auto_applies() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad); // sole eligible card
        rs.interactive_card_transform = true;
        rs.relics.push(RelicId::RoyalStamp);
        rs.last_obtained_relic = Some(RelicId::RoyalStamp);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert!(
            rs.pending_deck_enchants.is_empty(),
            "no interactive pick for a single candidate"
        );
        assert_eq!(
            rs.master_deck[0].enchantment(EnchantmentId::RoyallyApproved),
            1,
            "auto-applied to the sole eligible card"
        );
    }

    /// Bead `sts2-rs-70anj.4`: BiiigHug's `after_obtained` listener
    /// must remove `BIIIG_HUG_CARDS = 4` cards from `rs.master_deck`
    /// (head-N drain) when the relic is owned.
    #[test]
    fn listener_biiig_hug_removes_four_cards_from_master_deck_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..10 {
            push_deck_card(&mut rs, CardId::StrikeIronclad);
        }
        rs.relics.push(RelicId::BiiigHug);
        let before_len = rs.master_deck.len();
        rs.last_obtained_relic = Some(RelicId::BiiigHug);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len - BIIIG_HUG_CARDS);
    }

    /// Bead `sts2-rs-70anj.4`: BiiigHug's `after_obtained` listener is
    /// no-op without ownership; deck length unchanged.
    #[test]
    fn listener_biiig_hug_remove_is_noop_without_ownership() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            push_deck_card(&mut rs, CardId::StrikeIronclad);
        }
        let before_len = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    /// Beads `sts2-rs-70anj.4`, `sts2-rs-lfdaa`: BiiigHug's `after_shuffle`
    /// listener adds one Soot to the draw pile per shuffle event while owned,
    /// at a `CardPilePosition.Random` depth — which costs exactly one
    /// Shuffle-stream read (C# `AddInternal(card, Rng.Shuffle.NextInt(Count +
    /// 1))`, `CardPileCmd.cs:511-515`). The earlier top-insert consumed zero
    /// reads and desynced every later draw/reshuffle in the combat.
    #[test]
    fn listener_biiig_hug_shuffle_adds_soot_at_random_draw_depth() {
        use sts2_core::cards::{CardId, CardType};
        use sts2_core::relic_effects::{dispatch_after_shuffle_bus, ShuffleCtx};
        use sts2_core::CardUid;
        crate::relics::dispatch::ensure_installed();
        let mut state = sts2_core::CombatState::default();
        equip_relic(&mut state, RelicKind::BiiigHug);
        // Seed a draw pile so a random index has more than one slot to land in.
        for i in 1..=4u32 {
            state
                .player
                .draw
                .push(CardUid::new(i + 100).expect("non-zero"));
        }
        let before_draw_len = state.player.draw.len();
        let before_shuffle_reads = state.run_rng.shuffle.counter();

        let _ = dispatch_after_shuffle_bus(&mut state, &ShuffleCtx::default());

        assert_eq!(state.player.draw.len(), before_draw_len + 1);
        assert_eq!(
            state.run_rng.shuffle.counter(),
            before_shuffle_reads + 1,
            "CardPilePosition.Random costs exactly one Shuffle-stream read",
        );
        let soot_uid = *state
            .player
            .draw
            .iter()
            .find(|u| state.cards.iter().any(|c| c.uid == **u))
            .expect("the spawned Soot is the only uid backed by the card arena");
        let card = state
            .cards
            .iter()
            .find(|c| c.uid == soot_uid)
            .expect("soot card in arena");
        assert_eq!(card.card_id, CardId::Soot);
        assert_eq!(card.card_type, CardType::Status);
        assert_eq!(card.cost, -1);
    }

    /// The Soot POD comes from the pinned status spec, so it carries the
    /// `GeneratedInCombat` marker (C# `CardModel.DeckVersion == null`) that
    /// the old hand-rolled `Card::pod_defaults()` build dropped. Bead
    /// `sts2-rs-lfdaa`.
    #[test]
    fn listener_biiig_hug_soot_is_marked_generated_in_combat() {
        use sts2_core::cards::VarKey;
        use sts2_core::relic_effects::{dispatch_after_shuffle_bus, ShuffleCtx};
        crate::relics::dispatch::ensure_installed();
        let mut state = sts2_core::CombatState::default();
        equip_relic(&mut state, RelicKind::BiiigHug);
        let _ = dispatch_after_shuffle_bus(&mut state, &ShuffleCtx::default());
        let soot = state.cards.last().expect("soot card in arena");
        assert_eq!(soot.combat_var(VarKey::GeneratedInCombat), 1);
    }

    /// Bead `sts2-rs-70anj.4`: NinjaScroll spawns `NINJA_SCROLL_SHIVS = 3`
    /// Shivs into the player's hand on the round-1 player side-turn
    /// start. Subsequent rounds and enemy turns are no-op.
    #[test]
    fn listener_ninja_scroll_spawns_three_shivs_on_round_one_player_turn() {
        use std::num::NonZeroU32;
        use sts2_core::cards::CardId;
        crate::relics::dispatch::ensure_installed();
        let mut state = sts2_core::CombatState::default();
        let id = NonZeroU32::new(u32::try_from(state.relics.len() + 1).unwrap()).unwrap();
        state.relics.push(sts2_core::Relic {
            id,
            kind: RelicKind::NinjaScroll,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        state.round = 1;
        let before_hand = state.player.hand.len();
        let res = listener_ninja_scroll(&mut state, Side::Player);
        assert!(matches!(res, EffectResult::Resolved));
        assert_eq!(
            state.player.hand.len(),
            before_hand + usize::try_from(NINJA_SCROLL_SHIVS).unwrap()
        );
        // Every newly-added card is a Shiv.
        let new_uids = &state.player.hand[before_hand..];
        for uid in new_uids {
            let card = state.cards.iter().find(|c| c.uid == *uid).unwrap();
            assert_eq!(card.card_id, CardId::Shiv);
            assert_eq!(card.cost, 0);
            assert_eq!(card.base_damage, 4);
        }
    }

    /// Bead `sts2-rs-70anj.4`: NinjaScroll listener is no-op on
    /// enemy side and on rounds >= 2 (per Python's
    /// `if side == PLAYER and combat.round_number == 1` gate).
    #[test]
    fn listener_ninja_scroll_is_noop_off_round_one_or_enemy_side() {
        use std::num::NonZeroU32;
        crate::relics::dispatch::ensure_installed();
        let mut state = sts2_core::CombatState::default();
        let id = NonZeroU32::new(u32::try_from(state.relics.len() + 1).unwrap()).unwrap();
        state.relics.push(sts2_core::Relic {
            id,
            kind: RelicKind::NinjaScroll,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let before_hand = state.player.hand.len();

        // Enemy side at round 1: noop.
        let _ = listener_ninja_scroll(&mut state, Side::Enemy);
        assert_eq!(state.player.hand.len(), before_hand);
    }

    /// Bead `sts2-rs-70anj.4`: ElectricShrymp enchants exactly 1
    /// Skill card with `Imbued(1)`. With a deck of [Strike, Defend,
    /// Strike], only the Defend (Skill) gets enchanted — the first
    /// Strike (Attack) is skipped by the Skill-only Imbued gate.
    #[test]
    fn listener_electric_shrymp_enchants_one_skill_card_with_imbued_one() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad);
        push_deck_card(&mut rs, CardId::DefendIronclad);
        push_deck_card(&mut rs, CardId::StrikeIronclad);
        rs.relics.push(RelicId::ElectricShrymp);
        rs.last_obtained_relic = Some(RelicId::ElectricShrymp);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let enchanted: Vec<_> = rs
            .master_deck
            .iter()
            .filter(|c| c.enchantment(EnchantmentId::Imbued) == 1)
            .collect();
        assert_eq!(enchanted.len(), 1, "exactly 1 card enchanted");
        assert_eq!(
            enchanted[0].card_id,
            CardId::DefendIronclad,
            "the Defend (Skill) must be the enchanted card, not the Strikes"
        );
    }

    /// Bead `sts2-rs-70anj.4`: RadiantPearl spawns 1 Luminesce into
    /// the player's hand on the round-1 player side-turn start.
    #[test]
    fn listener_radiant_pearl_spawns_one_luminesce_on_round_one_player_turn() {
        use std::num::NonZeroU32;
        use sts2_core::cards::CardId;
        crate::relics::dispatch::ensure_installed();
        let mut state = sts2_core::CombatState::default();
        let id = NonZeroU32::new(u32::try_from(state.relics.len() + 1).unwrap()).unwrap();
        state.relics.push(sts2_core::Relic {
            id,
            kind: RelicKind::RadiantPearl,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        state.round = 1;
        let before_hand = state.player.hand.len();
        let res = listener_radiant_pearl(&mut state, Side::Player);
        assert!(matches!(res, EffectResult::Resolved));
        assert_eq!(state.player.hand.len(), before_hand + 1);
        let new_uid = *state.player.hand.last().unwrap();
        let card = state.cards.iter().find(|c| c.uid == new_uid).unwrap();
        assert_eq!(card.card_id, CardId::Luminesce);
    }

    /// Bead `sts2-rs-70anj.4`: RadiantPearl listener is no-op on
    /// enemy side and on rounds >= 2.
    #[test]
    fn listener_radiant_pearl_is_noop_off_round_one_or_enemy_side() {
        use std::num::NonZeroU32;
        crate::relics::dispatch::ensure_installed();
        let mut state = sts2_core::CombatState::default();
        let id = NonZeroU32::new(u32::try_from(state.relics.len() + 1).unwrap()).unwrap();
        state.relics.push(sts2_core::Relic {
            id,
            kind: RelicKind::RadiantPearl,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        state.round = 1;
        let before_hand = state.player.hand.len();
        // Enemy side round 1: noop.
        let _ = listener_radiant_pearl(&mut state, Side::Enemy);
        assert_eq!(state.player.hand.len(), before_hand);
        // Player side round 2: noop.
        state.round = 2;
        let _ = listener_radiant_pearl(&mut state, Side::Player);
        assert_eq!(state.player.hand.len(), before_hand);
    }

    /// Bead `sts2-rs-70anj.4`: ToyBox pushes `TOY_BOX_RELICS = 4`
    /// RelicReward sentinels onto `rs.pending_rewards`.
    #[test]
    fn listener_toy_box_pushes_four_relic_reward_sentinels_on_obtain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // Post-#69: gate on `last_obtained_relic` + roll real ids; the
        // per-entry ToyBox-identity check no longer holds.
        let (by_rarity, fallback, boss) = crate::run_callbacks::default_relic_grab_bag();
        rs.populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
        rs.relics.push(RelicId::ToyBox);
        rs.last_obtained_relic = Some(RelicId::ToyBox);
        let before = rs.relic_rewards.len();
        rs.last_obtained_relic = Some(RelicId::ToyBox);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let pushed = rs.relic_rewards.len() - before;
        assert_eq!(pushed, TOY_BOX_RELICS);
        // C# `ToyBox.cs:93` stamps `IsWax = true` on every offered model.
        assert!(
            rs.relic_rewards[before..].iter().all(|r| r.is_wax),
            "every ToyBox offer is wax"
        );
        // The flag rides the OFFER: only a TAKEN reward becomes wax.
        let taken = rs.relic_rewards[before].relic_id;
        let skipped = rs.relic_rewards[before + 1].relic_id;
        assert!(rs.wax_relics.is_empty(), "nothing wax before a pick");
        let _ = rs.accept_relic_reward(before);
        assert!(rs.wax_relics.contains(&taken), "taken offer is wax");
        assert!(
            !rs.wax_relics.contains(&skipped),
            "an unpicked offer never becomes wax"
        );
    }

    /// Bead `sts2-rs-70anj.4`: ToyBox listener is a no-op without
    /// ownership.
    #[test]
    fn listener_toy_box_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before = rs.relic_rewards.len();
        let res = listener_toy_box(&mut rs);
        assert!(matches!(res, EffectResult::NotImplemented));
        assert_eq!(rs.relic_rewards.len(), before);
    }

    /// Bead `sts2-rs-70anj.4`: PaelsClaw enchants every deck card
    /// with `Goopy(1)` unconditionally (Python `enchant_all_cards`
    /// has no eligibility check).
    #[test]
    fn listener_paels_claw_enchants_all_deck_cards_with_goopy() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad);
        push_deck_card(&mut rs, CardId::DefendIronclad);
        push_deck_card(&mut rs, CardId::Bash);
        let n = rs.master_deck.len();
        rs.relics.push(RelicId::PaelsClaw);
        rs.last_obtained_relic = Some(RelicId::PaelsClaw);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let goopy_count = rs
            .master_deck
            .iter()
            .filter(|c| c.enchantment(EnchantmentId::Goopy) >= 1)
            .count();
        assert_eq!(goopy_count, n, "every deck card receives Goopy(1)");
    }

    /// Bead `sts2-rs-70anj.4`: PaelsClaw listener is a no-op without
    /// ownership.
    #[test]
    fn listener_paels_claw_is_noop_without_ownership() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad);
        let res = listener_paels_claw(&mut rs);
        assert!(matches!(res, EffectResult::NotImplemented));
        assert_eq!(rs.master_deck[0].enchantment(EnchantmentId::Goopy), 0);
    }

    /// Bead `sts2-rs-70anj.4`: VelvetChoker's `after_card_played`
    /// listener bumps `relic.counter` once; the player-side
    /// `before_side_turn_start` resets it back to 0.
    #[test]
    fn listener_velvet_choker_counter_bumps_and_resets() {
        use std::num::NonZeroU32;
        use sts2_core::relic_effects::CardEventCtx;
        crate::relics::dispatch::ensure_installed();
        let mut state = sts2_core::CombatState::default();
        let id = NonZeroU32::new(u32::try_from(state.relics.len() + 1).unwrap()).unwrap();
        state.relics.push(sts2_core::Relic {
            id,
            kind: RelicKind::VelvetChoker,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let ctx = CardEventCtx {
            card_id: sts2_core::cards::CardId::StrikeIronclad,
            card_uid: None,
            target: None,
            card_tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
        };
        let _ = listener_velvet_choker_card_played(&mut state, &ctx);
        let ctx = CardEventCtx {
            card_id: sts2_core::cards::CardId::StrikeIronclad,
            card_uid: None,
            target: None,
            card_tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
        };
        let _ = listener_velvet_choker_card_played(&mut state, &ctx);
        assert_eq!(state.relics[0].counter, 2, "counter bumped twice");
        // Enemy side does not reset.
        let _ = listener_velvet_choker_turn_reset(&mut state, Side::Enemy);
        assert_eq!(state.relics[0].counter, 2, "enemy side: no reset");
        // Player side resets.
        let _ = listener_velvet_choker_turn_reset(&mut state, Side::Player);
        assert_eq!(state.relics[0].counter, 0, "player side: reset to 0");
    }

    /// Bead `sts2-rs-70anj.4`: VelvetChoker counter listener is a no-op
    /// without ownership.
    #[test]
    fn listener_velvet_choker_card_played_is_noop_without_ownership() {
        use sts2_core::relic_effects::CardEventCtx;
        crate::relics::dispatch::ensure_installed();
        let mut state = sts2_core::CombatState::default();
        let ctx = CardEventCtx {
            card_id: sts2_core::cards::CardId::StrikeIronclad,
            card_uid: None,
            target: None,
            card_tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
        };
        let res = listener_velvet_choker_card_played(&mut state, &ctx);
        assert!(matches!(res, EffectResult::NotImplemented));
    }

    /// Bead `sts2-rs-70anj.4`: Ectoplasm vetoes every positive gold
    /// gain via the should_gain_gold dispatcher.
    #[test]
    fn listener_ectoplasm_vetoes_positive_gold_gain() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::Ectoplasm);
        let gold_before = rs.gold;
        rs.gain_gold(100);
        assert_eq!(rs.gold, gold_before, "Ectoplasm vetoes positive gain");
    }

    /// Bead `sts2-rs-70anj.4`: Ectoplasm listener directly is a no-op
    /// without ownership and does not set the cancel flag.
    #[test]
    fn listener_ectoplasm_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut cancel = false;
        let res = listener_ectoplasm_should_gain_gold(&mut rs, 50, &mut cancel);
        assert!(matches!(res, EffectResult::NotImplemented));
        assert!(!cancel, "no veto without ownership");
    }

    /// Bead `sts2-rs-70anj.4`: PaelsGrowth enchants exactly 1 eligible
    /// deck card with `Clone(4)`. Rest-site CloneOption leg is deferred.
    #[test]
    fn listener_paels_growth_enchants_one_card_with_clone_four() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..3 {
            push_deck_card(&mut rs, CardId::StrikeIronclad);
        }
        rs.relics.push(RelicId::PaelsGrowth);
        rs.last_obtained_relic = Some(RelicId::PaelsGrowth);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let cloned = rs
            .master_deck
            .iter()
            .filter(|c| c.enchantment(EnchantmentId::Clone) == 4)
            .count();
        assert_eq!(cloned, 1, "exactly 1 card enchanted with Clone(4)");
    }

    /// Bead `sts2-rs-70anj.4`: PaelsGrowth listener is a no-op without
    /// ownership.
    #[test]
    fn listener_paels_growth_is_noop_without_ownership() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad);
        let res = listener_paels_growth(&mut rs);
        assert!(matches!(res, EffectResult::NotImplemented));
        assert_eq!(rs.master_deck[0].enchantment(EnchantmentId::Clone), 0);
    }

    /// Bead `sts2-rs-70anj.4`: WingCharm tags exactly one random card-
    /// reward option with `Swift(1)`.
    #[test]
    fn listener_wing_charm_enchants_exactly_one_option_with_swift_one() {
        use sts2_core::cards::{Card, CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::WingCharm);
        let mut options: Vec<Card> = (0..3)
            .map(|_| {
                let mut c = Card::pod_defaults();
                c.card_id = CardId::StrikeIronclad;
                c
            })
            .collect();
        let res = listener_wing_charm_card_reward(&mut rs, &mut options);
        assert!(matches!(res, EffectResult::Resolved));
        let swift_count = options
            .iter()
            .filter(|c| c.enchantment(EnchantmentId::Swift) == WING_CHARM_SWIFT)
            .count();
        assert_eq!(swift_count, 1, "exactly one option enchanted");
    }

    /// Bead `sts2-rs-70anj.4`: WingCharm is a no-op when the option
    /// list is empty.
    #[test]
    fn listener_wing_charm_is_noop_on_empty_options() {
        use sts2_core::cards::Card;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::WingCharm);
        let mut options: Vec<Card> = vec![];
        let res = listener_wing_charm_card_reward(&mut rs, &mut options);
        assert!(matches!(res, EffectResult::NotImplemented));
    }

    /// Bead `sts2-rs-70anj.4`: TriBoomerang enchants up to
    /// `TRI_BOOMERANG_CARDS = 3` Attack cards with `Instinct(1)`
    /// (a 2× powered-attack damage multiplier; no cost change).
    #[test]
    fn listener_tri_boomerang_enchants_three_attacks_with_instinct_one() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..5 {
            push_deck_card(&mut rs, CardId::StrikeIronclad);
        }
        let cost_before = rs.master_deck[0].cost;
        rs.relics.push(RelicId::TriBoomerang);
        rs.last_obtained_relic = Some(RelicId::TriBoomerang);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let enchanted = rs
            .master_deck
            .iter()
            .filter(|c| c.enchantment(EnchantmentId::Instinct) == 1)
            .count();
        assert_eq!(enchanted, TRI_BOOMERANG_CARDS);
        // C# `Instinct.cs` is a 2× powered-attack damage multiplier with no
        // cost behavior — enchanting must NOT change the card's cost.
        assert_eq!(rs.master_deck[0].cost, cost_before);
        assert_eq!(
            rs.master_deck[4].cost, cost_before,
            "unenchanted card cost unchanged"
        );
    }

    /// Bead `sts2-rs-70anj.4`: Storybook's `after_obtained` listener
    /// must push exactly one `BrightestFlame` card to `rs.master_deck`.
    #[test]
    fn listener_storybook_adds_one_brightest_flame_to_master_deck_on_obtain() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.master_deck.len();
        rs.relics.push(RelicId::Storybook);
        rs.last_obtained_relic = Some(RelicId::Storybook);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len + 1);
        assert_eq!(
            rs.master_deck.last().unwrap().card_id,
            CardId::BrightestFlame
        );
    }

    /// Bead `sts2-rs-70anj.4`: Storybook listener is no-op without
    /// ownership.
    #[test]
    fn listener_storybook_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    /// Bead `sts2-rs-5xg1w`: Glitter's
    /// `modify_card_reward_options_late` listener must add Glam=1 to
    /// every card in the reward options list when owned.
    #[test]
    fn listener_glitter_applies_glam_to_all_card_reward_options() {
        use sts2_core::cards::{Card, CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::Glitter);
        let mut cards: Vec<Card> = [CardId::StrikeIronclad, CardId::DefendIronclad]
            .iter()
            .map(|id| {
                let mut c = Card::pod_defaults();
                c.card_id = *id;
                c
            })
            .collect();
        let _ =
            sts2_core::run_effects::dispatch_modify_card_reward_options_late(&mut rs, &mut cards);
        assert!(cards
            .iter()
            .all(|c| c.enchantment(EnchantmentId::Glam) == 1));
    }

    /// Bead `sts2-rs-5xg1w`: Glitter listener is noop without
    /// ownership.
    #[test]
    fn listener_glitter_is_noop_without_ownership() {
        use sts2_core::cards::{Card, CardId, EnchantmentId};
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut cards: Vec<Card> = [CardId::StrikeIronclad]
            .iter()
            .map(|id| {
                let mut c = Card::pod_defaults();
                c.card_id = *id;
                c
            })
            .collect();
        let _ =
            sts2_core::run_effects::dispatch_modify_card_reward_options_late(&mut rs, &mut cards);
        assert_eq!(cards[0].enchantment(EnchantmentId::Glam), 0);
    }

    /// Bead `sts2-rs-psf9`: Orrery listener is noop without ownership.
    #[test]
    fn listener_orrery_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.pending_rewards.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.pending_rewards.len(), before_len);
    }

    /// Bead `sts2-rs-r4pr`: both r4pr listeners must be noop when their
    /// relic is not in `rs.relics`. Combined into one test to keep the
    /// fixture count tight.
    #[test]
    fn listener_r4pr_relics_are_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_max = rs.max_hp;
        let before_hp = rs.current_hp;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max);
        assert_eq!(rs.current_hp, before_hp);
    }

    /// No-op when the relic is not yet in `rs.relics`.
    #[test]
    fn listener_lees_waffle_is_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_max = rs.max_hp;
        let before_hp = rs.current_hp;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max);
        assert_eq!(rs.current_hp, before_hp);
    }

    // -----------------------------------------------------------------
    // PaelsFlesh listener tests — bead `sts2-rs-p6dj`
    // -----------------------------------------------------------------
    //
    // Single-channel `after_side_turn_start` listener: from player
    // round 3 onward, the player gains +1 energy each player turn.
    // Tests stand up a minimal CombatState with a player creature and
    // exercise the listener end-to-end via the dispatch bus.

    #[cfg(test)]
    fn paels_flesh_setup(state: &mut CombatState, round: u16, equip: bool) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        state.player.energy = 3;
        state.round = round;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::PaelsFlesh,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    /// **PaelsFlesh #1** — round 3 player turn-start grants +1 energy.
    #[test]
    fn listener_paels_flesh_grants_energy_on_round_three_player_turn() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        paels_flesh_setup(&mut state, 3, true);
        let before = state.player.energy;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(
            state.player.energy,
            before + i8::try_from(PAELS_FLESH_ENERGY).unwrap()
        );
    }

    /// **PaelsFlesh #2** — no-op on player rounds 1 / 2 (below the
    /// `round_min` gate).
    #[test]
    fn listener_paels_flesh_is_noop_before_round_three() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        for round in [1u16, 2] {
            let mut state = CombatState::default();
            paels_flesh_setup(&mut state, round, true);
            let before = state.player.energy;
            let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
            assert_eq!(state.player.energy, before, "round {round} must not fire");
        }
    }

    /// **PaelsFlesh #3** — enemy-side turn-start is a no-op even on
    /// round 3+. The listener gates on `side == Player`.
    #[test]
    fn listener_paels_flesh_is_noop_on_enemy_side() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        paels_flesh_setup(&mut state, 5, true);
        let before = state.player.energy;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Enemy);
        assert_eq!(state.player.energy, before);
    }

    /// **PaelsFlesh #4** — no-op when the relic is not owned, even
    /// on round 3+. Required because the dispatch bus iterates every
    /// registered listener regardless of ownership.
    #[test]
    fn listener_paels_flesh_is_noop_without_ownership() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        paels_flesh_setup(&mut state, 7, false);
        let before = state.player.energy;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.player.energy, before);
    }

    /// SealOfGold #1 — each player turn, while combat gold ≥ 5, spend 5 for +1
    /// energy (C# `SealOfGold.AfterSideTurnStart`). Bead `sts2-rs-RP17`.
    #[test]
    fn listener_seal_of_gold_spends_gold_for_energy() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::SealOfGold);
        state.gold = 12;
        let before = state.player.energy;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.gold, 7, "spends 5 combat gold");
        assert_eq!(
            state.player.energy,
            before + i8::try_from(SEAL_OF_GOLD_ENERGY).unwrap(),
            "gains 1 energy"
        );
    }

    /// SealOfGold #2 — no spend / no energy when gold < 5 (C# `Gold >= 5` gate).
    #[test]
    fn listener_seal_of_gold_noop_when_below_cost() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::SealOfGold);
        state.gold = SEAL_OF_GOLD_COST - 1;
        let before = state.player.energy;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.gold, SEAL_OF_GOLD_COST - 1, "no spend below the gate");
        assert_eq!(state.player.energy, before, "no energy below the gate");
    }

    /// Bread #1 — turn 1 loses `BREAD_LOSE_ENERGY = 2` energy (C#
    /// `Bread.cs:AfterSideTurnStart`, gate `TurnNumber == 1`).
    /// 2026-07-17 soft-drop census fix.
    #[test]
    fn listener_bread_turn_one_loses_two_energy() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::Bread);
        state.round = 1;
        state.player.energy = 3;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.player.energy, 1, "turn 1: 3 energy - 2 = 1");
    }

    /// Bread #2 — no energy loss from turn 2 onward, and none on the
    /// enemy side (`participants.Contains(Owner.Creature)` gate).
    #[test]
    fn listener_bread_noop_after_turn_one_and_on_enemy_side() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::Bread);
        state.round = 2;
        state.player.energy = 4;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.player.energy, 4, "turn 2+: no penalty");
        state.round = 1;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Enemy);
        assert_eq!(state.player.energy, 4, "enemy side: no penalty");
    }

    /// Bread #3 — the loss clamps at 0 (C# `PlayerCombatState.LoseEnergy`
    /// clamps: `Math.Clamp(Energy - amount, 0, …)`).
    #[test]
    fn listener_bread_energy_loss_clamps_at_zero() {
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::Bread);
        state.round = 1;
        state.player.energy = 1;
        assert_eq!(
            super::listener_bread(&mut state, Side::Player),
            EffectResult::Resolved
        );
        assert_eq!(state.player.energy, 0, "1 - 2 clamps to 0, not -1");
    }

    // -----------------------------------------------------------------
    // Sai + VeryHotCocoa listener tests — bead `sts2-rs-70anj.1`
    // -----------------------------------------------------------------

    #[cfg(test)]
    fn equip_relic(state: &mut CombatState, kind: RelicKind) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        let id = NonZeroU32::new(state.relics.len() as u32 + 1).expect("non-zero");
        state.relics.push(Relic {
            id,
            kind,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
    }

    #[test]
    fn listener_sai_grants_block_on_player_turn() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::Sai);
        let before = state.player.block;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.player.block, before + SAI_BLOCK);
    }

    #[test]
    fn listener_sai_is_noop_on_enemy_side() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::Sai);
        let before = state.player.block;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Enemy);
        assert_eq!(state.player.block, before);
    }

    #[test]
    fn listener_sai_is_noop_without_ownership() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::PaelsFlesh); // wrong relic
        let before = state.player.block;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.player.block, before);
    }

    #[test]
    fn listener_very_hot_cocoa_grants_energy_on_round_one() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::VeryHotCocoa);
        state.round = 1;
        state.player.energy = 3;
        let before = state.player.energy;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(
            state.player.energy,
            before + i8::try_from(VERY_HOT_COCOA_ENERGY).unwrap()
        );
    }

    #[test]
    fn listener_very_hot_cocoa_is_noop_after_round_one() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        for round in [2u16, 3, 5, 10] {
            let mut state = CombatState::default();
            equip_relic(&mut state, RelicKind::VeryHotCocoa);
            state.round = round;
            state.player.energy = 3;
            let before = state.player.energy;
            let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
            assert_eq!(state.player.energy, before, "round {round} must not fire");
        }
    }

    #[test]
    fn listener_very_hot_cocoa_is_noop_without_ownership() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::PaelsFlesh);
        state.round = 1;
        state.player.energy = 3;
        let before = state.player.energy;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.player.energy, before);
    }

    // -----------------------------------------------------------------
    // Wave 1 body-fill batch 2 listener tests — bead `sts2-rs-70anj.1`
    // -----------------------------------------------------------------

    #[test]
    fn listener_iron_club_draws_every_fourth_card() {
        use sts2_core::cards::{Card, CardId, CardType};
        use sts2_core::relic_effects::{dispatch_after_card_played, CardEventCtx};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::IronClub);
        // Seed the draw pile with at least 4 cards so the eventual
        // draw_into_hand call has something to move.
        for _ in 0..6 {
            let card = Card {
                card_id: CardId::StrikeIronclad,
                card_type: CardType::Attack,
                ..Card::pod_defaults()
            };
            let _ = state.add_card_to_draw_pile(card);
        }
        let ctx = CardEventCtx {
            card_id: CardId::StrikeIronclad,
            card_uid: None,
            target: None,
            card_tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
        };
        let hand_before = state.player.hand.len();
        // 3 plays — nothing drawn.
        for _ in 0..3 {
            let _ = dispatch_after_card_played(&mut state, &ctx);
        }
        assert_eq!(state.player.hand.len(), hand_before);
        // 4th play triggers draw 1.
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(
            state.player.hand.len(),
            hand_before + 1,
            "IronClub must draw 1 card on the 4th play"
        );
        // 5th-7th: silent again; 8th draws again.
        for _ in 0..3 {
            let _ = dispatch_after_card_played(&mut state, &ctx);
        }
        assert_eq!(state.player.hand.len(), hand_before + 1);
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(
            state.player.hand.len(),
            hand_before + 2,
            "IronClub must draw 1 card on the 8th play"
        );
    }

    #[test]
    fn listener_toasty_mittens_exhausts_and_grants_strength() {
        use sts2_core::cards::{Card, CardId, CardType};
        use sts2_core::powers::PowerKind;
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::ToastyMittens);
        let card = Card {
            card_id: CardId::StrikeIronclad,
            card_type: CardType::Attack,
            ..Card::pod_defaults()
        };
        let uid = state.add_card_to_draw_pile(card);
        let draw_before = state.player.draw.len();
        // C# fires this on `BeforeHandDraw` (before the natural draw), not on
        // the post-draw turn-start walk. Bead `sts2-rs-70anj.1`.
        let _ = dispatch_before_hand_draw(&mut state);
        assert_eq!(
            state.player.draw.len(),
            draw_before - 1,
            "ToastyMittens must exhaust the top card of the draw pile"
        );
        assert!(state.player.exhaust.contains(&uid));
        // Player gains +1 Strength.
        let player_uid = state.player.creature_uid.expect("player uid");
        let creature = state
            .creatures
            .iter()
            .find(|c| c.id == player_uid.0)
            .expect("player creature");
        let strength = creature
            .powers
            .iter()
            .filter_map(|p_uid| state.powers.get_by_id(p_uid.0))
            .find(|p| p.kind == PowerKind::Strength)
            .map_or(0, |p| p.amount);
        assert_eq!(strength, TOASTY_MITTENS_STRENGTH);
    }

    #[test]
    fn listener_toasty_mittens_skips_innate_on_round_one() {
        // C# `ToastyMittens.BeforeHandDraw`: on round 1 it exhausts the first
        // NON-Innate draw card (`cards.FirstOrDefault(!Innate)`), not the literal
        // top card. Bead `sts2-rs-70anj.1`.
        use sts2_core::cards::{Card, CardId, CardType, KeywordOverride};
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        state.round = 1;
        equip_relic(&mut state, RelicKind::ToastyMittens);
        // Top of draw is an Innate card; the next card is a normal Strike.
        let innate = Card {
            card_id: CardId::StrikeIronclad,
            card_type: CardType::Attack,
            keyword_override: Some(KeywordOverride::INNATE),
            ..Card::pod_defaults()
        };
        let normal = Card {
            card_id: CardId::DefendIronclad,
            card_type: CardType::Skill,
            ..Card::pod_defaults()
        };
        // `add_card_to_draw_pile` front-inserts, so add the normal card first
        // (lands below) then the innate card (lands on top).
        let normal_uid = state.add_card_to_draw_pile(normal);
        let innate_uid = state.add_card_to_draw_pile(innate);
        let _ = dispatch_before_hand_draw(&mut state);
        assert!(
            state.player.exhaust.contains(&normal_uid),
            "round-1 ToastyMittens must skip the Innate top card and exhaust the first non-Innate card"
        );
        assert!(
            state.player.draw.contains(&innate_uid),
            "the Innate top card must remain in the draw pile"
        );
    }

    #[test]
    fn listener_jeweled_mask_moves_random_power_to_hand_free() {
        use sts2_core::cards::{Card, CardId, CardType, VarKey};
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::JeweledMask);
        state.round = 1;
        // Mix of Attack + Power cards in draw pile.
        for _ in 0..3 {
            let attack = Card {
                card_id: CardId::StrikeIronclad,
                card_type: CardType::Attack,
                ..Card::pod_defaults()
            };
            let _ = state.add_card_to_draw_pile(attack);
        }
        let power = Card {
            card_id: CardId::Inflame,
            card_type: CardType::Power,
            ..Card::pod_defaults()
        };
        let power_uid = state.add_card_to_draw_pile(power);
        let hand_before = state.player.hand.len();
        let ccs_before = state.run_rng.combat_card_selection.counter();
        let _ = dispatch_before_hand_draw(&mut state);
        assert_eq!(
            state.player.hand.len(),
            hand_before + 1,
            "JeweledMask must move 1 Power card to hand"
        );
        assert!(state.player.hand.contains(&power_uid));
        let card = &state.cards[power_uid.0.get() as usize - 1];
        assert_eq!(
            card.combat_var(VarKey::FreeThisTurn),
            1,
            "Selected Power must have FreeThisTurn = 1"
        );
        // C# `Rng.CombatCardSelection.NextItem(list)` = exactly one
        // `NextInt` tick on the CombatCardSelection stream (Rng.cs:264).
        assert_eq!(
            state.run_rng.combat_card_selection.counter(),
            ccs_before + 1,
            "selection must consume exactly one combat_card_selection read"
        );
    }

    #[test]
    fn listener_jeweled_mask_is_noop_after_round_one() {
        use sts2_core::cards::{Card, CardId, CardType};
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::JeweledMask);
        state.round = 2;
        let power = Card {
            card_id: CardId::Inflame,
            card_type: CardType::Power,
            ..Card::pod_defaults()
        };
        let power_uid = state.add_card_to_draw_pile(power);
        let _ = dispatch_before_hand_draw(&mut state);
        assert!(
            state.player.draw.contains(&power_uid),
            "JeweledMask must not fire after round 1"
        );
    }

    #[test]
    fn listener_jeweled_mask_empty_pool_skips_rng_read() {
        // C# guards `list.Count != 0` BEFORE the `NextItem` call — a
        // Power-less draw pile must neither add a card to hand nor
        // consume a CombatCardSelection tick.
        use sts2_core::cards::{Card, CardId, CardType};
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::JeweledMask);
        state.round = 1;
        let attack = Card {
            card_id: CardId::StrikeIronclad,
            card_type: CardType::Attack,
            ..Card::pod_defaults()
        };
        let _ = state.add_card_to_draw_pile(attack);
        let hand_before = state.player.hand.len();
        let ccs_before = state.run_rng.combat_card_selection.counter();
        let _ = dispatch_before_hand_draw(&mut state);
        assert_eq!(
            state.player.hand.len(),
            hand_before,
            "no Power in draw pile: nothing moves to hand"
        );
        assert_eq!(
            state.run_rng.combat_card_selection.counter(),
            ccs_before,
            "no Power in draw pile: no combat_card_selection read"
        );
    }

    #[test]
    fn listener_burning_sticks_clones_first_skill_exhausted() {
        use sts2_core::cards::{Card, CardId, CardType};
        use sts2_core::relic_effects::{dispatch_after_card_exhausted, CardEventCtx};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::BurningSticks);
        // Stage a Skill card in the arena (no need to actually exhaust
        // through the engine — the listener only inspects card POD).
        let skill = Card {
            card_id: CardId::DefendIronclad,
            card_type: CardType::Skill,
            ..Card::pod_defaults()
        };
        let skill_uid = state.add_card_to_draw_pile(skill);
        let ctx = CardEventCtx {
            card_id: CardId::DefendIronclad,
            card_uid: Some(skill_uid),
            target: None,
            card_tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
        };
        let hand_before = state.player.hand.len();
        let _ = dispatch_after_card_exhausted(&mut state, &ctx);
        assert_eq!(
            state.player.hand.len(),
            hand_before + 1,
            "BurningSticks must spawn a clone of the Skill into hand"
        );
        // Second Skill exhaust does NOT clone (per-combat latch).
        let _ = dispatch_after_card_exhausted(&mut state, &ctx);
        assert_eq!(
            state.player.hand.len(),
            hand_before + 1,
            "BurningSticks must only fire once per combat"
        );
    }

    #[test]
    fn listener_burning_sticks_ignores_attacks() {
        use sts2_core::cards::{Card, CardId, CardType};
        use sts2_core::relic_effects::{dispatch_after_card_exhausted, CardEventCtx};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::BurningSticks);
        let attack = Card {
            card_id: CardId::StrikeIronclad,
            card_type: CardType::Attack,
            ..Card::pod_defaults()
        };
        let attack_uid = state.add_card_to_draw_pile(attack);
        let ctx = CardEventCtx {
            card_id: CardId::StrikeIronclad,
            card_uid: Some(attack_uid),
            target: None,
            card_tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
        };
        let hand_before = state.player.hand.len();
        let _ = dispatch_after_card_exhausted(&mut state, &ctx);
        assert_eq!(
            state.player.hand.len(),
            hand_before,
            "BurningSticks must ignore non-Skill exhausts"
        );
    }

    #[test]
    fn listener_mystic_lighter_adds_damage_to_enchanted_attacks() {
        use sts2_core::cards::{Card, CardId, CardType, EnchantmentId};
        use sts2_core::hooks::ValueProps;
        use sts2_core::relic_effects::{dispatch_modify_damage_additive, ModifyDamageCtx};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::MysticLighter);
        let mut card = Card {
            card_id: CardId::StrikeIronclad,
            card_type: CardType::Attack,
            ..Card::pod_defaults()
        };
        card.set_enchantment(EnchantmentId::Sharp, 3);
        let _ = state.add_card_to_draw_pile(card);
        let player_uid = state.player.creature_uid.expect("player uid");
        let target_uid = player_uid; // any uid — listener only inspects dealer + card
        let ctx = ModifyDamageCtx {
            dealer: Some(player_uid),
            target: target_uid,
            base_damage: 5,
            props: ValueProps::MOVE,
            card: Some(CardId::StrikeIronclad),
            card_uid: None,
        };
        let delta = dispatch_modify_damage_additive(&state, &ctx);
        assert_eq!(delta, MYSTIC_LIGHTER_EXTRA_DAMAGE);
    }

    #[test]
    fn listener_mystic_lighter_ignores_non_enchanted_attacks() {
        use sts2_core::cards::{Card, CardId, CardType};
        use sts2_core::hooks::ValueProps;
        use sts2_core::relic_effects::{dispatch_modify_damage_additive, ModifyDamageCtx};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::MysticLighter);
        let card = Card {
            card_id: CardId::StrikeIronclad,
            card_type: CardType::Attack,
            ..Card::pod_defaults()
        };
        let _ = state.add_card_to_draw_pile(card);
        let player_uid = state.player.creature_uid.expect("player uid");
        let ctx = ModifyDamageCtx {
            dealer: Some(player_uid),
            target: player_uid,
            base_damage: 5,
            props: ValueProps::MOVE,
            card: Some(CardId::StrikeIronclad),
            card_uid: None,
        };
        let delta = dispatch_modify_damage_additive(&state, &ctx);
        assert_eq!(delta, 0);
    }

    // -----------------------------------------------------------------
    // SneckoEye / BrilliantScarf / BlessedAntler listener tests —
    // bead `sts2-rs-70anj.1` (Wave 1 batch 3)
    // -----------------------------------------------------------------

    #[test]
    fn listener_snecko_eye_applies_confused_on_combat_start() {
        use sts2_core::powers::PowerKind;
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::SneckoEye);
        let _ = dispatch_before_combat_start(&mut state);
        let player_uid = state.player.creature_uid.expect("player uid");
        let player_idx = state
            .creatures
            .iter()
            .position(|c| c.id == player_uid.0)
            .expect("player creature");
        let player_power_ids: Vec<_> = state.creatures[player_idx]
            .powers
            .iter()
            .map(|p| p.0)
            .collect();
        let has_confused = state.powers.iter().any(|p| {
            player_power_ids.contains(&p.id) && p.kind == PowerKind::Confused && p.amount >= 1
        });
        assert!(
            has_confused,
            "SneckoEye must apply 1 stack of Confused to the player at combat start"
        );
    }

    #[test]
    fn listener_snecko_eye_grants_extra_hand_draw() {
        use sts2_core::relic_effects::dispatch_modify_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::SneckoEye);
        let base = 5;
        let modified = dispatch_modify_hand_draw(&state, base);
        assert_eq!(modified, base + SNECKO_EYE_EXTRA_DRAW);
    }

    #[test]
    fn listener_snecko_eye_is_noop_without_ownership() {
        use sts2_core::relic_effects::dispatch_modify_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let state = CombatState::default();
        assert_eq!(dispatch_modify_hand_draw(&state, 5), 5);
    }

    #[test]
    fn listener_brilliant_scarf_tracks_card_plays_and_resets_on_player_turn() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::{
            dispatch_after_card_played, dispatch_before_side_turn_start, CardEventCtx,
        };
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::BrilliantScarf);
        let ctx = CardEventCtx {
            card_id: CardId::StrikeIronclad,
            card_uid: None,
            target: None,
            card_tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
        };
        for _ in 0..4 {
            let _ = dispatch_after_card_played(&mut state, &ctx);
        }
        let counter = state
            .relics
            .iter()
            .find(|r| !r.is_melted && r.kind == RelicKind::BrilliantScarf)
            .map(|r| r.counter)
            .unwrap_or_default();
        assert_eq!(counter, 4);
        let _ = dispatch_before_side_turn_start(&mut state, Side::Player);
        let counter_after_reset = state
            .relics
            .iter()
            .find(|r| !r.is_melted && r.kind == RelicKind::BrilliantScarf)
            .map_or(99, |r| r.counter);
        assert_eq!(counter_after_reset, 0);
    }

    #[test]
    fn listener_brilliant_scarf_does_not_reset_on_enemy_turn() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::{
            dispatch_after_card_played, dispatch_before_side_turn_start, CardEventCtx,
        };
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::BrilliantScarf);
        let ctx = CardEventCtx {
            card_id: CardId::StrikeIronclad,
            card_uid: None,
            target: None,
            card_tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
        };
        let _ = dispatch_after_card_played(&mut state, &ctx);
        let _ = dispatch_before_side_turn_start(&mut state, Side::Enemy);
        let counter = state
            .relics
            .iter()
            .find(|r| !r.is_melted && r.kind == RelicKind::BrilliantScarf)
            .map(|r| r.counter)
            .unwrap_or_default();
        assert_eq!(counter, 1, "Enemy-turn start must not reset the counter");
    }

    #[test]
    fn listener_blessed_antler_inserts_three_dazed_on_round_one_hand_draw() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::BlessedAntler);
        state.round = 1;
        let draw_before = state.player.draw.len();
        let _ = dispatch_before_hand_draw(&mut state);
        assert_eq!(
            state.player.draw.len(),
            draw_before + BLESSED_ANTLER_DAZED_COUNT as usize,
        );
        // All newly-added cards should be Dazed.
        let dazed_count = state
            .player
            .draw
            .iter()
            .filter(|&&uid| {
                let idx = uid.0.get() as usize - 1;
                state.cards[idx].card_id == CardId::Dazed
            })
            .count();
        assert_eq!(dazed_count, BLESSED_ANTLER_DAZED_COUNT as usize);
    }

    /// Bead `sts2-rs-lfdaa`: C# adds each Dazed at `CardPilePosition.Random`,
    /// which costs one Shuffle-stream read apiece
    /// (`AddInternal(card, Rng.Shuffle.NextInt(Cards.Count + 1))`). The prior
    /// top-insert consumed none, desyncing every later draw/reshuffle in the
    /// combat. Also pins that a Dazed can land BELOW an existing card — a
    /// top-insert would put all three above the seeded pile.
    #[test]
    fn listener_blessed_antler_dazed_land_at_random_depths() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        use sts2_core::CardUid;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::BlessedAntler);
        state.round = 1;
        // Seed a draw pile so the random index has more than one slot.
        for i in 1..=6u32 {
            state
                .player
                .draw
                .push(CardUid::new(i + 500).expect("non-zero"));
        }
        let reads_before = state.run_rng.shuffle.counter();

        let _ = dispatch_before_hand_draw(&mut state);

        assert_eq!(
            state.run_rng.shuffle.counter(),
            reads_before + BLESSED_ANTLER_DAZED_COUNT.unsigned_abs(),
            "one Shuffle read per CardPilePosition.Random insert",
        );
        let is_dazed = |uid: &sts2_core::CardUid| {
            let idx = uid.0.get() as usize - 1;
            state
                .cards
                .get(idx)
                .is_some_and(|c| c.card_id == CardId::Dazed)
        };
        let last_dazed_at = state.player.draw.iter().rposition(is_dazed);
        assert!(
            last_dazed_at.is_some_and(|i| i >= BLESSED_ANTLER_DAZED_COUNT as usize),
            "a random-depth insert must be able to land below the seeded pile, \
             not stack all three on top",
        );
    }

    #[test]
    fn listener_blessed_antler_is_noop_after_round_one() {
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::BlessedAntler);
        state.round = 2;
        let draw_before = state.player.draw.len();
        let _ = dispatch_before_hand_draw(&mut state);
        assert_eq!(state.player.draw.len(), draw_before);
    }

    #[test]
    fn listener_blessed_antler_is_noop_without_ownership() {
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::VeryHotCocoa);
        state.round = 1;
        let draw_before = state.player.draw.len();
        let _ = dispatch_before_hand_draw(&mut state);
        assert_eq!(state.player.draw.len(), draw_before);
    }

    /// `TheAbacus` is inert when the player does not own the relic.
    #[test]
    fn the_abacus_inert_when_not_owned() {
        use sts2_core::relic_effects::{dispatch_after_shuffle_bus, ShuffleCtx};

        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        // No relic equipped.
        let _ = dispatch_after_shuffle_bus(&mut state, &ShuffleCtx::default());
        assert_eq!(
            state.player.block, 0,
            "TheAbacus listener must be owner-gated"
        );
    }

    /// Bead `sts2-rs-83sa`: BlackStar appends a `Reward::Relic` on
    /// Elite-room clears. The concrete `RelicId` carried by the
    /// `RelicReward` POD is BlackStar itself (placeholder sentinel for
    /// the deferred "random relic" semantics — see listener doc).
    #[test]
    fn listener_black_star_appends_relic_reward_on_elite_rooms() {
        use sts2_core::run_effects::dispatch_modify_rewards;
        use sts2_core::runstate::{RelicReward, Reward, RoomType, RunState};

        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::BlackStar);
        let mut rewards: Vec<Reward> = Vec::new();
        let _ = dispatch_modify_rewards(&mut rs, &mut rewards, RoomType::Elite);
        assert_eq!(rewards.len(), 1);
        assert_eq!(
            rewards[0],
            Reward::Relic(RelicReward::new(RelicId::BlackStar))
        );
    }

    /// Bead `sts2-rs-83sa`: BlackStar is silent on every non-Elite
    /// room — Monster rooms in particular are PrayerWheel's territory.
    #[test]
    fn listener_black_star_is_noop_on_non_elite_rooms() {
        use sts2_core::run_effects::dispatch_modify_rewards;
        use sts2_core::runstate::{Reward, RoomType, RunState};

        crate::relics::dispatch::ensure_installed();
        for rt in [
            RoomType::Monster,
            RoomType::Boss,
            RoomType::Shop,
            RoomType::Treasure,
            RoomType::Event,
            RoomType::RestSite,
            RoomType::Unknown,
        ] {
            let mut rs = RunState::new();
            rs.relics.push(RelicId::BlackStar);
            let mut rewards: Vec<Reward> = Vec::new();
            let _ = dispatch_modify_rewards(&mut rs, &mut rewards, rt);
            assert!(
                rewards.is_empty(),
                "BlackStar must be silent on {rt:?} rooms"
            );
        }
    }

    /// Bead `sts2-rs-83sa`: BlackStar listener is a no-op when the
    /// player does not own the relic.
    #[test]
    fn listener_black_star_is_noop_without_ownership() {
        use sts2_core::run_effects::dispatch_modify_rewards;
        use sts2_core::runstate::{Reward, RoomType, RunState};

        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut rewards: Vec<Reward> = Vec::new();
        let _ = dispatch_modify_rewards(&mut rs, &mut rewards, RoomType::Elite);
        assert!(rewards.is_empty());
    }

    // ─── 70anj.4.1: run-state-only no-op pins (bead `sts2-rs-qsxiy`) ─
    //
    // Five `shop_event.rs` relics whose only Python-side method is a
    // run-state hook (`after_obtained` for at-pickup pickups,
    // `modify_card_reward_creation_options` for reward-pool mutators).
    // Their `on_trigger_*` bodies are intentionally trivial — wiring
    // real combat-side logic would be a parity divergence from the
    // Python pin. These tests pin that contract so a future
    // body-fill pass doesn't accidentally introduce combat-side
    // behavior under the same RelicId.

    /// `Cauldron` — combat dispatch is a no-op by design. Future
    /// body-fill targets the run-state at-obtain channel
    /// (`POTIONS = 5` already pinned by [`CAULDRON_POTIONS`]).
    #[test]
    fn cauldron_combat_dispatch_is_noop() {
        assert_eq!(CAULDRON_POTIONS, 5);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::Cauldron.to_python_name(), "CAULDRON");
    }

    /// `DingyRug` — combat dispatch is a no-op by design. The
    /// effect lives on the run-state card-reward channel
    /// (`modify_card_reward_creation_options` in Python).
    #[test]
    fn dingy_rug_combat_dispatch_is_noop() {
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::DingyRug.to_python_name(), "DINGY_RUG");
    }

    /// `DollysMirror` — combat dispatch is a no-op by design.
    /// Future body-fill mutates the run-state master deck at obtain.
    #[test]
    fn dollys_mirror_combat_dispatch_is_noop() {
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::DollysMirror.to_python_name(), "DOLLYS_MIRROR");
    }

    /// `Orrery` — combat dispatch is a no-op by design. Future
    /// body-fill queues five card rewards on the run-state at-obtain
    /// channel (count pinned by [`ORRERY_CARDS`]).
    #[test]
    fn orrery_combat_dispatch_is_noop() {
        assert_eq!(ORRERY_CARDS, 5);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::Orrery.to_python_name(), "ORRERY");
    }

    /// `GnarledHammer` — combat dispatch is a no-op by design.
    /// Future body-fill enchants up to 3 deck cards with Sharp(3)
    /// at obtain (counts pinned by [`GNARLED_HAMMER_CARDS`] +
    /// [`GNARLED_HAMMER_SHARP`]).
    #[test]
    fn gnarled_hammer_combat_dispatch_is_noop() {
        assert_eq!(GNARLED_HAMMER_CARDS, 3);
        assert_eq!(GNARLED_HAMMER_SHARP, 3);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::GnarledHammer.to_python_name(), "GNARLED_HAMMER");
    }

    // ─── 70anj.4.2: second no-op pin batch (bead `sts2-rs-4nyt7`) ──

    /// `Kifuda` — combat dispatch is a no-op by design. Future body-
    /// fill enchants up to 3 deck cards with Adroit(3) at obtain
    /// (counts pinned by [`KIFUDA_CARDS`] + [`KIFUDA_ADROIT`]).
    #[test]
    fn kifuda_combat_dispatch_is_noop() {
        assert_eq!(KIFUDA_CARDS, 3);
        assert_eq!(KIFUDA_ADROIT, 3);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::Kifuda.to_python_name(), "KIFUDA");
    }

    /// `MiniatureTent` — combat dispatch is a no-op by design. The
    /// effect (`should_disable_remaining_rest_site_options`) is a
    /// run-state map-screen predicate and never fires in combat.
    #[test]
    fn miniature_tent_combat_dispatch_is_noop() {
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::MiniatureTent.to_python_name(), "MINIATURE_TENT");
    }

    /// `PunchDagger` — combat dispatch is a no-op by design. Future
    /// body-fill enchants 1 deck card with Momentum(5) at obtain
    /// (amount pinned by [`PUNCH_DAGGER_MOMENTUM`]).
    #[test]
    fn punch_dagger_combat_dispatch_is_noop() {
        assert_eq!(PUNCH_DAGGER_MOMENTUM, 5);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::PunchDagger.to_python_name(), "PUNCH_DAGGER");
    }

    /// `BeautifulBracelet` — combat dispatch is a no-op by design.
    /// Future body-fill enchants 3 deck cards with Swift(3) at obtain
    /// (counts pinned by [`BEAUTIFUL_BRACELET_CARDS`] +
    /// [`BEAUTIFUL_BRACELET_SWIFT`]).
    #[test]
    fn beautiful_bracelet_combat_dispatch_is_noop() {
        assert_eq!(BEAUTIFUL_BRACELET_CARDS, 3);
        assert_eq!(BEAUTIFUL_BRACELET_SWIFT, 3);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(
            RelicId::BeautifulBracelet.to_python_name(),
            "BEAUTIFUL_BRACELET"
        );
    }

    /// `CallingBell` — combat dispatch is a no-op by design. Future
    /// body-fill adds CurseOfTheBell to deck and queues 3 relic
    /// rewards at obtain (count pinned by [`CALLING_BELL_RELICS`]).
    #[test]
    fn calling_bell_combat_dispatch_is_noop() {
        assert_eq!(CALLING_BELL_RELICS, 3);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::CallingBell.to_python_name(), "CALLING_BELL");
    }

    // -----------------------------------------------------------------
    // 70anj.4.3 no-op pin tests — bead `sts2-rs-n06dm`
    // -----------------------------------------------------------------

    /// `MembershipCard` — combat dispatch is a no-op by design. The
    /// effect (`modify_merchant_price`) is a run-state shop-screen
    /// hook and never fires in combat.
    #[test]
    fn membership_card_combat_dispatch_is_noop() {
        assert_eq!(MEMBERSHIP_CARD_DISCOUNT, 50);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::MembershipCard.to_python_name(), "MEMBERSHIP_CARD");
    }

    /// `RoyalStamp` — combat dispatch is a no-op by design. Future
    /// body-fill enchants one deck card with `RoyallyApproved` at
    /// obtain (count hard-coded to 1 in Python; no separate const).
    #[test]
    fn royal_stamp_combat_dispatch_is_noop() {
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::RoyalStamp.to_python_name(), "ROYAL_STAMP");
    }

    /// `CursedPearl` — combat dispatch is a no-op by design. Future
    /// body-fill adds the Greed curse to the deck and grants
    /// `CURSED_PEARL_GOLD` gold at obtain.
    #[test]
    fn cursed_pearl_combat_dispatch_is_noop() {
        assert_eq!(CURSED_PEARL_GOLD, 333);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::CursedPearl.to_python_name(), "CURSED_PEARL");
    }

    /// `ArcaneScroll` — combat dispatch is a no-op by design. Future
    /// body-fill adds one random Rare card to the deck at obtain
    /// (count hard-coded to 1 in Python; no separate const).
    #[test]
    fn arcane_scroll_combat_dispatch_is_noop() {
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::ArcaneScroll.to_python_name(), "ARCANE_SCROLL");
    }

    /// `AlchemicalCoffer` — combat dispatch is a no-op by design.
    /// Future body-fill gains `ALCHEMICAL_COFFER_POTION_SLOTS` potion
    /// slots and procures that many random potions at obtain.
    #[test]
    fn alchemical_coffer_combat_dispatch_is_noop() {
        assert_eq!(ALCHEMICAL_COFFER_POTION_SLOTS, 4);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(
            RelicId::AlchemicalCoffer.to_python_name(),
            "ALCHEMICAL_COFFER"
        );
    }

    // -----------------------------------------------------------------
    // 70anj.4.4 no-op pin tests — bead `sts2-rs-kluld`
    // -----------------------------------------------------------------

    /// `ArchaicTooth` — combat dispatch is a no-op by design. The
    /// effect (`setup_for_player` + `after_obtained` starter-card
    /// transform) is pure run-state.
    #[test]
    fn archaic_tooth_combat_dispatch_is_noop() {
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::ArchaicTooth.to_python_name(), "ARCHAIC_TOOTH");
    }

    /// `Astrolabe` — combat dispatch is a no-op by design. Future
    /// body-fill transforms+upgrades [`ASTROLABE_CARDS`] deck cards
    /// at obtain.
    #[test]
    fn astrolabe_combat_dispatch_is_noop() {
        assert_eq!(ASTROLABE_CARDS, 3);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::Astrolabe.to_python_name(), "ASTROLABE");
    }

    /// `EmptyCage` — combat dispatch is a no-op by design. Future
    /// body-fill removes [`EMPTY_CAGE_CARDS`] deck cards at obtain.
    #[test]
    fn empty_cage_combat_dispatch_is_noop() {
        assert_eq!(EMPTY_CAGE_CARDS, 2);
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::EmptyCage.to_python_name(), "EMPTY_CAGE");
    }

    /// `Pomander` — combat dispatch is a no-op by design. Future
    /// body-fill upgrades 1 deck card at obtain (count hard-coded to
    /// 1 in Python; no separate const).
    #[test]
    fn pomander_combat_dispatch_is_noop() {
        crate::relics::dispatch::ensure_installed();
        assert_eq!(RelicId::Pomander.to_python_name(), "POMANDER");
    }

    /// `PreciseScissors` — combat dispatch is a no-op by design. Future
    /// body-fill removes 1 deck card at obtain (count hard-coded to
    /// 1 in Python; no separate const).
    #[test]
    fn precise_scissors_combat_dispatch_is_noop() {
        crate::relics::dispatch::ensure_installed();
        assert_eq!(
            RelicId::PreciseScissors.to_python_name(),
            "PRECISE_SCISSORS"
        );
    }

    // -----------------------------------------------------------------
    // RunicCapacitor listener tests — bead `sts2-rs-h450h`
    // -----------------------------------------------------------------

    #[test]
    fn listener_runic_capacitor_adds_three_slots_on_round_one() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::RunicCapacitor);
        state.round = 1;
        // Start from the default 3 Defect slots so the assertion
        // catches the +3 delta, not just `>= 3`.
        state.player.orbs = sts2_core::orbs::OrbSlots::with_default_capacity();
        let before = state.player.orbs.capacity();
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(
            state.player.orbs.capacity(),
            before + usize::try_from(RUNIC_CAPACITOR_ORB_SLOTS).unwrap(),
            "round-1 player adds {RUNIC_CAPACITOR_ORB_SLOTS} slots"
        );
    }

    #[test]
    fn listener_runic_capacitor_is_noop_after_round_one() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        for round in [2u16, 3, 5, 10] {
            let mut state = CombatState::default();
            equip_relic(&mut state, RelicKind::RunicCapacitor);
            state.round = round;
            state.player.orbs = sts2_core::orbs::OrbSlots::with_default_capacity();
            let before = state.player.orbs.capacity();
            let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
            assert_eq!(
                state.player.orbs.capacity(),
                before,
                "round {round} must not fire"
            );
        }
    }

    #[test]
    fn listener_runic_capacitor_is_noop_on_enemy_side() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::RunicCapacitor);
        state.round = 1;
        state.player.orbs = sts2_core::orbs::OrbSlots::with_default_capacity();
        let before = state.player.orbs.capacity();
        let _ = dispatch_after_side_turn_start(&mut state, Side::Enemy);
        assert_eq!(state.player.orbs.capacity(), before);
    }

    #[test]
    fn listener_runic_capacitor_is_noop_without_ownership() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::PaelsFlesh); // wrong relic
        state.round = 1;
        state.player.orbs = sts2_core::orbs::OrbSlots::with_default_capacity();
        let before = state.player.orbs.capacity();
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.player.orbs.capacity(), before);
    }

    // -----------------------------------------------------------------
    // DiamondDiadem listener tests — bead `sts2-rs-70anj.3.21`.
    // Player turn-end: if <=2 cards played, apply +1 DiamondDiadem power
    // to the player. The power-side dispatch arms (damage 0.5x + auto-
    // remove on enemy turn end) live in hooks.rs.
    // -----------------------------------------------------------------

    fn player_has_diamond_diadem_power(state: &CombatState) -> bool {
        state
            .powers
            .iter()
            .any(|p| p.kind == sts2_core::PowerKind::DiamondDiadem)
    }

    #[test]
    fn listener_diamond_diadem_applies_power_when_two_cards_played() {
        use sts2_core::relic_effects::dispatch_before_turn_end;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::DiamondDiadem);
        // 2 cards played (total) is <= threshold — power must apply.
        state.player.cards_played_this_turn = [2, 0, 0, 0, 0];
        assert!(!player_has_diamond_diadem_power(&state));
        let _ = dispatch_before_turn_end(&mut state, Side::Player);
        assert!(player_has_diamond_diadem_power(&state));
    }

    #[test]
    fn listener_diamond_diadem_applies_power_when_zero_cards_played() {
        use sts2_core::relic_effects::dispatch_before_turn_end;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::DiamondDiadem);
        // 0 cards played also satisfies the <=2 condition.
        state.player.cards_played_this_turn = [0; 5];
        let _ = dispatch_before_turn_end(&mut state, Side::Player);
        assert!(player_has_diamond_diadem_power(&state));
    }

    #[test]
    fn listener_diamond_diadem_skips_when_three_cards_played() {
        use sts2_core::relic_effects::dispatch_before_turn_end;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::DiamondDiadem);
        // 3 total > threshold — no application.
        state.player.cards_played_this_turn = [1, 1, 1, 0, 0];
        let _ = dispatch_before_turn_end(&mut state, Side::Player);
        assert!(!player_has_diamond_diadem_power(&state));
    }

    #[test]
    fn listener_diamond_diadem_skips_on_enemy_side() {
        use sts2_core::relic_effects::dispatch_before_turn_end;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::DiamondDiadem);
        state.player.cards_played_this_turn = [0; 5];
        let _ = dispatch_before_turn_end(&mut state, Side::Enemy);
        assert!(!player_has_diamond_diadem_power(&state));
    }

    #[test]
    fn listener_diamond_diadem_skips_without_relic() {
        use sts2_core::relic_effects::dispatch_before_turn_end;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::Sai); // wrong relic
        state.player.cards_played_this_turn = [0; 5];
        let _ = dispatch_before_turn_end(&mut state, Side::Player);
        assert!(!player_has_diamond_diadem_power(&state));
    }

    // -----------------------------------------------------------------
    // Sozu / should_procure_potion channel tests — bead `sts2-rs-70anj.3.2`.
    // -----------------------------------------------------------------

    #[test]
    fn procure_potion_pushes_reward_without_sozu() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        assert!(rs.procure_potion(PotionReward::empty()));
        assert!(matches!(rs.pending_rewards.last(), Some(Reward::Potion(_))));
    }

    #[test]
    fn procure_potion_vetoed_by_sozu() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::Sozu);
        let before_len = rs.pending_rewards.len();
        assert!(!rs.procure_potion(PotionReward::empty()));
        assert_eq!(rs.pending_rewards.len(), before_len);
    }

    #[test]
    fn sozu_trips_the_surface_veto_probe() {
        // GH #270: the shop action surface consults
        // `would_veto_procure_potion` before surfacing the potion row —
        // verify the probe fires for the REAL registered Sozu listener
        // (the sts2-core surface tests use a stand-in).
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        assert!(!sts2_core::run_effects::would_veto_procure_potion(&rs));
        rs.relics.push(RelicId::Sozu);
        assert!(sts2_core::run_effects::would_veto_procure_potion(&rs));
    }

    #[test]
    fn procure_potion_no_veto_without_sozu_relic_in_inventory() {
        // Sozu's listener is process-globally registered (via
        // ensure_installed); the veto fires only when the relic is in
        // rs.relics. Without it, the channel must not block.
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // Some unrelated relic in inventory.
        rs.relics.push(RelicId::PaelsFlesh);
        assert!(rs.procure_potion(PotionReward::empty()));
    }

    // -----------------------------------------------------------------
    // DollysMirror / duplicate_card_from_deck tests — bead `sts2-rs-70anj.3.10`.
    // -----------------------------------------------------------------

    fn make_card(id: sts2_core::cards::CardId) -> sts2_core::cards::Card {
        let mut c = sts2_core::Card::pod_defaults();
        c.card_id = id;
        c
    }

    #[test]
    fn duplicate_card_from_deck_appends_clone() {
        let mut rs = RunState::new();
        rs.master_deck
            .push(make_card(sts2_core::cards::CardId::StrikeIronclad));
        rs.master_deck
            .push(make_card(sts2_core::cards::CardId::DefendIronclad));
        let before_len = rs.master_deck.len();
        assert!(rs.duplicate_card_from_deck());
        assert_eq!(rs.master_deck.len(), before_len + 1);
    }

    #[test]
    fn duplicate_card_from_deck_returns_false_on_empty_deck() {
        let mut rs = RunState::new();
        rs.master_deck.clear();
        assert!(!rs.duplicate_card_from_deck());
        assert!(rs.master_deck.is_empty());
    }

    #[test]
    fn listener_dollys_mirror_grows_deck_when_relic_obtained() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck
            .push(make_card(sts2_core::cards::CardId::StrikeIronclad));
        rs.relics.push(RelicId::DollysMirror);
        let before_len = rs.master_deck.len();
        rs.last_obtained_relic = Some(RelicId::DollysMirror);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len + 1);
    }

    #[test]
    fn listener_dollys_mirror_noop_without_relic() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck
            .push(make_card(sts2_core::cards::CardId::StrikeIronclad));
        // No DollysMirror in inventory.
        let before_len = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    // -----------------------------------------------------------------
    // Pool helpers + ArcaneScroll tests — bead `sts2-rs-70anj.3.15`.
    // -----------------------------------------------------------------

    #[test]
    fn random_card_from_rarity_returns_rare_card() {
        use crate::cards::spec_index::random_card_from_rarity;
        use crate::cards::CardRarity;
        let mut rs = RunState::new();
        let pick = random_card_from_rarity(&mut rs, None, CardRarity::Rare);
        assert!(pick.is_some(), "pool must contain at least one Rare card");
    }

    #[test]
    fn random_power_card_returns_power_typed() {
        use crate::cards::spec_index::random_power_card;
        let mut rs = RunState::new();
        let pick = random_power_card(&mut rs, None);
        assert!(pick.is_some(), "pool must contain at least one Power card");
    }

    #[test]
    fn random_curse_card_returns_curse_typed() {
        use crate::cards::spec_index::random_curse_card;
        let mut rs = RunState::new();
        let pick = random_curse_card(&mut rs);
        assert!(
            pick.is_some(),
            "STATUS_SPECS must contain at least one Curse"
        );
    }

    #[test]
    fn random_card_from_rarity_per_character_ironclad_only() {
        use crate::cards::spec_index::random_card_from_rarity;
        use crate::cards::CardRarity;
        use sts2_core::potions::CharacterId;
        let mut rs = RunState::new();
        for _ in 0..8 {
            assert!(random_card_from_rarity(
                &mut rs,
                Some(CharacterId::Ironclad),
                CardRarity::Rare
            )
            .is_some());
        }
    }

    #[test]
    fn listener_arcane_scroll_adds_card_to_deck() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::ArcaneScroll);
        let before_len = rs.master_deck.len();
        rs.last_obtained_relic = Some(RelicId::ArcaneScroll);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len + 1);
    }

    #[test]
    fn listener_arcane_scroll_noop_without_relic() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_len = rs.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    #[test]
    fn listener_dusty_tome_adds_upgraded_ancient_card_to_deck() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::DustyTome);
        let before_len = rs.master_deck.len();
        rs.last_obtained_relic = Some(RelicId::DustyTome);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len + 1);
        // The added card must be upgraded.
        let added = rs.master_deck.last().expect("card added");
        assert!(added.upgraded, "DustyTome must add an upgraded card");
    }

    #[test]
    fn listener_sere_talon_adds_two_curse_cards_to_deck() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::SereTalon);
        let before_len = rs.master_deck.len();
        rs.last_obtained_relic = Some(RelicId::SereTalon);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.master_deck.len(), before_len + 2);
    }

    /// Bead `sts2-rs-70anj.3.5`: Driftwood — `allow_card_reward_reroll`
    /// returns `true` when owned, `false` (empty-bus default) otherwise.
    #[test]
    fn listener_driftwood_allows_card_reward_reroll_when_owned() {
        use sts2_core::run_effects::dispatch_allow_card_reward_reroll;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        assert!(
            !dispatch_allow_card_reward_reroll(&mut rs),
            "without Driftwood the channel must reject the reroll"
        );
        rs.relics.push(RelicId::Driftwood);
        assert!(
            dispatch_allow_card_reward_reroll(&mut rs),
            "with Driftwood the channel must allow the reroll"
        );
    }

    /// Bead `sts2-rs-70anj.3.7`: ChemicalX adds `+INCREASE = 2` to every
    /// X-cost play's effective `x` value when owned. With no relic owner
    /// the dispatcher returns the base x unchanged.
    #[test]
    fn listener_chemical_x_adds_two_to_x_when_owned() {
        use std::num::NonZeroU32;
        use sts2_core::relic_effects::dispatch_modify_x_value;
        use sts2_core::{Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        // No ChemicalX owned → fold returns base.
        assert_eq!(dispatch_modify_x_value(&s, 3), 3);
        s.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::ChemicalX,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        assert_eq!(
            dispatch_modify_x_value(&s, 3),
            3 + CHEMICAL_X_INCREASE,
            "ChemicalX must add INCREASE to running x"
        );
    }

    /// Bead `sts2-rs-70anj.3.3`: ThrowingAxe returns `+1` on the first
    /// card played each combat, then `0` thereafter. The latch lives on
    /// `Relic.counter`; a fresh combat (`before_combat_start` reset)
    /// re-arms.
    #[test]
    fn listener_throwing_axe_fires_once_per_combat() {
        use std::num::NonZeroU32;
        use sts2_core::hooks::{CardPlayCountCtx, CreatureRef};
        use sts2_core::relic_effects::{
            dispatch_before_combat_start, dispatch_relic_modify_card_play_count,
        };
        use sts2_core::{Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        let ctx = CardPlayCountCtx {
            owner: CreatureRef::Player,
            ..CardPlayCountCtx::default()
        };
        // No ThrowingAxe owned → 0 delta.
        assert_eq!(dispatch_relic_modify_card_play_count(&mut s, &ctx), 0);
        s.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::ThrowingAxe,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        // First play: +1; latch trips.
        assert_eq!(dispatch_relic_modify_card_play_count(&mut s, &ctx), 1);
        assert_eq!(s.relics[0].counter, 1, "latch must be set");
        // Subsequent plays: 0.
        assert_eq!(dispatch_relic_modify_card_play_count(&mut s, &ctx), 0);
        assert_eq!(dispatch_relic_modify_card_play_count(&mut s, &ctx), 0);
        // before_combat_start clears the latch.
        let _ = dispatch_before_combat_start(&mut s);
        assert_eq!(s.relics[0].counter, 0, "reset must clear latch");
        assert_eq!(
            dispatch_relic_modify_card_play_count(&mut s, &ctx),
            1,
            "next combat must re-arm"
        );
    }

    /// Bead `sts2-rs-70anj.3.3`: ThrowingAxe gates on `ctx.owner ==
    /// Player`. An enemy-side play (if such a play count dispatch ever
    /// fires for an enemy) must not consume the latch.
    #[test]
    fn listener_throwing_axe_player_only() {
        use std::num::NonZeroU32;
        use sts2_core::hooks::{CardPlayCountCtx, CreatureRef};
        use sts2_core::relic_effects::dispatch_relic_modify_card_play_count;
        use sts2_core::{CreatureUid, Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        s.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::ThrowingAxe,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let enemy_ctx = CardPlayCountCtx {
            owner: CreatureRef::Enemy(CreatureUid(NonZeroU32::new(2).expect("non-zero"))),
            ..CardPlayCountCtx::default()
        };
        assert_eq!(dispatch_relic_modify_card_play_count(&mut s, &enemy_ctx), 0);
        assert_eq!(s.relics[0].counter, 0, "enemy play must not trip latch");
    }

    /// Bead `sts2-rs-70anj.3.12`: DelicateFrond's `before_combat_start`
    /// fills empty potion slots up to `MAX_POTIONS` with random non-event,
    /// character-compatible potions. Asserts the fill brings the belt up
    /// to the cap; the exact picks are RNG-dependent so we only check
    /// counts.
    #[test]
    fn listener_delicate_frond_fills_potion_belt() {
        use std::num::NonZeroU32;
        use sts2_core::potions::MAX_POTIONS;
        use sts2_core::relic_effects::dispatch_before_combat_start;
        use sts2_core::{Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        s.player.character = Some(sts2_core::potions::CharacterId::Ironclad);
        s.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::DelicateFrond,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        assert!(s.player.potions.is_empty(), "fresh state has empty belt");
        let _ = dispatch_before_combat_start(&mut s);
        assert_eq!(
            s.player.potions.len(),
            MAX_POTIONS,
            "DelicateFrond fills the belt to MAX_POTIONS"
        );
    }

    /// DelicateFrond's `before_combat_start` listener is ownership-gated:
    /// when the player does NOT carry it, the belt stays empty (no phantom
    /// potions). Regression for the workbench autoplay showing 5 bogus
    /// drinkable potions on a run that owns no potion-filling relic.
    #[test]
    fn listener_delicate_frond_noop_when_not_owned() {
        use sts2_core::relic_effects::dispatch_before_combat_start;

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        s.player.character = Some(sts2_core::potions::CharacterId::Ironclad);
        // No relics owned.
        assert!(s.player.potions.is_empty(), "fresh state has empty belt");
        let _ = dispatch_before_combat_start(&mut s);
        assert!(
            s.player.potions.is_empty(),
            "belt must stay empty when DelicateFrond is not owned",
        );
    }

    /// Bead `sts2-rs-70anj.3.12`: DelicateFrond no-ops when the belt is
    /// already full — `procure_random_potion` short-circuits on cap.
    #[test]
    fn listener_delicate_frond_noop_when_full() {
        use std::num::NonZeroU32;
        use sts2_core::potions::{PotionKind, MAX_POTIONS};
        use sts2_core::relic_effects::dispatch_before_combat_start;
        use sts2_core::{Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        s.player.character = Some(sts2_core::potions::CharacterId::Ironclad);
        for _ in 0..MAX_POTIONS {
            s.player.potions.push(PotionKind::BlockPotion);
        }
        s.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::DelicateFrond,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let belt_before = s.player.potions.clone();
        let _ = dispatch_before_combat_start(&mut s);
        assert_eq!(s.player.potions, belt_before, "full belt must be untouched");
    }

    /// Bead `sts2-rs-70anj.3.9`: Astrolabe transforms ASTROLABE_CARDS (=3)
    /// random non-curse deck cards and upgrades them. Asserts the deck
    /// length is preserved (in-place replacement) and that at least the
    /// chosen replacements come out upgraded.
    #[test]
    fn listener_astrolabe_transforms_and_upgrades_three_cards() {
        use sts2_core::cards::CardId;
        use sts2_core::run_effects::dispatch_after_obtained;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new().with_rewards_seed(0xa57_201ab);
        // Seed the master deck with 10 unupgraded Strikes (rarity
        // `Basic`, NOT a curse/status — eligible for transform).
        for _ in 0..10 {
            let mut c = sts2_core::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        rs.relics.push(RelicId::Astrolabe);
        rs.last_obtained_relic = Some(RelicId::Astrolabe);
        let _ = dispatch_after_obtained(&mut rs);
        rs.last_obtained_relic = None;
        assert_eq!(
            rs.master_deck.len(),
            10,
            "deck length preserved by in-place transform"
        );
        let upgraded_count = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert!(
            upgraded_count >= 1,
            "astrolabe upgrades at least one replacement (got {upgraded_count})"
        );
        let non_strike = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id != CardId::StrikeIronclad)
            .count();
        assert!(
            non_strike >= 1,
            "astrolabe transforms at least one Strike (got {non_strike} non-Strike)"
        );
    }

    /// Bead `sts2-rs-70anj.3.9`: Astrolabe must NOT fire when an
    /// unrelated relic is obtained after pickup — `last_obtained_relic`
    /// gates the listener to its own pickup event.
    #[test]
    fn listener_astrolabe_is_noop_on_other_relic_pickup() {
        use sts2_core::cards::CardId;
        use sts2_core::run_effects::dispatch_after_obtained;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new().with_rewards_seed(0xa57_201ab);
        for _ in 0..10 {
            let mut c = sts2_core::Card::pod_defaults();
            c.card_id = CardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        rs.relics.push(RelicId::Astrolabe);
        // Some OTHER relic was just obtained.
        rs.last_obtained_relic = Some(RelicId::Strawberry);
        let before: Vec<_> = rs
            .master_deck
            .iter()
            .map(|c| (c.card_id, c.upgraded))
            .collect();
        let _ = dispatch_after_obtained(&mut rs);
        let after: Vec<_> = rs
            .master_deck
            .iter()
            .map(|c| (c.card_id, c.upgraded))
            .collect();
        assert_eq!(
            before, after,
            "astrolabe must not fire on unrelated relic pickup"
        );
    }

    /// Bead `sts2-rs-70anj.3.17`: StoneHumidifier gains MAX_HP=5 each
    /// time `RunState::rest_site_heal` actually heals (>0).
    #[test]
    fn listener_stone_humidifier_gains_max_hp_on_rest_heal() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 40;
        rs.relics.push(RelicId::StoneHumidifier);
        let before_max = rs.max_hp;
        let healed = rs.rest_site_heal(10);
        assert!(healed > 0, "rest-site heal must apply >0");
        assert_eq!(
            rs.max_hp,
            before_max + STONE_HUMIDIFIER_MAX_HP,
            "StoneHumidifier adds {STONE_HUMIDIFIER_MAX_HP} max HP on heal",
        );
    }

    /// Bead `sts2-rs-RP21`: StoneHumidifier grants +5 MaxHp on EVERY rest-site
    /// heal hook, even when the player is already at max HP (no HP restored).
    /// C# `Hook.AfterRestSiteHeal` carries no heal amount and fires
    /// unconditionally, so a full-HP campfire rest still bumps MaxHp. (Inverts
    /// the prior `noop_when_no_heal` assertion, which encoded the Python-pin
    /// `healed<=0` guard that drifted from the C# decomp.)
    #[test]
    fn listener_stone_humidifier_grants_max_hp_even_at_full_hp() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 80;
        rs.relics.push(RelicId::StoneHumidifier);
        let before_max = rs.max_hp;
        let healed = rs.rest_site_heal(10);
        assert_eq!(healed, 0, "no rest heal lands when already at max HP");
        assert_eq!(
            rs.max_hp,
            before_max + STONE_HUMIDIFIER_MAX_HP,
            "MaxHp still bumped at full HP (amount-agnostic C# hook)"
        );
    }

    /// Bead `sts2-rs-70anj.3.17`: ownership gate — `rest_site_heal`
    /// fires the channel but with StoneHumidifier absent the listener
    /// does nothing.
    #[test]
    fn listener_stone_humidifier_noop_without_ownership() {
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 40;
        let before_max = rs.max_hp;
        let healed = rs.rest_site_heal(10);
        assert!(healed > 0);
        assert_eq!(rs.max_hp, before_max, "no listener fires without relic");
    }

    // ------------------------------------------------------------------
    // Toolbox / ChoicesParadox (bead `sts2-rs-70anj.3.13.tb` / `.cp`)
    // ------------------------------------------------------------------

    /// Toolbox opens a 3-option AddGeneratedToHand choice on round 1
    /// and trips its latch.
    #[test]
    fn listener_toolbox_opens_3_choice_round_1() {
        use std::num::NonZeroU32;
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        use sts2_core::{ChoiceKind, CombatState, Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        // Alive player -- a dead one trips the C# IsOverOrEnding select guard.
        s.player.current_hp = 80;
        s.round = 1;
        s.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::Toolbox,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = dispatch_before_hand_draw(&mut s);
        let pc = s
            .pending_choice
            .as_ref()
            .expect("Toolbox must open a choice");
        assert_eq!(pc.kind, ChoiceKind::AddGeneratedToHand);
        assert_eq!(pc.options.len(), TOOLBOX_CARDS);
        assert_eq!(s.relics[0].counter, 1, "latch trips");
    }

    /// Toolbox is a no-op on round != 1.
    #[test]
    fn listener_toolbox_noop_post_round_1() {
        use std::num::NonZeroU32;
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        use sts2_core::{CombatState, Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        s.round = 2;
        s.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::Toolbox,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = dispatch_before_hand_draw(&mut s);
        assert!(s.pending_choice.is_none());
        assert_eq!(s.relics[0].counter, 0);
    }

    /// Toolbox fires exactly once per combat; latch resets at
    /// before_combat_start.
    #[test]
    fn listener_toolbox_latch_resets_per_combat() {
        use std::num::NonZeroU32;
        use sts2_core::relic_effects::{dispatch_before_combat_start, dispatch_before_hand_draw};
        use sts2_core::{CombatState, Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        // Alive player -- a dead one trips the C# IsOverOrEnding select guard.
        s.player.current_hp = 80;
        s.round = 1;
        s.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::Toolbox,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = dispatch_before_hand_draw(&mut s);
        assert!(s.pending_choice.is_some());
        s.pending_choice = None;
        let _ = dispatch_before_hand_draw(&mut s);
        assert!(s.pending_choice.is_none(), "latched; no re-fire");

        let _ = dispatch_before_combat_start(&mut s);
        assert_eq!(s.relics[0].counter, 0, "reset must clear latch");
        let _ = dispatch_before_hand_draw(&mut s);
        assert!(s.pending_choice.is_some(), "re-arms after reset");
    }

    /// Resolving the Toolbox choice adds the chosen card to hand.
    #[test]
    fn resolve_toolbox_choice_adds_card_to_hand() {
        use smallvec::smallvec;
        use std::num::NonZeroU32;
        use sts2_core::relic_effects::dispatch_before_hand_draw;
        use sts2_core::{apply, Action, CombatState, Relic, RelicKind};

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        // Alive player -- a dead one trips the C# IsOverOrEnding select guard.
        s.player.current_hp = 80;
        s.round = 1;
        s.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::Toolbox,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let hand_before = s.player.hand.len();
        let _ = dispatch_before_hand_draw(&mut s);
        assert!(s.pending_choice.is_some());
        let _ = apply(
            &mut s,
            Action::ResolveChoice {
                selection: smallvec![0u8],
            },
        );
        assert!(s.pending_choice.is_none(), "choice cleared");
        assert_eq!(
            s.player.hand.len(),
            hand_before + 1,
            "chosen card lands in hand",
        );
    }

    /// ChoicesParadox opens a 5-option AddGeneratedRetainedToHand
    /// choice on round 1 player-turn-start.
    #[test]
    fn listener_choices_paradox_opens_5_retained_choice() {
        use std::num::NonZeroU32;
        use sts2_core::potions::CharacterId;
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        use sts2_core::{ChoiceKind, CombatState, Relic, RelicKind, Side};

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        // Alive player -- a dead one trips the C# IsOverOrEnding select guard.
        s.player.current_hp = 80;
        s.round = 1;
        s.player.character = Some(CharacterId::Ironclad);
        s.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::ChoicesParadox,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = dispatch_after_side_turn_start(&mut s, Side::Player);
        let pc = s
            .pending_choice
            .as_ref()
            .expect("ChoicesParadox must open a choice");
        assert_eq!(pc.kind, ChoiceKind::AddGeneratedRetainedToHand);
        assert_eq!(pc.options.len(), CHOICES_PARADOX_CARDS);
        assert_eq!(s.relics[0].counter, 1, "latch trips");
    }

    // ------------------------------------------------------------------
    // MeatCleaver / MiniatureTent (bead `sts2-rs-ekf4j` + `.4`)
    // ------------------------------------------------------------------

    /// MeatCleaver appends a Cook option to the rest-site list when
    /// owned, with `enabled` reflecting removable-card count.
    #[test]
    fn listener_meat_cleaver_appends_cook_option() {
        use sts2_core::runstate::{RestSiteOption, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::MeatCleaver);
        // Any three removable cards clear Cook's `>= 2` bar.
        let mut c1 = sts2_core::Card::pod_defaults();
        c1.card_id = sts2_core::cards::CardId::Anger;
        let mut c2 = sts2_core::Card::pod_defaults();
        c2.card_id = sts2_core::cards::CardId::Bash;
        let mut c3 = sts2_core::Card::pod_defaults();
        c3.card_id = sts2_core::cards::CardId::Cinder;
        rs.master_deck.extend([c1, c2, c3]);
        let opts = rs.rest_site_options();
        assert!(opts.iter().any(|o| matches!(o, RestSiteOption::Rest)));
        assert!(opts
            .iter()
            .any(|o| matches!(o, RestSiteOption::Cook { enabled: true })));
    }

    /// C# `CookRestSiteOption.IsEnabled => GetRemovableCardCount(Owner) >= 2`
    /// counting `c.IsRemovable` (`CookRestSiteOption.cs:36,64`), so a
    /// starting deck of basic Strikes and Defends ENABLES Cook. Until
    /// 2026-08-26 `removable_deck_card_count` filtered on the legacy
    /// basics-excluding predicate, so this deck counted 0 and Cook was
    /// disabled on the exact deck the relic is picked up with.
    #[test]
    fn cook_is_enabled_on_a_deck_of_basics_and_disabled_by_eternal_curses() {
        use sts2_core::runstate::{RestSiteOption, RunState};
        crate::relics::dispatch::ensure_installed();
        // Cook counts through the CARD registry, not the relic one — without
        // this the removable predicate is absent and every card counts, which
        // would pass the basics half of this test for the wrong reason.
        crate::cards::dispatch::install_default_registry();
        let cook_enabled = |rs: &RunState| {
            rs.rest_site_options()
                .iter()
                .any(|o| matches!(o, RestSiteOption::Cook { enabled: true }))
        };
        let card = |id| sts2_core::Card {
            card_id: id,
            ..sts2_core::Card::pod_defaults()
        };

        let mut rs = RunState::new();
        rs.relics.push(RelicId::MeatCleaver);
        rs.master_deck.extend([
            card(sts2_core::cards::CardId::StrikeIronclad),
            card(sts2_core::cards::CardId::DefendIronclad),
        ]);
        assert!(cook_enabled(&rs), "two basics are two removable cards");

        // Eternal curses do NOT count toward the bar.
        let mut rs = RunState::new();
        rs.relics.push(RelicId::MeatCleaver);
        rs.master_deck.extend([
            card(sts2_core::cards::CardId::Greed),
            card(sts2_core::cards::CardId::AscendersBane),
            card(sts2_core::cards::CardId::StrikeIronclad),
        ]);
        assert!(
            !cook_enabled(&rs),
            "only the Strike is removable, so the count is 1"
        );
    }

    /// MeatCleaver appends Cook with `enabled = false` when the deck
    /// has fewer than COOK_OPTION_CARD_COUNT removable cards.
    #[test]
    fn listener_meat_cleaver_cook_disabled_when_deck_too_small() {
        use sts2_core::runstate::{RestSiteOption, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::MeatCleaver);
        let mut c1 = sts2_core::Card::pod_defaults();
        c1.card_id = sts2_core::cards::CardId::Anger;
        rs.master_deck.push(c1);
        let opts = rs.rest_site_options();
        assert!(opts
            .iter()
            .any(|o| matches!(o, RestSiteOption::Cook { enabled: false })));
    }

    /// MeatCleaver does NOT duplicate Cook if some other source has
    /// already pushed one.
    #[test]
    fn listener_meat_cleaver_does_not_duplicate_cook() {
        use sts2_core::runstate::RestSiteOption;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::MeatCleaver);
        let mut options = vec![RestSiteOption::Rest, RestSiteOption::Cook { enabled: true }];
        let _ = sts2_core::run_effects::dispatch_modify_rest_site_options(&rs, &mut options);
        let cook_count = options
            .iter()
            .filter(|o| matches!(o, RestSiteOption::Cook { .. }))
            .count();
        assert_eq!(cook_count, 1, "Cook must not duplicate");
    }

    /// MiniatureTent votes `false` on the disable-remaining channel.
    #[test]
    fn listener_miniature_tent_keeps_remaining_options_enabled() {
        use sts2_core::runstate::{RestSiteOption, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::MiniatureTent);
        let disable = rs.should_disable_remaining_rest_site_options(RestSiteOption::Rest);
        assert!(
            !disable,
            "MiniatureTent must keep remaining options enabled"
        );
    }

    /// Default (no MiniatureTent) is `true` — picking one rest-site
    /// option disables the rest.
    #[test]
    fn rest_site_disable_default_when_no_listener_or_opinion() {
        use sts2_core::runstate::{RestSiteOption, RunState};
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        assert!(rs.should_disable_remaining_rest_site_options(RestSiteOption::Rest));
    }

    /// Resolving a ChoicesParadox choice tags the new card for retention.
    #[test]
    fn resolve_choices_paradox_choice_retains_card() {
        use smallvec::smallvec;
        use std::num::NonZeroU32;
        use sts2_core::potions::CharacterId;
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        use sts2_core::{apply, Action, CombatState, Relic, RelicKind, Side};

        crate::relics::dispatch::ensure_installed();
        let mut s = CombatState::default();
        // Alive player -- a dead one trips the C# IsOverOrEnding select guard.
        s.player.current_hp = 80;
        s.round = 1;
        s.player.character = Some(CharacterId::Ironclad);
        s.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::ChoicesParadox,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let _ = dispatch_after_side_turn_start(&mut s, Side::Player);
        let hand_before = s.player.hand.len();
        let _ = apply(
            &mut s,
            Action::ResolveChoice {
                selection: smallvec![0u8],
            },
        );
        assert_eq!(s.player.hand.len(), hand_before + 1);
        let added_uid = *s.player.hand.last().expect("card was added");
        assert!(
            s.player.retain_this_turn.contains(&added_uid),
            "added card must be tagged for retain this turn",
        );
    }

    // -----------------------------------------------------------------
    // GhostSeed listener tests — bead `sts2-rs-sei89`
    // -----------------------------------------------------------------

    #[cfg(test)]
    fn ghost_seed_push_card(
        state: &mut CombatState,
        pile: GhostSeedPile,
        card_id: sts2_core::cards::CardId,
    ) -> sts2_core::CardUid {
        use std::num::NonZeroU32;
        use sts2_core::cards::{Card, CardTags, CardType, TargetType};
        let next_raw = state
            .cards
            .iter()
            .map(|c| c.uid.0.get())
            .max()
            .map_or(1, |m| m.saturating_add(1));
        let uid = sts2_core::CardUid(NonZeroU32::new(next_raw).expect("non-zero"));
        state.cards.push(Card {
            uid,
            card_id,
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
        match pile {
            GhostSeedPile::Draw => state.player.draw.push(uid),
            GhostSeedPile::Hand => state.player.hand.push(uid),
            GhostSeedPile::Discard => state.player.discard.push(uid),
            GhostSeedPile::Exhaust => state.player.exhaust.push(uid),
        }
        uid
    }

    #[cfg(test)]
    #[derive(Copy, Clone)]
    enum GhostSeedPile {
        Draw,
        Hand,
        Discard,
        Exhaust,
    }

    #[cfg(test)]
    fn card_has_ethereal_this_combat(state: &CombatState, uid: sts2_core::CardUid) -> bool {
        let idx = uid.0.get() as usize - 1;
        state.cards.get(idx).is_some_and(|c| {
            c.combat_vars
                .iter()
                .any(|(k, v)| *k == VarKey::EtherealThisCombat && *v == 1)
        })
    }

    /// **GhostSeed #1** — happy path: every basic Strike/Defend across
    /// every pile gets the `EtherealThisCombat` stamp.
    #[test]
    fn listener_ghost_seed_stamps_basic_strike_defend_across_all_piles() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::GhostSeed);
        let in_draw = ghost_seed_push_card(&mut state, GhostSeedPile::Draw, CardId::StrikeIronclad);
        let in_hand = ghost_seed_push_card(&mut state, GhostSeedPile::Hand, CardId::DefendIronclad);
        let in_disc =
            ghost_seed_push_card(&mut state, GhostSeedPile::Discard, CardId::StrikeSilent);
        let in_exh = ghost_seed_push_card(&mut state, GhostSeedPile::Exhaust, CardId::DefendDefect);

        let _ = dispatch_before_combat_start(&mut state);

        assert!(card_has_ethereal_this_combat(&state, in_draw));
        assert!(card_has_ethereal_this_combat(&state, in_hand));
        assert!(card_has_ethereal_this_combat(&state, in_disc));
        assert!(card_has_ethereal_this_combat(&state, in_exh));
    }

    /// **GhostSeed #2** — non-Strike/Defend cards are left alone.
    #[test]
    fn listener_ghost_seed_skips_non_strike_defend_cards() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::GhostSeed);
        // Bash and a generic non-basic card; neither is a basic
        // Strike/Defend so neither should be stamped.
        let bash = ghost_seed_push_card(&mut state, GhostSeedPile::Hand, CardId::Bash);

        let _ = dispatch_before_combat_start(&mut state);

        assert!(!card_has_ethereal_this_combat(&state, bash));
    }

    /// **GhostSeed #3** — derived-name cards like `SetupStrikeCard`
    /// (Python `CardRarity != BASIC`) are NOT stamped, even though
    /// their name contains "STRIKE".
    #[test]
    fn listener_ghost_seed_skips_setup_strike_card() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::GhostSeed);
        let setup = ghost_seed_push_card(&mut state, GhostSeedPile::Hand, CardId::SetupStrikeCard);

        let _ = dispatch_before_combat_start(&mut state);

        assert!(
            !card_has_ethereal_this_combat(&state, setup),
            "SetupStrikeCard is not basic — must be skipped"
        );
    }

    /// **GhostSeed #4** — no-op without relic ownership.
    #[test]
    fn listener_ghost_seed_no_stamp_without_ownership() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_before_combat_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        // Set up player creature without equipping GhostSeed.
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        let strike = ghost_seed_push_card(&mut state, GhostSeedPile::Hand, CardId::StrikeIronclad);

        let _ = dispatch_before_combat_start(&mut state);

        assert!(!card_has_ethereal_this_combat(&state, strike));
    }

    // -----------------------------------------------------------------
    // LordsParasol listener tests — bead `sts2-rs-apy0a`
    // -----------------------------------------------------------------

    /// Helper: minimal inventory with a single available card slot
    /// — verifies the listener clones the card onto `master_deck`,
    /// flips the slot to sold-out, and leaves gold untouched. Entries
    /// are hand-built rather than rolled to keep the test independent
    /// of the inventory-generation RNG plumbing.
    fn lords_parasol_test_card_entry() -> sts2_core::shop::ShopCardEntry {
        use sts2_core::card_rewards::RewardRarity;
        use sts2_core::cards::CardId as InnerCardId;
        sts2_core::shop::ShopCardEntry {
            rarity: RewardRarity::Common,
            card_type: sts2_core::cards::CardType::Attack,
            card: Some(sts2_core::Card {
                card_id: InnerCardId::StrikeIronclad,
                ..sts2_core::Card::pod_defaults()
            }),
            is_colorless: false,
            on_sale: false,
            price: 50,
        }
    }

    #[test]
    fn listener_lords_parasol_clones_available_cards_for_free() {
        use sts2_core::run_effects::dispatch_after_shop_inventory_generated;
        use sts2_core::runstate::RunState;
        use sts2_core::shop::{ShopInventory, SHOP_ENTRY_SOLD_OUT_PRICE};
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::LordsParasol);
        let gold_before = rs.gold;
        let deck_size_before = rs.master_deck.len();

        let mut inv = ShopInventory::empty();
        inv.cards.push(lords_parasol_test_card_entry());

        let result = dispatch_after_shop_inventory_generated(&mut rs, &mut inv);
        assert_eq!(result, EffectResult::Resolved);
        assert_eq!(
            rs.master_deck.len(),
            deck_size_before + 1,
            "available card must be cloned onto master_deck"
        );
        assert_eq!(rs.gold, gold_before, "auto-purchase must not deduct gold");
        assert_eq!(
            inv.cards[0].price, SHOP_ENTRY_SOLD_OUT_PRICE,
            "consumed slot must flip to sold-out"
        );
    }

    /// No-op when LordsParasol is not in `rs.relics`. Mirrors the
    /// `listener_lees_waffle_is_noop_without_ownership` pattern.
    #[test]
    fn listener_lords_parasol_is_noop_without_ownership() {
        use sts2_core::run_effects::dispatch_after_shop_inventory_generated;
        use sts2_core::runstate::RunState;
        use sts2_core::shop::ShopInventory;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        let deck_size_before = rs.master_deck.len();
        let mut inv = ShopInventory::empty();
        inv.cards.push(lords_parasol_test_card_entry());
        let original_price = inv.cards[0].price;

        let _ = dispatch_after_shop_inventory_generated(&mut rs, &mut inv);
        assert_eq!(
            rs.master_deck.len(),
            deck_size_before,
            "deck must be untouched without LordsParasol ownership"
        );
        assert_eq!(
            inv.cards[0].price, original_price,
            "inventory price must be untouched without ownership"
        );
    }

    // -----------------------------------------------------------------
    // PandorasBox listener tests — bead `sts2-rs-70anj.4` (PandorasBox)
    // -----------------------------------------------------------------

    /// Seeds an Ironclad master_deck with 5 basic Strikes + 4 basic
    /// Defends + 1 non-basic (uses CardId::Anger as a sentinel
    /// non-basic Ironclad card). Returns the original deck length.
    fn pandoras_box_seed_ironclad_deck(rs: &mut sts2_core::runstate::RunState) -> usize {
        use sts2_core::cards::CardId as InnerCardId;
        rs.master_deck.clear();
        for _ in 0..5 {
            let mut c = sts2_core::Card::pod_defaults();
            c.card_id = InnerCardId::StrikeIronclad;
            rs.master_deck.push(c);
        }
        for _ in 0..4 {
            let mut c = sts2_core::Card::pod_defaults();
            c.card_id = InnerCardId::DefendIronclad;
            rs.master_deck.push(c);
        }
        let mut anchor = sts2_core::Card::pod_defaults();
        anchor.card_id = InnerCardId::Anger;
        rs.master_deck.push(anchor);
        rs.master_deck.len()
    }

    /// Pickup with ownership replaces every basic Strike + Defend in
    /// the master deck with a non-basic character-pool card; the deck
    /// length is unchanged and the non-basic anchor card survives
    /// (shifted to the head, since the C# deck transform removes each
    /// basic and appends its replacement to the tail).
    #[test]
    fn listener_pandoras_box_transforms_all_basic_strikes_and_defends() {
        use sts2_core::cards::CardId as InnerCardId;
        use sts2_core::potions::CharacterId;
        use sts2_core::run_effects::dispatch_after_obtained;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.character_id = CharacterId::Ironclad;
        rs.relics.push(RelicId::PandorasBox);
        let deck_len = pandoras_box_seed_ironclad_deck(&mut rs);

        rs.last_obtained_relic = Some(RelicId::PandorasBox);
        let _ = dispatch_after_obtained(&mut rs);

        assert_eq!(
            rs.master_deck.len(),
            deck_len,
            "transform_specific_cards removes+appends — deck length preserved"
        );
        // Every basic Strike + Defend slot should be replaced; the
        // anchor non-basic (Anger at the tail) survives.
        let basics_remaining = rs
            .master_deck
            .iter()
            .filter(|c| {
                c.card_id == InnerCardId::StrikeIronclad || c.card_id == InnerCardId::DefendIronclad
            })
            .count();
        assert_eq!(
            basics_remaining, 0,
            "every basic Strike + Defend must be transformed"
        );
        // All 9 basics (indices 0..8) are removed and their replacements
        // appended, so the non-basic anchor (Anger, originally at the tail)
        // shifts to the head; the 9 replacements follow it.
        assert_eq!(
            rs.master_deck[0].card_id,
            InnerCardId::Anger,
            "non-basic anchor shifts to the head after all basics are removed",
        );
    }

    /// No-op when PandorasBox is not in `rs.relics`.
    #[test]
    fn listener_pandoras_box_is_noop_without_ownership() {
        use sts2_core::cards::CardId as InnerCardId;
        use sts2_core::potions::CharacterId;
        use sts2_core::run_effects::dispatch_after_obtained;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.character_id = CharacterId::Ironclad;
        let deck_len = pandoras_box_seed_ironclad_deck(&mut rs);

        let _ = dispatch_after_obtained(&mut rs);

        assert_eq!(rs.master_deck.len(), deck_len);
        let basics_remaining = rs
            .master_deck
            .iter()
            .filter(|c| {
                c.card_id == InnerCardId::StrikeIronclad || c.card_id == InnerCardId::DefendIronclad
            })
            .count();
        assert_eq!(
            basics_remaining, 9,
            "deck must be untouched without ownership"
        );
    }

    // REMOVED with the sixth `CharacterId` variant (2026-08-08):
    // `listener_pandoras_box_is_noop_for_unmapped_character` pinned the no-op
    // path taken when `basic_strike_defend_cards` returns `None`. Its only
    // unmapped character was one STS2 does not have (Necrobinder and Regent
    // have since been mapped), so that function is now total: every arm returns
    // `Some`. Its `Option` return and this listener's `None` branch are both
    // dead; collapsing them is a separate change.

    // -----------------------------------------------------------------
    // SilverCrucible listener tests — bead `sts2-rs-535i7`
    // -----------------------------------------------------------------

    /// Pack/unpack roundtrip pins the encoding contract: the same i32
    /// stores both counters independently.
    #[test]
    fn silver_crucible_pack_unpack_roundtrip() {
        let packed = super::silver_crucible_pack(2, 5);
        assert_eq!(super::silver_crucible_times_used(packed), 2);
        assert_eq!(super::silver_crucible_treasure_rooms_entered(packed), 5);
        // Edge: max bounds.
        let packed = super::silver_crucible_pack(0xFF, 0xFF);
        assert_eq!(super::silver_crucible_times_used(packed), 0xFF);
        assert_eq!(super::silver_crucible_treasure_rooms_entered(packed), 0xFF);
        // Edge: negative clamps to 0.
        let packed = super::silver_crucible_pack(-1, -42);
        assert_eq!(super::silver_crucible_times_used(packed), 0);
        assert_eq!(super::silver_crucible_treasure_rooms_entered(packed), 0);
    }

    /// `after_room_entered` increments the treasure-rooms counter
    /// on `RoomType::Treasure` and ignores other room types.
    #[test]
    fn silver_crucible_after_room_entered_increments_only_on_treasure() {
        use sts2_core::run_effects::dispatch_after_room_entered;
        use sts2_core::runstate::{RoomType, RunState};
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::SilverCrucible);

        let _ = dispatch_after_room_entered(&mut rs, RoomType::Monster);
        let packed = rs
            .relic_counters
            .get(&RelicId::SilverCrucible)
            .copied()
            .unwrap_or(0);
        assert_eq!(
            super::silver_crucible_treasure_rooms_entered(packed),
            0,
            "Monster room must not bump the treasure counter"
        );

        let _ = dispatch_after_room_entered(&mut rs, RoomType::Treasure);
        let packed = rs.relic_counters[&RelicId::SilverCrucible];
        assert_eq!(super::silver_crucible_treasure_rooms_entered(packed), 1);

        let _ = dispatch_after_room_entered(&mut rs, RoomType::Treasure);
        let packed = rs.relic_counters[&RelicId::SilverCrucible];
        assert_eq!(super::silver_crucible_treasure_rooms_entered(packed), 2);
    }

    /// `should_generate_treasure` vetoes the first treasure
    /// (counter == 1 after `after_room_entered`) and allows
    /// subsequent treasures (counter >= 2).
    #[test]
    fn silver_crucible_should_generate_treasure_skips_first_only() {
        use sts2_core::run_effects::{
            dispatch_after_room_entered, dispatch_should_generate_treasure,
        };
        use sts2_core::runstate::{RoomType, RunState};
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::SilverCrucible);

        // Simulate the room-driver flow: after_room_entered fires first,
        // then should_generate_treasure is consulted.
        let _ = dispatch_after_room_entered(&mut rs, RoomType::Treasure);
        assert!(
            !dispatch_should_generate_treasure(&mut rs),
            "first treasure must be skipped (counter == 1)"
        );

        let _ = dispatch_after_room_entered(&mut rs, RoomType::Treasure);
        assert!(
            dispatch_should_generate_treasure(&mut rs),
            "second treasure must NOT be skipped (counter == 2)"
        );
    }

    /// `modify_card_reward_options_late` pre-upgrades up to 3 reward
    /// generations, then stops. Counter ticks up by 1 per dispatch.
    #[test]
    fn silver_crucible_pre_upgrades_first_three_card_rewards_then_stops() {
        use sts2_core::cards::CardId as InnerCardId;
        use sts2_core::run_effects::dispatch_modify_card_reward_options_late;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::SilverCrucible);

        fn fresh_card() -> sts2_core::Card {
            let mut c = sts2_core::Card::pod_defaults();
            c.card_id = InnerCardId::StrikeIronclad;
            c.upgraded = false;
            c
        }

        for i in 1..=3 {
            let mut cards = vec![fresh_card(), fresh_card(), fresh_card()];
            let _ = dispatch_modify_card_reward_options_late(&mut rs, &mut cards);
            for card in &cards {
                assert!(card.upgraded, "reward #{i} must pre-upgrade every option");
            }
            let packed = rs.relic_counters[&RelicId::SilverCrucible];
            assert_eq!(
                super::silver_crucible_times_used(packed),
                i,
                "_times_used must tick by 1 per dispatch"
            );
        }

        // 4th dispatch: no more pre-upgrades.
        let mut cards = vec![fresh_card()];
        let _ = dispatch_modify_card_reward_options_late(&mut rs, &mut cards);
        assert!(
            !cards[0].upgraded,
            "4th reward must NOT pre-upgrade (saturated at CARD_REWARDS=3)"
        );
        let packed = rs.relic_counters[&RelicId::SilverCrucible];
        assert_eq!(
            super::silver_crucible_times_used(packed),
            3,
            "_times_used stays clamped at CARD_REWARDS once saturated"
        );
    }

    /// Ownership gate: all three listeners no-op when SilverCrucible
    /// isn't in `rs.relics`. Compact pin since each branch has the
    /// same `if !contains { return NotImplemented }` head — one fail
    /// reading from each channel is enough.
    #[test]
    fn silver_crucible_listeners_noop_without_ownership() {
        use sts2_core::cards::CardId as InnerCardId;
        use sts2_core::run_effects::{
            dispatch_after_room_entered, dispatch_modify_card_reward_options_late,
        };
        use sts2_core::runstate::{RoomType, RunState};
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();

        let _ = dispatch_after_room_entered(&mut rs, RoomType::Treasure);
        assert!(
            !rs.relic_counters.contains_key(&RelicId::SilverCrucible),
            "after_room_entered must not allocate a counter slot without ownership"
        );

        let mut cards = {
            let mut c = sts2_core::Card::pod_defaults();
            c.card_id = InnerCardId::StrikeIronclad;
            vec![c]
        };
        let _ = dispatch_modify_card_reward_options_late(&mut rs, &mut cards);
        assert!(
            !cards[0].upgraded,
            "modify_card_reward_options_late must not upgrade without ownership"
        );
    }

    // -----------------------------------------------------------------
    // GoldenCompass listener tests — bead `sts2-rs-k2nuu`
    // -----------------------------------------------------------------

    #[test]
    fn golden_compass_after_obtained_captures_current_act_index() {
        use sts2_core::run_effects::dispatch_after_obtained;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::GoldenCompass);
        rs.act_index = 2;
        rs.last_obtained_relic = Some(RelicId::GoldenCompass);
        let _ = dispatch_after_obtained(&mut rs);
        let captured = rs
            .relic_counters
            .get(&RelicId::GoldenCompass)
            .copied()
            .unwrap_or(0);
        assert_eq!(captured, 2, "after_obtained must capture rs.act_index");
    }

    #[test]
    fn golden_compass_after_obtained_is_noop_without_ownership() {
        use sts2_core::run_effects::dispatch_after_obtained;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.act_index = 2;
        let _ = dispatch_after_obtained(&mut rs);
        assert!(
            !rs.relic_counters.contains_key(&RelicId::GoldenCompass),
            "no counter slot allocated without ownership"
        );
    }

    /// `modify_generated_map` overwrites the act_map with the
    /// golden-path layout on the captured act. The point-count
    /// matches `generate_golden_path_map(1)` exactly.
    #[test]
    fn golden_compass_modify_generated_map_swaps_on_captured_act() {
        use sts2_core::map_generator::generate_golden_path_map;
        use sts2_core::run_effects::dispatch_modify_generated_map;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::GoldenCompass);
        rs.relic_counters.insert(RelicId::GoldenCompass, 1);

        let mut act_map = sts2_core::map::ActMap::new(0);
        let _ = dispatch_modify_generated_map(&mut rs, &mut act_map, 1);

        let expected = generate_golden_path_map(1);
        assert_eq!(
            act_map.points.len(),
            expected.points.len(),
            "post-listener map must match golden-path point count"
        );
    }

    /// `modify_generated_map` is a no-op when the dispatched
    /// act_index doesn't match the captured act.
    #[test]
    fn golden_compass_modify_generated_map_skips_other_acts() {
        use sts2_core::run_effects::dispatch_modify_generated_map;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::GoldenCompass);
        rs.relic_counters.insert(RelicId::GoldenCompass, 1);

        let mut act_map = sts2_core::map::ActMap::new(42);
        let original_len = act_map.points.len();
        let _ = dispatch_modify_generated_map(&mut rs, &mut act_map, 2);
        assert_eq!(
            act_map.points.len(),
            original_len,
            "non-captured-act dispatch must leave act_map untouched"
        );
    }

    /// `modify_unknown_map_point_room_types` narrows the candidate
    /// set to {Event} on the captured act.
    #[test]
    fn golden_compass_modify_unknown_map_point_room_types_narrows_to_event() {
        use sts2_core::run_effects::dispatch_modify_unknown_map_point_room_types;
        use sts2_core::runstate::{RoomType, RunState};
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::GoldenCompass);
        rs.act_index = 1;
        rs.relic_counters.insert(RelicId::GoldenCompass, 1);

        let mut room_types: std::collections::BTreeSet<RoomType> = [
            RoomType::Monster,
            RoomType::Shop,
            RoomType::RestSite,
            RoomType::Event,
            RoomType::Treasure,
        ]
        .into_iter()
        .collect();
        let _ = dispatch_modify_unknown_map_point_room_types(&mut rs, &mut room_types);
        assert_eq!(room_types.len(), 1);
        assert!(room_types.contains(&RoomType::Event));
    }

    /// On a non-captured act, the candidate set is untouched.
    #[test]
    fn golden_compass_modify_unknown_map_point_room_types_skips_other_acts() {
        use sts2_core::run_effects::dispatch_modify_unknown_map_point_room_types;
        use sts2_core::runstate::{RoomType, RunState};
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::GoldenCompass);
        rs.act_index = 2;
        rs.relic_counters.insert(RelicId::GoldenCompass, 1);

        let original: std::collections::BTreeSet<RoomType> =
            [RoomType::Monster, RoomType::Shop].into_iter().collect();
        let mut room_types = original.clone();
        let _ = dispatch_modify_unknown_map_point_room_types(&mut rs, &mut room_types);
        assert_eq!(
            room_types, original,
            "non-captured-act dispatch must leave candidates untouched"
        );
    }

    // ── cargo-mutants coverage (issue #65) ────────────────────────────
    // Kills surviving mutants in the shop_event listeners + enchant
    // predicates that the behavioural pins above did not discriminate.

    /// `PaelsLegion::after_side_turn_start` must NOT decrement an
    /// already-armed (counter == 0) cooldown. Kills `> 0` → `>= 0` at
    /// the decrement guard: with `>= 0` a re-armed counter would drift
    /// to -1 (a phantom "pending" state).
    #[test]
    fn paels_legion_turn_start_leaves_armed_counter_at_zero() {
        use std::num::NonZeroU32;
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();

        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        state.relics.push(sts2_core::Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::PaelsLegion,
            counter: 0, // already armed
            is_wax: false,
            is_melted: false,
        });
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(
            paels_legion_counter(&state),
            0,
            "armed (0) counter must stay 0 — not decrement to -1"
        );
    }

    // PaelsEye: bit 0 = used-this-combat, bit 1 = any-cards-played.
    // before_turn_end exhausts the hand only when BOTH bits are clear.

    #[cfg(test)]
    fn paels_eye_counter(state: &CombatState) -> i32 {
        state
            .relics
            .iter()
            .find(|r| !r.is_melted && r.kind == RelicKind::PaelsEye)
            .expect("PaelsEye present")
            .counter
    }

    #[cfg(test)]
    fn paels_eye_event_ctx() -> sts2_core::relic_effects::CardEventCtx {
        sts2_core::relic_effects::CardEventCtx {
            card_id: sts2_core::cards::CardId::DefendIronclad,
            card_uid: None,
            target: None,
            card_tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
        }
    }

    /// `PaelsEye::after_card_played` sets bit 1 (any-cards-played).
    /// Kills `|= 0b10` → `&= 0b10` and the ownership `== / !=` guards.
    #[test]
    fn paels_eye_card_played_sets_any_cards_bit() {
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::PaelsEye);
        assert_eq!(paels_eye_counter(&state), 0, "starts clear");
        let _ = dispatch_after_card_played(&mut state, &paels_eye_event_ctx());
        assert_eq!(
            paels_eye_counter(&state) & 0b10,
            0b10,
            "after a card play, the any-cards bit must be SET"
        );
        // Pre-set bit 0 (used) survives the OR — proves it's `|=` not `&=`.
        state
            .relics
            .iter_mut()
            .find(|r| !r.is_melted && r.kind == RelicKind::PaelsEye)
            .unwrap()
            .counter = 0b01;
        let _ = dispatch_after_card_played(&mut state, &paels_eye_event_ctx());
        assert_eq!(
            paels_eye_counter(&state),
            0b11,
            "|= must preserve bit 0 and set bit 1"
        );
    }

    /// `PaelsEye::before_side_turn_start` clears bit 1 on the player
    /// turn. Kills `&= !0b10` → `|= !0b10`, the `delete !`, and the
    /// side/ownership guards.
    #[test]
    fn paels_eye_side_turn_start_clears_any_cards_bit() {
        use sts2_core::relic_effects::dispatch_before_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::PaelsEye);
        // Both bits set: clearing must leave only bit 0 (used).
        state
            .relics
            .iter_mut()
            .find(|r| !r.is_melted && r.kind == RelicKind::PaelsEye)
            .unwrap()
            .counter = 0b11;
        // Enemy side: no clear.
        let _ = dispatch_before_side_turn_start(&mut state, Side::Enemy);
        assert_eq!(
            paels_eye_counter(&state),
            0b11,
            "enemy side-turn-start must not clear the bit"
        );
        // Player side: clears bit 1 only.
        let _ = dispatch_before_side_turn_start(&mut state, Side::Player);
        assert_eq!(
            paels_eye_counter(&state),
            0b01,
            "player side-turn-start clears bit 1, preserves bit 0"
        );
    }

    /// `PaelsEye::before_turn_end` exhausts the hand iff both bits are
    /// clear (`trailing_zeros() >= 2`). Kills the `delete !`, the `>=`
    /// boundary, the side guard, and the `&&`/`==` guards.
    #[test]
    fn paels_eye_before_turn_end_exhausts_only_when_both_bits_clear() {
        use sts2_core::cards::{Card, CardId, CardType};
        use sts2_core::relic_effects::dispatch_before_turn_end;
        crate::relics::dispatch::ensure_installed();

        let stage_hand = |state: &mut CombatState| {
            let c = Card {
                card_id: CardId::StrikeIronclad,
                card_type: CardType::Attack,
                ..Card::pod_defaults()
            };
            let uid = state.add_card_to_draw_pile(c);
            state.player.draw.retain(|u| *u != uid);
            state.player.hand.push(uid);
        };

        // Case A: counter == 0 (both clear) → exhaust the whole hand.
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::PaelsEye);
        stage_hand(&mut state);
        assert_eq!(state.player.hand.len(), 1);
        let _ = dispatch_before_turn_end(&mut state, Side::Player);
        assert!(
            state.player.hand.is_empty(),
            "both bits clear → hand exhausted"
        );

        // Case B: any-cards bit set (counter == 0b10) → no exhaust.
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::PaelsEye);
        state
            .relics
            .iter_mut()
            .find(|r| !r.is_melted && r.kind == RelicKind::PaelsEye)
            .unwrap()
            .counter = 0b10;
        stage_hand(&mut state);
        let _ = dispatch_before_turn_end(&mut state, Side::Player);
        assert_eq!(
            state.player.hand.len(),
            1,
            "any-cards bit set → hand NOT exhausted"
        );

        // Case C: both clear but ENEMY side → no exhaust (side guard).
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::PaelsEye);
        stage_hand(&mut state);
        let _ = dispatch_before_turn_end(&mut state, Side::Enemy);
        assert_eq!(
            state.player.hand.len(),
            1,
            "enemy turn-end must not exhaust the player hand"
        );
    }

    // MusicBox: bit 0 = used-this-turn. First Attack played clones into
    // hand (ethereal), then the latch blocks further clones until the
    // next player side-turn-start re-arms it.

    #[cfg(test)]
    fn music_box_counter(state: &CombatState) -> i32 {
        state
            .relics
            .iter()
            .find(|r| !r.is_melted && r.kind == RelicKind::MusicBox)
            .expect("MusicBox present")
            .counter
    }

    #[cfg(test)]
    fn music_box_play_card(
        state: &mut CombatState,
        card_type: sts2_core::cards::CardType,
    ) -> sts2_core::relic_effects::CardEventCtx {
        use sts2_core::cards::{Card, CardId};
        let c = Card {
            card_id: CardId::StrikeIronclad,
            card_type,
            ..Card::pod_defaults()
        };
        let uid = state.add_card_to_draw_pile(c);
        sts2_core::relic_effects::CardEventCtx {
            card_id: CardId::StrikeIronclad,
            card_uid: Some(uid),
            target: None,
            card_tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
        }
    }

    /// `MusicBox::after_card_played` clones the first Attack into hand
    /// and latches (bit 0). Kills the `card_type == Attack` guard
    /// (`== / !=`), the latch `&` mask mutations, `|=` → `&=`, the
    /// `delete !` on the consumed guard, and the `&& / ==` in the latch
    /// check.
    #[test]
    fn music_box_clones_first_attack_then_latches() {
        use sts2_core::cards::{CardType, VarKey};
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::MusicBox);
        assert_eq!(music_box_counter(&state), 0, "latch starts clear");

        // First Attack → clone into hand, latch set.
        let ctx = music_box_play_card(&mut state, CardType::Attack);
        let hand_before = state.player.hand.len();
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(
            state.player.hand.len(),
            hand_before + 1,
            "first Attack must clone a card into hand"
        );
        assert_eq!(
            music_box_counter(&state) & 0b01,
            0b01,
            "latch must set bit 0"
        );
        let clone_uid = *state.player.hand.last().unwrap();
        assert_eq!(
            state.cards[clone_uid.0.get() as usize - 1].combat_var(VarKey::EtherealThisCombat),
            1,
            "clone must be flagged EtherealThisCombat"
        );

        // Second Attack → latched, no further clone.
        let ctx2 = music_box_play_card(&mut state, CardType::Attack);
        let hand_now = state.player.hand.len();
        let _ = dispatch_after_card_played(&mut state, &ctx2);
        assert_eq!(
            state.player.hand.len(),
            hand_now,
            "latched MusicBox must not clone a second Attack"
        );
    }

    /// `MusicBox::after_card_played` ignores non-Attack cards — kills
    /// the `card_type == Attack` guard flipped to `!=`.
    #[test]
    fn music_box_ignores_non_attack_cards() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::MusicBox);
        let ctx = music_box_play_card(&mut state, CardType::Skill);
        let hand_before = state.player.hand.len();
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(
            state.player.hand.len(),
            hand_before,
            "Skill play must not clone"
        );
        assert_eq!(
            music_box_counter(&state),
            0,
            "non-Attack must not consume the latch"
        );
    }

    /// `MusicBox::before_side_turn_start` re-arms (clears bit 0) on the
    /// player turn. Kills `&= !0b01` → `|= !0b01`, the `delete !`, and
    /// the side/ownership guards.
    #[test]
    fn music_box_side_turn_start_rearms_latch() {
        use sts2_core::relic_effects::dispatch_before_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::MusicBox);
        // Latch set, plus a stray high bit that must survive the clear.
        state
            .relics
            .iter_mut()
            .find(|r| !r.is_melted && r.kind == RelicKind::MusicBox)
            .unwrap()
            .counter = 0b11;
        // Enemy side: no re-arm.
        let _ = dispatch_before_side_turn_start(&mut state, Side::Enemy);
        assert_eq!(
            music_box_counter(&state),
            0b11,
            "enemy side-turn-start must not re-arm"
        );
        // Player side: clears bit 0 only.
        let _ = dispatch_before_side_turn_start(&mut state, Side::Player);
        assert_eq!(
            music_box_counter(&state),
            0b10,
            "player side-turn-start clears bit 0, preserves bit 1"
        );
    }

    /// `Crossbow::after_side_turn_start` generates a free Attack in
    /// hand on the player turn. Kills the ownership `== / !=` guards,
    /// the side `delete !`, and the wrong-relic `==` guard.
    #[test]
    fn crossbow_generates_free_attack_on_player_turn() {
        use sts2_core::cards::VarKey;
        use sts2_core::potions::CharacterId;
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::cards::dispatch::ensure_installed();
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::Crossbow);
        state.player.character = Some(CharacterId::Ironclad);

        let hand_before = state.player.hand.len();
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(
            state.player.hand.len(),
            hand_before + 1,
            "Crossbow must add one free Attack to hand on the player turn"
        );
        let uid = *state.player.hand.last().unwrap();
        assert_eq!(
            state.cards[uid.0.get() as usize - 1].combat_var(VarKey::FreeThisTurn),
            1,
            "generated Attack must be FreeThisTurn"
        );
    }

    /// `Crossbow` is inert on the enemy side and when unowned — kills
    /// the side `delete !` and the ownership guard mutations from the
    /// positive direction.
    #[test]
    fn crossbow_is_noop_on_enemy_side_and_unowned() {
        use sts2_core::potions::CharacterId;
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::cards::dispatch::ensure_installed();
        crate::relics::dispatch::ensure_installed();

        // Owned, enemy side → no card.
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::Crossbow);
        state.player.character = Some(CharacterId::Ironclad);
        let before = state.player.hand.len();
        let _ = dispatch_after_side_turn_start(&mut state, Side::Enemy);
        assert_eq!(state.player.hand.len(), before, "enemy side must not fire");

        // Unowned (wrong relic), player side → no card.
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::PaelsFlesh);
        state.player.character = Some(CharacterId::Ironclad);
        let before = state.player.hand.len();
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(
            state.player.hand.len(),
            before,
            "unowned Crossbow must not fire"
        );
    }

    /// `SealOfGold` fires at exactly the cost boundary (gold == 5).
    /// Kills `gold < COST` → `gold <= COST`: with `<=`, gold == 5 would
    /// (wrongly) be treated as insufficient and no energy gained.
    #[test]
    fn seal_of_gold_fires_at_exact_cost_boundary() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        equip_relic(&mut state, RelicKind::SealOfGold);
        state.gold = SEAL_OF_GOLD_COST; // exactly 5
        let before = state.player.energy;
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(
            state.gold, 0,
            "spends exactly the 5-gold cost at the boundary"
        );
        assert_eq!(
            state.player.energy,
            before + i8::try_from(SEAL_OF_GOLD_ENERGY).unwrap(),
            "gains energy at the exact boundary (gold == COST)"
        );
    }

    /// `DingyRug` rarity-gating branch (`allow_rarity_modifications =
    /// false`): a colorless card whose rarity is NOT among the active
    /// character's rarities must be EXCLUDED. Pins the exact gated
    /// slate, killing `&& → ||` and both `delete !` on the
    /// `rarity_gated && !character_rarities.contains(..)` guard.
    #[test]
    fn dingy_rug_rarity_gating_keeps_in_rarity_colorless() {
        use sts2_core::runstate::{CardRewardGenerationOptions, RunState};
        crate::relics::dispatch::ensure_installed();

        // With rarity modifications DISABLED the gate
        // `rarity_gated && !character_rarities.contains(spec.rarity)`
        // skips ONLY colorless cards whose rarity is off the character's
        // rarity set. Every colorless rarity is covered by the Ironclad
        // pool, so NO colorless card is dropped. This pins that the
        // logical structure stays `&&` over the negated membership:
        //  - `&& → ||` would skip ALL colorless (rarity_gated == true).
        //  - dropping the `!` on `contains` would skip ALL colorless too.
        let mut rs = RunState::new();
        rs.character_id = sts2_core::potions::CharacterId::Ironclad;
        rs.relics.push(RelicId::DingyRug);
        let mut opts = CardRewardGenerationOptions {
            allow_rarity_modifications: false,
            ..CardRewardGenerationOptions::default()
        };
        let _ = sts2_core::run_effects::dispatch_modify_card_reward_creation_options(
            &mut rs, &mut opts,
        );
        assert!(
            opts.custom_card_ids
                .contains(&sts2_core::cards::CardId::Discovery),
            "rarity-gating must keep in-rarity colorless cards (Discovery)"
        );
        // The slate is strictly larger than the character-only pool:
        // colorless cards survived the rarity gate (proves it did NOT
        // skip the whole colorless tail).
        let char_only = crate::cards::spec_index::specs_for_character(
            sts2_core::potions::CharacterId::Ironclad,
        )
        .iter()
        .map(|f| f().id)
        .collect::<std::collections::HashSet<_>>()
        .len();
        assert!(
            opts.custom_card_ids.len() > char_only,
            "in-rarity colorless cards survive rarity-gating ({} > {})",
            opts.custom_card_ids.len(),
            char_only,
        );
    }

    /// `LeafyPoultice` ownership/last-obtained guard: the listener must
    /// be inert when a DIFFERENT relic was just obtained. Kills the
    /// `last_obtained_relic == Some(LeafyPoultice)` guard flipped to
    /// `!=` (which would fire on every other relic's obtain).
    #[test]
    fn leafy_poultice_noop_when_other_relic_obtained() {
        use sts2_core::run_effects::dispatch_after_obtained;
        use sts2_core::runstate::RunState;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::LeafyPoultice);
        // A different relic was the one just obtained.
        rs.last_obtained_relic = Some(RelicId::DingyRug);
        let before_max = rs.max_hp;
        let _ = dispatch_after_obtained(&mut rs);
        assert_eq!(
            rs.max_hp, before_max,
            "LeafyPoultice must not lose max HP when a different relic was obtained"
        );
    }

    /// `enchant_eligible_deck_cards` guard `max_count == 0 || deck empty`.
    /// Kills `|| → &&` (with `&&`, a zero max_count over a non-empty
    /// deck would wrongly proceed) and the empty-deck leg.
    #[test]
    fn enchant_eligible_deck_cards_guard_branches() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;

        // max_count == 0, non-empty deck → nothing enchanted.
        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad);
        let applied = enchant_eligible_deck_cards(&mut rs, EnchantmentId::Sharp, 3, 0);
        assert_eq!(applied, 0, "max_count == 0 must enchant nothing");
        assert_eq!(
            rs.master_deck[0].enchantment(EnchantmentId::Sharp),
            0,
            "no enchantment written when max_count == 0"
        );

        // Empty deck, max_count > 0 → nothing enchanted, no panic.
        let mut rs_empty = RunState::new();
        let applied = enchant_eligible_deck_cards(&mut rs_empty, EnchantmentId::Sharp, 3, 3);
        assert_eq!(applied, 0, "empty deck must enchant nothing");
    }

    /// `enchant_eligible_deck_cards` looks up each card's OWN spec via
    /// `s.id == card.card_id`. Kills the `== → !=` in the `find`: a
    /// mismatched-spec lookup makes the Attack-only `Sharp` gate read
    /// the wrong card type and skip the eligible Strike.
    #[test]
    fn enchant_eligible_deck_cards_matches_own_spec() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;

        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad);
        let applied = enchant_eligible_deck_cards(&mut rs, EnchantmentId::Sharp, 3, 3);
        assert_eq!(
            applied, 1,
            "the lone Strike is Sharp-eligible and enchanted"
        );
        assert_eq!(
            rs.master_deck[0].enchantment(EnchantmentId::Sharp),
            3,
            "Strike must receive Sharp(3) via its OWN spec lookup"
        );
    }

    /// `enchantable_candidate_card_ids` collects ids whose OWN spec is
    /// enchant-eligible (`s.id == card.card_id`). Kills the `== → !=`
    /// in the `find`: the mismatched spec would reject the eligible
    /// Strike (Sharp is Attack-only).
    #[test]
    fn enchantable_candidate_card_ids_matches_own_spec() {
        use sts2_core::cards::{CardId, EnchantmentId};
        use sts2_core::runstate::RunState;

        let mut rs = RunState::new();
        push_deck_card(&mut rs, CardId::StrikeIronclad);
        let ids = enchantable_candidate_card_ids(&rs, EnchantmentId::Sharp);
        assert_eq!(
            ids,
            vec![CardId::StrikeIronclad],
            "the Sharp-eligible Strike must be collected via its own spec"
        );
    }
}
