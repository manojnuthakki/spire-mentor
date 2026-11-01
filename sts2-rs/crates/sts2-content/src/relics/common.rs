// Ported from sts2-rl-agent@01c094d:sts2_env/relics/common.py
//! Common-tier relic factories — bead `sts2-rs-evs` (P7-2).
//!
//! Ports the 30 common relics from
//! `sts2-rl-agent@01c094d:sts2_env/relics/common.py`. Each factory
//! returns a [`RelicSpec`] (static template). On-trigger behaviour is
//! exposed through the [`crate::relics::dispatch`] registry — handler
//! bodies in this file are intentionally minimal (return
//! [`EffectResult::Resolved`]) because the full hook plumbing
//! (`dispatch_after_combat_start`, `dispatch_after_card_played`,
//! `modify_damage_*`, etc.) lands in the P7-8 integration bead
//! (`sts2-rs-d19`). The registry-side acceptance for this bead is
//! "no longer `NotImplemented` for any of these 30 `RelicId`s".
//!
//! Two early-ported uncommons — `Akabeko` and `BagOfMarbles` — are
//! included here because bead `sts2-rs-evs` is the canonical home for
//! the test-fixtures that exercise them (see module-level test
//! list). The P7-3 uncommon bead (`sts2-rs-l22`) will skip these two
//! and reference this module for the factories.

use sts2_core::card_effects::EffectResult;
use sts2_core::hooks::{self, BlockCtx, BlockGainedCtx, CreatureRef, ValueProps};
use sts2_core::relic_effects::{
    register_after_card_played, register_after_side_turn_start, register_before_combat_start,
    register_owned_after_energy_reset, AfterCardPlayedFn, AfterEnergyResetFn, AfterSideTurnStartFn,
    BeforeCombatStartFn, CardEventCtx,
};
use sts2_core::relics::RelicId;
use sts2_core::run_effects::{
    register_after_obtained, register_after_room_entered, register_modify_rest_site_heal_amount,
    register_modify_rest_site_heal_rewards, register_modify_rewards,
    register_modify_unknown_map_point_room_types, register_on_card_added_to_deck,
    AfterObtainedRunFn, AfterRoomEnteredRunFn, CardAddedRunCtx, ModifyRestSiteHealAmountRunFn,
    ModifyRestSiteHealRewardsRunFn, ModifyRewardsRunFn, ModifyUnknownMapPointRoomTypesRunFn,
    OnCardAddedToDeckRunFn,
};
use sts2_core::runstate::{PotionReward, Reward, RoomType, RunState};
use sts2_core::{CombatState, PowerKind, RelicKind, Side};

use crate::relics::{RelicEffect, RelicRarity, RelicSpec};

// ---------------------------------------------------------------------------
// Per-relic factories — 30 common relics
// ---------------------------------------------------------------------------

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

// Amethyst Aubergine — common. `modify_rewards` appends GoldReward(10,10)
// when a chest reward is opened. Mirrors `common.py:29-40`
// (`GOLD = 10`). Bead `sts2-rs-6ynt.5.3`.
relic_factory!(pub amethyst_aubergine, AmethystAubergine, "AMETHYST_AUBERGINE", Common, RelicEffect::AppendRewardGold { amount: AMETHYST_AUBERGINE_GOLD });
// Anchor — common. `before_combat_start`: gain 10 Block (unpowered).
// Mirrors `common.py:52-53` (`BLOCK = 10`). Bead `sts2-rs-6ynt.5.3`.
relic_factory!(pub anchor, Anchor, "ANCHOR", Common, RelicEffect::GainBlockAtCombatStart { amount: ANCHOR_BLOCK });
// Bag of Preparation — common. Round-1 only: hand draws +2 cards.
// Mirrors `sts2-rl-agent@01c094d:sts2_env/relics/common.py`
// `BagOfPreparation.modify_hand_draw` (`EXTRA_CARDS = 2`, gated by
// `combat.round_number == 1`). Reuses the existing
// `RelicEffect::ExtraHandDrawForRounds` AST variant already wired
// for Ring of the Snake's `modify_hand_draw` fold. The
// `EXTRA_CARDS = 2` constant lives further down (`BAG_OF_PREPARATION_EXTRA_CARDS`).
// Bead `sts2-rs-owus`.
relic_factory!(pub bag_of_preparation, BagOfPreparation, "BAG_OF_PREPARATION", Common, RelicEffect::ExtraHandDrawForRounds { extra_cards: 2, max_round: 1 });
// Blood Vial — common. `after_side_turn_start` (player side only):
// heal 2 HP. Mirrors `common.py:78-80` (`HEAL = 2`). Bead `sts2-rs-6ynt.5.3`.
relic_factory!(pub blood_vial, BloodVial, "BLOOD_VIAL", Common, RelicEffect::HealAtPlayerSideTurnStart { amount: BLOOD_VIAL_HEAL });
// Bone Flute — common (Necrobinder pool). Out of A–H scope for bead
// `sts2-rs-6ynt.5.3` (pool ∉ {IRONCLAD, SHARED, EVENT}); kept as
// `Noop` until the sibling Necrobinder-pool wave lands.
relic_factory!(pub bone_flute, BoneFlute, "BONE_FLUTE", Common, RelicEffect::Noop);
// Bronze Scales — common. `before_combat_start`: apply THORNS=3 to
// the player. Mirrors `common.py:105-106`. Bead `sts2-rs-6ynt.5.3`.
relic_factory!(pub bronze_scales, BronzeScales, "BRONZE_SCALES", Common, RelicEffect::ApplyPowerAtCombatStart { kind: PowerKind::Thorns, amount: BRONZE_SCALES_THORNS });
// Centennial Puzzle — common. On the first `after_damage_received`
// per combat, draw 3 cards. Mirrors `common.py:117-127`
// (`CARDS = 3`, `_active` resets in `after_combat_end`). Bead
// `sts2-rs-6ynt.5.3`.
relic_factory!(pub centennial_puzzle, CentennialPuzzle, "CENTENNIAL_PUZZLE", Common, RelicEffect::DrawCardsOnFirstDamagePerCombat { amount: CENTENNIAL_PUZZLE_CARDS });
// Circlet — fallback placeholder. Python pin
// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py:3102-3107`:
// `RelicPool.FALLBACK`; "stackable placeholder when no other relics
// available". No listener methods on the Python class, so the Rust
// surface is `Noop`. Bead `sts2-rs-70anj.5` (Wave-5 INERT placeholder
// completion — Circlet ships as a tribute / fallback artifact, e.g.
// `events/act2.rs:375` uses it as the Luminous Choir tribute relic).
relic_factory!(pub circlet, Circlet, "CIRCLET", Common, RelicEffect::Noop);
// DeprecatedRelic — placeholder for legacy relic ids. Python pin
// `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py:3110-3114`:
// `RelicPool.DEPRECATED`; "stackable placeholder for deprecated
// relics". No listener methods. Bead `sts2-rs-70anj.5` — paired with
// Circlet so the engine can round-trip a deprecated relic id through
// `RelicKind::DeprecatedRelic` without a missing-factory panic when a
// legacy save file is replayed.
relic_factory!(pub deprecated_relic, DeprecatedRelic, "DEPRECATED_RELIC", Common, RelicEffect::Noop);
// Data Disk — common (Defect pool). `before_combat_start`: apply
// FOCUS=1 to the player. Mirrors `common.py:139`. Runtime listener
// landed in bead `sts2-rs-bfuy` (`on_before_combat_start_data_disk`);
// declarative spec metadata sync'd here in `sts2-rs-bco9`.
relic_factory!(pub data_disk, DataDisk, "DATA_DISK", Common, RelicEffect::ApplyPowerAtCombatStart { kind: PowerKind::Focus, amount: DATA_DISK_FOCUS });
// Fencing Manual — common (Regent pool). `after_side_turn_start`
// (player side, round 1): forge `FORGE = 10`. Mirrors Python
// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::FencingManual`.
// `RelicEffect::Noop` because the forge primitive (`combat_forge` in
// [`crate::cards::regent`]) requires the same crate as the listener
// body; the listener is wired explicitly in [`install_pool`] via
// `register_after_side_turn_start`. Bead `sts2-rs-6lh46`.
relic_factory!(pub fencing_manual, FencingManual, "FENCING_MANUAL", Common, RelicEffect::Noop);
// Festive Popper — common. Round 1 only: on player
// `after_side_turn_start`, deal 9 unpowered damage to every alive
// enemy. Mirrors `common.py:166-170` (`DAMAGE = 9`,
// `combat.round_number == 1`). Bead `sts2-rs-6ynt.5.3`.
relic_factory!(pub festive_popper, FestivePopper, "FESTIVE_POPPER", Common, RelicEffect::DealDamageToAllEnemiesOnSideTurnStart { amount: FESTIVE_POPPER_DAMAGE, max_round: 1 });
// Gorget — common. `before_combat_start`: apply PLATING=4 to the
// player. Mirrors `common.py:180-181`. Bead `sts2-rs-6ynt.5.3`.
relic_factory!(pub gorget, Gorget, "GORGET", Common, RelicEffect::ApplyPowerAtCombatStart { kind: PowerKind::Plating, amount: GORGET_PLATING });
// Happy Flower — common. Every 3rd player `after_side_turn_start`,
// gain 1 energy. Mirrors `common.py:197-201` (`ENERGY = 1`,
// `TURNS = 3`). Bead `sts2-rs-6ynt.5.3`.
relic_factory!(pub happy_flower, HappyFlower, "HAPPY_FLOWER", Common, RelicEffect::GainEnergyEveryNPlayerSideTurns { energy: HAPPY_FLOWER_ENERGY, period: HAPPY_FLOWER_TURNS as u16 });
// JuzuBracelet — common (shared). Map-side effect only (removes
// MONSTER from unknown-room outcomes). Python
// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::JuzuBracelet`
// has an empty class body (the map's unknown-room resolver inspects
// owned relics directly). No combat-side hook exists, so
// `RelicEffect::Noop` is the faithful body. Bead `sts2-rs-70anj.2`
// (Wave 2): doc-only verification — no behavioral change.
relic_factory!(pub juzu_bracelet, JuzuBracelet, "JUZU_BRACELET", Common, RelicEffect::Noop);
relic_factory!(pub lantern, Lantern, "LANTERN", Common, RelicEffect::Noop);
relic_factory!(pub meal_ticket, MealTicket, "MEAL_TICKET", Common, RelicEffect::Noop);
// Oddly Smooth Stone — common. `before_combat_start`: apply DEXTERITY=1
// to the player. Mirrors `common.py:249`. Runtime listener landed in
// bead `sts2-rs-bfuy` (`on_before_combat_start_oddly_smooth_stone`);
// declarative spec metadata sync'd here in `sts2-rs-bco9`.
relic_factory!(pub oddly_smooth_stone, OddlySmoothStone, "ODDLY_SMOOTH_STONE", Common, RelicEffect::ApplyPowerAtCombatStart { kind: PowerKind::Dexterity, amount: ODDLY_SMOOTH_STONE_DEXTERITY });
relic_factory!(pub pendulum, Pendulum, "PENDULUM", Common, RelicEffect::Noop);
relic_factory!(pub permafrost, Permafrost, "PERMAFROST", Uncommon, RelicEffect::Noop);
// PotionBelt — common (shared). Python
// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::PotionBelt`:
// on `after_obtained`, `owner.gain_potion_slots(2)`. Body-fill
// landed in Wave 2 (`sts2-rs-70anj.2`) once `sts2-rs-dsgf9`
// added `RunState.gain_potion_slots` + `max_potion_slots`. The
// `RelicEffect` spec stays `Noop` because all behavior rides the
// `after_obtained` listener registered in [`install_pool`]; the
// declarative spec only flips when a `GainPotionSlotsOnObtained`
// variant is needed for other listeners. Listener at
// [`listener_potion_belt`].
relic_factory!(pub potion_belt, PotionBelt, "POTION_BELT", Common, RelicEffect::Noop);
relic_factory!(pub red_skull, RedSkull, "RED_SKULL", Common, RelicEffect::Noop);
// RegalPillow — common (shared). Python
// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::RegalPillow`:
// `modify_rest_site_heal_amount(amount) -> amount + 15`. Body-fill
// landed in Wave 2 (`sts2-rs-70anj.2`) once `sts2-rs-2tsrk` added
// the run-side `ModifyRestSiteHealAmountRunFn` channel. Spec stays
// `Noop`; all behavior rides the listener at
// [`listener_regal_pillow`].
relic_factory!(pub regal_pillow, RegalPillow, "REGAL_PILLOW", Common, RelicEffect::Noop);
relic_factory!(pub snecko_skull, SneckoSkull, "SNECKO_SKULL", Common, RelicEffect::Noop);
relic_factory!(pub strawberry, Strawberry, "STRAWBERRY", Common, RelicEffect::GainMaxHpOnObtained { amount: STRAWBERRY_MAX_HP });
relic_factory!(pub strike_dummy, StrikeDummy, "STRIKE_DUMMY", Common, RelicEffect::Noop);
relic_factory!(pub tiny_mailbox, TinyMailbox, "TINY_MAILBOX", Uncommon, RelicEffect::Noop);
// Vajra — common. `before_combat_start`: apply STRENGTH=1 to the
// player. Mirrors `common.py:417`. Runtime listener landed in bead
// `sts2-rs-bfuy` (`on_before_combat_start_vajra`); declarative spec
// metadata sync'd here in `sts2-rs-bco9`.
relic_factory!(pub vajra, Vajra, "VAJRA", Common, RelicEffect::ApplyPowerAtCombatStart { kind: PowerKind::Strength, amount: VAJRA_STRENGTH });
// VenerableTeaSet — common (shared). Python
// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::VenerableTeaSet`:
// `after_room_entered(rest_site)` sets `_gain_energy_next_combat`;
// `after_energy_reset` consumes the flag and grants `ENERGY = 2`.
// Real listeners landed in bead `sts2-rs-g1p2m`:
// - Run-side: [`listener_venerable_tea_set_rest_latch`] sets
//   `rs.relic_counters[VenerableTeaSet] = 1` on rest-site entry.
// - Combat-side: [`listener_venerable_tea_set_consume`] reads the
//   latch from the owned `Relic.counter` slot (which the
//   `enter_combat` bridge mirrors from `rs.relic_counters` when the
//   `RelicId↔RelicKind` mirror lands — `sts2-rs-46ms`), grants
//   `ENERGY = 2`, and resets the counter to 0. Uses
//   `before_combat_start` because there is no `after_energy_reset`
//   channel; both fire pre-first-turn so semantics match.
// At today's pin the combat-side listener is still inert in real
// runs (relic mirror deferred per the enter_combat doc), but it is
// correctly shaped and tested via manually-pushed `state.relics`.
relic_factory!(pub venerable_tea_set, VenerableTeaSet, "VENERABLE_TEA_SET", Common, RelicEffect::Noop);
// WarPaint — common (shared). Python
// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::WarPaint`:
// `after_obtained` calls `owner.upgrade_random_cards(SKILL, 2)`.
// Wave 2 (`sts2-rs-70anj.2`) deferred — `RunState::upgrade_random_cards`
// is a no-op surface stub (see `sts2-rs-jvk3` follow-up); it ignores
// any `CardType` filter and currently returns 0. Wiring the listener
// today would be a no-op against a no-op. Re-attempt when
// `sts2-rs-jvk3` follow-up lands run-side deck + RNG.
relic_factory!(pub war_paint, WarPaint, "WAR_PAINT", Common, RelicEffect::Noop);
// Whetstone — common (shared). Python pin
// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::Whetstone`:
// `after_obtained` calls `owner.upgrade_random_cards(ATTACK, 2,
// rng=run_state.rng.niche)`. Wave-2 `upgrade_random_cards_of_type`
// blocker cleared by `sts2-rs-tb36q`. Real run-side listener wired
// in [`install_pool`] via [`listener_whetstone`]. Bead `sts2-rs-e8p18`.
relic_factory!(pub whetstone, Whetstone, "WHETSTONE", Common, RelicEffect::Noop);
relic_factory!(pub book_of_five_rings, BookOfFiveRings, "BOOK_OF_FIVE_RINGS", Common, RelicEffect::Noop);

// Note: Akabeko (`uncommon.py:VIGOR=8`) and BagOfMarbles
// (`uncommon.py:VULNERABLE=1`) are explicitly called out in the
// bead's "≥3 deep tests" acceptance bar. Their RelicSpec factories
// live in the P7-3 uncommon module; the deep tests below exercise
// them via `crate::relics::uncommon::install_pool()`.

// ---------------------------------------------------------------------------
// Pool membership
// ---------------------------------------------------------------------------

/// Every common-tier `RelicId` ported in this module. Membership/identity
/// data only — live behaviour flows through the typed hook-channel listeners
/// registered by [`install_pool`], not through any dispatch marker.
const COMMON_RELIC_IDS: &[RelicId] = &[
    RelicId::AmethystAubergine,
    RelicId::Anchor,
    RelicId::BagOfPreparation,
    RelicId::BloodVial,
    RelicId::BoneFlute,
    RelicId::BronzeScales,
    RelicId::CentennialPuzzle,
    RelicId::DataDisk,
    RelicId::FencingManual,
    RelicId::FestivePopper,
    RelicId::Gorget,
    RelicId::HappyFlower,
    RelicId::JuzuBracelet,
    RelicId::Lantern,
    RelicId::MealTicket,
    RelicId::OddlySmoothStone,
    RelicId::Pendulum,
    RelicId::Permafrost,
    RelicId::PotionBelt,
    RelicId::RedSkull,
    RelicId::RegalPillow,
    RelicId::SneckoSkull,
    RelicId::Strawberry,
    RelicId::StrikeDummy,
    RelicId::TinyMailbox,
    RelicId::Vajra,
    RelicId::VenerableTeaSet,
    RelicId::WarPaint,
    RelicId::Whetstone,
    RelicId::BookOfFiveRings,
];

/// Every [`RelicId`] in this module's common pool — the droppable
/// relics of this tier. Used to seed the run's relic grab bag so
/// combat/treasure/shop relic rewards are stocked (the run loop's
/// `populate_relic_grab_bag_by_rarity` source). Bead `sts2-rs-64mq2`.
#[must_use]
pub fn pool_relic_ids() -> Vec<RelicId> {
    COMMON_RELIC_IDS.to_vec()
}

/// Register every typed hook-channel listener for this pool. Idempotent.
pub fn install_pool() {
    // Bead `sts2-rs-u8qa`: real `after_obtained` listener for Strawberry's
    // `MAX_HP = 7` (Python `common.py::Strawberry.MAX_HP`). The combat-side
    // `on_trigger_*` handler stays a `resolved_marker`; this is the
    // run-state side that actually mutates the player's max HP.
    register_after_obtained(
        RelicId::Strawberry,
        listener_strawberry as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.2` (Wave 2 body-fill): real `after_obtained`
    // listener for PotionBelt's `POTION_SLOTS = 2` (Python
    // `common.py::PotionBelt.POTION_SLOTS`). Unblocked by
    // `sts2-rs-dsgf9` (`RunState.gain_potion_slots` +
    // `max_potion_slots`). The combat-side `on_trigger_*` handler
    // stays a `resolved_marker`; this is the run-state side that
    // mutates max_potion_slots.
    register_after_obtained(
        RelicId::PotionBelt,
        listener_potion_belt as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-70anj.2` (Wave 2 body-fill): real
    // `modify_rest_site_heal_amount` listener for RegalPillow's
    // `HEAL = 15` (Python `common.py::RegalPillow.HEAL`). Unblocked
    // by `sts2-rs-2tsrk` (run-side scalar channel landed). The
    // combat-side `on_trigger_*` handler stays a `resolved_marker`;
    // this is the run-state side that adds the bonus to the heal
    // scalar via the additive-fold dispatcher.
    register_modify_rest_site_heal_amount(
        RelicId::RegalPillow,
        listener_regal_pillow as ModifyRestSiteHealAmountRunFn,
    );
    // Bead `sts2-rs-5pk6`: real `after_room_entered` listener for
    // MealTicket's `HEAL = 15` on Shop-room entry. The combat-side
    // `on_trigger_*` handler stays a `resolved_marker`; this is the
    // run-state side that actually heals the player.
    register_after_room_entered(
        RelicId::MealTicket,
        listener_meal_ticket as AfterRoomEnteredRunFn,
    );
    // Bead `sts2-rs-g1p2m`: cross-combat energy latch. Run-side
    // listener sets the latch on rest-site entry; the combat-side
    // listener (`listener_venerable_tea_set_consume`) reads + clears
    // it and grants `ENERGY = 2`. The consume MUST fire on
    // `after_energy_reset` (mirroring C# `VenerableTeaSet.AfterEnergyReset`),
    // NOT `before_combat_start`: the turn-1 energy reset in
    // `start_player_turn` runs AFTER `before_combat_start` and overwrites
    // `player.energy` to base, silently eating the +2 (shadow-census
    // seeds 1001/1003: DLL energy 5 vs engine 3). Same bus the
    // FakeVenerableTeaSet twin already uses.
    register_after_room_entered(
        RelicId::VenerableTeaSet,
        listener_venerable_tea_set_rest_latch as AfterRoomEnteredRunFn,
    );
    register_owned_after_energy_reset(
        RelicId::VenerableTeaSet,
        RelicKind::VenerableTeaSet,
        listener_venerable_tea_set_consume as AfterEnergyResetFn,
    );
    // Bead `sts2-rs-lqpw`: real `after_card_played` listener for
    // Permafrost's `BLOCK = 6` on the first Power card played per
    // combat. The combat-side `on_trigger_*` handler stays a
    // `resolved_marker`; the once-per-combat latch lives on
    // `Relic.counter` (3nis pattern, same as FakeHappyFlower /
    // EmberTea / TeaOfDiscourtesy). A companion `before_combat_start`
    // listener resets the counter to 0 so the next combat re-arms.
    register_after_card_played(
        RelicId::Permafrost,
        listener_permafrost as AfterCardPlayedFn,
    );
    register_before_combat_start(
        RelicId::Permafrost,
        listener_permafrost_reset as BeforeCombatStartFn,
    );
    // Bead `sts2-rs-sx27`: real `on_card_added_to_deck` listener for
    // BookOfFiveRings — every `CARDS_THRESHOLD=5` cards added to the
    // master deck, heal `HEAL=15`. The combat-side `on_trigger_*`
    // handler stays a `resolved_marker`; counter persistence lives
    // on `RunState.relic_counters[BookOfFiveRings]` (same pattern as
    // DreamCatcher / WongosMysteryTicket in event_reward.rs).
    register_on_card_added_to_deck(
        RelicId::BookOfFiveRings,
        listener_book_of_five_rings as OnCardAddedToDeckRunFn,
    );
    // Bead `sts2-rs-7z65`: real `modify_rest_site_heal_rewards`
    // listener for TinyMailbox — at every rest site the rewards list
    // gains a `PotionReward(owner.player_id)` entry. Python pin
    // `common.py::TinyMailbox`. Channel-only firing: the
    // rest-site reward generation site (when it lands) is the only
    // place this dispatches from, so no separate `is_rest_site` gate
    // is needed in the listener.
    register_modify_rest_site_heal_rewards(
        RelicId::TinyMailbox,
        listener_tiny_mailbox as ModifyRestSiteHealRewardsRunFn,
    );
    // Bead `sts2-rs-0463`: real `modify_rewards` listener for
    // AmethystAubergine. Python `common.py::AmethystAubergine.modify_rewards`
    // appends `GoldReward(player_id, GOLD, GOLD)` when room.room_type is
    // MONSTER or ELITE. Same shape as PrayerWheel (4ihz) / WhiteStar (b4tb)
    // in rare.rs but appends `Reward::Gold` instead of CardReward and
    // matches two room types instead of one.
    register_modify_rewards(
        RelicId::AmethystAubergine,
        listener_amethyst_aubergine as ModifyRewardsRunFn,
    );
    // Bead `sts2-rs-6lh46`: real `after_side_turn_start` listener for
    // FencingManual — round-1 player-side `gain_forge(10)` via the
    // shared `combat_forge` primitive (Python
    // `common.py::FencingManual`, `FORGE = 10`). The combat-side
    // `on_trigger_*` handler stays a `resolved_marker`; this is the
    // real behavioral wire-up.
    register_after_side_turn_start(
        RelicId::FencingManual,
        listener_fencing_manual as AfterSideTurnStartFn,
    );
    // Bead `sts2-rs-78l88`: real `modify_unknown_map_point_room_types`
    // listener for JuzuBracelet. Removes `RoomType::Monster` from the
    // `?`-room candidate set while the run is "before act-3 treasure
    // chest" — Python's `is_before_act3_treasure_chest(run_state)`
    // returns `total_floor < 41` (single-player). Unblocked by
    // sts2-rs-2c0ze (added `RunState.total_floor`) + sts2-rs-vuxmm
    // (channel). Channel is the same one GoldenCompass uses
    // (sts2-rs-k2nuu); ordering between the two is RelicId-sorted
    // by the registry but observationally either order produces the
    // same fixed-point set since GoldenCompass narrows to {Event}
    // and JuzuBracelet removes Monster.
    register_modify_unknown_map_point_room_types(
        RelicId::JuzuBracelet,
        listener_juzu_bracelet as ModifyUnknownMapPointRoomTypesRunFn,
    );
    // Bead `sts2-rs-e8p18`: real `after_obtained` listener for
    // Whetstone — upgrade `CARDS = 2` random non-upgraded Attacks in
    // the master deck via the niche RNG stream. Python pin
    // `common.py::Whetstone.after_obtained`. The combat-side
    // `on_trigger_*` handler stays a `resolved_marker`; this is the
    // run-state side that mutates the master deck.
    register_after_obtained(RelicId::Whetstone, listener_whetstone as AfterObtainedRunFn);
    // Bead `sts2-rs-8bqdw`: real `after_obtained` listener for
    // WarPaint — upgrade `CARDS = 2` random non-upgraded Skills in
    // the master deck via the niche RNG stream. Python pin
    // `common.py::WarPaint.after_obtained`. Same shape as
    // Whetstone (e8p18) but filters `CardType::Skill` instead of
    // `Attack`. Highest-impact INERT relic per the audit script
    // (1659 fixture freq).
    register_after_obtained(RelicId::WarPaint, listener_war_paint as AfterObtainedRunFn);
}

/// Single-player `is_before_act3_treasure_chest` threshold. Mirrors
/// Python `relics/base.py::RelicInstance.is_before_act3_treasure_chest`:
///
/// ```python
/// threshold = 38 if len(getattr(run_state, "players", [])) > 1 else 41
/// return getattr(run_state, "total_floor", 0) < threshold
/// ```
///
/// The Rust port is single-player at this pin so the 41 branch is
/// the only one used. Bead `sts2-rs-78l88`. Hoisted as a top-level
/// `const` so future relics that hit the same gate (JuzuBracelet
/// here; rare/event-pool relics in `shop_event.rs` per the audit
/// note in `tests/parity/known_divergences.md`) can re-use it
/// without duplicating the magic number.
const IS_BEFORE_ACT3_TREASURE_CHEST_TOTAL_FLOOR_SP: i32 = 41;

/// `JuzuBracelet::modify_unknown_map_point_room_types` — Python pin
/// `relics/common.py::JuzuBracelet`:
///
/// ```python
/// def is_allowed(self, run_state):
///     return self.is_before_act3_treasure_chest(run_state)
///
/// def modify_unknown_map_point_room_types(self, owner, room_types):
///     return {rt for rt in room_types if rt is not RoomType.MONSTER}
/// ```
///
/// Owner-gated via `rs.relics.contains` and run-gated via the
/// `is_before_act3_treasure_chest` (total_floor < 41) check. When
/// both gates pass, drops `RoomType::Monster` from the in-flight
/// candidate set.
fn listener_juzu_bracelet(
    rs: &mut RunState,
    room_types: &mut std::collections::BTreeSet<RoomType>,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::JuzuBracelet) {
        return EffectResult::NotImplemented;
    }
    if rs.total_floor >= IS_BEFORE_ACT3_TREASURE_CHEST_TOTAL_FLOOR_SP {
        return EffectResult::NotImplemented;
    }
    let removed = room_types.remove(&RoomType::Monster);
    if removed {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `FencingManual::after_side_turn_start` — on the player's round-1
/// turn start, forge `FORGE = 10` via the shared
/// [`crate::cards::regent::combat_forge`] primitive. Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::FencingManual`
/// (`after_side_turn_start`, gates: `side == PLAYER`,
/// `combat.round_number == 1`). The forge primitive itself
/// (`combat_forge`) handles the alive / player-side / `amount > 0`
/// guards, spawns a `SovereignBlade` if no active blade exists, buffs
/// every existing blade across hand/draw/discard/exhaust, and fires
/// `after_forge` listeners. Bead `sts2-rs-6lh46`.
fn listener_fencing_manual(state: &mut CombatState, side: Side) -> EffectResult {
    if side != Side::Player {
        return EffectResult::NotImplemented;
    }
    if state.round != 1 {
        return EffectResult::NotImplemented;
    }
    if !state.relics.owns(RelicKind::FencingManual) {
        return EffectResult::NotImplemented;
    }
    let Some(player_uid) = state.player.creature_uid else {
        return EffectResult::NotImplemented;
    };
    let _ = crate::cards::regent::combat_forge(state, player_uid, FENCING_MANUAL_FORGE, None);
    EffectResult::Resolved
}

/// `AmethystAubergine::TryModifyRewards` — append a `Reward::Gold`
/// (amount = `AMETHYST_AUBERGINE_GOLD = 15` per C# v0.103.2
/// `AmethystAubergine.cs:18` `GoldVar(15)`) on EVERY combat room EXCEPT the
/// final-act boss. C# (`AmethystAubergine.cs:35-44`): gate on
/// `room.RoomType.IsCombatRoom()` then exclude `RoomType == Boss &&
/// CurrentActIndex >= Acts.Count - 1`. So Monster, Elite, and act-1/2 bosses all
/// grant +15; only the final-act boss is excluded. Ownership-gated.
///
/// Bead `sts2-rs-0463`; `jakq1` rebased the amount from the Python pin's 10 to
/// the C# 15; `RP18` extended the room gate from Monster|Elite (the Python pin's
/// narrower set) to the C# combat-rooms-except-final-boss set — a boss-room
/// `+15` was missing (IC:1009 act-1 boss @398: C# +15, Rust +0).
fn listener_amethyst_aubergine(
    rs: &mut RunState,
    rewards: &mut Vec<Reward>,
    room_type: RoomType,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::AmethystAubergine) {
        return EffectResult::NotImplemented;
    }
    if !matches!(
        room_type,
        RoomType::Monster | RoomType::Elite | RoomType::Boss
    ) {
        return EffectResult::NotImplemented;
    }
    if room_type == RoomType::Boss && rs.is_last_act() {
        return EffectResult::NotImplemented;
    }
    rewards.push(Reward::Gold {
        amount: AMETHYST_AUBERGINE_GOLD,
    });
    EffectResult::Resolved
}

/// `Strawberry::after_obtained` — `owner.gain_max_hp(7)` (Python
/// `common.py::Strawberry`, `MAX_HP=7`). Bead `sts2-rs-u8qa`.
fn listener_strawberry(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::Strawberry) {
        return EffectResult::NotImplemented;
    }
    rs.gain_max_hp(STRAWBERRY_MAX_HP);
    EffectResult::Resolved
}

/// `RegalPillow::modify_rest_site_heal_amount` — `amount + HEAL = 15`
/// (Python `sts2-rl-agent@01c094d:sts2_env/relics/common.py::RegalPillow`,
/// `HEAL = 15`). The additive-fold dispatcher (`dispatch_modify_rest_
/// site_heal_amount`) sums each listener's delta onto the base heal
/// scalar at rest-site resolve time. Ownership-gated. Bead
/// `sts2-rs-70anj.2`.
fn listener_regal_pillow(rs: &RunState) -> i32 {
    if rs.owns_active_relic(RelicId::RegalPillow) {
        REGAL_PILLOW_HEAL
    } else {
        0
    }
}

/// `PotionBelt::after_obtained` — `owner.gain_potion_slots(2)` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::PotionBelt`,
/// `POTION_SLOTS = 2`). Body-fill landed once
/// [`sts2_core::runstate::RunState::gain_potion_slots`] +
/// [`sts2_core::runstate::RunState::max_potion_slots`] became
/// available via `sts2-rs-dsgf9`. Bead `sts2-rs-70anj.2`.
fn listener_potion_belt(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::PotionBelt) {
        return EffectResult::NotImplemented;
    }
    rs.gain_potion_slots(POTION_BELT_SLOTS);
    EffectResult::Resolved
}

/// `Whetstone::after_obtained` — upgrade `CARDS = 2` random
/// non-upgraded Attacks in the master deck. Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::Whetstone`
/// (`owner.upgrade_random_cards(CardType.ATTACK, 2,
/// rng=run_state.rng.niche)`). Routes through
/// [`sts2_core::runstate::RunState::upgrade_random_cards_of_type`]
/// which uses the niche RNG stream (matches Python). Bead
/// `sts2-rs-e8p18`.
fn listener_whetstone(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::Whetstone) {
        return EffectResult::NotImplemented;
    }
    // C# `Whetstone.AfterObtained` (Whetstone.cs:20) AUTO-upgrades
    // `CARDS = 2` random non-upgraded Attacks immediately —
    // `Deck.Cards.Where(Attack && IsUpgradable).StableShuffle(Rng.Niche)
    // .Take(2)` → `CardCmd.Upgrade`, with NO player choice. The prior
    // `defer_followup_rewards` arm queued a player-pick UpgradeCards
    // reward (Python pin `relics/common.py:532`, the NON-authoritative
    // sim); that diverged from the shipped game and, in the run-parity
    // flow, the queued reward was never surfaced → a silent no-op (the
    // relic upgraded nothing; whole-run seed RP78 Necrobinder, Unrest
    // Site → Whetstone). Always auto-upgrade to match C#. (Distinct from
    // the 5 enchant relics — GnarledHammer/Kifuda/PunchDagger/
    // BeautifulBracelet/RoyalStamp — which C# DOES gate behind a
    // CardSelectCmd player choice, so they keep their deferred arm.)
    // Bead `sts2-rs-whetstone-warpaint-auto`.
    let _ = rs
        .upgrade_random_cards_of_type(sts2_core::cards::CardType::Attack, WHETSTONE_CARDS as usize);
    EffectResult::Resolved
}

/// `Permafrost::after_card_played` — first Power-type card played per
/// combat grants `owner.gain_block(6, unpowered=True)`. Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::Permafrost`
/// (`BLOCK = 6` in that older port). The latch lives on
/// [`sts2_core::Relic::counter`] (3nis pattern, same as
/// FakeHappyFlower / EmberTea / TeaOfDiscourtesy): `0` = armed,
/// `1` = fired. [`listener_permafrost_reset`] clears the counter
/// during the existing next-combat setup. Installed v0.107.1
/// Permafrost.cs resets on CombatRoom entry, including nested event
/// fights; the setup path exposes no decision before this reset.
/// Combat end and noncombat rooms must retain the latch.
fn listener_permafrost(state: &mut CombatState, ctx: &CardEventCtx) -> EffectResult {
    let idx = state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::Permafrost);
    let Some(idx) = idx else {
        return EffectResult::NotImplemented;
    };
    if state.relics[idx].counter != 0 {
        return EffectResult::NotImplemented;
    }
    let Some(card_uid) = ctx.card_uid else {
        return EffectResult::NotImplemented;
    };
    let card_idx = card_uid.0.get() as usize - 1;
    let Some(card) = state.cards.get(card_idx) else {
        return EffectResult::NotImplemented;
    };
    if card.card_type != sts2_core::cards::CardType::Power {
        return EffectResult::NotImplemented;
    }
    state.relics[idx].counter = 1;
    // C# Permafrost.cs (v0.103.2) CanonicalVars = BlockVar(7m,
    // ValueProp.Unpowered); the GainBlock call uses the DynamicVar's
    // embedded prop so Frail/Dexterity skip the grant. Previously
    // routed through gain_block_player which hard-codes
    // ValueProps::MOVE → Frail folded the 7 down to 5. Bead
    // `sts2-rs-hnx1a` (same shape as 38pae / 7iag9 / jpq2s).
    let block_ctx = BlockCtx {
        target: CreatureRef::Player,
        props: ValueProps::MOVE.union(ValueProps::UNPOWERED),
        card_source: None,
    };
    let final_block = hooks::modify_block(state, PERMAFROST_BLOCK, &block_ctx);
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
    EffectResult::Resolved
}

/// Represent Permafrost.cs's CombatRoom-entry reset during the existing
/// combat setup (ordinary and event combats), before its first decision.
/// There is deliberately no combat-end reset: rewards retain the latch.
fn listener_permafrost_reset(state: &mut CombatState) -> EffectResult {
    let Some(idx) = state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::Permafrost)
    else {
        return EffectResult::NotImplemented;
    };
    state.relics[idx].counter = 0;
    EffectResult::Resolved
}

/// `MealTicket::after_room_entered` — `if room_type.is_merchant and
/// owner.current_hp > 0: owner.heal(15)`. Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::MealTicket`
/// (`HEAL = 15`). The alive-gate mirrors Python's `current_hp > 0`
/// guard so we don't revive a dead player. Bead `sts2-rs-5pk6`.
fn listener_meal_ticket(rs: &mut RunState, room_type: RoomType) -> EffectResult {
    if !rs.owns_active_relic(RelicId::MealTicket) {
        return EffectResult::NotImplemented;
    }
    if !matches!(room_type, RoomType::Shop) {
        return EffectResult::NotImplemented;
    }
    if rs.current_hp <= 0 {
        return EffectResult::NotImplemented;
    }
    rs.heal(MEAL_TICKET_HEAL);
    EffectResult::Resolved
}

/// `VenerableTeaSet::after_room_entered` — on rest-site entry, latch
/// `rs.relic_counters[VenerableTeaSet] = 1` so the next combat's
/// `before_combat_start` listener grants `ENERGY = 2`. Mirrors Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::VenerableTeaSet.
/// after_room_entered` (`if room_type.is_rest_site:
/// self._gain_energy_next_combat = True`). Bead `sts2-rs-g1p2m`.
///
/// Stays latched (idempotent) on repeated rest-site entries — Python's
/// boolean flag has the same shape, so the second rest-site re-set is
/// a no-op until a combat consumes the latch.
fn listener_venerable_tea_set_rest_latch(rs: &mut RunState, room_type: RoomType) -> EffectResult {
    if !rs.owns_active_relic(RelicId::VenerableTeaSet) {
        return EffectResult::NotImplemented;
    }
    if !matches!(room_type, RoomType::RestSite) {
        return EffectResult::NotImplemented;
    }
    rs.relic_counters.insert(RelicId::VenerableTeaSet, 1);
    EffectResult::Resolved
}

/// `VenerableTeaSet::after_energy_reset` — if the rest-site latch is
/// set (`Relic.counter == 1`), grant `ENERGY = 2` to the player and
/// clear the counter. Mirrors C# `VenerableTeaSet.AfterEnergyReset`
/// (`GainEnergy(EnergyVar.BaseValue=2)` once, then clears
/// `GainEnergyInNextCombat`) and Python's `after_energy_reset`
/// (`common.py:498-501`).
///
/// MUST fire on `after_energy_reset`, not `before_combat_start`: the
/// turn-1 energy reset in `start_player_turn` runs AFTER
/// `before_combat_start` and overwrites `player.energy` to base, so a
/// grant on the earlier bus is silently eaten (shadow-census seeds
/// 1001/1003: DLL 5 vs engine 3). `after_energy_reset` runs just after
/// the reset, so the +2 sticks.
///
/// The latch storage rides the combat-side `Relic.counter`, seeded from
/// `rs.relic_counters[VenerableTeaSet]` by the general counter mirror in
/// `run_bridge::enter_combat_from_roster` (both the EncounterDef and
/// legacy paths route through it).
fn listener_venerable_tea_set_consume(state: &mut CombatState) -> EffectResult {
    let Some(idx) = state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::VenerableTeaSet)
    else {
        return EffectResult::NotImplemented;
    };
    if state.relics[idx].counter != 1 {
        return EffectResult::NotImplemented;
    }
    state.relics[idx].counter = 0;
    state.gain_energy(VENERABLE_TEA_SET_ENERGY);
    EffectResult::Resolved
}

/// `BookOfFiveRings::on_card_added_to_deck` — increment a per-run
/// counter on every card append; on every `CARDS_THRESHOLD=5`th add,
/// reset the counter and `heal(HEAL=15)` (alive-gated). Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::BookOfFiveRings`.
/// Counter persistence rides on `RunState.relic_counters[BookOfFiveRings]`
/// (same scratchpad pattern as DreamCatcher / WongosMysteryTicket in
/// event_reward.rs). Ownership-gated; the `_ctx` payload is unused
/// because BookOfFiveRings fires on every card add regardless of type
/// or source (Python keys on relic identity, not the card). Bead
/// `sts2-rs-sx27`.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn listener_book_of_five_rings(rs: &mut RunState, _ctx: &CardAddedRunCtx) -> EffectResult {
    if !rs.owns_active_relic(RelicId::BookOfFiveRings) {
        return EffectResult::NotImplemented;
    }
    let counter = rs
        .relic_counters
        .entry(RelicId::BookOfFiveRings)
        .or_insert(0);
    *counter += 1;
    if *counter < BOOK_OF_FIVE_RINGS_CARDS_THRESHOLD {
        return EffectResult::Resolved;
    }
    *counter = 0;
    if rs.current_hp <= 0 {
        return EffectResult::Resolved;
    }
    rs.heal(BOOK_OF_FIVE_RINGS_HEAL);
    EffectResult::Resolved
}

/// `TinyMailbox::TryModifyRestSiteHealRewards` — append TWO `PotionReward`s
/// to the rest-site reward list (`TinyMailbox.cs:14-23`:
/// `rewards.Add(new PotionReward(player)); rewards.Add(new PotionReward(player));`).
/// The C# decomp is authoritative; the prior single-potion add followed the
/// stale Python pin (`common.py::TinyMailbox` returns one) and shorted a
/// potion whenever ≥2 belt slots were open (sts2-rs-RP23). The Rust
/// `PotionReward` POD carries an optional pre-rolled `potion_kind`; the
/// reward dispatcher rolls the concrete potion at drain/procure time.
/// Ownership-gated. (Was `sts2-rs-7z65`.)
fn listener_tiny_mailbox(rs: &mut RunState, rewards: &mut Vec<Reward>) -> EffectResult {
    if !rs.owns_active_relic(RelicId::TinyMailbox) {
        return EffectResult::NotImplemented;
    }
    rewards.push(Reward::Potion(PotionReward::empty()));
    rewards.push(Reward::Potion(PotionReward::empty()));
    EffectResult::Resolved
}

/// `WarPaint::after_obtained` — upgrade `WAR_PAINT_CARDS = 2`
/// random non-upgraded Skill cards in the master deck. Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/common.py::WarPaint`:
/// ```text
/// def after_obtained(self, owner):
///     if owner.run_state.defer_followup_rewards:
///         owner.offer_upgrade_cards_reward(self.CARDS,
///             cards=owner.upgradable_deck_cards(CardType.SKILL))
///         return
///     owner.upgrade_random_cards(CardType.SKILL, self.CARDS,
///         rng=owner.run_state.rng.niche)
/// ```
///
/// **Port stance (bead `sts2-rs-8bqdw`):** Ships only the
/// non-deferred direct-upgrade branch via the typed primitive
/// [`sts2_core::runstate::RunState::upgrade_random_cards_of_type`]
/// (bead `sts2-rs-tb36q`), which already uses the niche RNG stream
/// and matches Python's shuffle-then-take semantics. The deferred
/// `offer_upgrade_cards_reward` branch is gated on
/// `RunState.defer_followup_rewards`, which isn't modelled at this
/// pin — same gap as `roll_card_reward_cards` (jya0y) and
/// `listener_new_leaf` (ytiok).
///
/// Ownership-gated. Highest-impact INERT relic per the audit
/// script (1659 fixture frequency).
fn listener_war_paint(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::WarPaint) {
        return EffectResult::NotImplemented;
    }
    let count = usize::try_from(WAR_PAINT_CARDS).unwrap_or(0);
    // C# `WarPaint.AfterObtained` (WarPaint.cs:20) AUTO-upgrades
    // `CARDS` random non-upgraded Skills immediately —
    // `Deck.Cards.Where(Skill && IsUpgradable).StableShuffle(Rng.Niche)
    // .Take(CARDS)` → `CardCmd.Upgrade`, NO player choice. Same fix as
    // Whetstone (the `defer_followup_rewards` player-pick arm was a
    // Python-pin divergence that silently no-op'd in the run-parity
    // flow). Bead `sts2-rs-whetstone-warpaint-auto`.
    rs.upgrade_random_cards_of_type(sts2_core::cards::CardType::Skill, count);
    EffectResult::Resolved
}

// ---------------------------------------------------------------------------
// Static parity pins — per-relic constants mirrored from common.py
// ---------------------------------------------------------------------------
//
// Exposing these as `pub const` lets the P7-8 integration bead consume
// them when wiring real hook bodies, instead of re-typing the numbers
// from the Python source. The constants live here (not on each
// factory) because the factories themselves emit `RelicEffect::Noop`
// for now — once `RelicEffect` grows variants for these triggers
// (`GainBlockAtCombatStart`, `DrawExtraOnRoundOne`, ...) the factories
// will start carrying the numbers directly and these constants can be
// folded in.

/// `Anchor.BLOCK = 10` (`common.py:50`).
pub const ANCHOR_BLOCK: i32 = 10;
/// `AmethystAubergine.GoldVar = 15` per C# v0.103.2
/// `MegaCrit.Sts2.Core.Models.Relics/AmethystAubergine.cs:18`. The
/// Python pin (`common.py:27 GOLD = 10`) drifted; CLAUDE.md hard rule
/// says C# decomp wins. Bead `sts2-rs-jakq1`.
pub const AMETHYST_AUBERGINE_GOLD: i32 = 15;
/// `BagOfPreparation.EXTRA_CARDS = 2` (`common.py:62`).
pub const BAG_OF_PREPARATION_EXTRA_CARDS: i32 = 2;
/// `BloodVial.HEAL = 2` (`common.py:75`).
pub const BLOOD_VIAL_HEAL: i32 = 2;
/// `BoneFlute.BLOCK = 2` (`common.py:89`).
pub const BONE_FLUTE_BLOCK: i32 = 2;
/// `BronzeScales.THORNS = 3` (`common.py:103`).
pub const BRONZE_SCALES_THORNS: i32 = 3;
/// `CentennialPuzzle.CARDS = 3` (`common.py:115`).
pub const CENTENNIAL_PUZZLE_CARDS: i32 = 3;
/// `DataDisk.FOCUS = 1` (`common.py:139`).
pub const DATA_DISK_FOCUS: i32 = 1;
/// `FencingManual.FORGE = 10` (`common.py:151`).
pub const FENCING_MANUAL_FORGE: i32 = 10;
/// `FestivePopper.DAMAGE = 9` (`common.py:164`).
pub const FESTIVE_POPPER_DAMAGE: i32 = 9;
/// `Gorget.PLATING = 4` (`common.py:178`).
pub const GORGET_PLATING: i32 = 4;
/// `HappyFlower.ENERGY = 1`, `TURNS = 3` (`common.py:190-191`).
pub const HAPPY_FLOWER_ENERGY: i32 = 1;
/// `HappyFlower.TURNS = 3` (`common.py:191`).
pub const HAPPY_FLOWER_TURNS: i32 = 3;
/// `Lantern.ENERGY = 1` (`common.py:222`).
pub const LANTERN_ENERGY: i32 = 1;
/// `MealTicket.HEAL = 15` (`common.py:235`).
pub const MEAL_TICKET_HEAL: i32 = 15;
/// `OddlySmoothStone.DEXTERITY = 1` (`common.py:249`).
pub const ODDLY_SMOOTH_STONE_DEXTERITY: i32 = 1;
/// C# `Permafrost.cs` `BlockVar(7)` — grants 7 block at combat start. The
/// Python pin (`common.py:272`) said 6; C# v0.103.2 is the source of truth.
/// Bead `sts2-rs-bhkxp`.
pub const PERMAFROST_BLOCK: i32 = 7;
/// `PotionBelt.POTION_SLOTS = 2` (`common.py:294`).
pub const POTION_BELT_SLOTS: i32 = 2;
/// `RedSkull.STRENGTH = 3`, threshold = 50% HP (`common.py:306-307`).
pub const RED_SKULL_STRENGTH: i32 = 3;
/// `RedSkull.HP_THRESHOLD_PCT = 50` (`common.py:306`).
pub const RED_SKULL_HP_THRESHOLD_PCT: i32 = 50;
/// `RegalPillow.HEAL = 15` (`common.py:343`).
pub const REGAL_PILLOW_HEAL: i32 = 15;
/// `SneckoSkull.EXTRA_POISON = 1` (`common.py:355`).
pub const SNECKO_SKULL_EXTRA_POISON: i32 = 1;
/// `Strawberry.MAX_HP = 7` (`common.py:365`).
pub const STRAWBERRY_MAX_HP: i32 = 7;
/// `StrikeDummy.EXTRA_DAMAGE = 3` (`common.py:377`).
pub const STRIKE_DUMMY_EXTRA_DAMAGE: i32 = 3;
/// `Vajra.STRENGTH = 1` (`common.py:417`).
pub const VAJRA_STRENGTH: i32 = 1;
/// `VenerableTeaSet.ENERGY = 2` (`common.py:429`).
pub const VENERABLE_TEA_SET_ENERGY: i32 = 2;
/// `WarPaint.CARDS = 2` (`common.py:451`).
pub const WAR_PAINT_CARDS: i32 = 2;
/// `Whetstone.CARDS = 2` (`common.py:466`).
pub const WHETSTONE_CARDS: i32 = 2;
/// `BookOfFiveRings` — C# `BookOfFiveRings.cs` `CanonicalVars` is
/// `[CardsVar(5), HealVar(20m)]`. The Python pin's `HEAL = 15`
/// (`common.py:482`) drifted from the shipped game; corrected to 20
/// per bead `sts2-rs-npfkz`.
pub const BOOK_OF_FIVE_RINGS_CARDS_THRESHOLD: i32 = 5;
/// `BookOfFiveRings.HEAL = 20` (C# `HealVar(20m)`). Bead `sts2-rs-npfkz`.
pub const BOOK_OF_FIVE_RINGS_HEAL: i32 = 20;
/// `Akabeko.VIGOR = 8` (`uncommon.py:Akabeko.VIGOR`). Mirrored here
/// for the bead's deep-test acceptance even though the `RelicSpec`
/// factory itself lives in [`crate::relics::uncommon`].
pub const AKABEKO_VIGOR: i32 = 8;
/// `BagOfMarbles.VULNERABLE = 1` (`uncommon.py:BagOfMarbles.VULNERABLE`).
/// See [`AKABEKO_VIGOR`] for ownership note.
pub const BAG_OF_MARBLES_VULNERABLE: i32 = 1;

// ---------------------------------------------------------------------------
// Tests — per-relic spec pins + 3 deep tests each for Akabeko/Anchor/BagOfMarbles
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Every factory produces a `RelicSpec` whose `name` round-trips
    /// through `RelicId::to_python_name`. This is the parity-output
    /// invariant downstream parity exporters rely on.
    #[test]
    fn every_common_factory_name_round_trips() {
        let pairs: &[(RelicSpec, RelicId)] = &[
            (amethyst_aubergine(), RelicId::AmethystAubergine),
            (anchor(), RelicId::Anchor),
            (bag_of_preparation(), RelicId::BagOfPreparation),
            (blood_vial(), RelicId::BloodVial),
            (bone_flute(), RelicId::BoneFlute),
            (bronze_scales(), RelicId::BronzeScales),
            (centennial_puzzle(), RelicId::CentennialPuzzle),
            (circlet(), RelicId::Circlet),
            (data_disk(), RelicId::DataDisk),
            (deprecated_relic(), RelicId::DeprecatedRelic),
            (fencing_manual(), RelicId::FencingManual),
            (festive_popper(), RelicId::FestivePopper),
            (gorget(), RelicId::Gorget),
            (happy_flower(), RelicId::HappyFlower),
            (juzu_bracelet(), RelicId::JuzuBracelet),
            (lantern(), RelicId::Lantern),
            (meal_ticket(), RelicId::MealTicket),
            (oddly_smooth_stone(), RelicId::OddlySmoothStone),
            (pendulum(), RelicId::Pendulum),
            (permafrost(), RelicId::Permafrost),
            (potion_belt(), RelicId::PotionBelt),
            (red_skull(), RelicId::RedSkull),
            (regal_pillow(), RelicId::RegalPillow),
            (snecko_skull(), RelicId::SneckoSkull),
            (strawberry(), RelicId::Strawberry),
            (strike_dummy(), RelicId::StrikeDummy),
            (tiny_mailbox(), RelicId::TinyMailbox),
            (vajra(), RelicId::Vajra),
            (venerable_tea_set(), RelicId::VenerableTeaSet),
            (war_paint(), RelicId::WarPaint),
            (whetstone(), RelicId::Whetstone),
            (book_of_five_rings(), RelicId::BookOfFiveRings),
        ];
        for (spec, id) in pairs {
            assert_eq!(spec.name, id.to_python_name(), "spec {spec:?} vs id {id:?}");
        }
    }

    /// All commons carry rarity bucket matching their C#/grab_bag class.
    #[test]
    fn rarities_match_python() {
        // The commons.
        let commons = [
            amethyst_aubergine(),
            anchor(),
            bag_of_preparation(),
            blood_vial(),
            bone_flute(),
            bronze_scales(),
            centennial_puzzle(),
            circlet(),
            data_disk(),
            deprecated_relic(),
            fencing_manual(),
            festive_popper(),
            gorget(),
            happy_flower(),
            juzu_bracelet(),
            lantern(),
            meal_ticket(),
            oddly_smooth_stone(),
            pendulum(),
            potion_belt(),
            red_skull(),
            regal_pillow(),
            snecko_skull(),
            strawberry(),
            strike_dummy(),
            vajra(),
            venerable_tea_set(),
            war_paint(),
            whetstone(),
            book_of_five_rings(),
        ];
        // Bumped 30 → 32 by bead `sts2-rs-70anj.5` (Wave-5 INERT
        // placeholders): Circlet + DeprecatedRelic ship as Noop
        // factories so the engine can round-trip a fallback /
        // deprecated relic id without a missing-factory panic. Both
        // are `RelicRarity::Common` per the Python pin at
        // `shop_event.py:3102-3114`. Dropped 32 → 30 by bead
        // `sts2-rs-kw0z9`: Permafrost + TinyMailbox are `Uncommon` in
        // the C# / grab_bag (this module still hosts their effect-handler
        // registration, but the rarity tag now matches `*.Models.Relics`).
        assert_eq!(commons.len(), 30);
        for r in commons {
            assert_eq!(r.rarity, RelicRarity::Common, "wrong rarity for {r:?}");
        }
        // Permafrost + TinyMailbox live in this module for handler
        // registration but are `Uncommon` per C# (`Permafrost.cs:18`,
        // `TinyMailbox.cs:12`) and grab_bag's `SHARED_UNCOMMON` bucket.
        for r in [permafrost(), tiny_mailbox()] {
            assert_eq!(r.rarity, RelicRarity::Uncommon, "wrong rarity for {r:?}");
        }
    }

    /// `COMMON_RELIC_IDS` holds no duplicate ids — catches accidental
    /// paste-edits in the membership list (which feeds `pool_relic_ids`).
    /// Per-relic dispatch reachability is now covered by the
    /// crate-level RelicId completeness census.
    #[test]
    fn pool_relic_ids_are_unique() {
        let mut ids: Vec<RelicId> = COMMON_RELIC_IDS.to_vec();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate RelicId in COMMON_RELIC_IDS");
    }

    /// Factory output is pure (same value on repeat invocation).
    /// Important because `RelicSpec` is `Copy` and downstream consumers
    /// (run-builder) materialize instances by-value from these factories.
    #[test]
    fn factories_are_pure() {
        assert_eq!(anchor(), anchor());
        assert_eq!(red_skull(), red_skull());
        assert_eq!(strike_dummy(), strike_dummy());
        assert_eq!(blood_vial(), blood_vial());
    }

    // -----------------------------------------------------------------
    // Deep tests — Anchor (≥3)
    // -----------------------------------------------------------------

    /// **Anchor #1** spec pins. `BLOCK = 10` is the load-bearing number
    /// from `common.py:50`. A regression that drops this to 9 or 11
    /// would silently shift combat math by 1 block/turn-1 forever.
    #[test]
    fn anchor_spec_pin() {
        let a = anchor();
        assert_eq!(a.id, RelicKind::Anchor);
        assert_eq!(a.name, "ANCHOR");
        assert_eq!(a.rarity, RelicRarity::Common);
        assert_eq!(
            a.effect,
            RelicEffect::GainBlockAtCombatStart {
                amount: ANCHOR_BLOCK,
            }
        );
        assert_eq!(ANCHOR_BLOCK, 10);
    }

    /// **Anchor #2** name parity: the spec's `name` matches both the
    /// auto-generated `RelicId::Anchor.to_python_name()` and the legacy
    /// `RelicKind::Anchor.python_name()`. If a future bead diverges
    /// these two surfaces (e.g. by adding `Anchor` to one but not the
    /// other) this test catches it.
    #[test]
    fn anchor_name_consistent_across_id_surfaces() {
        let a = anchor();
        assert_eq!(a.name, RelicId::Anchor.to_python_name());
        assert_eq!(a.name, RelicKind::Anchor.python_name());
    }

    // -----------------------------------------------------------------
    // Deep test — Bag of Preparation (sts2-rs-owus)
    // -----------------------------------------------------------------

    /// Bag of Preparation spec pin: the effect is the existing
    /// [`RelicEffect::ExtraHandDrawForRounds`] AST variant with
    /// `extra_cards = 2` and `max_round = 1` — exactly the parameters
    /// `RingOfTheSnake` uses, mirrored from
    /// `sts2_env/relics/common.py::BagOfPreparation.modify_hand_draw`.
    /// Bead `sts2-rs-owus`.
    #[test]
    fn bag_of_preparation_spec_pin() {
        let r = bag_of_preparation();
        assert_eq!(r.id, RelicKind::BagOfPreparation);
        assert_eq!(r.name, "BAG_OF_PREPARATION");
        assert_eq!(r.rarity, RelicRarity::Common);
        assert_eq!(
            r.effect,
            RelicEffect::ExtraHandDrawForRounds {
                extra_cards: BAG_OF_PREPARATION_EXTRA_CARDS,
                max_round: 1,
            }
        );
        assert_eq!(BAG_OF_PREPARATION_EXTRA_CARDS, 2);
    }

    // -----------------------------------------------------------------
    // Deep tests — Akabeko (≥3)
    // -----------------------------------------------------------------

    /// **Akabeko #1** Python pin: VIGOR=8 mirrored here even though
    /// the `RelicSpec` factory lives in the P7-3 uncommon module.
    #[test]
    fn akabeko_python_pin() {
        assert_eq!(AKABEKO_VIGOR, 8);
        assert_eq!(RelicId::Akabeko.to_python_name(), "AKABEKO");
        assert_eq!(RelicKind::Akabeko.python_name(), "AKABEKO");
    }

    /// **Akabeko #2** name parity across `RelicId` and `RelicKind` —
    /// catches a future divergence where the auto-generated and
    /// legacy surfaces drift.
    #[test]
    fn akabeko_name_consistent_across_id_surfaces() {
        assert_eq!(
            RelicId::Akabeko.to_python_name(),
            RelicKind::Akabeko.python_name()
        );
    }

    // -----------------------------------------------------------------
    // Deep tests — BagOfMarbles (≥3)
    // -----------------------------------------------------------------

    /// **`BagOfMarbles` #1** Python pin: VULNERABLE=1 mirrored from the
    /// P7-3 uncommon module's source-of-truth.
    #[test]
    fn bag_of_marbles_python_pin() {
        assert_eq!(BAG_OF_MARBLES_VULNERABLE, 1);
        assert_eq!(RelicId::BagOfMarbles.to_python_name(), "BAG_OF_MARBLES");
        assert_eq!(RelicKind::BagOfMarbles.python_name(), "BAG_OF_MARBLES");
    }

    /// **`BagOfMarbles` #2** name parity across surfaces.
    #[test]
    fn bag_of_marbles_name_consistent_across_id_surfaces() {
        assert_eq!(
            RelicId::BagOfMarbles.to_python_name(),
            RelicKind::BagOfMarbles.python_name()
        );
    }

    // -----------------------------------------------------------------
    // Spec pins — A–H common-tier wave (bead `sts2-rs-6ynt.5.3`)
    //
    // Each pin asserts (a) the new `RelicEffect` variant the factory
    // emits and (b) the load-bearing Python constant. Pool-excluded
    // A–H relics (BoneFlute / DataDisk / FencingManual) stay `Noop`
    // and are NOT pinned here so the sibling pool-specific bead can
    // wire them without test churn.
    // -----------------------------------------------------------------

    #[test]
    fn amethyst_aubergine_spec_pin() {
        let r = amethyst_aubergine();
        assert_eq!(
            r.effect,
            RelicEffect::AppendRewardGold {
                amount: AMETHYST_AUBERGINE_GOLD,
            }
        );
        // sts2-rs-jakq1: C# GoldVar(15) per AmethystAubergine.cs:18; was 10 (Python pin).
        assert_eq!(AMETHYST_AUBERGINE_GOLD, 15);
    }

    #[test]
    fn blood_vial_spec_pin() {
        let r = blood_vial();
        assert_eq!(
            r.effect,
            RelicEffect::HealAtPlayerSideTurnStart {
                amount: BLOOD_VIAL_HEAL,
            }
        );
        assert_eq!(BLOOD_VIAL_HEAL, 2);
    }

    #[test]
    fn bronze_scales_spec_pin() {
        let r = bronze_scales();
        assert_eq!(
            r.effect,
            RelicEffect::ApplyPowerAtCombatStart {
                kind: PowerKind::Thorns,
                amount: BRONZE_SCALES_THORNS,
            }
        );
        assert_eq!(BRONZE_SCALES_THORNS, 3);
    }

    #[test]
    fn centennial_puzzle_spec_pin() {
        let r = centennial_puzzle();
        assert_eq!(
            r.effect,
            RelicEffect::DrawCardsOnFirstDamagePerCombat {
                amount: CENTENNIAL_PUZZLE_CARDS,
            }
        );
        assert_eq!(CENTENNIAL_PUZZLE_CARDS, 3);
    }

    #[test]
    fn festive_popper_spec_pin() {
        let r = festive_popper();
        assert_eq!(
            r.effect,
            RelicEffect::DealDamageToAllEnemiesOnSideTurnStart {
                amount: FESTIVE_POPPER_DAMAGE,
                max_round: 1,
            }
        );
        assert_eq!(FESTIVE_POPPER_DAMAGE, 9);
    }

    #[test]
    fn gorget_spec_pin() {
        let r = gorget();
        assert_eq!(
            r.effect,
            RelicEffect::ApplyPowerAtCombatStart {
                kind: PowerKind::Plating,
                amount: GORGET_PLATING,
            }
        );
        assert_eq!(GORGET_PLATING, 4);
    }

    /// Bead `sts2-rs-bco9`: spec metadata pin for `Vajra` after sync'ing
    /// the declarative `RelicEffect` AST with the bfuy runtime listener
    /// (`on_before_combat_start_vajra`). Mirrors `common.py:417`
    /// (`STRENGTH = 1`).
    #[test]
    fn vajra_spec_pin() {
        let r = vajra();
        assert_eq!(
            r.effect,
            RelicEffect::ApplyPowerAtCombatStart {
                kind: PowerKind::Strength,
                amount: VAJRA_STRENGTH,
            }
        );
        assert_eq!(VAJRA_STRENGTH, 1);
    }

    /// Bead `sts2-rs-bco9`: spec metadata pin for `OddlySmoothStone`
    /// after sync'ing the declarative AST with bfuy's
    /// `on_before_combat_start_oddly_smooth_stone`. Mirrors
    /// `common.py:249` (`DEXTERITY = 1`).
    #[test]
    fn oddly_smooth_stone_spec_pin() {
        let r = oddly_smooth_stone();
        assert_eq!(
            r.effect,
            RelicEffect::ApplyPowerAtCombatStart {
                kind: PowerKind::Dexterity,
                amount: ODDLY_SMOOTH_STONE_DEXTERITY,
            }
        );
        assert_eq!(ODDLY_SMOOTH_STONE_DEXTERITY, 1);
    }

    /// Bead `sts2-rs-bco9`: spec metadata pin for `DataDisk` after
    /// sync'ing the declarative AST with bfuy's
    /// `on_before_combat_start_data_disk`. Mirrors `common.py:139`
    /// (`FOCUS = 1`).
    #[test]
    fn data_disk_spec_pin() {
        let r = data_disk();
        assert_eq!(
            r.effect,
            RelicEffect::ApplyPowerAtCombatStart {
                kind: PowerKind::Focus,
                amount: DATA_DISK_FOCUS,
            }
        );
        assert_eq!(DATA_DISK_FOCUS, 1);
    }

    #[test]
    fn happy_flower_spec_pin() {
        let r = happy_flower();
        assert_eq!(
            r.effect,
            RelicEffect::GainEnergyEveryNPlayerSideTurns {
                energy: HAPPY_FLOWER_ENERGY,
                period: HAPPY_FLOWER_TURNS as u16,
            }
        );
        assert_eq!(HAPPY_FLOWER_ENERGY, 1);
        assert_eq!(HAPPY_FLOWER_TURNS, 3);
    }

    /// `BookOfFiveRings.on_card_added_to_deck` is a run-state hook;
    /// the spec stays `Noop` because the AST AST has no variant for
    /// "increment counter + threshold-trigger heal" — the real body
    /// lives on `listener_book_of_five_rings` (bead `sts2-rs-sx27`),
    /// registered via [`install_pool`]. Pinning Noop on the spec
    /// catches accidental drift on the static factory side.
    #[test]
    fn book_of_five_rings_spec_remains_noop_listener_lives_on_run_side() {
        let r = book_of_five_rings();
        assert_eq!(r.effect, RelicEffect::Noop);
        assert_eq!(BOOK_OF_FIVE_RINGS_CARDS_THRESHOLD, 5);
        assert_eq!(BOOK_OF_FIVE_RINGS_HEAL, 20);
    }

    // -----------------------------------------------------------------
    // Deep tests — StrikeDummy (sts2-rs-6ynt.5.4)
    //
    // Common-tier SHARED-pool relic that adds +3 damage to player-dealt
    // Strike-tagged Attack moves. Python pin:
    // `sts2-rl-agent@01c094d:sts2_env/relics/common.py::StrikeDummy`
    // (`EXTRA_DAMAGE = 3`, gates on `dealer is owner && is_strike &&
    // card_type==ATTACK && props & MOVE && !UNPOWERED`). The
    // `RelicSpec.effect` slot stays `Noop` because the AST has no
    // `ModifyDamageAdditive` variant; the listener registered in
    // `sts2_core::relic_effects::install_default_registry` is the
    // load-bearing path.
    // -----------------------------------------------------------------

    /// **StrikeDummy #1** Python pin: `EXTRA_DAMAGE = 3` mirrored as
    /// [`STRIKE_DUMMY_EXTRA_DAMAGE`]; spec name round-trips through
    /// both `RelicId` and `RelicKind` surfaces.
    #[test]
    fn strike_dummy_python_pin() {
        assert_eq!(STRIKE_DUMMY_EXTRA_DAMAGE, 3);
        assert_eq!(RelicId::StrikeDummy.to_python_name(), "STRIKE_DUMMY");
        assert_eq!(RelicKind::StrikeDummy.python_name(), "STRIKE_DUMMY");
        let r = strike_dummy();
        assert_eq!(r.id, RelicKind::StrikeDummy);
        assert_eq!(r.name, "STRIKE_DUMMY");
        assert_eq!(r.rarity, RelicRarity::Common);
        // Spec effect intentionally stays Noop until the AST grows a
        // `ModifyDamageAdditive` variant covering the load-bearing
        // gates. The per-channel listener in `sts2_core::relic_effects`
        // is what fires from the damage pipeline.
        assert_eq!(r.effect, RelicEffect::Noop);
    }

    /// **StrikeDummy #2** listener registered on the
    /// `modify_damage_additive` channel after the full registry
    /// install. Asserts the load-bearing path is live (channel
    /// `lookup_*` returns `Some` for this relic id).
    #[test]
    fn strike_dummy_listener_registered_after_full_install() {
        crate::relics::dispatch::ensure_installed();
        assert!(
            sts2_core::relic_effects::lookup_modify_damage_additive(RelicId::StrikeDummy).is_some(),
            "StrikeDummy listener not registered on modify_damage_additive channel"
        );
    }

    /// Bead `sts2-rs-q6bix` (F-T): `Pendulum.AfterPlayerTurnStart` draws
    /// `Cards`(1) every 3rd player-turn start (`TurnsSeen = (TurnsSeen+1)
    /// % Turns; if TurnsSeen == 0 draw`). The earlier `after_shuffle` wiring
    /// was a Python-pin fabrication — C# `Pendulum.cs` fires on the
    /// player-turn-start channel, NOT per shuffle. Seed 3 cards, dispatch
    /// three player-turn starts: no draw on turns 1-2, exactly 1 on turn 3.
    #[test]
    fn pendulum_draws_every_third_player_turn_start() {
        use std::num::NonZeroU32;
        use sts2_core::cards::{Card, CardId, CardType, TargetType};
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        use sts2_core::{CardUid, CombatState, Relic, RelicKind, Side};

        crate::relics::dispatch::ensure_installed();
        assert!(
            sts2_core::relic_effects::lookup_after_side_turn_start(RelicId::Pendulum).is_some(),
            "Pendulum listener must register on after_side_turn_start channel"
        );

        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();

        for n in 1..=3u32 {
            let uid = CardUid(NonZeroU32::new(n).expect("non-zero"));
            let card = Card {
                uid,
                card_id: CardId::StrikeIronclad,
                cost: 1,
                card_type: CardType::Attack,
                target_type: TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..Card::pod_defaults()
            };
            state.cards.push(card);
            state.player.draw.push(uid);
        }

        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::Pendulum,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        // Turns 1 and 2: no draw.
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.player.hand.len(), 0, "no draw on turn 1");
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(state.player.hand.len(), 0, "no draw on turn 2");
        // Turn 3: draw exactly 1.
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert_eq!(
            state.player.hand.len(),
            1,
            "Pendulum draws 1 card on the 3rd player-turn start"
        );
        assert_eq!(
            state.player.draw.len(),
            2,
            "exactly one card left the draw pile"
        );
    }

    /// Bead `sts2-rs-q6bix` (F-T): Pendulum owner-gate + enemy-side no-op.
    #[test]
    fn pendulum_turn_start_owner_gated_and_enemy_side_noop() {
        use std::num::NonZeroU32;
        use sts2_core::cards::{Card, CardId, CardType, TargetType};
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        use sts2_core::{CardUid, CombatState, Relic, RelicKind, Side};

        crate::relics::dispatch::ensure_installed();

        // No Pendulum equipped: three player-turn starts draw nothing.
        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        let uid = CardUid(NonZeroU32::new(1).expect("non-zero"));
        state.cards.push(Card {
            uid,
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        });
        state.player.draw.push(uid);
        for _ in 0..3 {
            let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        }
        assert_eq!(
            state.player.hand.len(),
            0,
            "owner-gate: no Pendulum, no draw"
        );

        // Pendulum equipped but enemy-side dispatch is a no-op.
        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::Pendulum,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        for _ in 0..3 {
            let _ = dispatch_after_side_turn_start(&mut state, Side::Enemy);
        }
        assert_eq!(
            state.player.hand.len(),
            0,
            "enemy-side turn starts never advance Pendulum"
        );
    }

    /// Bead `sts2-rs-u8qa`: Strawberry's `after_obtained` listener
    /// must add `MAX_HP=7` to the player's max HP (and current HP) when
    /// the run-state owns the relic. Mirrors the FakeMango test pattern.
    #[test]
    fn listener_strawberry_gains_seven_max_hp_on_obtain() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_max = rs.max_hp;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::Strawberry);
        rs.last_obtained_relic = Some(RelicId::Strawberry);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max + STRAWBERRY_MAX_HP);
        assert_eq!(rs.current_hp, before_hp + STRAWBERRY_MAX_HP);
    }

    /// Bead `sts2-rs-70anj.2`: PotionBelt's `after_obtained` listener
    /// must add `POTION_SLOTS=2` to the run state's max potion slots
    /// when the relic is owned. Mirrors Python pin
    /// `gain_potion_slots(POTION_SLOTS=2)`.
    #[test]
    fn listener_potion_belt_grants_two_slots_on_obtain() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before = rs.max_potion_slots;
        rs.relics.push(RelicId::PotionBelt);
        rs.last_obtained_relic = Some(RelicId::PotionBelt);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_potion_slots, before + POTION_BELT_SLOTS);
    }

    /// Bead `sts2-rs-e8p18`: Whetstone's `after_obtained` listener
    /// must upgrade `CARDS = 2` random non-upgraded Attacks in the
    /// master deck when owned. Mirrors Python pin
    /// `Whetstone.after_obtained` →
    /// `upgrade_random_cards(CardType.ATTACK, 2)`.
    #[test]
    fn listener_whetstone_upgrades_two_attacks_on_obtain() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..3 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Attack;
                c
            });
        }
        for _ in 0..3 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Skill;
                c
            });
        }
        rs.relics.push(RelicId::Whetstone);
        rs.last_obtained_relic = Some(RelicId::Whetstone);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let attacks_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Attack && c.upgraded)
            .count();
        let skills_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Skill && c.upgraded)
            .count();
        assert_eq!(attacks_upgraded, WHETSTONE_CARDS as usize);
        assert_eq!(skills_upgraded, 0, "no Skill was touched");
    }

    /// Whetstone's `after_obtained` is a no-op when the relic is not
    /// in `rs.relics` (ownership gate).
    #[test]
    fn listener_whetstone_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..3 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Attack;
                c
            });
        }
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let any_upgraded = rs.master_deck.iter().any(|c| c.upgraded);
        assert!(!any_upgraded, "no card upgraded without ownership");
    }

    /// Whetstone saturates at the candidate count when fewer Attacks
    /// are eligible than `CARDS = 2`.
    #[test]
    fn listener_whetstone_saturates_at_one_eligible_attack() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // 1 non-upgraded Attack + 2 already-upgraded Attacks + 3 Skills.
        rs.master_deck.push({
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_type = sts2_core::cards::CardType::Attack;
            c
        });
        for _ in 0..2 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Attack;
                c.upgraded = true;
                c
            });
        }
        for _ in 0..3 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Skill;
                c
            });
        }
        rs.relics.push(RelicId::Whetstone);
        rs.last_obtained_relic = Some(RelicId::Whetstone);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let attacks_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Attack && c.upgraded)
            .count();
        assert_eq!(
            attacks_upgraded, 3,
            "previously-upgraded 2 + newly-upgraded 1"
        );
        let skills_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Skill && c.upgraded)
            .count();
        assert_eq!(skills_upgraded, 0);
    }

    // ── WarPaint — bead 8bqdw ─────────────────────────────────────

    /// Bead `sts2-rs-8bqdw`: WarPaint's `after_obtained` listener
    /// must upgrade `CARDS = 2` random non-upgraded Skills in the
    /// master deck when owned. Mirrors Python pin
    /// `WarPaint.after_obtained` →
    /// `upgrade_random_cards(CardType.SKILL, 2)`.
    #[test]
    fn listener_war_paint_upgrades_two_skills_on_obtain() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..3 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Skill;
                c
            });
        }
        for _ in 0..3 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Attack;
                c
            });
        }
        rs.relics.push(RelicId::WarPaint);
        rs.last_obtained_relic = Some(RelicId::WarPaint);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let skills_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Skill && c.upgraded)
            .count();
        let attacks_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Attack && c.upgraded)
            .count();
        assert_eq!(skills_upgraded, WAR_PAINT_CARDS as usize);
        assert_eq!(attacks_upgraded, 0, "no Attack was touched");
    }

    /// WarPaint's `after_obtained` is a no-op when the relic is not
    /// in `rs.relics` (ownership gate).
    #[test]
    fn listener_war_paint_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..3 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Skill;
                c
            });
        }
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let any_upgraded = rs.master_deck.iter().any(|c| c.upgraded);
        assert!(!any_upgraded, "no card upgraded without ownership");
    }

    /// WarPaint saturates at the candidate count when fewer non-
    /// upgraded Skills are eligible than `CARDS = 2`.
    #[test]
    fn listener_war_paint_saturates_at_one_eligible_skill() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck.push({
            let mut c = sts2_core::cards::Card::pod_defaults();
            c.card_type = sts2_core::cards::CardType::Skill;
            c
        });
        for _ in 0..2 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Skill;
                c.upgraded = true;
                c
            });
        }
        for _ in 0..3 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Attack;
                c
            });
        }
        rs.relics.push(RelicId::WarPaint);
        rs.last_obtained_relic = Some(RelicId::WarPaint);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let skills_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Skill && c.upgraded)
            .count();
        assert_eq!(
            skills_upgraded, 3,
            "previously-upgraded 2 + newly-upgraded 1"
        );
        let attacks_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Attack && c.upgraded)
            .count();
        assert_eq!(attacks_upgraded, 0);
    }

    /// Bead `sts2-rs-whetstone-warpaint-auto`: C# `WarPaint.AfterObtained`
    /// (WarPaint.cs:20) AUTO-upgrades `WAR_PAINT_CARDS` random Skills
    /// immediately — `StableShuffle(Rng.Niche).Take(CARDS)` →
    /// `CardCmd.Upgrade`, NO player choice. This holds EVEN under
    /// `defer_followup_rewards` (the prior player-pick `Reward::UpgradeCards`
    /// arm was a non-authoritative Python-pin divergence that silently
    /// no-op'd in the run-parity flow; whole-run seed RP78). Contrast the
    /// enchant relics, which DO gate behind a player CardSelectCmd in C#.
    #[test]
    fn listener_war_paint_auto_upgrades_two_skills_even_when_deferred() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..3 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Skill;
                c.card_id = sts2_core::cards::CardId::StrikeIronclad;
                c
            });
        }
        for _ in 0..3 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Attack;
                c
            });
        }
        rs.relics.push(RelicId::WarPaint);
        rs.defer_followup_rewards = true;
        let pending_before = rs.pending_rewards.len();
        rs.last_obtained_relic = Some(RelicId::WarPaint);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let skills_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Skill && c.upgraded)
            .count();
        let attacks_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Attack && c.upgraded)
            .count();
        assert_eq!(
            skills_upgraded, WAR_PAINT_CARDS as usize,
            "auto-upgrades exactly CARDS Skills"
        );
        assert_eq!(attacks_upgraded, 0, "never touches Attacks");
        assert_eq!(
            rs.pending_rewards.len(),
            pending_before,
            "no player-pick reward queued — auto-applies"
        );
    }

    /// Bead `sts2-rs-whetstone-warpaint-auto`: symmetric coverage for
    /// Whetstone (Attacks instead of Skills) — auto-upgrades immediately
    /// even when `defer_followup_rewards` is set. C# `Whetstone.AfterObtained`
    /// (Whetstone.cs:20).
    #[test]
    fn listener_whetstone_auto_upgrades_two_attacks_even_when_deferred() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        for _ in 0..4 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Attack;
                c.card_id = sts2_core::cards::CardId::StrikeIronclad;
                c
            });
        }
        for _ in 0..2 {
            rs.master_deck.push({
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_type = sts2_core::cards::CardType::Skill;
                c
            });
        }
        rs.relics.push(RelicId::Whetstone);
        rs.defer_followup_rewards = true;
        let pending_before = rs.pending_rewards.len();
        rs.last_obtained_relic = Some(RelicId::Whetstone);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        let attacks_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Attack && c.upgraded)
            .count();
        let skills_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == sts2_core::cards::CardType::Skill && c.upgraded)
            .count();
        assert_eq!(
            attacks_upgraded, WHETSTONE_CARDS as usize,
            "auto-upgrades exactly CARDS Attacks"
        );
        assert_eq!(skills_upgraded, 0, "never touches Skills");
        assert_eq!(
            rs.pending_rewards.len(),
            pending_before,
            "no player-pick reward queued — auto-applies"
        );
    }

    /// Bead `sts2-rs-70anj.2`: RegalPillow's
    /// `modify_rest_site_heal_amount` listener returns `HEAL=15` when
    /// owned. Mirrors Python pin
    /// `modify_rest_site_heal_amount(amount) -> amount + 15`.
    #[test]
    fn listener_regal_pillow_adds_fifteen_to_rest_site_heal() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::RegalPillow);
        let base = 10;
        let total = sts2_core::run_effects::dispatch_modify_rest_site_heal_amount(&rs, base);
        assert_eq!(total, base + REGAL_PILLOW_HEAL);
    }

    /// No-op when RegalPillow isn't yet in `rs.relics`.
    #[test]
    fn listener_regal_pillow_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let rs = RunState::new();
        let base = 10;
        let total = sts2_core::run_effects::dispatch_modify_rest_site_heal_amount(&rs, base);
        assert_eq!(total, base);
    }

    /// No-op when PotionBelt isn't yet in `rs.relics`.
    #[test]
    fn listener_potion_belt_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before = rs.max_potion_slots;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_potion_slots, before);
    }

    /// No-op when the relic is not yet in `rs.relics`.
    #[test]
    fn listener_strawberry_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let before_max = rs.max_hp;
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max);
    }

    /// Bead `sts2-rs-5pk6`: MealTicket's `after_room_entered` listener
    /// heals `HEAL=15` on `RoomType::Shop` entry when the player is
    /// alive. Mirrors Python pin `is_merchant and current_hp > 0 →
    /// owner.heal(15)`.
    #[test]
    fn listener_meal_ticket_heals_fifteen_on_shop_room_entry() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.current_hp = rs.max_hp - 50;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::MealTicket);
        let _ = sts2_core::run_effects::dispatch_after_room_entered(&mut rs, RoomType::Shop);
        assert_eq!(rs.current_hp, before_hp + MEAL_TICKET_HEAL);
    }

    /// No-op on non-shop room types.
    #[test]
    fn listener_meal_ticket_is_noop_on_non_shop_rooms() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.current_hp = rs.max_hp - 50;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::MealTicket);
        for rt in [
            RoomType::Monster,
            RoomType::Elite,
            RoomType::Boss,
            RoomType::RestSite,
            RoomType::Treasure,
            RoomType::Event,
            RoomType::Unknown,
        ] {
            let _ = sts2_core::run_effects::dispatch_after_room_entered(&mut rs, rt);
        }
        assert_eq!(rs.current_hp, before_hp);
    }

    /// No-op when the player is already dead. Python's `current_hp > 0`
    /// gate matters because the runstate dispatcher fires
    /// `after_room_entered` on every room entry regardless of HP.
    #[test]
    fn listener_meal_ticket_is_noop_when_player_is_dead() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.current_hp = 0;
        rs.relics.push(RelicId::MealTicket);
        let _ = sts2_core::run_effects::dispatch_after_room_entered(&mut rs, RoomType::Shop);
        assert_eq!(rs.current_hp, 0);
    }

    /// No-op when the relic is not yet owned.
    #[test]
    fn listener_meal_ticket_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.current_hp = rs.max_hp - 50;
        let before_hp = rs.current_hp;
        let _ = sts2_core::run_effects::dispatch_after_room_entered(&mut rs, RoomType::Shop);
        assert_eq!(rs.current_hp, before_hp);
    }

    // -----------------------------------------------------------------
    // VenerableTeaSet listener tests — bead `sts2-rs-g1p2m`
    // -----------------------------------------------------------------

    /// Run-side: rest-site entry sets the latch to 1.
    #[test]
    fn listener_venerable_tea_set_latches_on_rest_site_entry() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::VenerableTeaSet);
        let _ = sts2_core::run_effects::dispatch_after_room_entered(&mut rs, RoomType::RestSite);
        assert_eq!(
            rs.relic_counters.get(&RelicId::VenerableTeaSet),
            Some(&1),
            "rest-site entry latches counter to 1"
        );
    }

    /// Run-side: non-rest-site rooms never set the latch.
    #[test]
    fn listener_venerable_tea_set_is_noop_on_non_rest_site_rooms() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::VenerableTeaSet);
        for rt in [
            RoomType::Monster,
            RoomType::Elite,
            RoomType::Boss,
            RoomType::Shop,
            RoomType::Treasure,
            RoomType::Event,
            RoomType::Unknown,
        ] {
            let _ = sts2_core::run_effects::dispatch_after_room_entered(&mut rs, rt);
        }
        assert!(
            !rs.relic_counters.contains_key(&RelicId::VenerableTeaSet),
            "no non-rest-site room should ever latch the counter"
        );
    }

    /// Run-side: no-op when the relic is not yet owned.
    #[test]
    fn listener_venerable_tea_set_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let _ = sts2_core::run_effects::dispatch_after_room_entered(&mut rs, RoomType::RestSite);
        assert!(
            !rs.relic_counters.contains_key(&RelicId::VenerableTeaSet),
            "ownership-gated — no latch without the relic"
        );
    }

    /// Run-side: repeated rest-site entries leave the latch at 1
    /// (idempotent — matches Python's boolean flag).
    #[test]
    fn listener_venerable_tea_set_repeated_rest_site_stays_latched() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::VenerableTeaSet);
        for _ in 0..3 {
            let _ =
                sts2_core::run_effects::dispatch_after_room_entered(&mut rs, RoomType::RestSite);
        }
        assert_eq!(
            rs.relic_counters.get(&RelicId::VenerableTeaSet),
            Some(&1),
            "latch is idempotent — no double-fire"
        );
    }

    /// Combat-side helper: push a VenerableTeaSet relic onto a
    /// `CombatState` with the given counter value, then init the
    /// player creature so the future `gain_energy` has somewhere to
    /// route. Mirrors `permafrost_push_relic` + `permafrost_init_player`.
    #[cfg(test)]
    fn vts_setup_combat(counter: i32) -> CombatState {
        use std::num::NonZeroU32;
        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.relics.push(sts2_core::Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::VenerableTeaSet,
            counter,
            is_wax: false,
            is_melted: false,
        });
        state
    }

    /// Combat-side: with the latch set, +2 energy is granted and the
    /// counter is cleared. Fires on `after_energy_reset` (mirroring C#
    /// `AfterEnergyReset`) so the grant lands AFTER the turn-1 energy
    /// reset instead of being overwritten by it.
    #[test]
    fn listener_venerable_tea_set_consumes_latch_and_grants_energy() {
        crate::relics::dispatch::ensure_installed();
        let mut state = vts_setup_combat(1);
        let before = state.player.energy;
        let _ = sts2_core::relic_effects::dispatch_after_energy_reset(&mut state);
        assert_eq!(
            i32::from(state.player.energy),
            i32::from(before) + VENERABLE_TEA_SET_ENERGY,
            "+2 energy on latched consume"
        );
        let consumed_idx = state
            .relics
            .iter()
            .position(|r| r.kind == RelicKind::VenerableTeaSet)
            .expect("relic still present");
        assert_eq!(
            state.relics[consumed_idx].counter, 0,
            "latch must clear after consume"
        );
    }

    /// Combat-side: counter=0 means "no rest site visited" — no energy
    /// gain, no mutation.
    #[test]
    fn listener_venerable_tea_set_is_noop_when_unlatched() {
        crate::relics::dispatch::ensure_installed();
        let mut state = vts_setup_combat(0);
        let before = state.player.energy;
        let _ = sts2_core::relic_effects::dispatch_after_energy_reset(&mut state);
        assert_eq!(
            state.player.energy, before,
            "unlatched combat must not grant energy"
        );
    }

    /// Combat-side: without the relic, the listener is fully inert
    /// (no panic, no energy mutation) even if a stray VenerableTeaSet
    /// entry is in `rs.relic_counters`.
    #[test]
    fn listener_venerable_tea_set_combat_side_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        let before = state.player.energy;
        let _ = sts2_core::relic_effects::dispatch_after_energy_reset(&mut state);
        assert_eq!(state.player.energy, before);
    }

    // -----------------------------------------------------------------
    // Permafrost listener tests — bead `sts2-rs-lqpw`
    // -----------------------------------------------------------------
    //
    // Shape: first Power card played in a combat grants +6 block (the
    // `PERMAFROST_BLOCK` constant); subsequent Powers in the same
    // combat are no-ops; non-Power cards never trigger. The latch
    // resets on `before_combat_start`. Tests stand up a minimal
    // `CombatState` with a player creature, push the relic with
    // `counter = 0`, and exercise the listener via the
    // `dispatch_after_card_played` bus end-to-end.

    /// Stand up a `Card` of the given type in `state.cards` and return
    /// its uid. Mirrors the shape used by event_reward's
    /// `push_attack_card` / `push_skill_card` helpers so the
    /// `CardEventCtx::card_uid → state.cards[idx]` lookup in
    /// `listener_permafrost` resolves.
    #[cfg(test)]
    fn permafrost_push_card(
        state: &mut CombatState,
        card_type: sts2_core::cards::CardType,
    ) -> sts2_core::CardUid {
        use std::num::NonZeroU32;
        use sts2_core::cards::{Card, CardId, TargetType};
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
            target_type: TargetType::Self_,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            combat_vars: sts2_core::cards::CombatVars::new(),
            enchantments: sts2_core::cards::CardEnchantments::new(),
            tags: sts2_core::cards::CardTags::NONE,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        uid
    }

    #[cfg(test)]
    fn permafrost_push_relic(state: &mut CombatState) {
        use std::num::NonZeroU32;
        let next = state
            .relics
            .iter()
            .map(|r| r.id.get())
            .max()
            .map_or(1, |m| m.saturating_add(1));
        state.relics.push(sts2_core::Relic {
            id: NonZeroU32::new(next).expect("non-zero"),
            kind: RelicKind::Permafrost,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
    }

    #[cfg(test)]
    fn permafrost_init_player(state: &mut CombatState) {
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
    }

    #[cfg(test)]
    fn permafrost_ctx_for(
        card_uid: sts2_core::CardUid,
        state: &CombatState,
    ) -> sts2_core::relic_effects::CardEventCtx {
        let idx = card_uid.0.get() as usize - 1;
        let card = &state.cards[idx];
        sts2_core::relic_effects::CardEventCtx {
            card_id: card.card_id,
            card_uid: Some(card_uid),
            target: None,
            card_tags: card.tags,
            energy_spent: 0,
        }
    }

    /// **Permafrost #1** — happy path: first Power played grants the
    /// `PERMAFROST_BLOCK` (7) of block and sets the latch.
    #[test]
    fn listener_permafrost_grants_block_on_first_power() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        permafrost_init_player(&mut state);
        permafrost_push_relic(&mut state);
        let uid = permafrost_push_card(&mut state, CardType::Power);
        let ctx = permafrost_ctx_for(uid, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(state.player.block, PERMAFROST_BLOCK);
        let idx = state
            .relics
            .iter()
            .position(|r| r.kind == RelicKind::Permafrost)
            .expect("relic present");
        assert_eq!(state.relics[idx].counter, 1, "latch must be set");
    }

    /// Bead `sts2-rs-hnx1a`: C# Permafrost is `BlockVar(7m, Unpowered)`,
    /// so Frail / Dexterity must NOT fold into the 7-block grant.
    #[test]
    fn listener_permafrost_ignores_frail_per_csharp_unpowered_prop() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::dispatch_after_card_played;
        use sts2_core::{Power, PowerKind, PowerUid};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        permafrost_init_player(&mut state);
        permafrost_push_relic(&mut state);
        // Install Frail(2) on the player mirror.
        let puid = PowerUid::new(1).expect("nz");
        state.powers.push(Power {
            id: puid.0,
            kind: PowerKind::Frail,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let player_uid = state.player.creature_uid.expect("set");
        state
            .creatures
            .iter_mut()
            .find(|c| c.id == player_uid.0)
            .expect("player creature row")
            .powers
            .push(puid);
        let uid = permafrost_push_card(&mut state, CardType::Power);
        let ctx = permafrost_ctx_for(uid, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(
            state.player.block, PERMAFROST_BLOCK,
            "Frail does not fold the Permafrost grant (would be 5 if it folded)"
        );
    }

    /// **Permafrost #2** — second Power play in the same combat is a
    /// no-op (latch held).
    #[test]
    fn listener_permafrost_is_noop_on_second_power_same_combat() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        permafrost_init_player(&mut state);
        permafrost_push_relic(&mut state);
        let uid1 = permafrost_push_card(&mut state, CardType::Power);
        let ctx1 = permafrost_ctx_for(uid1, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx1);
        let uid2 = permafrost_push_card(&mut state, CardType::Power);
        let ctx2 = permafrost_ctx_for(uid2, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx2);
        assert_eq!(state.player.block, PERMAFROST_BLOCK, "no second grant");
    }

    /// **Permafrost #3** — non-Power cards (Attack / Skill) never
    /// trigger, even when the latch is armed.
    #[test]
    fn listener_permafrost_is_noop_on_non_power_card() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        permafrost_init_player(&mut state);
        permafrost_push_relic(&mut state);
        for ct in [CardType::Attack, CardType::Skill] {
            let uid = permafrost_push_card(&mut state, ct);
            let ctx = permafrost_ctx_for(uid, &state);
            let _ = dispatch_after_card_played(&mut state, &ctx);
        }
        assert_eq!(state.player.block, 0);
        let idx = state
            .relics
            .iter()
            .position(|r| r.kind == RelicKind::Permafrost)
            .expect("relic present");
        assert_eq!(state.relics[idx].counter, 0, "latch stays armed");
    }

    /// **Permafrost #4** — no-op when the relic is not owned.
    #[test]
    fn listener_permafrost_is_noop_without_ownership() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        permafrost_init_player(&mut state);
        let uid = permafrost_push_card(&mut state, CardType::Power);
        let ctx = permafrost_ctx_for(uid, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(state.player.block, 0);
    }

    /// **Permafrost #5** — the `before_combat_start` listener clears
    /// the latch so the next combat re-arms. End-to-end via the
    /// existing dispatch bus.
    #[test]
    fn listener_permafrost_reset_clears_latch_between_combats() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::{dispatch_after_card_played, dispatch_before_combat_start};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        permafrost_init_player(&mut state);
        permafrost_push_relic(&mut state);
        // Combat 1: arm + fire.
        let uid1 = permafrost_push_card(&mut state, CardType::Power);
        let ctx1 = permafrost_ctx_for(uid1, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx1);
        assert_eq!(state.player.block, PERMAFROST_BLOCK);
        // Reset block (combat boundary in production code would also
        // zero this; we do it manually here so the second combat's
        // grant is observable in isolation).
        state.player.block = 0;
        // Combat 2: pre-combat dispatch clears the latch, then a
        // fresh Power grants block again.
        let _ = dispatch_before_combat_start(&mut state);
        let uid2 = permafrost_push_card(&mut state, CardType::Power);
        let ctx2 = permafrost_ctx_for(uid2, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx2);
        assert_eq!(state.player.block, PERMAFROST_BLOCK, "next combat re-arms");
    }

    /// **Permafrost block-gained dispatch** — pins the `final_block > 0`
    /// guard around `dispatch_after_block_gained`. With Juggernaut on the
    /// player and a live enemy, granting Permafrost's 7 block fires the
    /// block-gained bus, and Juggernaut's post-walk deals `amount` damage
    /// to the enemy. A `<`/`==` mutation of the `> 0` guard would skip the
    /// dispatch entirely (7 is neither `< 0` nor `== 0`), leaving the
    /// enemy unscathed.
    #[test]
    fn listener_permafrost_block_gain_fires_juggernaut_via_after_block_gained() {
        use sts2_core::cards::CardType;
        use sts2_core::relic_effects::dispatch_after_card_played;
        use sts2_core::{Creature, MonsterId, MonsterKind, Power, PowerKind, PowerUid};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        permafrost_init_player(&mut state);
        permafrost_push_relic(&mut state);

        // Install Juggernaut(5) on the player mirror + creature row.
        let puid = PowerUid::new(1).expect("nz");
        state.powers.push(Power {
            id: puid.0,
            kind: PowerKind::Juggernaut,
            amount: 5,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let player_uid = state.player.creature_uid.expect("set");
        state
            .creatures
            .iter_mut()
            .find(|c| c.id == player_uid.0)
            .expect("player creature row")
            .powers
            .push(puid);

        // Add a live enemy as the Juggernaut target.
        let enemy_uid = sts2_core::CreatureUid::new(100).expect("nz");
        state.creatures.push(Creature {
            id: std::num::NonZeroU32::new(100).expect("nz"),
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: sts2_core::IntentHistory::default(),
            monster_id: MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(enemy_uid);

        let uid = permafrost_push_card(&mut state, CardType::Power);
        let ctx = permafrost_ctx_for(uid, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx);

        assert_eq!(state.player.block, PERMAFROST_BLOCK, "block granted");
        let enemy_hp = state
            .creatures
            .iter()
            .find(|c| c.id == enemy_uid.0)
            .expect("enemy row")
            .current_hp;
        assert_eq!(
            enemy_hp, 45,
            "Juggernaut(5) dealt 5 via the after_block_gained dispatch"
        );
    }

    /// `pool_relic_ids()` returns the full `COMMON_RELIC_IDS` membership
    /// (non-empty, exact). Kills a `-> vec![]` body replacement.
    #[test]
    fn pool_relic_ids_returns_full_membership() {
        let ids = pool_relic_ids();
        assert_eq!(ids.len(), COMMON_RELIC_IDS.len());
        assert_eq!(ids, COMMON_RELIC_IDS.to_vec());
        assert!(ids.contains(&RelicId::Permafrost));
    }

    // -----------------------------------------------------------------
    // Deep tests — BookOfFiveRings (sts2-rs-sx27)
    //
    // Per-run counter on `RunState.relic_counters[BookOfFiveRings]`
    // increments on every `dispatch_on_card_added_to_deck` while
    // owned; every CARDS_THRESHOLD=5th add resets the counter to 0
    // and heals HEAL=15 (alive-gated).
    // -----------------------------------------------------------------

    fn book_ctx() -> sts2_core::run_effects::CardAddedRunCtx {
        sts2_core::run_effects::CardAddedRunCtx {
            card_id: sts2_core::cards::CardId::StrikeIronclad,
            card_type: sts2_core::cards::CardType::Attack,
            source: sts2_core::relic_effects::AddSource::Reward,
        }
    }

    /// Cards 1–4 increment the counter but don't heal yet.
    #[test]
    fn listener_book_of_five_rings_no_heal_below_threshold() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.current_hp = rs.max_hp - 50;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::BookOfFiveRings);
        let ctx = book_ctx();
        for _ in 0..(BOOK_OF_FIVE_RINGS_CARDS_THRESHOLD - 1) {
            let _ = sts2_core::run_effects::dispatch_on_card_added_to_deck(&mut rs, &ctx);
        }
        assert_eq!(rs.current_hp, before_hp);
        assert_eq!(
            rs.relic_counters.get(&RelicId::BookOfFiveRings),
            Some(&(BOOK_OF_FIVE_RINGS_CARDS_THRESHOLD - 1))
        );
    }

    /// The 5th card add triggers the heal and resets the counter.
    #[test]
    fn listener_book_of_five_rings_heals_at_threshold_and_resets() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.current_hp = rs.max_hp - 50;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::BookOfFiveRings);
        let ctx = book_ctx();
        for _ in 0..BOOK_OF_FIVE_RINGS_CARDS_THRESHOLD {
            let _ = sts2_core::run_effects::dispatch_on_card_added_to_deck(&mut rs, &ctx);
        }
        assert_eq!(rs.current_hp, before_hp + BOOK_OF_FIVE_RINGS_HEAL);
        assert_eq!(rs.relic_counters.get(&RelicId::BookOfFiveRings), Some(&0));
    }

    /// Alive-gate: dead player accrues counter resets but no heal.
    #[test]
    fn listener_book_of_five_rings_alive_gate_blocks_heal_when_dead() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.current_hp = 0;
        rs.relics.push(RelicId::BookOfFiveRings);
        let ctx = book_ctx();
        for _ in 0..BOOK_OF_FIVE_RINGS_CARDS_THRESHOLD {
            let _ = sts2_core::run_effects::dispatch_on_card_added_to_deck(&mut rs, &ctx);
        }
        assert_eq!(rs.current_hp, 0);
        assert_eq!(rs.relic_counters.get(&RelicId::BookOfFiveRings), Some(&0));
    }

    /// No-op when the relic is not owned: counter must not be created.
    #[test]
    fn listener_book_of_five_rings_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.current_hp = rs.max_hp - 50;
        let before_hp = rs.current_hp;
        let ctx = book_ctx();
        for _ in 0..BOOK_OF_FIVE_RINGS_CARDS_THRESHOLD {
            let _ = sts2_core::run_effects::dispatch_on_card_added_to_deck(&mut rs, &ctx);
        }
        assert_eq!(rs.current_hp, before_hp);
        assert!(!rs.relic_counters.contains_key(&RelicId::BookOfFiveRings));
    }

    // -----------------------------------------------------------------
    // Deep tests — TinyMailbox (sts2-rs-7z65 / sts2-rs-RP23)
    //
    // `modify_rest_site_heal_rewards` listener appends TWO
    // `Reward::Potion(PotionReward::empty())` when owned; no-op otherwise.
    // -----------------------------------------------------------------

    /// When TinyMailbox is owned, dispatching the rest-site rewards channel
    /// appends exactly TWO `Reward::Potion(empty)` (C# `TinyMailbox.cs:20-21`
    /// adds two; the prior single-add followed the stale Python pin).
    #[test]
    fn listener_tiny_mailbox_appends_two_potion_rewards_when_owned() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::TinyMailbox);
        let mut rewards: Vec<Reward> = Vec::new();
        let _ =
            sts2_core::run_effects::dispatch_modify_rest_site_heal_rewards(&mut rs, &mut rewards);
        assert_eq!(rewards.len(), 2);
        assert!(rewards.iter().all(|r| matches!(r, Reward::Potion(_))));
    }

    /// No-op when the relic is not owned: rewards list untouched.
    #[test]
    fn listener_tiny_mailbox_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut rewards: Vec<Reward> = Vec::new();
        let _ =
            sts2_core::run_effects::dispatch_modify_rest_site_heal_rewards(&mut rs, &mut rewards);
        assert!(rewards.is_empty());
    }

    /// Bead `sts2-rs-0463` / `sts2-rs-jakq1`: AmethystAubergine appends
    /// a `Reward::Gold` of `AMETHYST_AUBERGINE_GOLD = 15` (per C#
    /// `AmethystAubergine.cs:18` `GoldVar(15)`) on Monster-room clears.
    #[test]
    fn listener_amethyst_aubergine_appends_ten_gold_on_monster_rooms() {
        use sts2_core::run_effects::dispatch_modify_rewards;

        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::AmethystAubergine);
        let mut rewards: Vec<Reward> = Vec::new();
        let _ = dispatch_modify_rewards(&mut rs, &mut rewards, RoomType::Monster);
        assert_eq!(rewards.len(), 1);
        assert_eq!(
            rewards[0],
            Reward::Gold {
                amount: AMETHYST_AUBERGINE_GOLD,
            }
        );
    }

    /// Bead `sts2-rs-0463`: AmethystAubergine also fires on Elite rooms
    /// (Python "non-boss combat rooms" covers both MONSTER and ELITE).
    #[test]
    fn listener_amethyst_aubergine_appends_ten_gold_on_elite_rooms() {
        use sts2_core::run_effects::dispatch_modify_rewards;

        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::AmethystAubergine);
        let mut rewards: Vec<Reward> = Vec::new();
        let _ = dispatch_modify_rewards(&mut rs, &mut rewards, RoomType::Elite);
        assert_eq!(rewards.len(), 1);
        assert_eq!(
            rewards[0],
            Reward::Gold {
                amount: AMETHYST_AUBERGINE_GOLD,
            }
        );
    }

    /// Bead `sts2-rs-0463`: AmethystAubergine is silent on Boss (the
    /// Bead `sts2-rs-RP18`: AmethystAubergine is silent on every NON-combat room
    /// type (C# gate `room.RoomType.IsCombatRoom()`).
    #[test]
    fn listener_amethyst_aubergine_is_noop_on_non_combat_rooms() {
        use sts2_core::run_effects::dispatch_modify_rewards;

        crate::relics::dispatch::ensure_installed();
        for rt in [
            RoomType::Shop,
            RoomType::Treasure,
            RoomType::Event,
            RoomType::RestSite,
            RoomType::Unknown,
        ] {
            let mut rs = RunState::new();
            rs.relics.push(RelicId::AmethystAubergine);
            let mut rewards: Vec<Reward> = Vec::new();
            let _ = dispatch_modify_rewards(&mut rs, &mut rewards, rt);
            assert!(
                rewards.is_empty(),
                "AmethystAubergine must be silent on {rt:?} rooms"
            );
        }
    }

    /// Bead `sts2-rs-RP18`: AmethystAubergine grants +15 on a NON-final-act boss
    /// but NOT on the final-act boss (C# `room.RoomType == Boss &&
    /// CurrentActIndex >= Acts.Count - 1` exclusion). IC:1009 act-1 boss @398.
    #[test]
    fn listener_amethyst_aubergine_boss_gold_only_before_final_act() {
        use sts2_core::run_effects::dispatch_modify_rewards;

        crate::relics::dispatch::ensure_installed();

        // Act 1 boss (not the last act) ⇒ +15.
        let mut rs = RunState::new();
        rs.act_index = 1;
        assert!(!rs.is_last_act(), "act 1 is not the final act");
        rs.relics.push(RelicId::AmethystAubergine);
        let mut rewards: Vec<Reward> = Vec::new();
        let _ = dispatch_modify_rewards(&mut rs, &mut rewards, RoomType::Boss);
        assert_eq!(
            rewards,
            vec![Reward::Gold {
                amount: AMETHYST_AUBERGINE_GOLD
            }],
            "non-final-act boss grants the bonus"
        );

        // Final-act boss ⇒ no bonus.
        let mut rs = RunState::new();
        rs.act_index = 3;
        assert!(rs.is_last_act(), "act 3 is the final act");
        rs.relics.push(RelicId::AmethystAubergine);
        let mut rewards: Vec<Reward> = Vec::new();
        let _ = dispatch_modify_rewards(&mut rs, &mut rewards, RoomType::Boss);
        assert!(rewards.is_empty(), "final-act boss is excluded");
    }

    /// Bead `sts2-rs-0463`: AmethystAubergine listener is a no-op when
    /// the player does not own the relic.
    #[test]
    fn listener_amethyst_aubergine_is_noop_without_ownership() {
        use sts2_core::run_effects::dispatch_modify_rewards;

        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut rewards: Vec<Reward> = Vec::new();
        let _ = dispatch_modify_rewards(&mut rs, &mut rewards, RoomType::Monster);
        assert!(rewards.is_empty());
    }

    // -----------------------------------------------------------------
    // FencingManual listener tests — bead `sts2-rs-6lh46`
    // -----------------------------------------------------------------
    //
    // Mirrors Python
    // `sts2-rl-agent@01c094d:sts2_env/relics/common.py::FencingManual`:
    // round-1 player-side `gain_forge(FORGE = 10)`. The forge primitive
    // (`combat_forge`) spawns a SovereignBlade in hand if none exists
    // and buffs base_damage by `amount`. Spec'd base_damage = 10
    // (`SovereignBlade.base_damage = 10`), so post-forge the lone
    // blade carries `10 + 10 = 20`.

    #[cfg(test)]
    fn fencing_manual_setup(state: &mut CombatState, equip: bool, round: u16) {
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
                kind: RelicKind::FencingManual,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    /// **FencingManual #1** — round-1 player-side forges 10:
    /// SovereignBlade spawns in hand with base_damage `10 + 10 = 20`.
    #[test]
    fn listener_fencing_manual_round_one_player_forges_ten() {
        use sts2_core::cards::CardId;
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        fencing_manual_setup(&mut state, true, 1);
        assert!(state.player.hand.is_empty());

        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);

        assert_eq!(state.player.hand.len(), 1, "Forge spawns a blade");
        let blade_uid = state.player.hand[0];
        let blade = &state.cards[blade_uid.0.get() as usize - 1];
        assert_eq!(blade.card_id, CardId::SovereignBlade);
        assert_eq!(blade.base_damage, 10 + FENCING_MANUAL_FORGE);
    }

    /// **FencingManual #2** — round 2 inert.
    #[test]
    fn listener_fencing_manual_round_two_no_forge() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        fencing_manual_setup(&mut state, true, 2);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert!(
            state.player.hand.is_empty(),
            "FencingManual only fires on round 1"
        );
    }

    /// **FencingManual #3** — enemy side on round 1 inert.
    #[test]
    fn listener_fencing_manual_enemy_side_no_forge() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        fencing_manual_setup(&mut state, true, 1);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Enemy);
        assert!(state.player.hand.is_empty());
    }

    /// **FencingManual #4** — no-op when relic not owned.
    #[test]
    fn listener_fencing_manual_no_forge_without_ownership() {
        use sts2_core::relic_effects::dispatch_after_side_turn_start;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        fencing_manual_setup(&mut state, false, 1);
        let _ = dispatch_after_side_turn_start(&mut state, Side::Player);
        assert!(state.player.hand.is_empty());
    }

    // -----------------------------------------------------------------
    // JuzuBracelet listener tests — bead `sts2-rs-78l88`
    // -----------------------------------------------------------------

    fn juzu_setup_candidate_set() -> std::collections::BTreeSet<RoomType> {
        [
            RoomType::Monster,
            RoomType::Shop,
            RoomType::RestSite,
            RoomType::Event,
            RoomType::Treasure,
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn listener_juzu_bracelet_drops_monster_before_act3_threshold() {
        use sts2_core::run_effects::dispatch_modify_unknown_map_point_room_types;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::JuzuBracelet);
        rs.total_floor = 10;

        let mut room_types = juzu_setup_candidate_set();
        let _ = dispatch_modify_unknown_map_point_room_types(&mut rs, &mut room_types);

        assert!(
            !room_types.contains(&RoomType::Monster),
            "Monster must be removed from `?`-room candidate set"
        );
        assert_eq!(room_types.len(), 4, "only Monster removed");
    }

    #[test]
    fn listener_juzu_bracelet_noop_at_or_past_threshold() {
        use sts2_core::run_effects::dispatch_modify_unknown_map_point_room_types;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::JuzuBracelet);
        rs.total_floor = 41;

        let mut room_types = juzu_setup_candidate_set();
        let _ = dispatch_modify_unknown_map_point_room_types(&mut rs, &mut room_types);

        assert!(
            room_types.contains(&RoomType::Monster),
            "at threshold the gate must be closed → Monster stays"
        );
    }

    #[test]
    fn listener_juzu_bracelet_noop_without_ownership() {
        use sts2_core::run_effects::dispatch_modify_unknown_map_point_room_types;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.total_floor = 5;

        let mut room_types = juzu_setup_candidate_set();
        let _ = dispatch_modify_unknown_map_point_room_types(&mut rs, &mut room_types);

        assert!(
            room_types.contains(&RoomType::Monster),
            "no JuzuBracelet → candidate set untouched"
        );
    }

    #[test]
    fn listener_juzu_bracelet_abstains_when_no_monster_in_set() {
        use sts2_core::run_effects::dispatch_modify_unknown_map_point_room_types;
        crate::relics::dispatch::ensure_installed();

        let mut rs = RunState::new();
        rs.relics.push(RelicId::JuzuBracelet);
        rs.total_floor = 0;

        let mut room_types: std::collections::BTreeSet<RoomType> =
            [RoomType::Shop, RoomType::Event].into_iter().collect();
        let original = room_types.clone();

        let _ = dispatch_modify_unknown_map_point_room_types(&mut rs, &mut room_types);

        assert_eq!(room_types, original, "no Monster to remove → set unchanged");
    }
}
