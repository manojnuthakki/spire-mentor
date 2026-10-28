//! `RunState` reward queue + `Room` / `Reward` POD set — bead `sts2-rs-62t`
//! (P7-surf).
//!
//! Mirrors the reward-flow surfaces in
//! `sts2-rl-agent@01c094d:sts2_env/run/run_state.py` and
//! `sts2-rl-agent@01c094d:sts2_env/run/reward_objects.py`. This bead
//! only lands the plain-old-data types + the `RunState.pending_rewards`
//! field. The three reward-modifier hook channels
//! (`modify_rewards`, `modify_rest_site_heal_rewards`,
//! `modify_card_reward_options_late`) live in
//! [`crate::relic_effects`] alongside the rest of the relic hook bus.
//!
//! Scope of this module:
//!
//! - [`RoomType`] — full variant set from
//!   `sts2_env/core/enums.py::RoomType` (`MONSTER`, `ELITE`, `BOSS`,
//!   `SHOP`, `REST_SITE`, `TREASURE`, `EVENT`).
//! - [`Room`] — minimal `{room_type}` POD (extend with fields as the
//!   downstream retrofit beads need them).
//! - [`CardReward`], [`RelicReward`], [`Reward`] — the three reward
//!   shapes the audit `DreamCatcher` / `FresnelLens` /
//!   `WongosMysteryTicket` chain depends on. Other Python `Reward`
//!   subclasses (`GoldReward`, `PotionReward`, etc.) are TODO and will
//!   be added by the umbrella retrofit bead `sts2-rs-01o`.
//! - [`RunState`] — minimal struct hosting `pending_rewards` (extend
//!   with deck / map / gold / hp once those surfaces land).
//!
//! `RoomType` replaces the placeholder `relic_effects::RoomKind` shim
//! from bead `sts2-rs-2ax`; `AfterRoomEnteredFn` now takes a
//! [`RoomType`] payload directly.

use std::collections::BTreeMap;

use rand_xoshiro::rand_core::{RngCore, SeedableRng};
use rand_xoshiro::Xoshiro256PlusPlus;

use crate::act_selection::ActVariant;
use crate::cards::{Card, CardId, EnchantmentId};
use crate::potions::CharacterId;
use crate::relics::RelicId;

// ---------------------------------------------------------------------------
// Room / RoomType
// ---------------------------------------------------------------------------

// ── Encounter gold-drop constants — bead `sts2-rs-gccz5` (P9.2.q) ──
//
// Mirrors Python
// `sts2-rl-agent@01c094d:sts2_env/run/reward_objects.py:874-881`
// (`ENCOUNTER_GOLD_REWARD_RANGES` + `POVERTY_ASCENSION_*` consts).
// Consumed by [`RunState::append_encounter_gold_drop`].

/// Monster-room gold drop lower bound (Python `(10, 20)`).
pub const ENCOUNTER_GOLD_LOW_MONSTER: i32 = 10;
/// Monster-room gold drop upper bound.
pub const ENCOUNTER_GOLD_HIGH_MONSTER: i32 = 20;
/// Elite-room gold drop lower bound (Python `(35, 45)`).
pub const ENCOUNTER_GOLD_LOW_ELITE: i32 = 35;
/// Elite-room gold drop upper bound.
pub const ENCOUNTER_GOLD_HIGH_ELITE: i32 = 45;
/// Boss-room gold drop lower bound (Python `(100, 100)` — fixed).
pub const ENCOUNTER_GOLD_LOW_BOSS: i32 = 100;
/// Boss-room gold drop upper bound.
pub const ENCOUNTER_GOLD_HIGH_BOSS: i32 = 100;

/// Stream-name salt for the Neow event's per-event RNG. Mirrors C#
/// `EventModel.cs:193` which mixes
/// `deterministic_hash_code(base.Id.Entry)` into the run-master seed;
/// `Id.Entry` for the Neow event resolves to the class name "Neow"
/// in v0.103.2. Parity sub-bead `sts2-rs-k8z6m` re-confirms against
/// the scraped corpus; if it diverges the name moves here.
pub const NEOW_EVENT_RNG_NAME: &str = "NEOW";
/// Ascension level at which the Poverty multiplier kicks in
/// (Python `POVERTY_ASCENSION_LEVEL = 3`).
pub const POVERTY_ASCENSION_LEVEL: u8 = 3;
/// Multiplier applied to encounter gold drops at ascension >= 3
/// (Python `POVERTY_ASCENSION_GOLD_MULTIPLIER = 0.75`).
pub const POVERTY_ASCENSION_GOLD_MULTIPLIER: f64 = 0.75;
/// Ascension level at which `WearyTraveler` reduces the Ancient-event
/// entry heal to 80%. Mirrors C# `AscensionLevel.WearyTraveler` (enum
/// discriminant 2) —
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Entities.Ascension/AscensionLevel.cs`.
pub const WEARY_TRAVELER_ASCENSION_LEVEL: u8 = 2;
/// Fraction of the missing HP recovered by an Ancient-event entry heal
/// at ascension >= 2. Mirrors C# `amount *= 0.8m` in
/// `AncientEventModel.BeforeEventStarted`.
pub const WEARY_TRAVELER_HEAL_MULTIPLIER: f64 = 0.8;

/// Room-category enum. One-to-one with
/// `sts2-rl-agent@01c094d:sts2_env/core/enums.py::RoomType`.
///
/// Python variant order is preserved so future parity exporters can
/// emit the enum index directly. `Unknown` is a Rust-side catch-all
/// for engine bring-up paths that don't yet know which room a relic
/// listener is firing inside (e.g. unit tests that exercise
/// `after_room_entered` without a `RunState`); it has no Python
/// counterpart.
/// Relic-rarity bucket key for the relic-reward grab-bag. Mirrors
/// the four buckets Python uses to partition the relic registry for
/// reward selection (`run_state.py:248-251`). Starter / Event / Boss
/// / Ancient relics never enter the bucketed pool — they're filtered
/// out by the content-side sampler before the buckets are built.
///
/// Sts2-core hosts this enum (rather than re-exporting from
/// `sts2-content`'s richer `RelicRarity`) because the workspace dep
/// direction is `content → core`. Bead `sts2-rs-l9vkh` (P9.2.r).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RelicRarity {
    /// `RelicRarity.COMMON` — base-tier reward relic.
    Common,
    /// `RelicRarity.UNCOMMON` — mid-tier reward relic.
    Uncommon,
    /// `RelicRarity.RARE` — top-tier reward relic.
    Rare,
    /// `RelicRarity.SHOP` — shop-only relic.
    Shop,
}

/// Roll a relic-reward rarity bucket off `rng` — 50% Common, 33% Uncommon,
/// 17% Rare. Mirrors Python `Player._roll_relic_rarity` / C#
/// `RelicFactory.RollRarity`.
///
/// Free-standing so a caller holding a *copy* of a stream can roll without
/// touching the run's: the Relic Trader has to know all three relics it will
/// offer in order to name them in its option text, and building an option list
/// must not advance the rewards RNG.
#[must_use]
pub fn roll_relic_rarity(rng: &mut crate::dotnet_rng::Rng) -> RelicRarity {
    let roll = rng.next_float(1.0);
    if roll < 0.50 {
        RelicRarity::Common
    } else if roll < 0.83 {
        RelicRarity::Uncommon
    } else {
        RelicRarity::Rare
    }
}

impl RelicRarity {
    /// Iteration order for grab-bag flattening: Common → Uncommon →
    /// Rare → Shop. Mirrors Python's `allowed_rarities` tuple ordering
    /// in `populate_relic_grab_bag` (`run_state.py:248-251`).
    pub const ALL: [RelicRarity; 4] = [
        RelicRarity::Common,
        RelicRarity::Uncommon,
        RelicRarity::Rare,
        RelicRarity::Shop,
    ];

    /// Python `_relic_grab_bag_order(rarity)` — fallback chain when
    /// the rolled bucket is empty (or all-owned). Mirrors
    /// `run_state.py:284-292`:
    /// - `Shop` → `[Shop, Common, Uncommon, Rare]`
    /// - `Common` → `[Common, Uncommon, Rare]`
    /// - `Uncommon` → `[Uncommon, Rare]`
    /// - `Rare` → `[Rare]`
    #[must_use]
    pub fn fallback_order(self) -> &'static [RelicRarity] {
        match self {
            RelicRarity::Shop => &[
                RelicRarity::Shop,
                RelicRarity::Common,
                RelicRarity::Uncommon,
                RelicRarity::Rare,
            ],
            RelicRarity::Common => &[
                RelicRarity::Common,
                RelicRarity::Uncommon,
                RelicRarity::Rare,
            ],
            RelicRarity::Uncommon => &[RelicRarity::Uncommon, RelicRarity::Rare],
            RelicRarity::Rare => &[RelicRarity::Rare],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RoomType {
    /// Engine-side catch-all when a caller doesn't yet have a real
    /// room category. No Python equivalent.
    Unknown,
    /// `RoomType.MONSTER` — normal-encounter combat room.
    Monster,
    /// `RoomType.ELITE` — elite-encounter combat room.
    Elite,
    /// `RoomType.BOSS` — boss-encounter combat room.
    Boss,
    /// `RoomType.SHOP` — merchant room.
    Shop,
    /// `RoomType.REST_SITE` — rest-site / campfire room.
    RestSite,
    /// `RoomType.TREASURE` — chest / treasure room.
    Treasure,
    /// `RoomType.EVENT` — random-event room.
    Event,
}

/// Minimal `Room` POD. Python's `Room` carries spawn data and an
/// `entered` flag that downstream retrofit beads will add as needed.
///
/// The optional `map_coord` + `map_point_type` fields (bead
/// `sts2-rs-2mjei`) thread the originating map node through into
/// [`RunState::enter_room`] so the map-traversal trackers
/// (`visited_map_coords`, `act_floor`, `total_floor`,
/// `map_point_history` — bead `sts2-rs-2c0ze`) can be updated
/// automatically. Callers that don't have a map context yet
/// (combat-only scaffolding, parity fixtures, boss rooms not driven
/// by the act-map) construct via [`Self::new`] and leave both
/// `None` — `enter_room` then skips the visit-registration branch
/// and preserves the original "set current_room + fire
/// `after_room_entered`" behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Room {
    /// Category of this room.
    pub room_type: RoomType,
    /// Originating map coordinate, if the room came from a map node.
    /// Bead `sts2-rs-2mjei`.
    pub map_coord: Option<crate::map::MapCoord>,
    /// Originating map-node type, if the room came from a map node.
    /// Distinguishes Monster-via-`M` from Monster-via-`?` for the
    /// `map_point_history` audit trail. Bead `sts2-rs-2mjei`.
    pub map_point_type: Option<crate::map::MapPointType>,
}

impl Room {
    /// Construct a minimal `Room` of the given type with no map
    /// context attached. [`RunState::enter_room`] skips the
    /// visit-registration branch for these rooms.
    #[must_use]
    pub const fn new(room_type: RoomType) -> Self {
        Self {
            room_type,
            map_coord: None,
            map_point_type: None,
        }
    }

    /// Construct a `Room` originating from a specific map node.
    /// [`RunState::enter_room`] will route through
    /// [`RunState::add_visited_coord`] with `(coord, Some(mpt),
    /// Some(room_type))` — bumping `act_floor`, `total_floor`,
    /// appending to `visited_map_coords`, and logging a
    /// `MapPointHistoryEntry`. Bead `sts2-rs-2mjei`.
    #[must_use]
    pub const fn at(
        coord: crate::map::MapCoord,
        map_point_type: crate::map::MapPointType,
        room_type: RoomType,
    ) -> Self {
        Self {
            room_type,
            map_coord: Some(coord),
            map_point_type: Some(map_point_type),
        }
    }
}

// ---------------------------------------------------------------------------
// Reward POD set
// ---------------------------------------------------------------------------

/// `CardReward` POD — mirrors
/// `sts2-rl-agent@01c094d:sts2_env/run/reward_objects.py::CardReward`.
///
/// Only `options` (the list of `CardId`s presented to the player) is
/// landed today. The umbrella retrofit (`sts2-rs-01o`) will extend
/// with `rarity`, `picked`, `card_reward_generation_options`, etc.
/// as the rewards-flow surfaces land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardReward {
    /// Card-id choices on offer for this reward. Index-keyed; the
    /// `modify_card_reward_options_late` hook bus mutates this vector
    /// in place.
    pub options: Vec<CardId>,
}

impl CardReward {
    /// Construct an empty `CardReward` (no options).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            options: Vec::new(),
        }
    }

    /// Construct a `CardReward` from the given option list.
    #[must_use]
    pub fn from_options(options: Vec<CardId>) -> Self {
        Self { options }
    }
}

/// A pending "choose 1 of N cards (or skip)" screen raised by a run-side
/// effect (C# `CardSelectCmd.FromChooseACardScreen(canSkip: true)`), queued
/// on [`RunState::pending_card_selects`] for the run loop to surface as an
/// interactive `card_select`.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingCardSelect {
    /// The cards on offer (pick at most one).
    pub options: Vec<CardId>,
    /// Per-option upgrade flags, parallel to [`Self::options`]. The chosen
    /// card is granted upgraded when its flag is set. Models C#
    /// card-reward-modify relics that pre-upgrade an offered slate — most
    /// notably `SilverCrucible.TryModifyCardRewardOptionsLate`
    /// (`SilverCrucible.cs:94-115`), which clones+Upgrades every upgradable
    /// offered card while its 3-reward budget lasts. Defaults to all-false
    /// (no offered upgrade) for callers that surface bare ids without a
    /// `modify_card_reward_options_late` pass (HeftyTablet's choose screen,
    /// the shop-event 1-of-N). Must stay `options.len()`-long.
    pub upgraded: Vec<bool>,
    /// Cards appended to the master deck AFTER the screen resolves, in
    /// order, *after* the chosen card (if any). Models C# `HeftyTablet`,
    /// which builds `[chosen?, Injury]` and adds them in ONE
    /// `CardPileCmd.Add(list, Deck)` only once the select returns — so the
    /// deck-tail order (load-bearing for the combat-start shuffle input) is
    /// chosen-then-Injury, and the Injury still lands on a skip.
    pub post_add: Vec<Card>,
    /// Whether the player may take none (C# `canSkip` /
    /// `CardSelectorPrefs.Cancelable`). `true` for the choose-a-card screens;
    /// `false` for a pick the game forces, Brain Leech's Share Knowledge grid
    /// (`BrainLeech.cs:67-70`, `Cancelable = false`). sts2-agent fork.
    pub can_skip: bool,
}

impl PendingCardSelect {
    /// A select with options only (nothing added after resolution); no
    /// offered card is upgraded.
    #[must_use]
    pub fn from_options(options: Vec<CardId>) -> Self {
        let upgraded = vec![false; options.len()];
        Self {
            options,
            upgraded,
            post_add: Vec::new(),
            can_skip: true,
        }
    }

    /// A skippable select over bare ids with cards deferred to AFTER the
    /// screen resolves (C# `HeftyTablet` `[chosen?, Injury]` tail). No
    /// offered card is upgraded.
    #[must_use]
    pub fn with_post_add(options: Vec<CardId>, post_add: Vec<Card>) -> Self {
        let upgraded = vec![false; options.len()];
        Self {
            options,
            upgraded,
            post_add,
            can_skip: true,
        }
    }

    /// A select built from fully-rolled card PODs (e.g. a
    /// `CardFactory.CreateForReward` slate that has been run through
    /// [`RunState::offer_card_reward`], so per-card upgrade state reflects
    /// SilverCrucible et al.). Carries each card's `.upgraded` flag through
    /// to the grant so the picked card lands with the offered upgrade state.
    /// Nothing is added after resolution.
    #[must_use]
    pub fn from_cards(cards: &[Card]) -> Self {
        let options = cards.iter().map(|c| c.card_id).collect();
        let upgraded = cards.iter().map(|c| c.upgraded).collect();
        Self {
            options,
            upgraded,
            post_add: Vec::new(),
            can_skip: true,
        }
    }
}

/// A pending "choose EXACTLY `pick` of N generated cards, add the chosen ones
/// to the deck" screen raised by a run-side effect — C#
/// `CardSelectCmd.FromSimpleGridForRewards(prefs(prompt, pick))` over a list of
/// freshly-generated `CardCreationResult`s, each chosen card then
/// `CardPileCmd.Add(card, PileType.Deck)`. Distinct from
/// [`PendingCardSelect`] (a skippable 1-of-N over bare `CardId`s routed through
/// the card-reward screen): this is a MANDATORY multi-pick over fully-built
/// `Card` PODs (carrying their rolled upgrade state) that lands the picks on
/// the master deck. Queued on [`RunState::pending_card_grants`]; the run loop
/// surfaces it as a `card_select` (min == max == `pick`, no skip) that preempts
/// the phase, mirroring the `pending_deck_transforms` / `_enchants` deck-op
/// selects. Used by `RoomFullOfCheese.Gorge` (pick 2 of 8 Common cards). Bead
/// `sts2-rs-gorge-select`.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingCardGrant {
    /// The freshly-generated cards on offer (in C# generation order).
    pub options: Vec<Card>,
    /// How many of `options` the player must pick (C# `prefs.MinSelect ==
    /// MaxSelect`). Each picked card is appended to the master deck; the
    /// unpicked remainder is discarded once `pick` reaches 0.
    pub pick: usize,
}

/// `CardRewardGenerationOptions` POD — mirrors
/// `sts2-rl-agent@01c094d:sts2_env/run/rewards.py::CardRewardGenerationOptions`.
///
/// Held by the card-reward generator (Python `generate_card_reward`)
/// and mutated by the `modify_card_reward_creation_options` relic hook
/// before the card pool is rolled (bead `sts2-rs-tu7g5`). Listeners
/// (DingyRug → `include_colorless = true`; PrismaticGem → widen
/// `character_ids`; SilverCrucible pre-upgrade leg →
/// `roll_upgrade = false`) flip these fields to reshape the eventual
/// `CardReward.options` slate.
///
/// Field set tracks the Python `@dataclass(frozen=True)` as of the pin;
/// the rarity-typed fields (`forced_rarities`,
/// `card_pool_rarity_filter`) are deferred until [`CardRarity`] moves
/// from `sts2-content` into `sts2-core` (it isn't reachable from this
/// crate today without a dependency cycle). Add them in a follow-up
/// once a content-side relic listener actually needs them.
///
/// Defaults match the Python `@dataclass` defaults so call sites can
/// rely on `CardRewardGenerationOptions::default()` for the "regular"
/// post-combat card-reward roll.
///
/// [`CardRarity`]: ../../sts2_content/cards/types/enum.CardRarity.html
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardRewardGenerationOptions {
    /// Python `context` — free-form discriminator ("regular", "shop",
    /// "event", …) carried for downstream filters. Defaults to
    /// `"regular"`.
    pub context: &'static str,
    /// Python `num_cards` — how many card-options to roll. Default 3
    /// (Python `DEFAULT_CARD_REWARD_OPTION_COUNT`).
    pub num_cards: u8,
    /// Python `character_ids` — explicit character pool override. Empty
    /// means "fall back to the run's default character pool when
    /// `use_default_character_pool` is true". PrismaticGem fills this
    /// with all six character IDs to widen the colour pool.
    pub character_ids: Vec<CharacterId>,
    /// Python `include_colorless` — when true, the colorless card pool
    /// is unioned into the candidates. DingyRug sets this.
    pub include_colorless: bool,
    /// Python `use_default_character_pool` — when true, an empty
    /// `character_ids` falls back to `(default_character_id,)`.
    pub use_default_character_pool: bool,
    /// Python `roll_upgrade` — when false, suppresses the per-card
    /// upgrade roll. SilverCrucible's rewards leg flips this to false
    /// for the first 3 card rewards of the run.
    pub roll_upgrade: bool,
    /// Python `card_creation_source` — provenance tag for the listener
    /// chain (`"encounter"`, `"event"`, `"shop"`). Default
    /// `"encounter"` matches Python `CARD_CREATION_SOURCE_ENCOUNTER`.
    pub card_creation_source: &'static str,
    /// Python `allow_card_pool_modifications` — when false, the
    /// per-relic `modify_card_pool` walk is skipped. Default true.
    pub allow_card_pool_modifications: bool,
    /// Python `allow_rarity_modifications` — when false, the per-relic
    /// `modify_card_rarity` walk is skipped. Default true.
    pub allow_rarity_modifications: bool,
    /// Python `allow_hook_upgrades` — when false, the per-relic
    /// `modify_card_being_added_to_deck` upgrade injectors are
    /// suppressed. Default true.
    pub allow_hook_upgrades: bool,
    /// Python `has_custom_card_pool` — when true, the generator reads
    /// `custom_card_ids` instead of rolling from the character pool.
    pub has_custom_card_pool: bool,
    /// Python `custom_card_ids` — explicit card-id slate consumed when
    /// `has_custom_card_pool` is true.
    pub custom_card_ids: Vec<CardId>,
}

impl Default for CardRewardGenerationOptions {
    fn default() -> Self {
        Self {
            context: "regular",
            // `DEFAULT_CARD_REWARD_OPTION_COUNT` in
            // `sts2_env/run/rewards.py` resolves to 3 at pin `01c094d`.
            num_cards: 3,
            character_ids: Vec::new(),
            include_colorless: false,
            use_default_character_pool: true,
            roll_upgrade: true,
            card_creation_source: "encounter",
            allow_card_pool_modifications: true,
            allow_rarity_modifications: true,
            allow_hook_upgrades: true,
            has_custom_card_pool: false,
            custom_card_ids: Vec::new(),
        }
    }
}

impl CardRewardGenerationOptions {
    /// Python `CardRewardGenerationOptions.rarity_modifications_allowed`
    /// — the per-relic rarity-modification walk is skipped both when
    /// the caller opts out and when the rarity slate is forced. Today
    /// the forced-rarity branch is identity-true because the
    /// `forced_rarities` field hasn't landed yet (see struct doc).
    #[must_use]
    pub fn rarity_modifications_allowed(&self) -> bool {
        self.allow_rarity_modifications
    }
}

/// `RelicReward` POD — mirrors
/// `sts2-rl-agent@01c094d:sts2_env/run/reward_objects.py::RelicReward`.
///
/// `WongosMysteryTicket` pushes one of these per relic on its 5-combat
/// trigger (see audit doc row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelicReward {
    /// The specific relic on offer.
    pub relic_id: RelicId,
    /// `ToyBox` stamps `IsWax = true` on the 4 relic *models* it offers
    /// (`MegaCrit.Sts2.Core.Models.Relics/ToyBox.cs:90-95`) before wrapping
    /// them in `RelicReward`s, so the flag rides the OFFER: skip the reward
    /// and the relic is never wax. Recorded into
    /// [`RunState::wax_relics`] only when the reward is actually taken.
    pub is_wax: bool,
}

impl RelicReward {
    /// Construct a `RelicReward` for `relic_id`.
    #[must_use]
    pub const fn new(relic_id: RelicId) -> Self {
        Self {
            relic_id,
            is_wax: false,
        }
    }

    /// Construct a wax (`ToyBox`-offered, meltable) `RelicReward`.
    #[must_use]
    pub const fn new_wax(relic_id: RelicId) -> Self {
        Self {
            relic_id,
            is_wax: true,
        }
    }
}

/// `PotionReward` POD — mirrors
/// `sts2-rl-agent@01c094d:sts2_env/run/reward_objects.py::PotionReward`.
///
/// Python's `PotionReward(player_id, potion_id=None)` rolls the
/// concrete potion in the dispatcher at reward-drain time; the
/// constructor only carries the `player_id`, with `potion_id`
/// optionally pre-set. The Rust port mirrors that contract: the POD
/// is empty by default (the dispatcher fills in `potion_id` when the
/// rarity / RNG surface lands), but pre-rolled rewards can supply an
/// explicit `potion_id` via [`Self::with_id`]. Bead `sts2-rs-ib04`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PotionReward {
    /// Optional pre-rolled potion kind. `None` means "the dispatcher
    /// rolls one at drain time" (matches the unspecified-id Python
    /// shape used by every event/relic that just pushes a potion
    /// reward without naming a specific potion). The Rust port uses
    /// `PotionKind` rather than a separate `PotionId` because the
    /// engine identifies potions by `PotionKind` end-to-end (no Python
    /// `PotionId.<NAME>` indirection lands today).
    pub potion_kind: Option<crate::potions::PotionKind>,
}

impl PotionReward {
    /// Construct an empty `PotionReward` (dispatcher rolls the potion
    /// at reward-drain time). Equivalent to Python
    /// `PotionReward(player_id)` with no explicit `potion_id`.
    #[must_use]
    pub const fn empty() -> Self {
        Self { potion_kind: None }
    }

    /// Construct a `PotionReward` with a pre-rolled `potion_kind`.
    /// Equivalent to Python `PotionReward(player_id, potion_id=...)`.
    #[must_use]
    pub const fn with_kind(potion_kind: crate::potions::PotionKind) -> Self {
        Self {
            potion_kind: Some(potion_kind),
        }
    }
}

/// `UpgradeCardsReward` POD — mirrors
/// `sts2-rl-agent@01c094d:sts2_env/run/reward_objects.py::UpgradeCardsReward`.
///
/// Pushed by `RunState::offer_upgrade_cards_reward` (Python pin
/// `run_state.py:1095`); consumed by `WarHammer` (post-combat upgrade
/// reward, `shop_event.py::WarHammer`) and a couple of event-room
/// retrofits (`shop_event.py:2924` / `:2927`). The player chooses
/// `count` cards out of `options`; the resolution path will land
/// alongside the run-side `PendingChoice` analogue. Bead
/// `sts2-rs-70anj.3.14`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpgradeCardsReward {
    /// Number of cards the player upgrades from `options`.
    pub count: usize,
    /// Eligible upgrade pool — `CardId`s pulled from the master deck
    /// that are non-upgraded and have an upgradeable variant. Python
    /// passes `CardInstance`s, but the Rust port carries `CardId`
    /// because the master_deck holds the canonical `Card` POD already.
    pub options: Vec<CardId>,
}

impl UpgradeCardsReward {
    /// Construct an `UpgradeCardsReward` for `count` picks from
    /// `options`.
    #[must_use]
    pub const fn new(count: usize, options: Vec<CardId>) -> Self {
        Self { count, options }
    }
}

/// `EnchantCardsReward` POD — mirrors
/// `sts2-rl-agent@01c094d:sts2_env/run/reward_objects.py::EnchantCardsReward`
/// (lines 821-872).
///
/// Pushed by [`RunState::offer_enchant_cards_reward`] (Python pin
/// `run_state.py:1127-1146`); consumed by the deferred-reward arm of
/// ~8 shop_event relic listeners (GnarledHammer/Sharp,
/// MagicFlower/Adroit, Momentum-grant, RoyallyApproved-grant,
/// Swift-grant, Imbued-grant, Clone-grant, Instinct-grant). The
/// player picks up to `count` cards out of `options` to receive
/// `amount` stacks of `enchantment`; `min_count` (if set) lets the
/// player pick fewer than `count` but at least `min_count`. The
/// `after_selected` callback in Python is not ported at this pin —
/// the ~2 relics that use it are not yet body-filled in Rust. Bead
/// `sts2-rs-2v8y2`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnchantCardsReward {
    /// Enchantment to apply (typed; Python carries a string id).
    pub enchantment: EnchantmentId,
    /// Stacks of `enchantment` to apply to each picked card.
    pub amount: i32,
    /// Maximum number of cards the player may pick.
    pub count: usize,
    /// Eligible candidate pool. Python's `cards=None` semantics
    /// (default to full master deck) is resolved at the caller — the
    /// POD always carries an explicit list.
    pub options: Vec<CardId>,
    /// Optional lower bound; `None` means "exactly `count`".
    pub min_count: Option<usize>,
}

impl EnchantCardsReward {
    /// Construct an `EnchantCardsReward`.
    #[must_use]
    pub const fn new(
        enchantment: EnchantmentId,
        amount: i32,
        count: usize,
        options: Vec<CardId>,
        min_count: Option<usize>,
    ) -> Self {
        Self {
            enchantment,
            amount,
            count,
            options,
            min_count,
        }
    }
}

/// One queued interactive deck-transform pick (see
/// [`RunState::pending_deck_transforms`]). NewLeaf-style transforms queue
/// `{ basic_only: false, fixed_target: None }` (replacement rolled off
/// `Rng.Niche` only after the pick, via
/// `ContentCallbacks::roll_transform_replacement`); Wood Carvings'
/// Bird/Torus queue a Basic-only filter with a FIXED replacement (C#
/// `FromDeckGeneric(c => c.IsTransformable && c.Rarity == Basic)` then
/// `CardCmd.TransformTo<Peck/ToricToughness>` — no RNG read).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingDeckTransform {
    /// Restrict the pick pool to Basic-rarity deck cards (rarity resolved
    /// via the registered [`crate::lookup_card_rarity_rank`] fn; falls
    /// back to the whole deck when unregistered, e.g. raw-core tests).
    pub basic_only: bool,
    /// `Some(card)` — the C# `TransformTo<T>` fixed replacement, applied
    /// as remove-original + append-at-tail. `None` — roll via the content
    /// callback (NewLeaf shape).
    pub fixed_target: Option<Card>,
    /// `Some((entry_name, skip_reads))` — roll the replacement off the
    /// event's own non-shared stream (`Rng(uint(seed + NetId +
    /// hash(entry_name)))`, NetId = 1 in single-player per
    /// `EventModel.cs:193` / `NGame.cs:884`) instead of `Rng.Niche`,
    /// discarding `skip_reads` small-range reads first — C#
    /// `CalculateVars` rolls consume the stream head at event entry
    /// (WhisperingHollow Hug: gold roll = read #1, transform = read #2).
    /// Ignored when `fixed_target` is set.
    pub event_stream_roll: Option<(&'static str, u32)>,
    /// Unblockable HP loss applied after the transform resolves — C#
    /// `WhisperingHollow.Hug` fires `CreatureCmd.Damage(9, Unblockable)`
    /// only AFTER the picked card transforms. `0` = none.
    pub post_hp_loss: i32,
}

/// One queued interactive deck-enchant pick (see
/// [`RunState::pending_deck_enchants`]). The pick pool mirrors C#
/// `EnchantmentModel.CanEnchant` + `Slither.CanEnchant`: deck cards whose
/// type is not Status/Curse/Quest, with no existing enchantment, and a
/// concrete energy cost (`cost >= 0` excludes both Unplayable sentinels
/// and X-cost, which Slither's override rejects).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDeckEnchant {
    /// Enchantment to stamp on the picked card.
    pub enchantment: EnchantmentId,
    /// Stacks to apply (C# `CardCmd.Enchant<T>(card, 1m)` ⇒ 1).
    pub amount: i32,
    /// `Some(type)` — restrict the pick pool to deck cards of this
    /// `CardType` on top of the `CanEnchant` predicate. C#
    /// `SelfHelpBook.SelectAndEnchant` passes
    /// `c => c.Type == typeRestriction` into
    /// `CardSelectCmd.FromDeckForEnchantment` (Sharp→Attack,
    /// Nimble→Skill, Swift→Power). `None` — any enchantable card.
    pub type_restriction: Option<crate::cards::CardType>,
    /// A SECOND allowed `CardType`, OR-ed with [`Self::type_restriction`],
    /// for enchantments whose `can_enchant_card` gate accepts two types.
    /// C# `RoyallyApproved.CanEnchant` (RoyalStamp) admits Attack OR Skill,
    /// which a single `type_restriction` can't express. `None` for the
    /// single-type / unrestricted cases.
    pub secondary_type: Option<crate::cards::CardType>,
}

/// One reward of a set the game offers through C# `RewardsCmd.OfferCustom`
/// (an event's or a relic's rewards; sts2-agent fork). Under
/// [`RunState::interactive_rewards`] a producer queues these on
/// [`RunState::offered_reward_sets`]; the run manager rolls whatever is left
/// to roll when it opens the set (C# `RewardsSet.GenerateWithoutOffering`
/// populates every reward in list order at offer time) and shows the set on
/// its rewards screen. The same shape stages a won combat room's extra
/// rewards (C# `CombatRoom.AddExtraReward`, [`RunState::room_extra_rewards`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OfferedReward {
    /// `GoldReward(amount)`.
    Gold(i32),
    /// `PotionReward`: a named potion, or `None` to roll one (C#
    /// `PotionReward.Populate`, the `roll_potion_reward_kind` callback).
    Potion(Option<crate::potions::PotionKind>),
    /// `RelicReward(relic)`: a predetermined relic.
    Relic(RelicReward),
    /// `RelicReward(player)` / `RelicReward(rarity, player)`: rolled at offer
    /// time off the grab bag (the rarity on `Rewards` when `None`), as
    /// [`RunState::offer_random_relic_rewards`] does; `is_wax` stamps Toy
    /// Box's offers.
    RandomRelic {
        rarity: Option<RelicRarity>,
        is_wax: bool,
    },
    /// `CardReward`: pre-rolled options (with their offered upgrades, empty
    /// for none), or `None` to roll a non-combat reward at offer time.
    Card {
        options: Option<Vec<CardId>>,
        upgraded: Vec<bool>,
    },
    /// `CardReward(ForRoom(room))` added to a combat room (The Hunt): rolled
    /// with the room's own card-reward roller.
    RoomCard,
    /// `SpecialCardReward(card)`: this card, added to the deck when claimed.
    SpecialCard(Card),
    /// `CardRemovalReward`: one deck removal when claimed.
    RemoveCard,
}

/// A set of [`OfferedReward`]s offered together (one `RewardsCmd.OfferCustom`
/// call, or a `RewardsSet` a relic offers itself). `can_skip` is false for a
/// set built `WithSkippingDisallowed()` (Neow's Bones): every reward must be
/// claimed before the screen can be left. sts2-agent fork.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferedRewardSet {
    pub rewards: Vec<OfferedReward>,
    pub can_skip: bool,
}

/// `Reward` enum — the open variant set Python has as a class
/// hierarchy under `Reward`. This bead landed the four variants the
/// event-reward retrofit chain depends on (`Card`, `Relic`, `Gold`,
/// `Potion`). More variants (`AddCards`, `ObtainRelics`, `LoseHp`,
/// `LoseGold`, `RemoveCard`, `TransformCards`, `DuplicateCard`,
/// `EnchantCards`) will be added by the umbrella bead `sts2-rs-01o`
/// as each downstream relic body needs them. `UpgradeCards` landed
/// in bead `sts2-rs-70anj.3.14`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reward {
    /// Card-pick reward — the player chooses 1-of-N card-id options.
    Card(CardReward),
    /// Relic-pick reward — the player gains the listed relic.
    Relic(RelicReward),
    /// Gold reward — `amount` gold credited to the player.
    Gold {
        /// Gold to grant.
        amount: i32,
    },
    /// Potion-pick reward — the dispatcher rolls a concrete potion at
    /// reward-drain time, or `Self::Potion(PotionReward { potion_id:
    /// Some(...) })` carries a pre-rolled id. Mirrors Python
    /// `PotionReward` (`reward_objects.py`). Bead `sts2-rs-ib04`.
    Potion(PotionReward),
    /// Upgrade-cards reward — the player picks `count` cards from
    /// `options` to upgrade. Bead `sts2-rs-70anj.3.14`.
    UpgradeCards(UpgradeCardsReward),
    /// Enchant-cards reward — the player picks up to `count` cards
    /// from `options` to receive `amount` stacks of `enchantment`.
    /// Bead `sts2-rs-2v8y2`.
    EnchantCards(EnchantCardsReward),
    /// Card-removal reward — offers the player `count` "remove a card from
    /// your deck" picks. Mirrors C# `CardRemovalReward` (`RewardType.RemoveCard`),
    /// whose `OnSelect` runs `DoLocalCardRemoval`. Drained by
    /// `RunManager::drain_pending_rewards` into
    /// [`RunState::remove_cards_from_deck`] (which queues `count` interactive
    /// deck picks, or auto-removes `count` in parity mode). Pushed at combat
    /// end by `ForbiddenGrimoirePower.AfterCombatEnd` via the `exit_combat`
    /// bridge. Bead `sts2-rs-nfoh0`.
    RemoveCard {
        /// Number of card-removal picks to grant.
        count: u32,
    },
}

// ---------------------------------------------------------------------------
// RestSiteOption
// ---------------------------------------------------------------------------

/// `PumpkinCandle`'s per-Rekindle `KindleCount` increment
/// (C# `PumpkinCandle.cs:18`, `kindleAmount = 5`). Applied on pickup
/// (`AfterObtained → Rekindle`) and on each rest-site `Kindle` pick
/// (`KindleRestSiteOption.OnSelect → Rekindle`). Stored in
/// `RunState::relic_counters[PumpkinCandle]`; while `> 0` the relic
/// grants +1 max energy in combat and the count decays by 1 per
/// combat end.
pub const PUMPKIN_CANDLE_KINDLE_AMOUNT: i32 = 5;

/// One available rest-site action. Mirrors Python
/// `sts2-rl-agent@01c094d:sts2_env/run/rest_site.py::RestSiteOption`
/// (the eight concrete subclasses: `HealOption`, `SmithOption`,
/// `DigOption`, `LiftOption`, `CookOption`, `CloneOption`, `HatchOption`,
/// `MendOption`).
///
/// Python carries a per-instance `enabled` flag (greyed-out in UI when
/// `False`) plus per-variant payload (e.g. `LiftOption.lifts_done`,
/// `CookOption.has_enough_removable`). The Rust port collapses both
/// into the `enabled` field plus a tiny payload struct on the data
/// variants that actually consume them at this pin (Cook + Smith +
/// Lift). Adding listeners that need deeper per-option state can grow
/// the per-variant payload locally without churning every call site.
///
/// Bead `sts2-rs-ekf4j`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RestSiteOption {
    /// Heal floor(max_hp * 0.3). Always available.
    Rest,
    /// Upgrade one deck card. `enabled = false` iff every deck card is
    /// already upgraded.
    Smith {
        /// Whether the player has any non-upgraded deck card.
        enabled: bool,
    },
    /// Obtain a random relic (granted by `Shovel`).
    Dig,
    /// Increment `Girya` lift counter (max 3 lifts/run).
    Lift {
        /// Lifts done so far (0..=3); `enabled` derived from `< 3`.
        lifts_done: u8,
    },
    /// Remove `CARD_COUNT` cards, gain `MAX_HP` max HP (granted by
    /// `MeatCleaver`). `enabled = false` iff deck has fewer than
    /// `CARD_COUNT` removable cards.
    Cook {
        /// Whether the player has at least `CARD_COUNT` removable cards.
        enabled: bool,
    },
    /// Duplicate every card carrying the `Clone` enchantment (granted
    /// by `Pael's Growth`).
    Clone,
    /// Hatch the Byrdpip egg (`Byrdoni's Nest` reward).
    Hatch,
    /// Rekindle the `PumpkinCandle` (`KindleRestSiteOption`, granted by
    /// the PumpkinCandle ANCIENT relic): adds `kindleAmount = 5` to the
    /// relic's `KindleCount`, refreshing its +1-max-energy window.
    /// Always available while owned. C# `KindleRestSiteOption.cs:37-41`
    /// (`OnSelect → PumpkinCandle.Rekindle`).
    Kindle,
    /// Heal another player in multiplayer. Multiplayer is single-player
    /// at the pin, so this variant currently never appears.
    Mend,
}

/// Outcome of [`RunState::do_rest_site_action`]. Mirrors the
/// observable side-effects of Python's `RestSiteOption.execute`
/// dispatch (`run_manager.py:739` / `rest_site.py:67-107`) without
/// the descriptor-string layer (Python returns prose; the engine
/// surfaces structured state to MCTS).
///
/// Bead `sts2-rs-qwp1g` (P9.2.d).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RestSiteActionOutcome {
    /// `RestSiteOption::Rest` applied. `healed` is the actual HP
    /// added post-clamp to [`RunState::max_hp`].
    Rested {
        /// Post-clamp HP delta.
        healed: i32,
    },
    /// `Hatch` selected: the player obtained the `Byrdpip` relic
    /// (C# `HatchRestSiteOption.OnSelect` → `RelicCmd.Obtain<Byrdpip>`).
    /// `Byrdpip`'s `after_obtained` listener transforms every
    /// `ByrdonisEgg` in the deck into `ByrdSwoop`.
    Hatched,
    /// `Smith` with exactly one upgradable card: auto-upgraded.
    /// `card_id` is the card whose `upgraded` flag flipped.
    SmithUpgraded {
        /// The card just upgraded.
        card_id: CardId,
    },
    /// `Smith` with >1 upgradable cards: a `pending_choice` is now
    /// open over the upgradable set. Caller resolves via
    /// [`RunState::resolve_pending_card_choice`] then applies the
    /// upgrade on the resulting [`crate::ChoiceOption`].
    SmithAwaitingChoice,
    /// `Kindle` selected: `PumpkinCandle`'s `KindleCount`
    /// (`relic_counters[PumpkinCandle]`) was increased by
    /// `kindleAmount = 5` (C# `PumpkinCandle.Rekindle`).
    Kindled,
    /// `Lift` selected: `Girya`'s `TimesLifted`
    /// (`relic_counters[Girya]`) was increased by 1 (C#
    /// `LiftRestSiteOption.OnSelect`). The combat-side +Strength
    /// grant reads the counter mirror at `before_combat_start`
    /// (`relic_effects::girya`).
    Lifted,
    /// The chosen option's [`RestSiteOption::enabled`] is `false`,
    /// or a pre-existing pending-choice blocked the request. No
    /// state mutation occurred.
    Disabled,
    /// The chosen variant (Dig / Lift / Cook / Clone / Hatch /
    /// Mend) is deferred — its handler hasn't landed yet. No state
    /// mutation occurred.
    Unsupported,
}

impl RestSiteOption {
    /// True when the option is selectable. Disabled options can be
    /// listed in the UI but the player cannot resolve them.
    #[must_use]
    pub fn enabled(&self) -> bool {
        match self {
            Self::Rest | Self::Dig | Self::Clone | Self::Hatch | Self::Kindle | Self::Mend => true,
            Self::Smith { enabled } | Self::Cook { enabled } => *enabled,
            Self::Lift { lifts_done } => *lifts_done < 3,
        }
    }

    /// Stable string discriminant, matching Python's `option_id` field
    /// on each `RestSiteOption` subclass. Used by listeners that need
    /// to check whether an option of a given kind is already present
    /// before adding their own (Cook etc.). Bead `sts2-rs-ekf4j`.
    #[must_use]
    pub fn option_id(&self) -> &'static str {
        match self {
            Self::Rest => "HEAL",
            Self::Smith { .. } => "SMITH",
            Self::Dig => "DIG",
            Self::Lift { .. } => "LIFT",
            Self::Cook { .. } => "COOK",
            Self::Clone => "CLONE",
            Self::Hatch => "HATCH",
            Self::Kindle => "KINDLE",
            Self::Mend => "MEND",
        }
    }
}

// ---------------------------------------------------------------------------
// RunState
// ---------------------------------------------------------------------------

/// `RunState` POD — minimal surface area landed by bead `sts2-rs-62t`.
///
/// Mirrors `sts2-rl-agent@01c094d:sts2_env/run/run_state.py::RunState`.
/// Today this struct only carries the reward queue (`pending_rewards`)
/// — the deck / map / gold / hp / relic-roster fields will be added
/// by downstream surface beads as the retrofit chain needs them.
///
/// Note: `CombatState` does **not** hold a `&mut RunState` back-edge
/// today. The hook-bus dispatcher (`relic_effects::dispatch_modify_*`)
/// takes the reward vector by `&mut` directly so the relic listeners
/// can mutate it without a `RunState` plumb-through. Once the wider
/// run-loop lands we'll wire `RunState` onto the engine driver above
/// `CombatState`.
// `Eq` intentionally dropped (bead `sts2-rs-4aunm`): `RunState` now
// embeds `current_act_map: Option<ActMap>`, and `ActMap`/`MapPoint`
// carry `x_offset: f32` which can't impl `Eq`. `PartialEq` is
// retained — all in-tree consumers use `assert_eq!` / `==`, none
// require the stricter `Eq` bound (verified via grep).
/// Boss encounter pre-rolled at act start. Identifies the encounter the
/// player will fight in this act's Boss room and carries the
/// per-encounter RNG seed so the content-side boss builder can finish
/// the roster + HP rolls deterministically (matching what a fresh
/// "roll on boss-room entry" path would have produced). `act_index` is
/// 1-based and tracked so a stale pin (e.g. surviving across an
/// `advance_act` bug) is detectable / ignorable. `lead_kind` and
/// `display_name` are duplicated from the encounter pool so display
/// code (the `full_map` boss label) doesn't need to re-resolve the
/// pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedActBoss {
    /// 1-based act index this pin belongs to.
    pub act_index: u8,
    /// Index into the act's boss-tier encounter pool (act-specific:
    /// for Act 1 this indexes the Boss-tier subset of
    /// `sts2_content::encounters::act1_overgrowth::ACT1_ENCOUNTERS`).
    pub encounter_index: usize,
    /// `run_manager` per-encounter seed, taken at pre-roll time so the
    /// boss builder constructs the same roster as if it had rolled
    /// fresh on entry.
    pub per_encounter_seed: i32,
    /// Lead monster of the boss encounter — used by the workbench to
    /// resolve a display name without re-walking the encounter pool.
    pub lead_kind: crate::MonsterKind,
    /// Encounter display name from the pool (e.g. "Ceremonial Beast").
    pub display_name: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunState {
    /// Reward queue. The combat-end / rest-site / event-room reward
    /// fan-out pushes onto this; the modify_* hook channels mutate it
    /// in place; the run-loop drains it into `pending_choice` once the
    /// player is prompted. Defaults to empty.
    pub pending_rewards: Vec<Reward>,

    /// Dedicated queue of pending relic-pick rewards (bead `sts2-rs-bms`).
    /// Mirrors the `RelicReward` instances appended by Python
    /// `RunState.offer_relic_rewards` (`run_state.py:786`). Maw-Bank-style
    /// "gain X gold per X relics owned" / `MrStruggles` / `WongosMysteryTicket`
    /// retrofit beads (`sts2-rs-n3w-d` / `-e`) push here without having
    /// to wrap each entry in [`Reward::Relic`]. The shop / rest-site
    /// reward fan-out drains this into [`Self::pending_rewards`] when
    /// the player is prompted. Defaults to empty.
    pub relic_rewards: Vec<RelicReward>,

    /// Queue of pending *custom* card-select choices — "choose 1 of N
    /// cards (or skip)" offers raised by run-side effects that aren't the
    /// standard post-combat card reward (which is generated by the
    /// `RunManager.generate_card_reward_options` callback). Each entry
    /// carries an explicit option list in [`CardReward::options`].
    ///
    /// The canonical producer is `HeftyTablet::after_obtained` (bead
    /// `sts2-rs-w6hdj`, W9d): it cannot reach `RunManager`'s
    /// `card_reward_options` from a `&mut RunState` listener, so it
    /// enqueues the rolled 3-Rare offer here. `RunManager`'s
    /// `advance_after_card_reward` drains the front entry into
    /// `card_reward_options` once the relic-reward queue empties and marks
    /// the offering as a *choose screen* (C# `FromChooseACardScreen`,
    /// `canSkip: true` → `card_select` with `min 0 / max 1`), distinct from
    /// a post-combat `card_reward`. Any [`PendingCardSelect::post_add`]
    /// cards land after the screen resolves. Defaults to empty.
    pub pending_card_selects: Vec<PendingCardSelect>,

    /// Pending MANDATORY multi-pick card grants — choose EXACTLY `pick` of N
    /// freshly-generated cards, add the picks to the deck (C#
    /// `FromSimpleGridForRewards` → `CardPileCmd.Add(.., Deck)`). The run loop
    /// surfaces the FRONT entry as a phase-preempting `card_select`
    /// (`min == max == pick`, no skip), mirroring the `pending_deck_transforms`
    /// deck-op selects. Populated by `RoomFullOfCheese.Gorge`; drained by
    /// `RunManager::do_grant_deck_card_at`. Defaults to empty. Bead
    /// `sts2-rs-gorge-select`.
    pub pending_card_grants: Vec<PendingCardGrant>,

    /// Pending Scroll Boxes bundle choice (Necrobinder/any-character ANCIENT
    /// relic `ScrollBoxes`). Each inner `Vec<CardId>` is one "pack" (2 common
    /// + 1 uncommon); the player picks exactly one pack and **all** its cards
    /// are added to the deck (no skip), unlike `pending_card_selects` which is
    /// a 1-of-N single-card pick. Populated by `ScrollBoxes::after_obtained`
    /// (`listener_scroll_boxes`); drained by `RunManager` into its
    /// `card_reward_bundles` marker and surfaced as a `bundle_select` decision.
    /// Empty when no bundle choice is outstanding.
    pub pending_bundles: Vec<Vec<crate::cards::CardId>>,

    /// Per-option upgrade flags for the most recent card-reward offering,
    /// parallel to the `Vec<CardId>` the `generate_card_reward_options`
    /// callback returns. Side channel: the callback signature carries bare
    /// `CardId`s, which silently dropped the upgrade state C#'s
    /// `TryModifyCardRewardOptionsLate` relics (SilverCrucible) stamp on the
    /// offered cards — the picked card entered the deck unupgraded (RP11
    /// MOLTENFIST|1 vs |0, roster-wide BEAMCELL/CELESTIALMIGHT family). Set
    /// by the content-side callback, immediately `take`n by
    /// `RunManager::enter_card_reward_phase` / `surface_pending_rewards`
    /// into its offer-aligned `card_reward_upgraded` list.
    pub card_reward_upgraded_flags: Vec<bool>,

    /// Relics the player has acquired this run. Mirrors Python
    /// `RunState.relics` (`run_state.py`). Bead `sts2-rs-zq8` (j87-b)
    /// landed this alongside [`Self::accept_relic_reward`] so the
    /// `after_obtained` hook channel
    /// ([`crate::run_effects::dispatch_after_obtained`]) has a real
    /// firing site. `count_pets`-style listeners that walk the player's
    /// relic list read from here. Defaults to empty.
    pub relics: Vec<RelicId>,

    /// Transient slot set by [`Self::accept_relic_reward`] to the
    /// id of the relic *just* added to [`Self::relics`], for the
    /// duration of the
    /// [`crate::run_effects::dispatch_after_obtained`] walk only.
    /// Listeners that fire one-shot on their own pickup (Astrolabe's
    /// transform-and-upgrade-3, …) self-gate with
    /// `rs.last_obtained_relic == Some(RelicId::SelfId)`. Listeners
    /// wanting any-pickup semantics (MawBank, … tracker-style)
    /// ignore the field. `None` outside of that window. Bead
    /// `sts2-rs-70anj.3.9`.
    pub last_obtained_relic: Option<RelicId>,

    /// Combat gold-reward proportion for the just-ended combat:
    /// `1 - escaped_creatures / total_monsters` (C# `CombatRoom.GoldProportion`,
    /// `CombatRoom.cs:216`). Set by `exit_combat` from the combat's
    /// [`crate::CombatState::escaped_creatures`]; read by the combat
    /// gold-reward roll (`apply_combat_gold_and_potion`). Defaults to `1.0`
    /// (no escapes) and is overwritten on every combat exit. Bead
    /// `sts2-rs-RP15`.
    pub last_combat_gold_proportion: f64,

    /// The Ancient-rarity card `DustyTome` will add when obtained, chosen at
    /// the Darv-event option-generation moment (`Phase::BossRelic` entry) by
    /// `DustyTome.SetupForPlayer` — `player.PlayerRng.Rewards.NextItem(items)`,
    /// `DustyTome.cs:55`. C# rolls this ONE `Rewards` read at SETUP (whenever
    /// DustyTome is *offered* by Darv), not at obtain, and `AfterObtained`
    /// (`DustyTome.cs:58`) consumes the stored card with no further roll. Set
    /// by the `setup_boss_relic_ancient` callback and `take`n by
    /// `listener_dusty_tome`; `None` when DustyTome was not offered. Mirroring
    /// the setup-time read is what keeps the shared `Rewards` stream aligned
    /// (else a Darv act-transition that offers but doesn't pick DustyTome
    /// leaves Rust one read short for the rest of the run).
    pub dusty_tome_setup_card: Option<CardId>,

    /// The room the player is currently inside, or `None` outside any
    /// room (e.g. on the map screen between rooms or before the first
    /// `enter_room` call). Bead `sts2-rs-ouv` (j87-c) landed this
    /// alongside [`Self::enter_room`] so the `after_room_entered` hook
    /// channel ([`crate::run_effects::dispatch_after_room_entered`])
    /// has a real firing site. Mirrors Python
    /// `RunState.current_room` (`run_state.py`). `MawBank` /
    /// `DreamCatcher` / `FakeVenerableTeaSet` / `LostWisp` listeners
    /// read the room category from this field via the dispatcher
    /// payload; listeners that need the full `Room` structure (when
    /// extended past `room_type`) read from here directly.
    pub current_room: Option<Room>,

    /// The player's current HP outside of combat. Mirrors Python
    /// `RunState.current_hp` (`run_state.py:39`). Defaults to `80`
    /// to match the Python pin `sts2-rl-agent@01c094d`. Bead
    /// `sts2-rs-xqvn` (P7-surf prereq for `sts2-rs-46ms`).
    ///
    /// Mutate through [`Self::heal`] (clamped to `max_hp`) or
    /// [`Self::gain_max_hp`] (which bumps both `max_hp` and
    /// `current_hp`); both broadcast through
    /// [`crate::run_effects::dispatch_after_max_hp_changed`].
    pub current_hp: i32,

    /// The player's current `max_hp` outside of combat. Mirrors Python
    /// `RunState.max_hp` (`run_state.py:38`). Defaults to `80` (Python
    /// pin `sts2-rl-agent@01c094d`). Bead `sts2-rs-xqvn`.
    pub max_hp: i32,

    /// The player's current gold pile. Mirrors Python `RunState.gold`
    /// (`run_state.py:40`). Defaults to `99` (Python pin
    /// `sts2-rl-agent@01c094d`; matches the Ironclad / Silent / Defect
    /// / Necrobinder starting-gold constant in
    /// `characters/all.py`). Bead `sts2-rs-xqvn`.
    ///
    /// Mutate through [`Self::gain_gold`] (positive or negative
    /// delta) or [`Self::apply_pending_combat_gold_loss`] for DEBT
    /// drain (bead `sts2-rs-3bie`).
    pub gold: i32,

    /// Per-relic counter store for run-side relics. Mirrors the
    /// per-instance counters Python `RelicInstance` subclasses keep
    /// (e.g. `WongosMysteryTicket._combats_finished` /
    /// `_gave_relic`, `DreamCatcher._procs_left`). The combat-side
    /// `Relic.counter` POD covers in-combat relics; this map covers
    /// the run-side ones whose lifecycle is between-combat. Defaults
    /// to empty.
    ///
    /// Some relics (`WongosMysteryTicket`) carry two Python counters
    /// — see the per-listener body for the packed encoding used (the
    /// audit doc footer flagged this as the canonical resolution to
    /// the "single `Relic.counter` slot" problem named in
    /// `2026-05-12-event-reward-audit.md`). Bead `sts2-rs-9wx7`.
    pub relic_counters: BTreeMap<RelicId, i32>,

    /// Relics `ToyBox` marked wax (C# `RelicModel.IsWax`, set on the 4
    /// relics its `AfterObtained` offers — `ToyBox.cs:93`). Only these are
    /// eligible for melting. Keyed by id rather than inventory slot because
    /// `ToyBox` pulls each offer off the front of the *unowned* pool, so an
    /// id can never be wax twice.
    pub wax_relics: std::collections::BTreeSet<RelicId>,

    /// Relics `ToyBox` has already melted (C# `RelicModel.IsMelted`, set by
    /// `RelicCmd.Melt` → `Player.MeltRelicInternal`). The relic STAYS in
    /// [`Self::relics`] — melting only removes it from the run-side hook
    /// listener list (`RunState.cs:148` appends
    /// `player2.Relics.Where(r => !r.IsMelted)`), so every ownership *guard*
    /// must consult [`Self::owns_active_relic`] while everything that
    /// *counts* or serializes relics keeps reading [`Self::relics`].
    pub melted_relics: std::collections::BTreeSet<RelicId>,

    /// Currently-active event in the [`RoomType::Event`] room the
    /// player is inside, or `None` outside any event room. Set by
    /// [`Self::enter_event_room`]; cleared by
    /// [`Self::exit_event_room`] (the dispatcher in
    /// `sts2-content::events::dispatch::resolve` calls
    /// [`Self::exit_event_room`] once a choice resolves to a
    /// `finished` outcome).
    ///
    /// Mirrors Python `RunState.current_event`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py`) — a slot
    /// pointing at the live `EventModel` instance for the room. Rust
    /// stores only the [`EventId`] discriminant; the spec lookup
    /// happens in `sts2-content::events::dispatch::spec_for`.
    ///
    /// Bead `sts2-rs-6ynt.4.1` (P7 events framework).
    pub current_event: Option<crate::events::EventId>,

    /// Scratch state for the active multi-page event (page cursor +
    /// event RNG + generic slots). Default/empty for single-page
    /// events. Reset on [`Self::enter_event_room`], cleared on
    /// [`Self::exit_event_room`]. Bead `sts2-rs-2guyj`.
    pub event_state: crate::events::MultiPageEventState,

    /// A pending / in-flight event→combat request (bead `sts2-rs-ck9z6`,
    /// GH #166). An event-option handler stages this instead of granting a
    /// combat reward directly; [`crate::run_manager::RunManager`] consumes
    /// it to enter [`crate::run_manager::Phase::Combat`] against the
    /// [`crate::events::EventEncounter`], then drives the parent-event
    /// resume / victory-reward grant on combat exit. Stays set across the
    /// combat phase (so the exit path knows to resume) and is cleared once
    /// the fight resolves. Mirrors C#
    /// `EventModel.EnterCombatWithoutExitingEvent` + the
    /// `CombatRoom.ShouldResumeParentEventAfterCombat` bridge.
    pub event_combat: Option<crate::events::EventCombatRequest>,

    /// Cached relic offers rolled for the run-start Neow event.
    /// Populated by [`Self::roll_neow_offers_if_needed`] (called once
    /// at Neow-room entry); read by the `Neow` event spec's
    /// `generate_options` and choice handlers in
    /// `sts2-content::events::act3_glory` so the labels + grants reflect the
    /// rolled relics rather than the placeholder constants.
    ///
    /// Mirrors the cache `Neow._modifierOptions` /
    /// `AncientEventModel._generatedOptions` slots in
    /// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models/AncientEventModel.cs`
    /// — C# rolls once on first visit and caches.
    ///
    /// `None` outside a Neow visit (and on a fresh RunState). The
    /// run-start phase wiring (sub-bead `sts2-rs-g24zv`) is what
    /// actually transitions into the Neow room; until that lands,
    /// callers can populate the cache directly for tests.
    ///
    /// Bead `sts2-rs-14cid` (R3.1.2 under `sts2-rs-nr8l4`).
    pub neow_offers: Option<crate::NeowOffers>,

    /// Number of potion-belt slots the player currently has. Mirrors
    /// Python `RunState.max_potion_slots` (`run_state.py:45`,
    /// default `3`). Bumped by `PotionBelt::after_obtained`
    /// (`gain_potion_slots(2)`) and similar slot-granting relics; can
    /// also be reduced (Sozu-style relics) — see
    /// [`Self::gain_potion_slots`]'s `amount: i32` signature.
    ///
    /// Combat-side `PlayerCombatState.potions` remains hard-capped at
    /// [`crate::potions::MAX_POTIONS`] (= 5); this field tracks the
    /// run-state intent so PotionBelt / AlchemicalCoffer-style relics
    /// can grow / shrink the belt before the next combat seeds
    /// `PlayerCombatState`. Bead `sts2-rs-dsgf9`.
    pub max_potion_slots: i32,

    /// Run-level potion belt — the potions the player currently holds
    /// between rooms. Mirrors Python `Creature.potions` carried on the
    /// run-state player. Seeded into `PlayerCombatState.potions` at
    /// combat entry and written back on exit (`run_bridge.rs`), so a
    /// potion procured out of combat (Lost Coffer, PhialHolster, Tiny
    /// Mailbox …) survives to be drunk in the next fight. Capacity is
    /// bounded by [`Self::max_potion_slots`] via
    /// [`Self::add_potion_to_belt`]. Bead `sts2-rs-01o`.
    pub potions: Vec<crate::potions::PotionKind>,

    /// Physical belt-slot index of each entry in [`Self::potions`],
    /// parallel by position. Mirrors C# `Player._potionSlots` hole
    /// semantics: drinking leaves the slot empty, the next add fills the
    /// FIRST empty slot, and the visible list is non-empty slots in slot
    /// order (see `PlayerCombatState::potion_slot_indices`; shadow census
    /// v8 seeds 1012/1013/1028/1035/1044/1051). Threaded through combat by
    /// `run_bridge::{enter_combat, exit_combat}`. Re-synced to the compact
    /// `0..len` mapping whenever it falls out of parallel (fixture escape
    /// hatch).
    pub potion_slot_indices: Vec<u8>,

    /// Run-side master deck — full per-instance [`Card`] objects,
    /// mirroring Python `RunState.master_deck` (a list of
    /// `CardInstance`). The combat-side
    /// [`crate::CombatState::master_deck`] field is `Vec<CardId>`
    /// (no per-instance state), used only for in-combat eval-feature
    /// counts; this run-side field carries the upgrade flag +
    /// enchantments + combat-vars that survive between combats.
    ///
    /// Defaults to empty. The full deck-population path (character
    /// starter, card-reward acceptance, transform / remove / upgrade
    /// of cards) is filled in by subsequent retrofit beads; this bead
    /// (`sts2-rs-7krx0`) only wires the slot + the surface
    /// [`Self::upgrade_random_cards`] needs to mutate.
    pub master_deck: Vec<Card>,

    /// Number of times the merchant's remove-card slot has been used in
    /// this run. Mirrors Python `Player.card_shop_removals_used`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/player.py`). Drives the
    /// step-pricing curve on the next shop entry via
    /// [`crate::shop::card_removal_cost`] (75 + 25 * uses). Incremented
    /// by [`crate::shop::purchase_card_removal`] on a successful
    /// purchase. Bead `sts2-rs-jbqw4` (P9.2.c.4).
    pub card_shop_removals_used: u32,

    /// Seed state for the run-side "rewards" / random-effect RNG.
    /// Mirrors Python's `RunState.rng.rewards` stream. The helper
    /// [`Self::rewards_rng`] vends a fresh
    /// `Xoshiro256PlusPlus` seeded from this value, and advances
    /// the seed so back-to-back calls draw independent sequences.
    ///
    /// Defaults to `0` so deterministic golden tests can rely on a
    /// fixed sequence; production code that needs a real RNG should
    /// initialize via [`Self::with_rewards_seed`] before invoking
    /// any helper that consumes the stream.
    pub rewards_rng_seed: u64,

    /// Current act index (1, 2, or 3). Mirrors Python
    /// `RunState.act_index` (`run_state.py`), which is derived from
    /// map progress. Defaults to `1` (Act 1). Until the map / floor
    /// progression port lands, this is set explicitly by tests and by
    /// the boss-clear transition. Listeners gated on act (LavaRock
    /// post-Act-1 boss, PumpkinCandle, FurCoat) read this via
    /// [`Self::act_index`]. Bead `sts2-rs-70anj.3.16`.
    pub act_index: u8,

    /// Act index latched when `FurCoat` was obtained, or `None` if
    /// the relic was never picked up. Mirrors Python
    /// `FurCoat._fur_coat_act_index` (`shop_event.py:1968`) lifted to
    /// `RunState` because the Rust port has no per-relic instance
    /// state. Set by [`listener_fur_coat_after_obtained`] in
    /// `sts2-content` on the first `after_obtained` dispatch. Read
    /// by `listener_fur_coat_modify_generated_map_late` (gates the
    /// per-act mark roll) and by a future combat-side 1-HP clamp
    /// listener (gates the enemy-HP clamp to the obtain act). Bead
    /// `sts2-rs-eboqq`.
    pub fur_coat_act_index: Option<u8>,

    /// Count of `WingedBoots` "ignore-paths" uses spent so far. The
    /// relic ([`crate::RelicId::WingedBoots`], a W9 Neow reward) lets
    /// the player travel to a next-row map node that is NOT a
    /// connected child of the current node, a limited number of times
    /// ([`Self::WINGED_BOOTS_MAX_USES`] = 3). While uses remain the
    /// map-choice surface offers every grid-live node in the next row
    /// (the connected children are a subset); moving to an off-path
    /// node spends one use. Mirrors the C# headless tracker
    /// `RunSimulator._wingedBootsOffPathUsed` (off-path detection in
    /// `IsOffPathChoice`, charge cap in `WingedBootsMaxUses`). Lifted
    /// to `RunState` because the Rust port has no per-relic instance
    /// state and the counter must survive `RunManager` reconstruction.
    pub winged_boots_off_path_used: u32,

    /// Re-entry guard for `BowlerHat::on_gold_gained` (Python
    /// `relics/uncommon.py::BowlerHat._is_applying_bonus`). The
    /// hook applies a 20%-of-amount bonus by re-calling
    /// [`Self::gain_gold`], which re-fires the gold-channel
    /// listeners. This flag is set during the bonus application so
    /// the BowlerHat listener can skip its own recursive call and
    /// avoid the geometric `0.2 * 0.2 * ...` compounding. Mirrors
    /// Python's instance-side bool flag; lifted to `RunState`
    /// because the Rust port has no per-relic state. Set/cleared
    /// inside `listener_bowler_hat_on_gold_gained`; defaults to
    /// `false` at run start. Bead `sts2-rs-b31s6`.
    pub bowler_hat_applying_bonus: bool,

    /// Re-entry guard set during pickup-time reward consumption.
    /// Mirrors Python `RunState.defer_followup_rewards: bool`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1415`). When
    /// `true`, relic `after_obtained` listeners that would otherwise
    /// auto-apply a follow-up effect (WarPaint upgrading 2 random
    /// Skills, Whetstone upgrading 2 random Attacks, several shop_event
    /// relics) instead push a player-choice [`Reward::UpgradeCards`]
    /// onto [`Self::pending_rewards`] via
    /// [`Self::offer_upgrade_cards_reward`]. The flag is flipped on
    /// during `RelicReward.select` redemption (Python pin
    /// `run/reward_objects.py:190-199`) so the relic-induced effect
    /// becomes player-pickable, then restored to its prior value.
    /// Use [`Self::with_deferred_followup_rewards`] for scoped
    /// set/restore. Defaults to `false`. Bead `sts2-rs-w785o`.
    pub defer_followup_rewards: bool,

    /// Per-act log of `MapCoord`s the player has visited, in visit
    /// order. Cleared at every act boundary. Mirrors Python
    /// `RunState.visited_map_coords: list[MapCoord]`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1393`).
    /// Mutated via [`Self::add_visited_coord`]; cleared by
    /// [`Self::advance_act`]. Bead `sts2-rs-2c0ze` (P9.2.m audit §2.2).
    pub visited_map_coords: Vec<crate::map::MapCoord>,

    /// Full-run log of (map-node-type, resolved-room-type) pairs the
    /// player has visited. Mirrors Python
    /// `RunState.map_point_history: list[MapPointHistoryEntry]`
    /// (`run_state.py:1394`). NOT cleared on act-advance — the log is
    /// run-scoped so listeners can ask "how many rest sites this run?".
    /// Mutated via [`Self::append_to_map_point_history`] (typically as
    /// a side-effect of [`Self::add_visited_coord`]); queried via
    /// [`Self::count_map_point_history_entries`]. Bead `sts2-rs-2c0ze`.
    pub map_point_history: Vec<crate::map::MapPointHistoryEntry>,

    /// `coord.row + 1` for the most recent visited coord, or `0`
    /// before any visit in the current act. Cleared at act-advance.
    /// Mirrors Python `RunState.act_floor: int` (`run_state.py:1395`).
    /// Note Python uses `int` (signed); the `i32` type matches.
    /// Bead `sts2-rs-2c0ze`.
    pub act_floor: i32,

    /// Cumulative visited-coord count across the entire run. Bumps by
    /// `1` per fresh visit in [`Self::add_visited_coord`]; never
    /// resets (run-scoped). Mirrors Python `RunState.total_floor:
    /// int` (`run_state.py:1396`). Bead `sts2-rs-2c0ze`.
    pub total_floor: i32,

    /// Number of `Monster`-room combats entered so far in the current
    /// act. The first three monster fights of an act draw from the
    /// `Weak` encounter tier and the rest from `Normal` (mirrors C#'s
    /// per-act weak-then-strong encounter sequence); the
    /// EncounterDef-based combat builder reads this to pick the tier and
    /// bumps it on each monster room. Reset to `0` by [`Self::advance_act`].
    pub monster_combats_this_act: u32,

    /// Number of `Elite`-room combats entered so far in the current act —
    /// the C# `RoomSet.eliteEncountersVisited` cursor. The encounter
    /// routing reads `generated_rooms[act].elite_encounters[this]` on elite
    /// entry, then bumps it. Reset to `0` by [`Self::advance_act`].
    /// Bead `sts2-rs-jenwp` (M3).
    pub elite_combats_this_act: u32,

    /// When `true`, on-obtain card-removal effects (Precise Scissors,
    /// Empty Cage, …) defer to an *interactive* deck pick instead of the
    /// default front-of-deck auto-removal: [`Self::remove_cards_from_deck`]
    /// adds to [`Self::pending_deck_removals`] and the run loop surfaces a
    /// removal choice (mirroring C#'s `card_select`). Default `false`
    /// preserves the Python-parity auto-removal that the parity-fuzz suite
    /// pins; runtime hosts that want the interactive flow (the workbench)
    /// opt in at construction.
    pub interactive_card_removal: bool,

    /// Number of master-deck cards the player still owes an interactive
    /// removal pick for (only ever non-zero when
    /// [`Self::interactive_card_removal`] is set). The run loop offers a
    /// `RemoveDeckCard` action per deck card while this is `> 0`.
    pub pending_deck_removals: u32,

    /// Cards to append to [`Self::master_deck`] AFTER the current batch of
    /// interactive [`Self::pending_deck_removals`] fully drains (i.e. when
    /// the count next hits 0). Models C# events that do
    /// "remove a card, THEN add a card" in sequence — e.g. Wellspring's
    /// Bathe (`CardSelectCmd.FromDeckForRemoval` → `CardPileCmd.RemoveFromDeck`
    /// → `AddGuilty`). Adding the new card eagerly would wrongly include it in
    /// the removal `card_select` pool (and inflate the deck size shown at that
    /// decision); deferring keeps the removal offering C#-identical. Drained by
    /// [`crate::run_manager::RunManager::do_remove_deck_card_at`]. Bead
    /// `sts2-rs-1hiht` (M5 event decision-granularity).
    pub pending_post_removal_cards: Vec<crate::cards::Card>,

    /// HP damage to apply AFTER the current batch of interactive
    /// [`Self::pending_deck_removals`] fully drains. Models C# relics whose
    /// `AfterObtained` does "remove a card, THEN take damage" in sequence —
    /// e.g. `PrecariousShears` (`CardSelectCmd.FromDeckForRemoval` →
    /// `CardPileCmd.RemoveFromDeck` → `CreatureCmd.Damage(16)`). Applying the
    /// damage eagerly at obtain (as the non-interactive auto-removal path does)
    /// would lower HP before the removal `card_select` resolves, diverging from
    /// C#, which suspends on the pick and damages only once it returns. Only
    /// ever non-zero when [`Self::interactive_card_removal`] is set. Drained by
    /// [`crate::run_manager::RunManager::do_remove_deck_card_at`]. Bead
    /// `sts2-rs-1hiht` (M5 event decision-granularity).
    pub pending_post_removal_hp_loss: i32,

    /// Cards to append to [`Self::master_deck`] only AFTER every interactive
    /// deck pick a relic-grant batch queued (upgrades, removals, transforms,
    /// enchants) AND any card/relic-reward offering it opened have fully
    /// resolved. Models C# `NeowsBones.AfterObtained`, which `await`s the
    /// granted-relics `RewardsSet.Offer()` (each relic's own `AfterObtained` —
    /// e.g. a granted Pomander's `FromDeckForUpgrade` pick — runs to
    /// completion) and only THEN adds its curse via `CardPileCmd.Add`. Adding
    /// the curse eagerly in the grant listener lands it in the deck a decision
    /// step early (visible at the granted relic's own card_select) and, for a
    /// grant that appends a card, reverses the deck tail vs C#. Drained by
    /// [`crate::run_manager::RunManager::drain_post_pick_deck_cards_if_settled`]
    /// at each pick-resolution / reward-settle point. Distinct from
    /// [`Self::pending_post_removal_cards`], which is scoped to the removal
    /// batch alone. Bead `sts2-rs-1hiht` lineage (decision-granularity).
    pub pending_post_pick_cards: Vec<crate::cards::Card>,

    /// Count of `NeowsBones` curses whose `Niche` ROLL (not just the card-add)
    /// is deferred to grant-batch settle. C# rolls the curse on `Rng.Niche`
    /// only AFTER `RewardsSet.Offer()` returns, so it must come after any
    /// granted relic's own interactive `Niche` read (NewLeaf's
    /// `TransformToRandom`). Rolling eagerly in the listener stole that value
    /// and flipped the transformed card (RP135). Drained by
    /// [`crate::run_manager::RunManager::drain_post_pick_deck_cards_if_settled`]
    /// via the `roll_neows_bones_curse` content callback once the batch settles.
    pub pending_neows_bones_curses: u32,

    /// Relics a multi-grant listener still owes, granted one at a time only
    /// AFTER the previously-granted relic's interactive `AfterObtained`
    /// (a card-select / transform / bundle pick) fully resolves. Models C#
    /// `NeowsBones.AfterObtained` awaiting `RewardsSet(...).Offer()`: each
    /// granted relic's `AfterObtained` runs to completion — including any
    /// interactive choice it surfaces — BEFORE the next relic is obtained.
    /// Granting all relics synchronously (the old shape) made the 2nd relic
    /// appear a decision step early — visible during the 1st relic's own pick
    /// (NewLeaf's transform select, ScrollBoxes' bundle pick) — even though
    /// the two engines reconverge once that pick resolves. Drained by
    /// [`crate::run_manager::RunManager::drain_post_pick_relic_grants_if_settled`],
    /// which grants strictly before the curse in
    /// [`Self::pending_post_pick_cards`]. Order-preserving (front = next).
    pub pending_post_pick_relic_grants: Vec<RelicId>,

    /// When `true`, on-obtain card-upgrade effects (Pomander, …) defer to
    /// an *interactive* deck pick instead of the default
    /// random-card auto-upgrade: [`Self::upgrade_random_cards`] adds to
    /// [`Self::pending_deck_upgrades`] (consuming NO rewards RNG) and the
    /// run loop surfaces an upgrade choice, mirroring C#'s
    /// `Pomander.AfterObtained` → `CardSelectCmd.FromDeckForUpgrade`
    /// (`CardSelectorPrefs(UpgradeSelectionPrompt, count)`). Default `false`
    /// preserves the Python-parity auto-upgrade (rewards-stream Fisher-Yates)
    /// that the parity-fuzz suite pins; runtime hosts that want the
    /// interactive flow (the workbench / `run_parity` lockstep) opt in at
    /// construction. Bead `sts2-rs-1hiht`.
    pub interactive_card_upgrade: bool,

    /// Number of non-upgraded master-deck cards the player still owes an
    /// interactive upgrade pick for (only ever non-zero when
    /// [`Self::interactive_card_upgrade`] is set). The run loop offers an
    /// `UpgradeDeckCard` action per non-upgraded deck card while this is
    /// `> 0`. Bead `sts2-rs-1hiht`.
    pub pending_deck_upgrades: u32,

    /// When `true`, on-obtain card-transform effects (NewLeaf, …) defer to
    /// an *interactive* deck pick instead of opening the legacy
    /// run-level `pending_choice` (which only the rest-site Smith surface
    /// consumes): [`Self::offer_transform_cards_reward`] adds to
    /// [`Self::pending_deck_transforms`] (consuming NO RNG — C# rolls the
    /// replacement only after the pick, `NewLeaf.AfterObtained` →
    /// `CardSelectCmd.FromDeckForTransformation` then
    /// `CardCmd.TransformToRandom(item, Rng.Niche)`) and the run loop
    /// surfaces a transform choice. Default `false` preserves the legacy
    /// prompt behavior existing hosts pin.
    pub interactive_card_transform: bool,

    /// Queue of interactive transform picks the player still owes, oldest
    /// first (only ever non-empty when [`Self::interactive_card_transform`]
    /// is set). The run loop offers a `TransformDeckCard` action per
    /// ELIGIBLE deck card while non-empty (eligibility per the FRONT
    /// entry's filter); resolving one pops the front, rolls or takes the
    /// fixed replacement, and applies the C# Deck-pile transform shape
    /// (remove original, append replacement at the tail).
    pub pending_deck_transforms: Vec<PendingDeckTransform>,

    /// Queue of interactive deck-enchant picks (Wood Carvings' Snake →
    /// Slither), oldest first; gated on
    /// [`Self::interactive_card_transform`] like the transform queue (the
    /// hosts that surface one surface both). The run loop offers an
    /// `EnchantDeckCard` action per eligible deck card while non-empty;
    /// resolving one stamps `amount` stacks of `enchantment` onto the
    /// chosen master-deck card. C# `CardSelectCmd.FromDeckForEnchantment`
    /// → `CardCmd.Enchant<T>(card, stacks)`.
    pub pending_deck_enchants: Vec<PendingDeckEnchant>,

    /// When `true`, a potion REWARD landing on a full belt surfaces an
    /// interactive discard-to-claim decision instead of silently dropping
    /// the potion: [`Self::claim_potion_reward`] queues the kind on
    /// [`Self::pending_potion_swaps`] and the run loop offers a
    /// `PotionSwap` choice (discard a held potion and take the new one,
    /// or decline it). Mirrors C# v0.107.1, where `PotionReward.OnSelect`
    /// fails with `PotionProcureFailureReason.TooFull`
    /// (`PotionReward.cs:86`, `Player.cs:649,667`) but the reward stays
    /// claimable on the reward screen and the player may discard any held
    /// potion at will (`DiscardPotionGameAction`) before re-taking it.
    /// Default `false` preserves the historical silent drop (which also
    /// matches the headless C# oracle's `TestMode` auto-collect, where a
    /// TooFull potion reward is simply never obtained) — parity sweeps
    /// stay byte-identical; agent hosts opt in. GH #252.
    pub interactive_potion_swap: bool,

    /// Potion-reward kinds awaiting an interactive full-belt swap
    /// decision, oldest first (only ever non-empty when
    /// [`Self::interactive_potion_swap`] is set). The run loop offers a
    /// `PotionSwap` action per held belt slot plus a decline while
    /// non-empty. GH #252.
    pub pending_potion_swaps: Vec<crate::potions::PotionKind>,

    /// Number of potions irrecoverably lost to a full belt this run —
    /// silent drops on the non-interactive path plus declined
    /// [`Self::pending_potion_swaps`] decisions. Diagnostic only (never
    /// read by game logic); surfaced in the arena report so overflow
    /// waste is measurable for the potion-economics experiments
    /// (GH #252/#253).
    pub potions_overflow_dropped: u32,

    /// When `true`, event relic grants (Neow's blessing relics) are added
    /// to the player *directly* via [`Self::obtain_relic`] instead of being
    /// queued as a [`Self::relic_rewards`] entry that needs a second
    /// `PickRelicReward` confirmation. Matches C#'s Neow, where choosing a
    /// blessing immediately grants its relic (no "Take relic / Skip" step).
    /// Default `false` keeps the queued-reward flow the Python-parity tests
    /// pin; the workbench opts in at construction.
    pub direct_event_relic_grant: bool,

    /// Opt-in rewards screen after a won combat (sts2-agent fork). When
    /// `true`, the room's rewards (gold, a dropped potion, the card reward,
    /// an elite's relic) are rolled in the same `Rewards`-stream order as
    /// the auto-collect path but NOT applied: they wait on
    /// `RunManager::reward_screen`, where the player claims each one
    /// (`RunAction::ClaimReward`) or leaves the rest (`ProceedRewards`),
    /// as the game's rewards screen does (C# `RewardsSet` with
    /// `Reward.OnSelect` per claim, `OnSkipped` on leave). A potion reward
    /// is unclaimable while the belt is full (C#
    /// `PotionProcureFailureReason.TooFull`) and stays listed. Default
    /// `false` keeps the auto-collect path every parity test pins.
    pub interactive_rewards: bool,

    /// Reward sets offered through C# `RewardsCmd.OfferCustom` under
    /// [`Self::interactive_rewards`], in offer order, waiting for the run
    /// manager to roll and show them (`RunManager::reward_screen`). A
    /// producer calls [`Self::offer_custom_rewards`]. sts2-agent fork.
    pub offered_reward_sets: Vec<OfferedRewardSet>,

    /// A won combat room's extra rewards (C# `CombatRoom.AddExtraReward`:
    /// Swipe's returned card, Heist's and Royalties' gold, The Hunt's card
    /// reward, Forbidden Grimoire's removals), staged by the combat exit under
    /// [`Self::interactive_rewards`] and listed on the room's rewards screen
    /// after its own rewards. sts2-agent fork.
    pub room_extra_rewards: Vec<OfferedReward>,

    /// Opt-in potion actions outside the combat search's action set
    /// (sts2-agent fork): `RunAction::DiscardPotion` in any phase that shows
    /// the belt (C# `PotionModel.Discard`), and `RunAction::DrinkPotion`
    /// outside combat for the AnyTime potions the game lets the player drink
    /// there (Blood Potion, Fruit Juice, Entropic Brew; Foul Potion at the
    /// merchant). Combat-side `legal_actions` is unchanged, so the search
    /// never sees a discard. Default `false`.
    pub interactive_potions: bool,

    /// C# `Player.CanRemovePotions == false`: the belt is locked (no drink, no
    /// discard). The Future of Potions and Stone of All Time set it in
    /// `BeforeEventStarted` and clear it in `OnEventFinished`
    /// (`TheFutureOfPotions.cs:90-99`, `StoneOfAllTime.cs:64-73`). Set when
    /// either event's room is entered, cleared by the next room's entry; the
    /// run manager reads it as locked until the run is back on the map
    /// ([`crate::run_manager::RunManager::potions_locked`]). sts2-agent fork.
    pub potions_locked: bool,

    /// Set on first map-generation, used by Python to guard re-rolling
    /// the layout. Mirrors Python `RunState._rooms_generated: bool`
    /// (`run_state.py:1397`). Currently a flag holder — the actual
    /// guard logic lives on the map generator and will be wired in a
    /// follow-up bead. Bead `sts2-rs-2c0ze`.
    pub rooms_generated: bool,

    /// Run-scoped ascension level (0 at the current pin's identity
    /// behaviour; the audit notes "Ascension is currently identity
    /// at pin 01c094d — 0 combat-side scaling, all 7 ascension
    /// consumers in run/map/observation layer"). Threaded from
    /// `RunManager::with_ascension` so [`Self::generate_map`] and
    /// other map-side consumers can read it without taking an
    /// extra arg. Mirrors Python `RunState.ascension_level`
    /// (`run_state.py:1380`). Bead `sts2-rs-4aunm`.
    pub ascension_level: u8,

    /// Storage for the current act's map. Populated by
    /// [`Self::generate_map`] (the immediate-regen primitive) and
    /// — when the run-loop driver runs — by the act-advance flow
    /// at [`crate::run_manager::RunManager::advance_act_on_boss_kill`]
    /// which mirrors into this field so future
    /// [`Self::generate_map`] calls regenerate the same act in
    /// place. Mirrors Python `RunState.map: ActMap | None`
    /// (`run_state.py:1575`). `None` before the first generation;
    /// `Some(ActMap)` thereafter. Bead `sts2-rs-4aunm`.
    pub current_act_map: Option<crate::map::ActMap>,

    /// Boss encounter pre-rolled at act start, matching the C# game
    /// where the act's boss identity is fixed when the act map is laid
    /// out (visible from the map widget on floor 1). Consumed by the
    /// content-side boss-room dispatch (see
    /// `build_combat_for_room_act1`) so the same encounter that's
    /// surfaced on the map is the one the player fights. `None` before
    /// the first pre-roll, and reset to `None` on `advance_act` so the
    /// next act's pre-roll fires fresh. The two `i32` rolls off
    /// `RunRngSet::run_manager` are taken at pre-roll time (mirroring
    /// what `build_combat_for_room_act1` would otherwise have consumed
    /// on boss-room entry), so total `run_manager` advancement across
    /// the act is unchanged — only the *timing* of the boss roll shifts
    /// from "boss-room entry" to "act start". Wired in
    /// `sts2_content::run_callbacks::pre_roll_act_boss`.
    pub pinned_act_boss: Option<PinnedActBoss>,

    /// Per-act room contents pre-generated at run start from the shared
    /// `up_front` RNG stream, mirroring C# `RunManager.GenerateRooms`
    /// (`crate::run_room_gen`). One entry per act (index 0-based). Empty
    /// until [`crate::run_room_gen::generate_all_rooms`] runs (currently
    /// invoked from the PyO3 `RunManager` constructor after the relic
    /// grab-bags). The act boss id here is the C#-faithful
    /// `up_front.NextItem(AllBossEncounters)` selection — the source of
    /// truth surfaced on the map widget. See bead `sts2-rs-jenwp`.
    pub generated_rooms: Vec<crate::run_room_gen::GeneratedActRooms>,

    /// Per-run set of [`crate::events::EventId`]s already visited.
    /// Drives Python's "no-repeat events" guarantee in the event-room
    /// roller. Mirrors Python `RunState.visited_event_ids: set[str]`
    /// (`run_state.py:1400`). `BTreeSet` over `HashSet` for the
    /// canonical Rust-side determinism convention (insertion order
    /// independent, golden-test safe). Bead `sts2-rs-2c0ze`.
    pub visited_event_ids: std::collections::BTreeSet<crate::events::EventId>,

    /// Per-run snapshot of the 3-act configuration registry. Initialized
    /// to a fresh [`crate::acts::all_acts`] (events_visited = 0 across
    /// the board) on construction; downstream code mutates
    /// `acts[rs.act_index() as usize - 1].events_visited` when the
    /// player consumes an event room. Bead `sts2-rs-7kptz` (P9.1.d).
    /// Python pin
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py::RunState.acts`
    /// (the per-run mutable mirror of `ALL_ACTS`).
    pub acts: Vec<crate::acts::ActConfig>,

    /// Card-reward rarity roller with rolling pity offset. Mirrors
    /// Python `RunState.card_rarity_odds` (`run_state.py:1410`). Reset
    /// rare-threshold offset on rare roll, growth-capped on non-rare;
    /// boss context bypasses the offset. Reads via
    /// [`Self::generate_card_reward_rarities`] /
    /// [`Self::generate_combat_card_rewards`]. Bead `sts2-rs-jyfvu`
    /// (P9.1.e thin slice).
    pub card_rarity_odds: crate::card_rewards::CardRarityOdds,

    /// Rolling pity-odds for `?` (unknown) map-point rooms. Mirrors
    /// Python `Player.unknown_odds: UnknownMapPointOdds`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py`). Reset
    /// on-roll / grow-on-skip per-type semantics; consumed by
    /// [`Self::resolve_unknown_room`]. Bead `sts2-rs-khzyk` (P9.2.f
    /// bounded slice).
    pub unknown_map_point_odds: crate::unknown_room_odds::UnknownMapPointOdds,

    /// Open run-side card-choice prompt. `Some(..)` means the run loop
    /// is parked waiting for the player (or MCTS) to pick from the
    /// listed options; `None` is the steady state.
    ///
    /// Mirrors Python `RunState.pending_choice: PendingCardChoice |
    /// None` (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1413`).
    /// Opened via [`Self::request_card_choice`]; resolved via
    /// [`Self::resolve_pending_card_choice`] /
    /// [`Self::skip_pending_card_choice`].
    ///
    /// Distinct from `CombatState.pending_choice`: this slot is for
    /// non-combat prompts (Toolbox pickup, ChoicesParadox re-offer,
    /// event-room card choices, deck-pick rewards). Bead
    /// `sts2-rs-70anj.3.13`.
    pub pending_choice: Option<crate::PendingChoice>,

    /// Multi-stream named RNG bundle. Mirrors Python
    /// `RunState.rng: RunRngSet` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1340`. Each
    /// named stream (rewards, encounter / map per-act, card_reward,
    /// monster_ai, shuffle, …) advances independently — drawing from
    /// one stream never perturbs another. Bead `sts2-rs-ewm6b`
    /// (P9.1.g).
    ///
    /// Defaults to `RunRngSet::new(0)` for deterministic golden tests.
    /// Tests / production code that need a non-zero master seed should
    /// construct the run with `RunState { rng: RunRngSet::new(seed),
    /// ..Default::default() }` or mutate `rs.rng` directly. The legacy
    /// [`Self::rewards_rng_seed`] / [`Self::rewards_rng`] surface is
    /// kept in place; downstream migration beads move per-call-site
    /// rolls onto the appropriate named stream (`rs.rng.rewards`,
    /// `rs.rng.shops`, `rs.rng.treasure_room`, …).
    pub rng: crate::run_rng_set::RunRngSet,

    /// Pre-shuffled flat relic-reward pool drained by post-combat
    /// rewards. Holds `RelicId`s already filtered to the player's
    /// character pool + the COMMON/UNCOMMON/RARE/SHOP rarities,
    /// ordered by rarity (common-first → shop-last) within
    /// rarity-shuffled subsequences. Populated via
    /// [`Self::populate_relic_grab_bag`] using a caller-supplied
    /// sampler (the content-side `walk_RELIC_REGISTRY` lives in
    /// `sts2-content` and is wired by a follow-up bead).
    ///
    /// Mirrors Python `Player.relic_grab_bag`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:77`). Bead
    /// `sts2-rs-597ws` (P9.1.f).
    pub relic_grab_bag: Vec<RelicId>,

    /// Per-rarity grab-bag buckets — the source of truth for the
    /// relic-reward sampler. [`Self::relic_grab_bag`] above is the
    /// pre-flattened view ordered (Common → Uncommon → Rare → Shop)
    /// and is rebuilt from these buckets whenever
    /// [`Self::populate_relic_grab_bag_by_rarity`] runs.
    ///
    /// Mirrors Python `Player.relic_grab_bag_by_rarity`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:243-271`).
    /// Bead `sts2-rs-l9vkh` (P9.2.r). Per-rarity pop happens via
    /// [`Self::pop_relic_from_grab_bag`] which walks Python's
    /// fallback chain (`_relic_grab_bag_order`).
    pub relic_grab_bag_by_rarity: BTreeMap<RelicRarity, Vec<RelicId>>,

    /// Fallback relic-reward pool consulted when
    /// [`Self::relic_grab_bag`] is exhausted of usable entries.
    /// Mirrors Python `Player.relic_grab_bag_fallback`
    /// (`run_state.py:79`). Empty until the populator runs. Bead
    /// `sts2-rs-597ws` (P9.1.f).
    pub relic_grab_bag_fallback: Vec<RelicId>,

    /// Boss-relic pool. Pre-shuffled at populate-time; the
    /// post-act-boss room offers 3 distinct relics from this pool via
    /// [`Self::offer_boss_relic_choice`]. Mirrors Python's boss-relic
    /// drain in `run_manager.py:685-694` (the BOSS_RELIC phase). Bead
    /// `sts2-rs-597ws` (P9.1.f).
    pub boss_relic_pool: Vec<RelicId>,

    /// The SHARED-only relic grab bag (`State.SharedRelicGrabBag`,
    /// shuffled FIRST off `up_front` — see
    /// `sts2_content::relics::grab_bag::shared_grab_bag_buckets`). The
    /// treasure-room chest relic pulls from THIS bag
    /// (`TreasureRoomRelicSynchronizer.BeginRelicPicking` →
    /// `_sharedGrabBag.PullFromFront`), NOT the player bag that
    /// `RelicFactory.PullNextRelicFromFront` draws from. Empty until the
    /// run initializer stores the shuffled buckets.
    pub shared_relic_grab_bag_by_rarity: BTreeMap<RelicRarity, Vec<RelicId>>,

    /// Shared trackers for the Pael relic cluster (pet level,
    /// cumulative sacrifices, cumulative feeds). Per-relic listeners
    /// (`PaelsWing`, `PaelsTears`, …) read / mutate this field via
    /// the accessor / mutator methods on [`Self`] so multiple Pael
    /// relics can share one ledger without each relying on its own
    /// `RelicInstance` instance state. Bead `sts2-rs-70anj.3.20`.
    pub pael_state: crate::pael_state::PaelState,

    /// Oscillating 40-50% potion-drop pity counter. Mirrors Python
    /// `RunState.potion_reward_odds = PotionRewardOdds()`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1411`). Read /
    /// mutated by [`Self::roll_potion_reward`] at combat-end reward
    /// generation. Bead `sts2-rs-vk9av`.
    pub potion_reward_odds: crate::card_rewards::PotionRewardOdds,

    /// Per-run card storage for [`crate::RelicId::PaelsTooth`].
    /// Mirrors Python `PaelsTooth._stored_cards: list[CardInstance]`
    /// (`shop_event.py:2785`). On pickup, up to 5 removable+upgradable
    /// master-deck cards are cloned in here and removed from
    /// `master_deck`; each subsequent `after_combat_end` draws one
    /// via the `rng.rewards` stream, upgrades it, and pushes it back
    /// onto the deck. Empty on a fresh `RunState`.
    ///
    /// Kept as a dedicated field rather than a generic
    /// `BTreeMap<RelicId, Vec<Card>>` because PaelsTooth is the only
    /// relic with per-run card storage at this pin; lift to a generic
    /// map if a second relic ever needs the same shape.
    ///
    /// Bead `sts2-rs-v9zer`.
    pub paels_tooth_stored: Vec<crate::cards::Card>,

    /// Run-terminal flag. Mirrors Python `RunState.is_over`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py`). `true` iff
    /// the run has ended — set by `RunManager::finish_run_on_final_boss_kill`
    /// on Act-3 boss-clear (player_won=true), or by the player-death
    /// path once it lands (player_won=false). The run-loop driver
    /// checks this each tick and routes to [`crate::Phase::RunOver`].
    /// Bead `sts2-rs-8i1kn` (P9.2.g.3).
    pub is_over: bool,

    /// Outcome flag — meaningful only when [`Self::is_over`] is `true`.
    /// `true` on Act-3 boss-clear; `false` on player-death. Mirrors
    /// Python `RunState.player_won`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py`). Bead
    /// `sts2-rs-8i1kn` (P9.2.g.3).
    pub player_won: bool,

    /// A10 DoubleBoss flag. Mirrors Python `RunState.has_double_boss`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1420,1545`).
    /// `true` iff the run was started at ascension ≥ 10, driving the
    /// future map-gen path to place two boss fights per act. Set at
    /// run-construction by
    /// [`crate::run_manager::RunManager::with_ascension`]; not mutated
    /// again during the run. Map wiring lands in a P9.A.10b follow-up.
    /// Bead `sts2-rs-oyzw5`.
    pub has_double_boss: bool,

    /// Which of the two Act-1 models this run walks.
    ///
    /// STS2 ships two acts at `Index => 0` — `Overgrowth` (`IsDefault`) and
    /// `Underdocks` — and the real game picks between them at run start on a
    /// dedicated `act_selection` stream. The encounter table, the event pool
    /// and the boss pool all swing with it, so this is not cosmetic: it
    /// selects which content act 1 is made of (GH #544).
    ///
    /// Defaults to [`ActVariant::Overgrowth`], which is what every run this
    /// engine produced before the Underdocks port. Set at run-construction by
    /// [`crate::run_manager::RunManager::with_act1`]; not mutated during the
    /// run. Only [`ActVariant::Overgrowth`] and [`ActVariant::Underdocks`]
    /// are meaningful here — the acts at index 1 and 2 are singletons.
    pub act1: ActVariant,

    /// The character class for this run. Mirrors Python
    /// `RunState.__init__(character_id="Ironclad")` (the constructor
    /// arg threaded onto `self.player = PlayerState(character_id=...)`
    /// at `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1383-1387`).
    /// Set at run-construction by
    /// [`crate::run_manager::RunManager::with_ascension`]; not mutated
    /// during the run. Defaults to [`CharacterId::Ironclad`] so
    /// hand-built `RunState`s (tests, scripted fixtures) match the
    /// Python default without an explicit constructor.
    ///
    /// Direct slot, not derived: prior to this field the character
    /// class was reverse-engineered per call site (sampler walks
    /// inspecting the starter deck, etc.). Read this instead of
    /// inferring from deck contents — the inference is brittle once
    /// the run picks up reward cards from the colorless pool or via
    /// PrismaticGem-widened character pools.
    ///
    /// Bead `sts2-rs-xbkhd` (P9.3.b).
    pub character_id: crate::potions::CharacterId,

    /// Stable index (into the concatenated ACT1++ACT2++ACT3 encounter
    /// tables — see `sts2_content::encounters::encounter_index`) of the
    /// [`crate::EncounterDef`] most recently rolled for a combat entry.
    /// **Harvest-only metadata**, not engine state: nothing in the
    /// combat/run simulation reads it, it exists purely so the AlphaZero
    /// outer-loop combat-surrogate label harvester
    /// (`combat_surrogate_data --realistic`) can recover which encounter
    /// a real self-play pre-combat snapshot is about to face without the
    /// content layer's `build_combat_for_room` otherwise discarding that
    /// identity once the `CombatState` is built. `None` until the first
    /// EncounterDef-based combat entry; the legacy `enter_combat`/
    /// `select_encounter` raw-core fallback path does not set it, so a
    /// stale `Some` from an earlier act/combat can persist across a
    /// legacy-path entry — callers that need per-combat freshness should
    /// treat a stale value as "unknown" rather than trusting it blindly.
    /// Deliberately excluded from [`crate::hashing::run_hash`] (hand-written,
    /// enumerates fields explicitly) so it cannot perturb determinism/parity.
    /// Bead: AZ Milestone 1 Task 2.
    pub current_encounter_idx: Option<usize>,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            pending_rewards: Vec::new(),
            relic_rewards: Vec::new(),
            pending_card_selects: Vec::new(),
            pending_card_grants: Vec::new(),
            pending_bundles: Vec::new(),
            card_reward_upgraded_flags: Vec::new(),
            relics: Vec::new(),
            last_obtained_relic: None,
            last_combat_gold_proportion: 1.0,
            dusty_tome_setup_card: None,
            current_room: None,
            // Python pin `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:38-40`.
            current_hp: 80,
            max_hp: 80,
            gold: 99,
            relic_counters: BTreeMap::new(),
            wax_relics: std::collections::BTreeSet::new(),
            melted_relics: std::collections::BTreeSet::new(),
            current_event: None,
            potions_locked: false,
            event_state: crate::events::MultiPageEventState::default(),
            event_combat: None,
            neow_offers: None,
            // Python pin `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:45`.
            max_potion_slots: 3,
            // Bead `sts2-rs-01o`: run-level potion belt starts empty.
            potions: Vec::new(),
            potion_slot_indices: Vec::new(),
            // Bead `sts2-rs-7krx0`: run-side per-instance deck + rewards RNG.
            master_deck: Vec::new(),
            // Bead `sts2-rs-jbqw4`: fresh run, no merchant removals yet.
            card_shop_removals_used: 0,
            rewards_rng_seed: 0,
            // Bead `sts2-rs-70anj.3.16`: default to Act 1.
            act_index: 1,
            // Bead `sts2-rs-eboqq`: FurCoat un-obtained at run-start.
            fur_coat_act_index: None,
            winged_boots_off_path_used: 0,
            // Bead `sts2-rs-b31s6`: BowlerHat re-entry guard, clear at run-start.
            bowler_hat_applying_bonus: false,
            // Bead `sts2-rs-w785o`: pickup-time reward-redemption guard,
            // matches Python `run_state.py:1415` default.
            defer_followup_rewards: false,
            // Bead `sts2-rs-2c0ze`: map-traversal trackers (Python pin
            // `run_state.py:1393-1400`). All zero / empty at run-start.
            visited_map_coords: Vec::new(),
            map_point_history: Vec::new(),
            act_floor: 0,
            total_floor: 0,
            monster_combats_this_act: 0,
            elite_combats_this_act: 0,
            interactive_card_removal: false,
            pending_deck_removals: 0,
            pending_post_removal_cards: Vec::new(),
            pending_post_removal_hp_loss: 0,
            pending_post_pick_cards: Vec::new(),
            pending_neows_bones_curses: 0,
            pending_post_pick_relic_grants: Vec::new(),
            interactive_card_upgrade: false,
            pending_deck_upgrades: 0,
            interactive_card_transform: false,
            pending_deck_transforms: Vec::new(),
            pending_deck_enchants: Vec::new(),
            interactive_potion_swap: false,
            interactive_rewards: false,
            offered_reward_sets: Vec::new(),
            room_extra_rewards: Vec::new(),
            interactive_potions: false,
            pending_potion_swaps: Vec::new(),
            potions_overflow_dropped: 0,
            direct_event_relic_grant: false,
            rooms_generated: false,
            visited_event_ids: std::collections::BTreeSet::new(),
            // Bead `sts2-rs-4aunm`: identity ascension + no map
            // generated yet at construction; RunManager::with_ascension
            // and Self::generate_map populate these.
            ascension_level: 0,
            current_act_map: None,
            pinned_act_boss: None,
            generated_rooms: Vec::new(),
            // Bead `sts2-rs-7kptz`: snapshot the 3-act registry into a
            // per-run mutable Vec so the future events-visited counter
            // can increment per act.
            acts: crate::acts::all_acts().to_vec(),
            // Bead `sts2-rs-jyfvu`: pity-counter rarity roller defaults
            // to ascension 0 (identity at the current pin). Tests /
            // future ascension wiring construct via
            // `CardRarityOdds::new(level)`.
            card_rarity_odds: crate::card_rewards::CardRarityOdds::new(0),
            // Bead `sts2-rs-khzyk` (P9.2.f): default-construct the
            // unknown-room pity-odds at Python's base values.
            unknown_map_point_odds: crate::unknown_room_odds::UnknownMapPointOdds::new(),
            // Bead `sts2-rs-70anj.3.13`: pending-choice slot defaults
            // to `None` (no open prompt); callers (Toolbox,
            // ChoicesParadox, event rooms) open it via
            // `request_card_choice`.
            pending_choice: None,
            // Bead sts2-rs-ewm6b (P9.1.g): per-stream named RNGs
            // seeded from master 0 by default. Tests overriding seed
            // can reassign `rng` post-construction.
            rng: crate::run_rng_set::RunRngSet::new(0),
            // Bead `sts2-rs-597ws` (P9.1.f): relic grab-bag + boss-relic
            // pool default to empty; populator wires them at run-start.
            relic_grab_bag: Vec::new(),
            // Bead `sts2-rs-l9vkh` (P9.2.r): per-rarity buckets start
            // empty; `populate_relic_grab_bag_by_rarity` fills them at
            // run-start.
            relic_grab_bag_by_rarity: BTreeMap::new(),
            relic_grab_bag_fallback: Vec::new(),
            boss_relic_pool: Vec::new(),
            shared_relic_grab_bag_by_rarity: BTreeMap::new(),
            // Bead `sts2-rs-70anj.3.20`: Pael cluster trackers default
            // to zero — fresh run, pet at level 0, no sacrifices /
            // feeds yet.
            pael_state: crate::pael_state::PaelState::default(),
            // Bead `sts2-rs-vk9av`: PotionRewardOdds starts at the
            // Python initial value (0.40); the per-combat swing
            // pushes it toward 0.50 across runs.
            potion_reward_odds: crate::card_rewards::PotionRewardOdds::new(),
            // Bead `sts2-rs-v9zer`: PaelsTooth per-run card storage
            // — empty on a fresh run; populated by the after_obtained
            // listener once the relic is picked up.
            paels_tooth_stored: Vec::new(),
            // Bead `sts2-rs-8i1kn` (P9.2.g.3): run-terminal flags
            // default to `false` (run in progress).
            is_over: false,
            player_won: false,
            // Bead `sts2-rs-oyzw5`: A10 DoubleBoss flag defaults off;
            // `RunManager::with_ascension` flips it on at asc ≥ 10.
            has_double_boss: false,
            // GH #544: the act-1 variant defaults to the historical
            // single-Act-1 behaviour. `RunManager::with_act1` overrides it.
            act1: ActVariant::Overgrowth,
            // Bead `sts2-rs-xbkhd` (P9.3.b): default to Ironclad to
            // match Python's `RunState.__init__(character_id="Ironclad")`.
            // `RunManager::with_ascension` overwrites this with the
            // caller-supplied class at run-construction.
            character_id: crate::potions::CharacterId::Ironclad,
            // Harvest-only metadata (AZ M1 Task 2): unknown until the first
            // EncounterDef-based combat entry sets it.
            current_encounter_idx: None,
        }
    }
}

impl RunState {
    /// Construct a fresh, empty `RunState`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current act index (1, 2, or 3). Mirrors Python
    /// `RunState.act_index`. Bead `sts2-rs-70anj.3.16`.
    #[must_use]
    pub fn act_index(&self) -> u8 {
        self.act_index
    }

    // -- Pael cluster trackers (bead `sts2-rs-70anj.3.20`) -------------
    //
    // Accessors / mutators for the shared Pael cluster ledger. Read by
    // Pael relic listeners (`PaelsWing` etc., not yet ported) instead
    // of those relics carrying private `RelicInstance` state.

    /// Pet maturity level. Mirrors a future `pael_pet_level` slot.
    #[must_use]
    pub const fn pael_pet_level(&self) -> u32 {
        self.pael_state.level
    }

    /// Cumulative count of card rewards sacrificed to the Pael
    /// cluster. Read by [`crate::RelicId::PaelsWing`]'s "every N
    /// sacrifices, obtain a random relic" branch.
    #[must_use]
    pub const fn pael_sacrificed_count(&self) -> u32 {
        self.pael_state.sacrificed_count
    }

    /// Cumulative count of feed-the-pet interactions across the run.
    #[must_use]
    pub const fn pael_fed_count(&self) -> u32 {
        self.pael_state.fed_count
    }

    /// Level the Pael pet up by `delta`. Saturates at `u32::MAX`.
    pub fn pael_level_up(&mut self, delta: u32) {
        self.pael_state.level = self.pael_state.level.saturating_add(delta);
    }

    /// Record one card-reward sacrifice. Returns the new cumulative
    /// sacrifice count so callers (e.g. `PaelsWing`) can branch on
    /// `count % N == 0` without re-reading the field.
    pub fn pael_record_sacrifice(&mut self) -> u32 {
        self.pael_state.sacrificed_count = self.pael_state.sacrificed_count.saturating_add(1);
        self.pael_state.sacrificed_count
    }

    /// Record one feed-the-pet interaction. Returns the new
    /// cumulative feed count.
    pub fn pael_record_feed(&mut self) -> u32 {
        self.pael_state.fed_count = self.pael_state.fed_count.saturating_add(1);
        self.pael_state.fed_count
    }

    /// Advance the run to the next act (1 → 2 → 3, clamped at 3).
    /// Called by the boss-clear flow once the act-boss room transitions.
    /// Bead `sts2-rs-70anj.3.16`.
    ///
    /// Per-act state reset (bead `sts2-rs-2c0ze`, mirroring Python
    /// `enter_act` at `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1577-1583`):
    /// clears [`Self::visited_map_coords`] and resets
    /// [`Self::act_floor`] to `0`. [`Self::total_floor`] is run-scoped
    /// and intentionally NOT reset. [`Self::map_point_history`] is
    /// also run-scoped (cross-act room-type counters need it).
    pub fn advance_act(&mut self) {
        if self.act_index < 3 {
            self.act_index += 1;
        }
        self.visited_map_coords.clear();
        self.act_floor = 0;
        self.monster_combats_this_act = 0;
        self.elite_combats_this_act = 0;
        // The previous act's pinned boss is now stale; the next act's
        // pre-roll will populate this afresh once the new act map is
        // generated and the content callbacks fire.
        self.pinned_act_boss = None;
    }

    /// Register a fresh visit to `coord` and update the run's floor
    /// counters. Returns `true` iff the visit was novel (i.e. `coord`
    /// was not already in [`Self::visited_map_coords`]); when a repeat
    /// visit lands, this is a no-op and the counters stay put.
    ///
    /// Side-effects on a fresh visit:
    /// 1. Push `coord` onto [`Self::visited_map_coords`].
    /// 2. Set [`Self::act_floor`] to `coord.row + 1`.
    /// 3. Bump [`Self::total_floor`] by `1`.
    /// 4. If both `map_point_type` and `room_type` are supplied,
    ///    append a [`crate::map::MapPointHistoryEntry`] via
    ///    [`Self::append_to_map_point_history`].
    ///
    /// Mirrors Python `RunState.add_visited_coord(coord, room_type)`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1599-1612`).
    /// Python's caller resolves `map_point_type` from `self.map`'s
    /// reverse-lookup or from a room→point fallback table; since
    /// `RunState` doesn't own an [`crate::map::ActMap`] at this Rust
    /// pin, the resolution lives in the caller (run-loop driver) and
    /// is threaded in as `map_point_type` directly. Callers that
    /// don't have a map-node-type to record (e.g. boss-rooms or
    /// non-map-driven transitions in early scaffolding) can pass
    /// `None` for `map_point_type` — Python conditionally appends
    /// history only when `room_type is not None`, and this Rust port
    /// strengthens that to "only when both are Some" so the history
    /// invariant ("every entry has both fields") is preserved.
    ///
    /// Bead `sts2-rs-2c0ze` (P9.2.m).
    pub fn add_visited_coord(
        &mut self,
        coord: crate::map::MapCoord,
        map_point_type: Option<crate::map::MapPointType>,
        room_type: Option<RoomType>,
    ) -> bool {
        if self.visited_map_coords.contains(&coord) {
            return false;
        }
        self.visited_map_coords.push(coord);
        self.act_floor = coord.row + 1;
        self.total_floor += 1;
        if let (Some(mpt), Some(rt)) = (map_point_type, room_type) {
            self.append_to_map_point_history(mpt, rt);
        }
        true
    }

    /// Append a [`crate::map::MapPointHistoryEntry`] to
    /// [`Self::map_point_history`]. Mirrors Python
    /// `RunState.append_to_map_point_history(map_point_type, room_type)`
    /// (`run_state.py:1614-1615`). Typically called from inside
    /// [`Self::add_visited_coord`]; exposed standalone so drivers that
    /// need to log a synthetic history entry (e.g. for boss rooms not
    /// driven by `add_visited_coord`) can do so directly. Bead
    /// `sts2-rs-2c0ze`.
    pub fn append_to_map_point_history(
        &mut self,
        map_point_type: crate::map::MapPointType,
        room_type: RoomType,
    ) {
        self.map_point_history
            .push(crate::map::MapPointHistoryEntry::new(
                map_point_type,
                room_type,
            ));
    }

    /// Count entries in [`Self::map_point_history`] matching the
    /// supplied filters. Either filter `None` means "any". Mirrors
    /// Python `RunState.count_map_point_history_entries(*, room_type,
    /// map_point_type)` (`run_state.py:1617-1628`). The Rust port
    /// drops the keyword-only call convention (not a Rust idiom) and
    /// surfaces both filters as plain `Option` args; both `None`
    /// returns the total entry count. Bead `sts2-rs-2c0ze`.
    #[must_use]
    pub fn count_map_point_history_entries(
        &self,
        room_type: Option<RoomType>,
        map_point_type: Option<crate::map::MapPointType>,
    ) -> usize {
        self.map_point_history
            .iter()
            .filter(|entry| match room_type {
                Some(rt) => entry.room_type == rt,
                None => true,
            })
            .filter(|entry| match map_point_type {
                Some(mpt) => entry.map_point_type == mpt,
                None => true,
            })
            .count()
    }

    /// Push `count` [`RelicReward`]s onto [`Self::relic_rewards`] for the
    /// given `relic_id` (bead `sts2-rs-bms`). Mirrors the relic-rewards
    /// path of Python `RunState.offer_relic_rewards` (`run_state.py:786`)
    /// minus the rarity / RNG plumbing — call-site beads will extend
    /// the signature when those surfaces land.
    ///
    /// Use this only when the granted relic is already known (events
    /// that grant a specific relic, treasure rooms after the rarity
    /// roll). For random-roll offers (Small/Large Capsule, CallingBell,
    /// ToyBox), use [`Self::offer_random_relic_rewards`] which rolls a
    /// fresh id from the grab bag — passing the source relic here grants
    /// it again and re-fires `after_obtained`, causing infinite loops.
    pub fn offer_relic_rewards(&mut self, relic_id: RelicId, count: usize) {
        for _ in 0..count {
            self.relic_rewards.push(RelicReward::new(relic_id));
        }
    }

    /// Offer `rewards` as one set (C# `RewardsCmd.OfferCustom`; `can_skip`
    /// false for `WithSkippingDisallowed()`), to be rolled and shown by the run
    /// manager. Only under [`Self::interactive_rewards`]: a producer checks
    /// the flag and keeps its auto-collect path otherwise. An empty set is
    /// never shown (C# `RewardsSet.Offer` returns at once). sts2-agent fork.
    pub fn offer_custom_rewards(&mut self, rewards: Vec<OfferedReward>, can_skip: bool) {
        debug_assert!(
            self.interactive_rewards,
            "offered sets are an interactive_rewards path"
        );
        if !rewards.is_empty() {
            self.offered_reward_sets
                .push(OfferedRewardSet { rewards, can_skip });
        }
    }

    /// Roll a relic-reward rarity bucket via the `rewards` RNG stream.
    /// Mirrors Python `Player._roll_relic_rarity(rng=run_state.rng.rewards)`
    /// (`run_state.py:235-241`): 50% Common, 33% Uncommon, 17% Rare.
    /// Companion to [`Self::roll_treasure_relic_rarity`] (same buckets
    /// but on the `treasure_room` stream).
    pub fn roll_reward_relic_rarity(&mut self) -> RelicRarity {
        roll_relic_rarity(&mut self.rng.rewards)
    }

    /// Drop the first `relic_id` from the player's inventory — C#
    /// `RelicCmd.Remove` (`Commands/RelicCmd.cs:61`), which calls
    /// `RemoveRelicInternal` and then the relic's `AfterRemoved`.
    ///
    /// The relic does NOT go back in the grab bag; C# leaves it gone for the
    /// rest of the run. Returns whether anything was actually removed.
    ///
    /// **The `AfterRemoved` hook bus is not modelled** — there is no run-side
    /// dispatch for it. The one caller today is `RelicTrader`, whose stock is
    /// filtered to `IsTradable`, and relics with a meaningful pickup/removal
    /// effect are excluded from that filter, so nothing observable is dropped.
    /// Wire a dispatch here before adding a caller that trades freely.
    pub fn lose_relic(&mut self, relic_id: RelicId) -> bool {
        let Some(idx) = self.relics.iter().position(|&r| r == relic_id) else {
            return false;
        };
        self.relics.remove(idx);
        // Counters are keyed by id, so only drop the charge once the LAST
        // copy is gone (stackable relics can be held more than once).
        if !self.relics.contains(&relic_id) {
            self.relic_counters.remove(&relic_id);
        }
        true
    }

    /// Queue `count` random relic-reward picks, drawing each from the
    /// grab bag at the supplied rarity (rolled via
    /// [`Self::roll_reward_relic_rarity`] when `None`). Each pop is
    /// non-owned and drains the bag, so the same relic can't surface
    /// twice within one offer; an exhausted bag silently skips that
    /// slot (Python returns the `"CIRCLET"` sentinel — not modelled).
    ///
    /// Mirrors Python `Player.offer_relic_rewards(count, rarities=...)`
    /// (`run_state.py:1036`) which pushes `RelicReward(rarity=...)`
    /// sentinels that `RelicReward.populate` rolls into ids lazily; the
    /// Rust port rolls eagerly here because `RelicReward` carries only
    /// the resolved id.
    pub fn offer_random_relic_rewards(&mut self, count: usize, rarities: &[Option<RelicRarity>]) {
        self.offer_random_relic_rewards_wax(count, rarities, false);
    }

    /// [`Self::offer_random_relic_rewards`], optionally stamping every offer
    /// as wax. `ToyBox::AfterObtained` is the only `is_wax = true` caller
    /// (`ToyBox.cs:90-95` sets `IsWax` on each pulled model before wrapping
    /// it in a `RelicReward`).
    pub fn offer_random_relic_rewards_wax(
        &mut self,
        count: usize,
        rarities: &[Option<RelicRarity>],
        is_wax: bool,
    ) {
        for i in 0..count {
            let rarity = rarities
                .get(i)
                .copied()
                .flatten()
                .unwrap_or_else(|| self.roll_reward_relic_rarity());
            if let Some(id) = self.pop_relic_from_grab_bag(rarity) {
                self.relic_rewards.push(if is_wax {
                    RelicReward::new_wax(id)
                } else {
                    RelicReward::new(id)
                });
            }
        }
    }

    /// Roll a treasure-room relic-rarity bucket via the dedicated
    /// `treasure_room` RNG stream. Mirrors Python
    /// `Player._roll_relic_rarity(rng=run_state.rng.treasure_room)`
    /// at `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:235-241`:
    ///
    /// ```text
    /// roll = rng.next_float()  # [0, 1)
    /// 0.00 <= roll < 0.50  → COMMON
    /// 0.50 <= roll < 0.83  → UNCOMMON
    /// 0.83 <= roll < 1.00  → RARE
    /// ```
    ///
    /// Returns a [`crate::card_rewards::RewardRarity`] — the three
    /// variants (`Common` / `Uncommon` / `Rare`) line up exactly with
    /// the three relic-rarity buckets used by the treasure-room
    /// driver, and the enum is already cross-referenced by
    /// `card_rewards.rs`. Bead `sts2-rs-ycria` (P9.2.a).
    pub fn roll_treasure_relic_rarity(&mut self) -> crate::card_rewards::RewardRarity {
        let roll = self.rng.treasure_room.next_float(1.0);
        if roll < 0.50 {
            crate::card_rewards::RewardRarity::Common
        } else if roll < 0.83 {
            crate::card_rewards::RewardRarity::Uncommon
        } else {
            crate::card_rewards::RewardRarity::Rare
        }
    }

    /// Add `amount` to [`Self::max_potion_slots`], clamping at 0 so a
    /// negative delta cannot drive the slot count below zero. Mirrors
    /// Python `RunState.gain_potion_slots`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:316-317` —
    /// `self.max_potion_slots = max(0, self.max_potion_slots + amount)`).
    /// Note: Python also exposes a sibling method
    /// (`run_state.py:137-139`) that only applies positive deltas; this
    /// surface picks the stricter sibling that handles both directions,
    /// since downstream Sozu / slot-stealing surfaces will need it
    /// without a second method. Bead `sts2-rs-dsgf9`.
    ///
    /// Does NOT enforce the combat-side [`crate::potions::MAX_POTIONS`]
    /// ceiling — Python doesn't either; the relic spec gates apply at
    /// the call site (PotionBelt: +2, AlchemicalCoffer: +1, etc.).
    pub fn gain_potion_slots(&mut self, amount: i32) {
        self.max_potion_slots = self.max_potion_slots.saturating_add(amount).max(0);
    }

    /// Add `kind` to the run-level potion belt ([`Self::potions`]) if a
    /// free slot is available, returning `true` when the potion was
    /// added and `false` when the belt is already full (the potion is
    /// NOT added; whether that means dropped, counted, or deferred to a
    /// swap decision is the caller's business — reward claims should go
    /// through [`Self::claim_potion_reward`], which models the C#
    /// "discard one to make room" flow. GH #252). The slot ceiling is
    /// [`Self::max_potion_slots`] clamped to the combat-side
    /// [`crate::potions::MAX_POTIONS`] hard cap. Bead `sts2-rs-01o`.
    pub fn add_potion_to_belt(&mut self, kind: crate::potions::PotionKind) -> bool {
        let cap = (self.max_potion_slots.max(0) as usize).min(crate::potions::MAX_POTIONS);
        if self.potions.len() >= cap {
            return false;
        }
        // C# `Player.AddPotionInternal`: the potion fills the FIRST empty
        // physical slot (`_potionSlots.IndexOf(null)`), and the visible
        // belt is non-empty slots in slot order — so a drop claimed after
        // drinking slot 0 lists BEFORE the held potion, not after it.
        self.sync_potion_slot_indices();
        let mut slot: u8 = 0;
        while self.potion_slot_indices.contains(&slot) {
            slot += 1;
        }
        let pos = self
            .potion_slot_indices
            .iter()
            .position(|&s| s > slot)
            .unwrap_or(self.potions.len());
        self.potions.insert(pos, kind);
        self.potion_slot_indices.insert(pos, slot);
        true
    }

    /// Remove the potion at visible index `idx` from the run-level belt,
    /// leaving its physical slot empty (C# `Player.RemovePotionInternal`
    /// nulls the slot — no compaction).
    pub fn remove_potion_from_belt(&mut self, idx: usize) -> crate::potions::PotionKind {
        self.sync_potion_slot_indices();
        self.potion_slot_indices.remove(idx);
        self.potions.remove(idx)
    }

    /// Re-derive [`Self::potion_slot_indices`] as the compact `0..len`
    /// mapping when it has fallen out of parallel with [`Self::potions`]
    /// (fixtures that push/clear `potions` directly).
    fn sync_potion_slot_indices(&mut self) {
        if self.potion_slot_indices.len() != self.potions.len() {
            self.potion_slot_indices = (0..self.potions.len() as u8).collect();
        }
    }

    /// Claim a potion REWARD (post-combat drop, event / relic
    /// `Reward::Potion` sentinel): [`Self::add_potion_to_belt`] when a
    /// slot is free; on a full belt, either queue an interactive
    /// discard-to-claim decision ([`Self::pending_potion_swaps`], when
    /// [`Self::interactive_potion_swap`] is set) or count the potion as
    /// irrecoverably lost ([`Self::potions_overflow_dropped`]). Returns
    /// `true` iff the potion landed in the belt NOW — a queued swap
    /// resolves later through `RunAction::PotionSwap`.
    ///
    /// Reward claims only — direct procure loops with no reward screen
    /// (AlchemicalCoffer's `PotionCmd.TryToProcure` fill) drop on TooFull
    /// in C# too and must not queue a swap. GH #252.
    pub fn claim_potion_reward(&mut self, kind: crate::potions::PotionKind) -> bool {
        if self.add_potion_to_belt(kind) {
            return true;
        }
        if self.interactive_potion_swap {
            self.pending_potion_swaps.push(kind);
        } else {
            self.potions_overflow_dropped += 1;
        }
        false
    }

    /// Set [`Self::current_room`] to `room` and fire the
    /// `after_room_entered` hook channel
    /// ([`crate::run_effects::dispatch_after_room_entered`]) with the
    /// new room's [`RoomType`].
    ///
    /// Mirrors Python `RunState.advance_to_room(idx)` (`run_state.py`)
    /// which sets `self.current_room` and fires
    /// `relic.after_room_entered(room, run_state)`. Bead `sts2-rs-ouv`
    /// (j87-c). Idempotent at the consumer level — calling
    /// `enter_room` twice with the same value still fires the
    /// dispatcher both times (matches Python: each room transition is
    /// an event, even if the engine briefly re-enters the same room
    /// type).
    pub fn enter_room(&mut self, room: Room) {
        let room_type = room.room_type;
        // A new room ends any event's belt lock (`enter_event_room` sets it
        // after this for the two events that lock it).
        self.potions_locked = false;
        // Bead `sts2-rs-2mjei`: when the room carries a map-coord +
        // node-type (i.e. was constructed via `Room::at`), auto-bump
        // the map-traversal trackers — `visited_map_coords`,
        // `act_floor`, `total_floor`, and append a
        // `MapPointHistoryEntry`. Mirrors Python's `_enter_room` flow
        // at `sts2-rl-agent@01c094d:sts2_env/run/run_manager.py`
        // where the driver calls `add_visited_coord(coord, room_type)`
        // before `advance_to_room`. Repeat-visit semantics live in
        // `add_visited_coord` itself (no double-counting). Coord-less
        // rooms (combat-only scaffolding, boss rooms not driven by the
        // act-map, parity fixtures) skip this branch and preserve the
        // original behaviour.
        if let (Some(coord), Some(mpt)) = (room.map_coord, room.map_point_type) {
            let _ = self.add_visited_coord(coord, Some(mpt), Some(room_type));
        }
        self.current_room = Some(room);
        let _ = crate::run_effects::dispatch_after_room_entered(self, room_type);
    }

    /// Enter an event room: set [`Self::current_event`] to `event_id`
    /// and route the room transition through [`Self::enter_room`]
    /// with [`RoomType::Event`] (so `after_room_entered` listeners
    /// — `MawBank`, `DreamCatcher`, etc. — still fire for the event
    /// room as well).
    ///
    /// Pairs with [`Self::exit_event_room`], which the
    /// `sts2-content::events::dispatch::resolve` helper invokes once
    /// a choice yields `EventOutcome { finished: true, .. }`.
    ///
    /// Mirrors Python's transition into the event room (the per-room
    /// driver in `RunState.advance_to_room`): the `EventModel`
    /// instance is bound to the room, options are presented, the
    /// agent picks one, and on `finished=True` the event is cleared
    /// so the run loop can request the next room.
    ///
    /// Bead `sts2-rs-6ynt.4.1` (P7 events framework).
    pub fn enter_event_room(&mut self, event_id: crate::events::EventId) {
        self.current_event = Some(event_id);
        // Fresh event → fresh multi-page scratch (page 0, no RNG, no
        // slots). Single-page events never read it. Bead `sts2-rs-2guyj`.
        self.event_state = crate::events::MultiPageEventState::default();
        // Preserve the map coord/point-type of the room we're standing in.
        // Entering a coord-less `Room::new` here wiped `current_room.map_coord`
        // to `None`, so the next `MapChoice` tick read coord=None, fell back to
        // the start row's children, and teleported the player back to row 1 on
        // every event visit (bead `sts2-rs-26qg3` patched this after-the-fact in
        // `do_event_choice`, but the workbench's `materialize_event` re-enters
        // the room on *arrival*, before any choice, so the coord was already
        // gone). When entered from a map node — the normal case — the current
        // room already carries the destination coord; carry it into the Event
        // room. `add_visited_coord` is idempotent, so re-entering the same coord
        // does not double-count the visit or floor. The coord-less branch keeps
        // the original behaviour for callers with no standing room (Neow at run
        // start). Bead `sts2-rs-26qg3` follow-up.
        let room = match (
            self.current_room.and_then(|r| r.map_coord),
            self.current_room.and_then(|r| r.map_point_type),
        ) {
            (Some(coord), Some(point_type)) => Room::at(coord, point_type, RoomType::Event),
            _ => Room::new(RoomType::Event),
        };
        self.enter_room(room);
        // `BeforeEventStarted`: `Owner.CanRemovePotions = false`.
        self.potions_locked = matches!(
            event_id,
            crate::events::EventId::TheFutureOfPotions | crate::events::EventId::StoneOfAllTime
        );
    }

    /// Roll the run-start Neow offers if not already cached. Idempotent:
    /// subsequent calls within the same run see [`Self::neow_offers`]
    /// already populated and return without re-rolling, mirroring C#
    /// `AncientEventModel._generatedOptions` caching at
    /// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models/AncientEventModel.cs`.
    ///
    /// The Neow event RNG is seeded per `EventModel.cs:193`:
    /// `Rng(uint(RunState.Rng.Seed + NetId + deterministic_hash_code(Id.Entry)))`.
    /// For single-player runs `NetId == 0` is assumed (parity sub-bead
    /// `sts2-rs-k8z6m` will verify against the scraped corpus and tune
    /// the seed formula here if the assumption is off — see the
    /// `NEOW_EVENT_RNG_NAME` constant below).
    ///
    /// Bead `sts2-rs-14cid` (R3.1.2 under `sts2-rs-nr8l4`).
    pub fn roll_neow_offers_if_needed(&mut self) {
        if self.neow_offers.is_some() {
            return;
        }
        // v0.107.1 `EventModel.cs:238`:
        //   Rng = new Rng((uint)((uint)((int)RunState.Rng.Seed
        //          + ((!IsShared) ? GetPlayerSlotIndex(Owner) : 0))
        //          + GetDeterministicHashCode(Id.Entry)))
        //
        // - `Id.Entry` for the Neow event is "NEOW" (the SCREAMING_SNAKE_CASE
        //   form of the class name `Neow`; cross-checked against
        //   `LostWisp.cs::SetEventFinished(L10NLookup("LOST_WISP.pages..."))`
        //   which uses the same key-derivation convention).
        // - Neow is not shared (`IsShared = false` per the default in
        //   `EventModel.cs:85`), so the player-slot offset applies — but
        //   `GetPlayerSlotIndex(Owner) == Players.IndexOf(Owner) == 0` in
        //   single-player (`RunState.cs:620-623`), so the additive term is 0.
        //   v0.103.2 used `Owner.NetId == 1` (`EventModel.cs:193`,
        //   `NetSingleplayerGameService.cs:21`); v0.107.1 swapped `NetId` for
        //   `GetPlayerSlotIndex` and thereby dropped the +1 (bead `sts2-rs-ms8mm`).
        //
        // `DotNetRng::with_name(s, n) = Rng(s + deterministic_hash_code(n))`
        // (modulo u32 wrap), matching the C# seed with a 0 player term.
        let mut event_rng = crate::DotNetRng::with_name(self.rng.seed, NEOW_EVENT_RNG_NAME);
        self.neow_offers = Some(crate::roll_neow_offers(&mut event_rng));
    }

    /// Clear [`Self::current_event`]. Called by the events dispatcher
    /// once a choice resolves to a finished outcome. Returns the
    /// previously-active event id, if any.
    ///
    /// Bead `sts2-rs-6ynt.4.1` (P7 events framework).
    pub fn exit_event_room(&mut self) -> Option<crate::events::EventId> {
        // Drop any multi-page scratch so a later single-page event can't
        // read a stale page/RNG/slot. Bead `sts2-rs-2guyj`.
        self.event_state = crate::events::MultiPageEventState::default();
        self.current_event.take()
    }

    /// Consume the [`RelicReward`] at `idx` from [`Self::relic_rewards`],
    /// push its `relic_id` onto [`Self::relics`], and fire the
    /// `after_obtained` hook channel
    /// ([`crate::run_effects::dispatch_after_obtained`]).
    ///
    /// Returns the acquired [`RelicId`] on success, or `None` if `idx`
    /// is out of bounds. Mirrors Python `RunState.take_relic_reward`
    /// (`run_state.py`) which appends to `self.relics` and fires
    /// `relic.after_obtained(combat=None, run_state=self)`. Bead
    /// `sts2-rs-zq8` (j87-b).
    pub fn accept_relic_reward(&mut self, idx: usize) -> Option<RelicId> {
        if idx >= self.relic_rewards.len() {
            return None;
        }
        let reward = self.relic_rewards.remove(idx);
        let relic_id = reward.relic_id;
        // C# stamps `IsWax` on the model BEFORE the reward is offered, so
        // the flag is already live when `AfterObtained` listeners run.
        if reward.is_wax {
            self.wax_relics.insert(relic_id);
        }
        self.obtain_relic(relic_id);
        Some(relic_id)
    }

    /// True when a just-completed relic/event grant has left an unresolved
    /// interactive deck pick or reward offering on the RunState queues — the
    /// run loop must surface and resolve it before the grant batch settles.
    /// RunState-side subset of the `RunManager` settle predicate (which also
    /// requires its `card_reward_options` empty). Used by multi-relic grant
    /// listeners ([`crate::run_effects`] NeowsBones) to decide whether the
    /// previously-granted relic surfaced a choice and the next grant must be
    /// deferred onto [`Self::pending_post_pick_relic_grants`].
    #[must_use]
    pub fn has_pending_grant_choice(&self) -> bool {
        self.pending_deck_removals != 0
            || self.pending_deck_upgrades != 0
            || !self.pending_deck_transforms.is_empty()
            || !self.pending_deck_enchants.is_empty()
            || !self.pending_card_selects.is_empty()
            || !self.pending_rewards.is_empty()
            || !self.relic_rewards.is_empty()
            || !self.pending_bundles.is_empty()
            || !self.offered_reward_sets.is_empty()
    }

    /// Drain every queued [`Self::relic_rewards`] entry by calling
    /// [`Self::obtain_relic`] in FIFO order, looping until the queue is
    /// empty so chained follow-ups (e.g. SmallCapsule → AmethystAubergine
    /// → … under [`Self::direct_event_relic_grant`]) all resolve in one
    /// pass. Mirrors C#'s `RewardsSet.Offer()` under `TestMode.IsOn`
    /// (`MegaCrit.Sts2.Core.Rewards/RewardsSet.cs:125-136`): every reward
    /// has `OnSelectWrapper()` auto-invoked, with no UI step. Run-loop
    /// drivers call this after event/reward resolution when
    /// [`Self::direct_event_relic_grant`] is set so the engine doesn't
    /// surface a phantom `PickRelicReward / SkipRelic` decision for
    /// follow-up rewards an after-obtain listener queued. Returns the
    /// number of rewards drained.
    pub fn drain_relic_rewards_directly(&mut self) -> usize {
        let mut drained = 0;
        // FIFO drain: pop from the front so chain order matches the C#
        // List<Reward> iteration in RewardsSet.Offer(). Looping handles
        // the chained case where the inner obtain_relic re-queues.
        while !self.relic_rewards.is_empty() {
            let reward = self.relic_rewards.remove(0);
            if reward.is_wax {
                self.wax_relics.insert(reward.relic_id);
            }
            self.obtain_relic(reward.relic_id);
            drained += 1;
        }
        drained
    }

    /// The player's relics minus the melted ones, in inventory order — the
    /// run-side hook-listener view C# builds at `RunState.cs:148`. Borrows
    /// [`Self::relics`] outright in the (overwhelmingly common) case where
    /// nothing has been melted.
    #[must_use]
    pub fn active_relics(&self) -> std::borrow::Cow<'_, [RelicId]> {
        if self.melted_relics.is_empty() {
            std::borrow::Cow::Borrowed(&self.relics)
        } else {
            std::borrow::Cow::Owned(
                self.relics
                    .iter()
                    .copied()
                    .filter(|id| !self.melted_relics.contains(id))
                    .collect(),
            )
        }
    }

    /// True iff the player owns `relic_id` AND it has not been melted.
    ///
    /// This is the run-layer twin of the combat-side
    /// `relic_effects::helpers::player_owns_relic`: C# builds the run-level
    /// hook-listener list from `player2.Relics.Where(r => !r.IsMelted)`
    /// (`MegaCrit.Sts2.Core.Runs/RunState.cs:148`), so a melted relic's
    /// run hooks (rewards, map, shop-price, rest-site, card-reward) all go
    /// silent while the relic itself stays in the inventory.
    ///
    /// Use this for every *hook guard*. Keep using [`Self::relics`] directly
    /// for anything that counts, serializes, or pool-excludes relics — C#
    /// does not filter those by melt.
    #[must_use]
    pub fn owns_active_relic(&self, relic_id: RelicId) -> bool {
        self.relics.contains(&relic_id) && !self.melted_relics.contains(&relic_id)
    }

    /// Melt the first still-unmelted wax relic in inventory order, mirroring
    /// C# `ToyBox.AfterCombatEnd`'s
    /// `Owner.Relics.FirstOrDefault(r => r != null && r.IsWax && !r.IsMelted)`
    /// followed by `RelicCmd.Melt` (`ToyBox.cs:105-108`). Returns the melted
    /// relic, or `None` when nothing is left to melt.
    pub fn melt_next_wax_relic(&mut self) -> Option<RelicId> {
        let victim = self
            .relics
            .iter()
            .copied()
            .find(|id| self.wax_relics.contains(id) && !self.melted_relics.contains(id))?;
        self.melted_relics.insert(victim);
        Some(victim)
    }

    /// Add `relic_id` to the player's relics and fire the RunState-side
    /// `after_obtained` channel (under deferred follow-up rewards). Shared
    /// by [`Self::accept_relic_reward`] (the queued-reward path) and the
    /// direct-grant event path (e.g. Neow when
    /// [`Self::direct_event_relic_grant`] is set), so on-obtain listeners
    /// fire identically regardless of how the relic was acquired.
    pub fn obtain_relic(&mut self, relic_id: RelicId) {
        self.relics.push(relic_id);
        // C# `RelicCmd.Obtain` removes a non-stackable relic from BOTH
        // the player bag and the SHARED bag (`RelicCmd.cs:29-30`). Strip
        // it from the shared per-rarity buckets here so a relic obtained
        // off the reward/event path can't later resurface from the
        // treasure-room pull. Stackable relics never live in the grab
        // bags, so this is a no-op for them (no stackable gate needed).
        self.remove_relic_from_shared_grab_bag(relic_id);
        // Bead `sts2-rs-pfiwm`: initialize the run-side counter for
        // countdown relics at pickup, mirroring C# constructing the
        // relic instance with its `_combatsLeft` seeded (EmberTea = 5,
        // BoneTea / TeaOfDiscourtesy = 1, SwordOfStone = 5). Without
        // this the run carries no counter until the first combat seeds
        // it lazily via `RelicKind::initial_counter`; that produces the
        // same combat behavior, but a run-side reader of the counter
        // (display / reward listener) before any combat would see 0
        // instead of the real starting charge. `entry().or_insert`
        // never clobbers an already-recorded counter. Relics with a
        // zero initial counter (the vast majority) stay absent, keeping
        // the map sparse. The five W9 Neow relics map to `None`.
        if let Some(kind) = relic_id.to_relic_kind() {
            let initial = kind.initial_counter();
            if initial != 0 {
                self.relic_counters.entry(relic_id).or_insert(initial);
            }
        }
        // Fire the RunState-side `after_obtained` channel. Listeners
        // registered via `run_effects::register_after_obtained` walk
        // every relic-acquisition; per-listener relic-id gating happens
        // inside the handler (the dispatcher does not key on RelicId
        // because some listeners — e.g. `MawBank` reacting to ANY
        // relic acquisition — care about the event itself, not the
        // identity of the newly-acquired relic).
        // Bead `sts2-rs-70anj.3.9`: stash the just-obtained relic id
        // so listeners can self-gate (Astrolabe et al.) without
        // broadening the channel signature. Cleared once dispatch
        // returns so subsequent unrelated `after_obtained` walks (e.g.
        // re-entrant dispatch from a listener body) see `None`.
        self.last_obtained_relic = Some(relic_id);
        // Bead `sts2-rs-w785o`: redeeming a relic must run its
        // `after_obtained` listeners under `defer_followup_rewards`
        // so any "give the player a follow-up effect" listener (e.g.
        // WarPaint upgrading Skills) queues a player-pickable reward
        // instead of auto-applying. Python pin
        // `run/reward_objects.py:190-199`.
        self.with_deferred_followup_rewards(|rs| {
            let _ = crate::run_effects::dispatch_after_obtained(rs);
        });
        self.last_obtained_relic = None;
    }

    /// Grant a relic from an EVENT that C# gives via `RelicCmd.Obtain<T>`
    /// (immediate, no follow-up "Take relic / Skip" reward step). Under
    /// [`Self::direct_event_relic_grant`] (the run-parity / workbench flow that
    /// matches C#) obtain it inline so on-obtain effects fire right away;
    /// otherwise queue it as a relic-reward (the Python-parity flow). The
    /// single source of truth for the `act3_glory::grant_neow_relic` /
    /// `RoomFullOfCheese.search` / SunkenStatue / … event grant gate.
    pub fn grant_event_relic(&mut self, relic_id: RelicId) {
        if self.direct_event_relic_grant {
            self.obtain_relic(relic_id);
        } else {
            self.offer_relic_rewards(relic_id, 1);
        }
    }

    /// Add `delta` to both [`Self::max_hp`] and [`Self::current_hp`] and
    /// broadcast through
    /// [`crate::run_effects::dispatch_after_max_hp_changed`].
    ///
    /// Mirrors Python `RunState.gain_max_hp(amount)`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:308`): both
    /// `max_hp` and `current_hp` increase by `amount`. Negative
    /// `delta` is supported as a convenience and `max_hp` is clamped
    /// to a floor of `1` (mirroring Python `lose_max_hp`'s
    /// `max(max_hp, 1)` invariant; bead `sts2-rs-vd9tm`). Arithmetic
    /// uses `saturating_add` so extreme deltas saturate rather than
    /// wrap. Calls with `delta == 0` still fire the dispatcher so
    /// listeners that care about every gain-event invocation see them
    /// (matches the relic-side `after_max_hp_changed` convention
    /// pre-`tcf`).
    ///
    /// Bead `sts2-rs-xqvn` (P7-surf prereq for `sts2-rs-46ms`).
    pub fn gain_max_hp(&mut self, delta: i32) {
        // saturating_add guards i32 wrap on stacked relic deltas; the
        // post-delta `.max(1)` floor ports Python's lose_max_hp
        // `max(max_hp, 1)` clamp so a negative delta cannot drive
        // max_hp <= 0 (which would silently break heal's `min(max_hp)`
        // invariant downstream).
        self.max_hp = self.max_hp.saturating_add(delta).max(1);
        self.current_hp = self.current_hp.saturating_add(delta);
        let ctx = crate::run_effects::MaxHpChangeRunCtx {
            delta,
            current_hp_delta: delta,
        };
        let _ = crate::run_effects::dispatch_after_max_hp_changed(self, &ctx);
    }

    /// Lose `amount` max HP the way the C# game does, then broadcast
    /// through [`crate::run_effects::dispatch_after_max_hp_changed`].
    ///
    /// This is **NOT** `gain_max_hp(-amount)`. C# `CreatureCmd.LoseMaxHp`
    /// (`MegaCrit.Sts2.Core.Commands/CreatureCmd.cs:600-617`) computes
    /// `new_max = max_hp - amount` and only *damages* current HP — by
    /// `current_hp - new_max` — when `new_max < current_hp` (a clamp-down);
    /// otherwise current HP is left untouched. `gain_max_hp` instead moves
    /// current HP by the full (negative) delta unconditionally, which
    /// over-subtracts whenever current HP already fit under the new max
    /// (e.g. A2 WearyTraveler's 80% start, where `current < max`). That
    /// drift was invisible at ascension 0 — there `current == max`, so
    /// `new_max < current` always holds and both formulas agree — but a
    /// `LeafyPoultice`/`DistinguishedCape`-style max-HP loss at A2+ left
    /// the player 12/9 HP too low. Mirrors the combat-side
    /// [`crate::Creature::lose_max_hp`], which was already faithful.
    pub fn lose_max_hp(&mut self, amount: i32) {
        if amount <= 0 {
            return;
        }
        let old_max = self.max_hp;
        let old_current = self.current_hp;
        self.max_hp = (self.max_hp - amount).max(1);
        if self.current_hp > self.max_hp {
            self.current_hp = self.max_hp;
        }
        let ctx = crate::run_effects::MaxHpChangeRunCtx {
            delta: self.max_hp - old_max,
            current_hp_delta: self.current_hp - old_current,
        };
        let _ = crate::run_effects::dispatch_after_max_hp_changed(self, &ctx);
    }

    /// Heal the player by up to `amount` HP, clamped to
    /// [`Self::max_hp`]. Returns the actual delta applied to
    /// [`Self::current_hp`] (may be `< amount` when the heal would
    /// exceed `max_hp`; may be `0` when already at full HP). Broadcasts
    /// through [`crate::run_effects::dispatch_after_max_hp_changed`]
    /// with `delta == 0` and `current_hp_delta == applied`.
    ///
    /// Mirrors Python `RunState.heal(amount)`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:110-113`):
    /// `current_hp = min(current_hp + amount, max_hp)`; returns the
    /// signed delta. Bead `sts2-rs-xqvn`.
    pub fn heal(&mut self, amount: i32) -> i32 {
        let before = self.current_hp;
        let after = self.current_hp.saturating_add(amount).min(self.max_hp);
        self.current_hp = after;
        let applied = after - before;
        let ctx = crate::run_effects::MaxHpChangeRunCtx {
            delta: 0,
            current_hp_delta: applied,
        };
        let _ = crate::run_effects::dispatch_after_max_hp_changed(self, &ctx);
        applied
    }

    /// Apply the Ancient-event entry heal — the between-acts recovery
    /// that lets a multi-act run survive boss damage. Every act opens
    /// with its Ancient event (Neow for act 1; the boss-relic Ancients
    /// for acts 2/3), whose `BeforeEventStarted` heals the player toward
    /// full HP. At base ascension the full missing amount is restored
    /// (heal to full); at ascension >= 2 `WearyTraveler` recovers only
    /// 80% of the missing HP.
    ///
    /// Faithful to C#
    /// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models/AncientEventModel.cs`:
    /// `amount = MaxHp - CurrentHp; if (WearyTraveler) amount *= 0.8m;
    /// Heal(amount)`, where `Creature.SetCurrentHpInternal` truncates the
    /// decimal result (`CurrentHp = (int)Math.Min(amount, MaxHp)`) — so
    /// the A2 heal floors `0.8 * missing`. Returns the HP actually added.
    /// Neow (act 1) is handled at run construction in
    /// [`crate::run_manager::RunManager::with_ascension`].
    pub fn apply_ancient_event_heal(&mut self) -> i32 {
        let missing = self.max_hp - self.current_hp;
        if missing <= 0 {
            return 0;
        }
        let amount = if self.ascension_level >= WEARY_TRAVELER_ASCENSION_LEVEL {
            (f64::from(missing) * WEARY_TRAVELER_HEAL_MULTIPLIER).floor() as i32
        } else {
            missing
        };
        self.heal(amount)
    }

    /// Offer rest-site heal rewards. Builds an empty rewards vector,
    /// walks every listener registered on
    /// [`crate::run_effects::dispatch_modify_rest_site_heal_rewards`]
    /// so it can append (e.g. `DreamCatcher` pushes a `CardReward`),
    /// and returns the final list. Mirrors Python's rest-site reward
    /// fan-out at `RunState.do_room_rest` invoking each relic's
    /// `modify_rest_site_heal_rewards`. Bead `sts2-rs-9wx7`.
    pub fn offer_rest_site_rewards(&mut self) -> Vec<Reward> {
        let mut rewards: Vec<Reward> = Vec::new();
        let _ = crate::run_effects::dispatch_modify_rest_site_heal_rewards(self, &mut rewards);
        rewards
    }

    /// Compute the rest-site REST heal amount, folding every
    /// listener registered on
    /// [`crate::run_effects::dispatch_modify_rest_site_heal_amount`]
    /// onto the supplied `base`. The base mirrors Python's
    /// `RunState.do_room_rest`'s `amount = self.player.max_hp // 3`
    /// before relic mods. Listeners (e.g. `RegalPillow` +15) return
    /// additive deltas. Bead `sts2-rs-2tsrk`.
    #[must_use]
    pub fn compute_rest_site_heal_amount(&self, base: i32) -> i32 {
        crate::run_effects::dispatch_modify_rest_site_heal_amount(self, base)
    }

    /// Apply a REST-option rest-site heal end-to-end: folds the
    /// `modify_rest_site_heal_amount` channel onto `base`, applies the
    /// heal to [`Self::current_hp`] (clamped to [`Self::max_hp`]), then
    /// fires the `after_rest_site_heal` channel with the actual amount
    /// added (post-clamp). Returns the actually-applied delta.
    ///
    /// Mirrors Python `RunState.do_room_rest`'s heal leg at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py` —
    /// `amount = compute_rest_site_heal_amount(base); current_hp =
    /// min(max_hp, current_hp + amount); for relic in relics:
    /// relic.after_rest_site_heal(owner, healed=amount, ...)`. The
    /// reward fan-out (`offer_rest_site_rewards`) stays a separate call
    /// — callers compose the full do_room_rest semantics. Bead
    /// `sts2-rs-70anj.3.17`.
    pub fn rest_site_heal(&mut self, base: i32) -> i32 {
        let amount = self.compute_rest_site_heal_amount(base).max(0);
        let healed = if amount == 0 {
            0
        } else {
            let before = self.current_hp;
            self.current_hp = (self.current_hp.saturating_add(amount)).min(self.max_hp);
            self.current_hp - before
        };
        let _ = crate::run_effects::dispatch_after_rest_site_heal(self, healed);
        // C# `HealRestSiteOption.ExecuteRestSiteHeal` (`HealRestSiteOption.cs:102-109`)
        // then builds the reward list via `Hook.ModifyRestSiteHealRewards` and
        // offers it (`RewardsCmd.OfferCustom`); the headless oracle auto-collects.
        // Grant the offered rewards now so the rest-site reward fan-out (Tiny
        // Mailbox potions, Dream Catcher card) actually fires — previously
        // `offer_rest_site_rewards` had ZERO production callers, so resting with
        // Tiny Mailbox gained no potion (sts2-rs-RP23).
        self.grant_rest_site_rewards();
        healed
    }

    /// Offer + grant the rest-site heal rewards (C# `ExecuteRestSiteHeal`'s
    /// `ModifyRestSiteHealRewards` + `OfferCustom` leg). `Reward::Potion`
    /// routes through [`Self::procure_potion`] (Sozu veto + belt-slot
    /// overflow); any other reward kind is queued onto [`Self::pending_rewards`]
    /// for the run loop to drain (e.g. Dream Catcher's `CardReward`). Bead
    /// `sts2-rs-RP23`.
    fn grant_rest_site_rewards(&mut self) {
        for reward in self.offer_rest_site_rewards() {
            match reward {
                Reward::Potion(p) => {
                    let _ = self.procure_potion(p);
                }
                other => self.pending_rewards.push(other),
            }
        }
    }

    /// Count of master-deck cards that can be removed. C# spells this out
    /// once, in `CookRestSiteOption.GetRemovableCardCount`
    /// (`CookRestSiteOption.cs:64`): `Deck.Cards.Count(c => c.IsRemovable)`.
    /// Pael's PaelsTooth offer counts the same thing (`Pael.cs:166`), and
    /// both are wired here.
    ///
    /// Until 2026-08-26 this filtered on the legacy `CardId::is_removable`,
    /// which excluded basic Strike/Defend — so a starting deck of 10 basics
    /// counted 0 removable cards, disabling Cook on a deck where C# enables
    /// it, and it counted Eternal curses that C# excludes. Both ends wrong,
    /// in opposite directions. Bead `sts2-rs-ekf4j`.
    #[must_use]
    pub fn removable_deck_card_count(&self) -> usize {
        let removable = crate::lookup_card_removable();
        self.master_deck
            .iter()
            .filter(|c| removable.map_or(true, |f| f(c)))
            .count()
    }

    /// Master-deck indices in C# `CardSelectCmd.FromDeckForRemoval` offer order
    /// (`CardSelectCmd.cs:405-409`): Curse-type cards sorted to the FRONT via
    /// the stable key `(Type != Curse) ? deck.IndexOf(c) : -999999999`,
    /// everything else in deck order. The lockstep harness picks an index into
    /// THIS order, so the removal offer AND resolution must both map through it
    /// — when a curse is not already at the deck front, raw master-deck order
    /// maps every removal index to a different card than C# (RP94 Necrobinder).
    ///
    /// C# `FromDeckForRemoval` first filters `Where(c.IsRemovable)`
    /// (`CardSelectCmd.cs:405-409`), where `CardModel.IsRemovable =>
    /// !Keywords.Contains(CardKeyword.Eternal)` (`CardModel.cs:574`). Eternal
    /// cards (Greed `{Eternal,Unplayable}`, AscendersBane `{Eternal,…}`) are
    /// EXCLUDED from the offer, so they never occupy an offer index nor shift
    /// the lockstep pick. We mirror this via the content-registered
    /// [`sts2_core::lookup_card_removable`] predicate (Eternal keyword lookup).
    ///
    /// The predicate reads the whole `Card`, not its id: `TezcatarasEmber`
    /// adds the Eternal keyword on enchant (`TezcatarasEmber.cs:18`) and
    /// NutritiousSoup applies it to every basic Strike, so an id-only check
    /// offers cards the game withholds — and since these indices are
    /// positional, one wrongly-offered card shifts every later pick.
    /// When the content registry isn't installed (pure-core tests) the
    /// predicate is absent and every card is kept (legacy unfiltered order).
    /// Bead `sts2-rs-RP22`.
    #[must_use]
    pub fn removal_offer_order(&self) -> Vec<usize> {
        let removable = crate::lookup_card_removable();
        let mut order: Vec<usize> = (0..self.master_deck.len())
            .filter(|&i| removable.map_or(true, |f| f(&self.master_deck[i])))
            .collect();
        // Stable sort: Curse → key 0 (front, deck order preserved), else key 1
        // (deck order). `sort_by_key` is stable, matching C#'s OrderBy.
        order.sort_by_key(|&i| {
            u8::from(self.master_deck[i].card_type != crate::cards::CardType::Curse)
        });
        order
    }

    /// `CardId`s in the master deck that are eligible for upgrade —
    /// non-upgraded cards only. Mirrors Python
    /// `RunState.upgradable_deck_cards`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:551`); the
    /// Python pin additionally checks `create_card(card_id,
    /// upgraded=True).upgraded`, which the Rust port collapses into
    /// "non-upgraded" because the upgrade table is exhaustive at this
    /// pin and any unknown id would have failed earlier in the
    /// pipeline. Bead `sts2-rs-70anj.3.14`.
    #[must_use]
    pub fn upgradable_deck_cards(&self) -> Vec<CardId> {
        // C# `FromDeckForUpgrade` filters `Where(IsUpgradable)` — exclude
        // non-upgradable Curse/Status so they don't occupy offer indices and
        // shift the lockstep pick (matches the RunManager driver path).
        self.master_deck
            .iter()
            .filter(|c| c.is_upgradable())
            .map(|c| c.card_id)
            .collect()
    }

    /// Push an [`Reward::UpgradeCards`] entry onto [`Self::pending_rewards`].
    ///
    /// If `cards` is supplied, those become the eligible upgrade pool
    /// verbatim (mirrors Python's `cards=` kwarg used by event-room
    /// retrofits and `WarHammer` to pre-filter). If `None`, the pool
    /// defaults to [`Self::upgradable_deck_cards`].
    ///
    /// Resolving the choice (the actual `upgrade_selected_cards` walk)
    /// will land alongside the run-side `PendingChoice` analogue; this
    /// method only queues the reward. Mirrors Python
    /// `RunState.offer_upgrade_cards_reward`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1095`). Bead
    /// `sts2-rs-70anj.3.14`.
    pub fn offer_upgrade_cards_reward(&mut self, count: usize, cards: Option<Vec<CardId>>) {
        let options = cards.unwrap_or_else(|| self.upgradable_deck_cards());
        self.pending_rewards
            .push(Reward::UpgradeCards(UpgradeCardsReward::new(
                count, options,
            )));
    }

    /// Queue a [`Reward::RemoveCard`] granting `count` card-removal picks.
    ///
    /// The single combat→run carry for `AddExtraReward(CardRemovalReward)`-
    /// style effects (C# `room.AddExtraReward`): a combat power computes its
    /// count at combat end and the `exit_combat` bridge calls this to stage it
    /// on [`Self::pending_rewards`]. `RunManager::drain_pending_rewards` later
    /// resolves it through [`Self::remove_cards_from_deck`]. No-op for
    /// `count == 0`. Bead `sts2-rs-nfoh0`.
    pub fn add_card_removal_reward(&mut self, count: u32) {
        if count == 0 {
            return;
        }
        self.pending_rewards.push(Reward::RemoveCard { count });
    }

    /// Push an [`Reward::EnchantCards`] entry onto
    /// [`Self::pending_rewards`]. Mirrors Python
    /// `PlayerState.offer_enchant_cards_reward`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1127-1146`).
    ///
    /// `cards` carries the candidate pool verbatim (Python's `cards=None`
    /// default is resolved by the caller in Rust — typically by walking
    /// the master deck with a relic-specific eligibility predicate).
    /// `min_count` of `None` matches Python's "must pick exactly
    /// `count`" semantics; `Some(0)` allows skipping. Bead
    /// `sts2-rs-2v8y2`.
    pub fn offer_enchant_cards_reward(
        &mut self,
        enchantment: EnchantmentId,
        amount: i32,
        count: usize,
        cards: Vec<CardId>,
        min_count: Option<usize>,
    ) {
        self.pending_rewards
            .push(Reward::EnchantCards(EnchantCardsReward::new(
                enchantment,
                amount,
                count,
                cards,
                min_count,
            )));
    }

    /// Run `body` with [`Self::defer_followup_rewards`] forced to `true`,
    /// restoring the prior value on return. Mirrors the Python pattern
    /// at `run/reward_objects.py:190-199` (RelicReward.select) and the
    /// `_deferred_followup_rewards` context manager at
    /// `run/run_manager.py:418-424`. Bead `sts2-rs-w785o`.
    pub fn with_deferred_followup_rewards<F>(&mut self, body: F)
    where
        F: FnOnce(&mut Self),
    {
        let previous = self.defer_followup_rewards;
        self.defer_followup_rewards = true;
        body(self);
        self.defer_followup_rewards = previous;
    }

    /// Build the base rest-site option list ([`RestSiteOption::Rest`] +
    /// [`RestSiteOption::Smith`]) and fold the
    /// `modify_rest_site_options` channel over it. Mirrors Python's
    /// `generate_rest_site_options(player)` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/rest_site.py:252`.
    ///
    /// At this pin `Mend` (multiplayer) and the per-card
    /// `modify_rest_site_options` walk are NOT included — multiplayer
    /// is single-player and per-card options have no consumer yet.
    /// Bead `sts2-rs-ekf4j`.
    pub fn rest_site_options(&self) -> Vec<RestSiteOption> {
        let has_upgradable = self.master_deck.iter().any(Card::is_upgradable);
        let mut options = vec![
            RestSiteOption::Rest,
            RestSiteOption::Smith {
                enabled: has_upgradable,
            },
        ];
        let _ = crate::run_effects::dispatch_modify_rest_site_options(self, &mut options);
        // C# `ByrdonisEgg.TryModifyRestSiteOptions` (`ByrdonisEgg.cs:19-27`):
        // when the player owns the egg it appends a `HatchRestSiteOption`.
        // The egg is the ONLY *card* implementing the modify hook (the other
        // implementers — Girya / Shovel / MeatCleaver / PaelsGrowth / Midas —
        // are relics/modifiers, already served by the relic-keyed channel
        // above), so a deck-scan here is the right-sized mirror rather than a
        // new card-keyed dispatch bus. In single-player, an egg in the master
        // deck means `player == Owner`. Appended AFTER the relic channel,
        // matching C# `Generate` (Heal/Smith first, then the
        // `Hook.ModifyRestSiteOptions` listener walk). Bead `sts2-rs-RP24`.
        if self
            .master_deck
            .iter()
            .any(|c| c.card_id == crate::cards::CardId::ByrdonisEgg)
        {
            options.push(RestSiteOption::Hatch);
        }
        options
    }

    /// Consult the `should_disable_remaining_rest_site_options` channel
    /// to decide whether picking one rest-site option locks the rest.
    /// Default at this pin is **`true`** (one-action-per-rest, matching
    /// the canonical Slay the Spire UX). `MiniatureTent` (Python
    /// `shop_event.py::MiniatureTent`) overrides to `false`, allowing
    /// the player to take multiple actions per rest stop.
    ///
    /// Consensus rule (matches Python's listener walk in
    /// `do_rest_site_action`): any explicit `Some(false)` wins (do not
    /// disable); else any `Some(true)` wins; else the default applies.
    /// Bead `sts2-rs-ekf4j` / `.4`.
    pub fn should_disable_remaining_rest_site_options(&mut self, chosen: RestSiteOption) -> bool {
        crate::run_effects::dispatch_should_disable_remaining_rest_site_options(self, chosen)
    }

    /// Apply the player's chosen rest-site action and return the
    /// outcome. Mirrors Python's `_enter_rest_site` →
    /// `RestSiteOption.execute(player, ...)` dispatch
    /// (`run_manager.py:739` / `rest_site.py:67-107`). Pure driver:
    /// composes the already-wired primitives (`rest_site_heal`,
    /// `request_card_choice`, etc.).
    ///
    /// Currently supports:
    /// - [`RestSiteOption::Rest`] → applies the 30%-of-max-HP heal
    ///   (Python `rest_site.py:34`: `floor(max_hp * 0.3)`) through
    ///   [`Self::rest_site_heal`], which folds the
    ///   `modify_rest_site_heal_amount` channel + fires
    ///   `after_rest_site_heal`. Returns
    ///   [`RestSiteActionOutcome::Rested`] carrying the post-clamp
    ///   delta. Reward fan-out (`offer_rest_site_rewards` →
    ///   DreamCatcher etc.) is the caller's responsibility — the
    ///   driver intentionally doesn't push rewards so the run loop
    ///   can compose them with the chosen option.
    /// - [`RestSiteOption::Smith`] (enabled) → if exactly one card
    ///   is upgradable, auto-upgrade it (matches Python's
    ///   `_can_auto_resolve_deck_choice` short-circuit when
    ///   `len(cards) <= min_count`); else open a `pending_choice`
    ///   over [`Self::upgradable_deck_cards`] for the agent to pick.
    /// - [`RestSiteOption::Smith`] (disabled) → no-op.
    ///
    /// - [`RestSiteOption::Hatch`] → obtains the `Byrdpip` relic
    ///   (C# `HatchRestSiteOption.OnSelect`), whose `after_obtained`
    ///   listener transforms every `ByrdonisEgg` in the deck into
    ///   `ByrdSwoop`. Returns [`RestSiteActionOutcome::Hatched`].
    ///
    /// Deferred (returns [`RestSiteActionOutcome::Unsupported`]):
    /// `Dig` (needs Shovel-relic-driven relic-pool draw), `Lift`
    /// (Girya `_times_lifted` counter), `Cook` (multi-card removal
    /// + max-HP gain), `Clone` / `Mend` (boutique relic hooks not yet
    /// wired). File a sibling bead per option once a caller needs it.
    ///
    /// Bead `sts2-rs-qwp1g` (P9.2.d).
    pub fn do_rest_site_action(&mut self, chosen: RestSiteOption) -> RestSiteActionOutcome {
        if !chosen.enabled() {
            return RestSiteActionOutcome::Disabled;
        }
        match chosen {
            RestSiteOption::Rest => {
                // Python `rest_site.py:34`: amount = floor(max_hp * 0.3).
                // Compute with integer math to avoid f64 introduction
                // in the engine's deterministic-state path.
                let base = (self.max_hp * 30) / 100;
                let healed = self.rest_site_heal(base);
                RestSiteActionOutcome::Rested { healed }
            }
            RestSiteOption::Smith { .. } => {
                let upgradable = self.upgradable_deck_cards();
                if upgradable.is_empty() {
                    // Should be unreachable when `chosen.enabled()` is
                    // true, but defensive: a stale Smith chosen after
                    // the deck became fully upgraded falls through to
                    // disabled.
                    return RestSiteActionOutcome::Disabled;
                }
                if upgradable.len() == 1 {
                    // Auto-resolve single-card case. Mirrors Python
                    // `_can_auto_resolve_deck_choice` when
                    // `len(cards) <= min_count == 1`.
                    let target_id = upgradable[0];
                    for c in &mut self.master_deck {
                        if c.card_id == target_id && !c.upgraded && c.upgrade() {
                            return RestSiteActionOutcome::SmithUpgraded { card_id: target_id };
                        }
                    }
                    return RestSiteActionOutcome::Disabled;
                }
                // Open a pending_choice over the upgradable set; the
                // agent / run loop calls `resolve_pending_card_choice`
                // and then applies the upgrade based on the returned
                // ChoiceOption. Mirrors Python's
                // `player.request_deck_choice(... resolver=upgrade_card_instance)`
                // contract — the resolver is reified by the caller
                // applying the upgrade after resolution.
                let options: Vec<crate::ChoiceOption> = upgradable
                    .iter()
                    .map(|&id| crate::ChoiceOption {
                        card_id: id,
                        upgraded: false,
                        card_uid: None,
                        source_pile: None,
                    })
                    .collect();
                let opened =
                    self.request_card_choice(crate::ChoiceKind::CardSelect, options, 1, 1, false);
                if opened {
                    RestSiteActionOutcome::SmithAwaitingChoice
                } else {
                    // A pre-existing prompt blocked us — surface as
                    // Disabled so the caller can re-try after the
                    // outstanding choice resolves.
                    RestSiteActionOutcome::Disabled
                }
            }
            RestSiteOption::Hatch => {
                // C# `HatchRestSiteOption.OnSelect` (`HatchRestSiteOption.cs:25-29`):
                // `await RelicCmd.Obtain<Byrdpip>(Owner)`. `obtain_relic` fires
                // the run-side `after_obtained` channel, where Byrdpip's
                // listener (`event_reward.rs::listener_byrdpip_transform_eggs`)
                // transforms every `ByrdonisEgg` in the master deck into
                // `ByrdSwoop`. The pet summon is combat-only and so a no-op at
                // the (out-of-combat) rest site. Bead `sts2-rs-RP24`.
                self.obtain_relic(RelicId::Byrdpip);
                RestSiteActionOutcome::Hatched
            }
            RestSiteOption::Kindle => {
                // C# `KindleRestSiteOption.OnSelect` (`KindleRestSiteOption.cs:37-41`):
                // `Owner.GetRelic<PumpkinCandle>()?.Rekindle()` → `KindleCount += 5`
                // (`PumpkinCandle.cs:87-91`, `kindleAmount = 5`). The run-side
                // `KindleCount` lives in `relic_counters[PumpkinCandle]`; the
                // combat-side +1-max-energy window reads its mirror via the
                // `enter_combat` bridge. Bead `sts2-rs-run-rest-option`.
                *self
                    .relic_counters
                    .entry(RelicId::PumpkinCandle)
                    .or_insert(0) += PUMPKIN_CANDLE_KINDLE_AMOUNT;
                RestSiteActionOutcome::Kindled
            }
            RestSiteOption::Lift { .. } => {
                // C# `LiftRestSiteOption.OnSelect` (`LiftRestSiteOption.cs:40-43`):
                // `Owner.GetRelic<Girya>().TimesLifted++`. The run-side
                // `TimesLifted` lives in `relic_counters[Girya]`; the
                // combat-side Strength grant reads the mirror via the
                // `enter_combat` bridge (`relic_effects::girya`,
                // `Girya.cs:50-56` AfterRoomEntered).
                *self.relic_counters.entry(RelicId::Girya).or_insert(0) += 1;
                RestSiteActionOutcome::Lifted
            }
            RestSiteOption::Dig
            | RestSiteOption::Cook { .. }
            | RestSiteOption::Clone
            | RestSiteOption::Mend => RestSiteActionOutcome::Unsupported,
        }
    }

    /// Offer a card-reward set. Walks
    /// [`crate::run_effects::dispatch_modify_card_reward_options_late`]
    /// against the supplied card-instance options so listeners (e.g.
    /// `FresnelLens`) can enchant each option in place. Returns the
    /// (possibly-mutated) option list. Mirrors Python's
    /// `RunState.offer_card_reward` invoking each relic's
    /// `modify_card_reward_options_late`. Bead `sts2-rs-9wx7`.
    pub fn offer_card_reward(&mut self, mut options: Vec<Card>) -> Vec<Card> {
        let _ = crate::run_effects::dispatch_modify_card_reward_options_late(self, &mut options);
        options
    }

    /// Walk the `modify_card_reward_creation_options` channel against
    /// `opts`, returning the listener-mutated value. Called by the
    /// card-reward generator (bead `sts2-rs-jyfvu`, downstream) before
    /// the card pool is rolled so relic listeners (DingyRug,
    /// PrismaticGem, SilverCrucible-rewards) can reshape rarity, pool
    /// composition, and upgrade probability. Mirrors Python's
    /// `for relic in self.relics: opts = relic.modify_card_reward_creation_options(...)`
    /// in `sts2_env/run/rewards.py::generate_card_reward`. Bead
    /// `sts2-rs-tu7g5`.
    pub fn modify_card_reward_creation_options(
        &mut self,
        mut opts: CardRewardGenerationOptions,
    ) -> CardRewardGenerationOptions {
        let _ = crate::run_effects::dispatch_modify_card_reward_creation_options(self, &mut opts);
        opts
    }

    /// Generate the rarity slate for a card reward.
    ///
    /// Mirrors Python `generate_card_reward(run_state, context,
    /// num_cards)` (`sts2_env/run/rewards.py:152-173`): first walks
    /// the `modify_card_reward_creation_options` channel against
    /// `opts` so relic listeners (DingyRug, PrismaticGem,
    /// SilverCrucible) can reshape the roll, then rolls
    /// `opts.num_cards` rarities through [`Self::card_rarity_odds`]
    /// using `opts.context` (parsed via
    /// [`crate::card_rewards::RarityContext::from_python_str`]).
    ///
    /// The per-rarity pity-counter side-effects are applied as each
    /// roll lands.
    ///
    /// Returns the rolled `RewardRarity` slate; concrete-card
    /// selection from the per-character card pool lives in
    /// `sts2-content` and is deferred to a follow-up bead (the
    /// generator can't reach `eligible_character_cards` from
    /// `sts2-core` today). Bead `sts2-rs-jyfvu` (P9.1.e thin slice).
    pub fn generate_card_reward_rarities(
        &mut self,
        opts: CardRewardGenerationOptions,
    ) -> Vec<crate::card_rewards::RewardRarity> {
        let folded = self.modify_card_reward_creation_options(opts);
        let context = crate::card_rewards::RarityContext::from_python_str(folded.context);
        let num_cards = usize::from(folded.num_cards);
        let mut out = Vec::with_capacity(num_cards);
        // Parity: Python uses `run_state.rng.rewards` for rarity
        // rolls (`run/rewards.py:169-171`). Bead `sts2-rs-7e0ek`
        // migrated this off the legacy Xoshiro `rewards_rng` shim.
        for _ in 0..num_cards {
            out.push(
                self.card_rarity_odds
                    .roll_with_dotnet(&mut self.rng.rewards, context),
            );
        }
        out
    }

    /// Generate the `(rarity, upgraded)` slate for a combat card
    /// reward. Mirrors Python `generate_combat_card_rewards(run_state,
    /// context, num_cards)` (`sts2_env/run/rewards.py:210-226`).
    ///
    /// Each rarity rolled by [`Self::generate_card_reward_rarities`]
    /// is paired with an act-scaled
    /// [`crate::card_rewards::roll_for_upgrade`] (base chance =
    /// [`crate::card_rewards::combat_card_upgrade_base_chance`]).
    /// Honors `opts.roll_upgrade`: when false (SilverCrucible's
    /// rewards-leg), every entry pairs with `false`. Bead
    /// `sts2-rs-jyfvu` (P9.1.e thin slice).
    /// Caller-supplied populator output for
    /// [`Self::populate_relic_grab_bag`]. Fields land directly onto
    /// the matching `RunState` fields.
    ///
    /// The content-side sampler (which walks the relic registry,
    /// filters by character pool + rarity, and shuffles per rarity)
    /// produces this triple; the run-side state holds it.
    fn _grab_bag_doc_anchor() {}

    /// Populate the relic-reward pools from a caller-supplied sampler.
    /// Mirrors Python `Player.populate_relic_grab_bag`
    /// (`run_state.py:243-271`).
    ///
    /// The sampler returns `(grab_bag, fallback, boss_pool)` — three
    /// `Vec<RelicId>` already filtered by character pool + rarity
    /// and pre-shuffled per rarity. The run-side just stores them.
    ///
    /// The actual sampler lives in `sts2-content` (it needs to walk
    /// the relic registry by [`crate::relics::RelicId`] and inspect
    /// each spec's `rarity`/`pool`, both of which live there) and is
    /// wired by a follow-up bead. This method's contract is
    /// content-agnostic: any caller can supply pools (tests use
    /// hand-built fixtures).
    ///
    /// Open a run-side card-choice prompt. Mirrors Python's
    /// `RunState.pending_choice = PendingCardChoice(...)` assignment
    /// pattern (`run_state.py:139-146`).
    ///
    /// Returns `true` when the prompt was opened, `false` when:
    /// - `options` is empty (Python guard), OR
    /// - a `pending_choice` is already open.
    ///
    /// `min_select` / `max_select` are clamped so
    /// `max_select >= min_select` and `max_select <= options.len()`.
    /// The `u8` index space caps `options` at 255 entries.
    ///
    /// Bead `sts2-rs-70anj.3.13`. Pure plumbing: no card-pool
    /// sampling, no filter walk — the caller supplies the resolved
    /// option list.
    pub fn request_card_choice(
        &mut self,
        kind: crate::ChoiceKind,
        mut options: Vec<crate::ChoiceOption>,
        min_select: u8,
        max_select: u8,
        can_skip: bool,
    ) -> bool {
        if options.is_empty() {
            return false;
        }
        if self.pending_choice.is_some() {
            return false;
        }
        debug_assert!(
            u8::try_from(options.len()).is_ok(),
            "RunState::request_card_choice options exceed u8 index space ({} > 255)",
            options.len(),
        );
        if options.len() > usize::from(u8::MAX) {
            options.truncate(usize::from(u8::MAX));
        }
        let len_u8 = u8::try_from(options.len()).expect("len capped at 255 above");
        let max_clamped = max_select.min(len_u8).max(1);
        let min_clamped = min_select.min(max_clamped);
        self.pending_choice = Some(crate::PendingChoice {
            kind,
            options,
            min_select: min_clamped,
            max_select: max_clamped,
            can_skip,
            // Run-state choices carry no in-combat post-effect. Bead `sts2-rs-pe5zo`.
            post_effect: None,
            // No sequential multi-pick accumulation on run-state choices. Bead `sts2-rs-sopgf`.
            picked: smallvec::SmallVec::new(),
        });
        true
    }

    /// Open an interactive `PaelsToothStash` choice prompt over the
    /// given master-deck candidate indices. Returns `true` when the
    /// prompt was opened, `false` when an auto-resolve short-circuit
    /// applies (caller falls back to the auto-pick path of
    /// [`crate::RelicId::PaelsTooth`]'s `after_obtained` listener).
    ///
    /// Auto-resolve rule mirrors Python
    /// `_can_auto_resolve_deck_choice`
    /// (`sts2_env/run/run_state.py`): when the agent has no actual
    /// discretion (forced choice — `candidates.len() <= max_count`),
    /// return `false` and let the caller auto-pick all.
    ///
    /// The `candidates` slice carries `(master_deck_idx, card_id,
    /// upgraded)` tuples; only `card_id` + `upgraded` survive into
    /// [`crate::ChoiceOption`]. The resolver
    /// [`Self::resolve_paels_tooth_stash`] matches options back to
    /// master_deck entries by first-match on `(card_id, upgraded)` —
    /// duplicate copies of the same card are interchangeable for
    /// PaelsTooth's stash purpose.
    ///
    /// Bead `sts2-rs-n7tha`.
    pub fn open_paels_tooth_stash_choice(
        &mut self,
        candidates: &[(usize, crate::cards::CardId, bool)],
        max_count: u8,
    ) -> bool {
        if usize::from(max_count) >= candidates.len() || max_count == 0 {
            return false;
        }
        let options: Vec<crate::ChoiceOption> = candidates
            .iter()
            .map(|(_idx, card_id, upgraded)| crate::ChoiceOption {
                card_id: *card_id,
                upgraded: *upgraded,
                card_uid: None,
                source_pile: None,
            })
            .collect();
        self.request_card_choice(
            crate::ChoiceKind::PaelsToothStash,
            options,
            max_count,
            max_count,
            false,
        )
    }

    /// Resolve the open `PaelsToothStash` choice. For each chosen
    /// option, finds the first matching master-deck card by
    /// `(card_id, upgraded)`, clones it into
    /// [`Self::paels_tooth_stored`], and removes it from
    /// [`Self::master_deck`]. Returns `true` on success.
    ///
    /// Failure modes (returns `false` without mutating storage):
    /// - The generic [`Self::resolve_pending_card_choice`] guard
    ///   rejects the selection (count out of range, duplicate index,
    ///   etc.), OR
    /// - The open prompt isn't a [`crate::ChoiceKind::PaelsToothStash`]
    ///   (defensive: in that case the choice was for a different
    ///   purpose and the caller routed wrong).
    ///
    /// Bead `sts2-rs-n7tha`.
    pub fn resolve_paels_tooth_stash(&mut self, selected_indices: &[u8]) -> bool {
        // Defensive: only resolve if the open choice is actually
        // ours. Don't consume someone else's pending_choice.
        if !self
            .pending_choice
            .as_ref()
            .is_some_and(|pc| pc.kind == crate::ChoiceKind::PaelsToothStash)
        {
            return false;
        }
        let Some(chosen) = self.resolve_pending_card_choice(selected_indices) else {
            return false;
        };
        // Match each ChoiceOption to a master_deck index by
        // (card_id, upgraded), claiming each index at most once.
        let mut to_remove: Vec<usize> = Vec::with_capacity(chosen.len());
        for opt in &chosen {
            if let Some(idx) = self.master_deck.iter().enumerate().find_map(|(i, c)| {
                (c.card_id == opt.card_id && c.upgraded == opt.upgraded && !to_remove.contains(&i))
                    .then_some(i)
            }) {
                to_remove.push(idx);
            }
        }
        // Stage clones, then remove in reverse-index order so earlier
        // indices stay valid.
        for &idx in &to_remove {
            self.paels_tooth_stored.push(self.master_deck[idx]);
        }
        to_remove.sort_unstable();
        for idx in to_remove.into_iter().rev() {
            self.master_deck.remove(idx);
        }
        true
    }

    /// Resolve the open card-choice prompt with the agent's pick(s).
    /// Mirrors Python `RunState.resolve_pending_choice(choice_index)`
    /// (`run_state.py:1439-1464`).
    ///
    /// Returns `Some(chosen_options)` when:
    /// - all `selected_indices` are unique + in `[0, options.len())`,
    /// - the count is in `[min_select, max_select]`.
    ///
    /// Returns `None` otherwise. On success the slot is cleared.
    ///
    /// Bead `sts2-rs-70anj.3.13`.
    pub fn resolve_pending_card_choice(
        &mut self,
        selected_indices: &[u8],
    ) -> Option<Vec<crate::ChoiceOption>> {
        let pc = self.pending_choice.as_ref()?;
        let len = pc.options.len();
        let count = u8::try_from(selected_indices.len()).ok()?;
        if count < pc.min_select || count > pc.max_select {
            return None;
        }
        let mut seen = std::collections::BTreeSet::new();
        for &idx in selected_indices {
            if usize::from(idx) >= len {
                return None;
            }
            if !seen.insert(idx) {
                return None;
            }
        }
        let chosen: Vec<crate::ChoiceOption> = selected_indices
            .iter()
            .map(|&idx| pc.options[usize::from(idx)])
            .collect();
        self.pending_choice = None;
        Some(chosen)
    }

    /// Skip the open card-choice prompt. Returns `true` when cleared.
    /// Mirrors Python's skip path (`run_state.py:1455-1462`): honored
    /// only when `pending.can_skip` is true.
    ///
    /// Bead `sts2-rs-70anj.3.13`.
    pub fn skip_pending_card_choice(&mut self) -> bool {
        match self.pending_choice.as_ref() {
            Some(pc) if pc.can_skip => {
                self.pending_choice = None;
                true
            }
            _ => false,
        }
    }

    /// Bead `sts2-rs-597ws` (P9.1.f).
    pub fn populate_relic_grab_bag(
        &mut self,
        grab_bag: Vec<RelicId>,
        fallback: Vec<RelicId>,
        boss_pool: Vec<RelicId>,
    ) {
        self.relic_grab_bag = grab_bag;
        self.relic_grab_bag_fallback = fallback;
        self.boss_relic_pool = boss_pool;
    }

    /// Populate the relic grab bag from per-rarity buckets. Mirrors
    /// Python's `populate_relic_grab_bag` end-state
    /// (`run_state.py:243-271`): per-rarity buckets are the source of
    /// truth, and the flat `relic_grab_bag` is the (Common → Uncommon
    /// → Rare → Shop) flattening of those buckets.
    ///
    /// Caller is responsible for:
    /// - filtering the registry by character pool + allowed rarities,
    /// - shuffling each bucket via the run's RNG stream
    ///   (`rng.rewards` in Python) before passing it in.
    ///
    /// Stores `boss_pool` on [`Self::boss_relic_pool`] alongside the
    /// per-rarity buckets, mirroring the Python `populate_relic_grab_bag`
    /// → boss-pool side-channel pattern (`run_state.py:259-262` in
    /// the BOSS-pool branch). Stores `fallback` on
    /// [`Self::relic_grab_bag_fallback`].
    ///
    /// Unblocks `sts2-rs-cdle3` (PaelsWing per-rarity pop) +
    /// `sts2-rs-7epcg` (TreasureRoom driver). Bead `sts2-rs-l9vkh`
    /// (P9.2.r).
    pub fn populate_relic_grab_bag_by_rarity(
        &mut self,
        by_rarity: BTreeMap<RelicRarity, Vec<RelicId>>,
        fallback: Vec<RelicId>,
        boss_pool: Vec<RelicId>,
    ) {
        // Rebuild the flat (Common → Uncommon → Rare → Shop) view.
        let mut flat: Vec<RelicId> = Vec::new();
        for rarity in RelicRarity::ALL {
            if let Some(bucket) = by_rarity.get(&rarity) {
                flat.extend(bucket.iter().copied());
            }
        }
        self.relic_grab_bag = flat;
        self.relic_grab_bag_by_rarity = by_rarity;
        self.relic_grab_bag_fallback = fallback;
        self.boss_relic_pool = boss_pool;
    }

    /// Pop the first non-owned relic from the per-rarity grab-bag,
    /// walking Python's fallback chain (`_relic_grab_bag_order`). On
    /// match: drains the entry from its bucket AND from the flat
    /// [`Self::relic_grab_bag`] view, then returns the id.
    ///
    /// When every bucket in the fallback chain is exhausted of
    /// non-owned entries, falls back to
    /// [`Self::relic_grab_bag_fallback`] (also drained on match).
    /// Returns `None` when both bucket chain and fallback are empty
    /// of non-owned entries (Python returns the `"CIRCLET"` sentinel
    /// at this point — the Rust port surfaces `None` so the caller
    /// can decide the sentinel-vs-skip path).
    ///
    /// Owned relics are read from [`Self::relics`]; any caller that
    /// needs to exclude additional ids (e.g. "already in this
    /// reward's offered set") should drain in a loop and re-insert
    /// undesired picks afterward, mirroring Python's
    /// `excluded_relic_ids` pattern at the call site.
    ///
    /// Bead `sts2-rs-l9vkh` (P9.2.r). Mirrors Python
    /// `pull_next_relic_reward_id` (`run_state.py:304-339`) minus
    /// the `is_allowed` content-gate (no `RelicInstance::is_allowed`
    /// in Rust at this pin; reintroduce when relic-prerequisite
    /// modeling lands).
    pub fn pop_relic_from_grab_bag(&mut self, rarity: RelicRarity) -> Option<RelicId> {
        self.pop_relic_from_grab_bag_where(rarity, |_| true)
    }

    /// [`Self::pop_relic_from_grab_bag`] with C#'s `filter` argument wired.
    ///
    /// `RelicFactory.PullNextRelicFromFront(player, rarity, filter)` takes a
    /// predicate the shop-facing callers actually use — `WelcomeToWongos`
    /// passes `r => r.IsAllowedInShops` for both the bargain bin and the
    /// featured item, so Wongo can never sell you a relic the merchant is
    /// forbidden to stock. The no-filter entry point passes `_ => true`.
    pub fn pop_relic_from_grab_bag_where(
        &mut self,
        rarity: RelicRarity,
        filter: impl Fn(RelicId) -> bool,
    ) -> Option<RelicId> {
        let (bucket, idx, picked) = self.find_relic_in_grab_bag(rarity, &filter, &[])?;
        match bucket {
            Some(bucket_rarity) => {
                self.relic_grab_bag_by_rarity
                    .get_mut(&bucket_rarity)
                    .map(|b| b.remove(idx));
            }
            None => {
                self.relic_grab_bag_fallback.remove(idx);
            }
        }
        self.relic_grab_bag.retain(|&r| r != picked);
        self.remove_relic_from_shared_grab_bag(picked);
        Some(picked)
    }

    /// What [`Self::pop_relic_from_grab_bag_where`] WOULD return, without
    /// draining anything.
    ///
    /// Events that name the relic on offer before you buy it need this:
    /// Wongo's featured item and each Relic Trader slot print a relic title in
    /// their option text, and the option list is re-derived on every
    /// observation, so building it must not consume the bag. `excluded` lets a
    /// caller walk several pulls in sequence (Relic Trader takes three) by
    /// feeding back what the earlier peeks claimed.
    ///
    /// The handler that later commits the pull sees the same front-of-bucket
    /// state — nothing else draws from the bag mid-event — so peek and pop
    /// agree by construction.
    #[must_use]
    pub fn peek_relic_from_grab_bag_where(
        &self,
        rarity: RelicRarity,
        filter: impl Fn(RelicId) -> bool,
        excluded: &[RelicId],
    ) -> Option<RelicId> {
        self.find_relic_in_grab_bag(rarity, &filter, excluded)
            .map(|(_, _, picked)| picked)
    }

    /// The single grab-bag walk both the pop and the peek run: the first entry
    /// of `rarity`'s fallback chain that is unowned, un-`excluded` and passes
    /// `filter`, else the first such entry of the fallback vec.
    ///
    /// Returns `(bucket rarity or None for the fallback vec, index, id)` so the
    /// draining caller knows where to remove from.
    fn find_relic_in_grab_bag(
        &self,
        rarity: RelicRarity,
        filter: &impl Fn(RelicId) -> bool,
        excluded: &[RelicId],
    ) -> Option<(Option<RelicRarity>, usize, RelicId)> {
        let owned: std::collections::BTreeSet<RelicId> = self.relics.iter().copied().collect();
        let eligible = |id: &RelicId| !owned.contains(id) && !excluded.contains(id) && filter(*id);
        for &candidate_rarity in rarity.fallback_order() {
            let Some(bucket) = self.relic_grab_bag_by_rarity.get(&candidate_rarity) else {
                continue;
            };
            if let Some(idx) = bucket.iter().position(eligible) {
                return Some((Some(candidate_rarity), idx, bucket[idx]));
            }
        }
        let idx = self.relic_grab_bag_fallback.iter().position(eligible)?;
        Some((None, idx, self.relic_grab_bag_fallback[idx]))
    }

    /// Strip `relic_id` from every per-rarity bucket of the SHARED relic
    /// grab bag. Mirrors C# `player.RunState.SharedRelicGrabBag.Remove(
    /// relicModel)`, which fires on EVERY player-bag pull (front or back)
    /// in `RelicFactory.PullNextRelicFromFront` / `PullNextRelicFromBack`
    /// (`MegaCrit.Sts2.Core.Factories/RelicFactory.cs:33,45`) and on
    /// obtaining a non-stackable relic (`RelicCmd.cs:30`).
    ///
    /// The treasure-room pull ([`Self::pop_relic_from_shared_grab_bag`])
    /// draws straight from this bag with no owned-skip in C#
    /// (`TreasureRoomRelicSynchronizer.cs:88-89` →
    /// `RelicGrabBag.PullFromFront` filter `_=>true`), so a relic that was
    /// merely OFFERED at a shop (pulled-but-not-bought) MUST still be
    /// removed here or it shifts the later treasure pull. The owned-skip
    /// on the treasure side only covers relics the player actually
    /// obtained, not ones merely pulled-and-offered.
    fn remove_relic_from_shared_grab_bag(&mut self, relic_id: RelicId) {
        for bucket in self.shared_relic_grab_bag_by_rarity.values_mut() {
            bucket.retain(|&r| r != relic_id);
        }
    }

    /// [`Self::pop_relic_from_grab_bag`] sibling drawing from the
    /// SHARED-only bag ([`Self::shared_relic_grab_bag_by_rarity`]) — the
    /// treasure-chest pull (C# `TreasureRoomRelicSynchronizer.
    /// BeginRelicPicking` → `SharedRelicGrabBag.PullFromFront(rarity)`).
    /// Same owned-exclusion + `RelicRarity::fallback_order` escalation as
    /// C# `RelicGrabBag.GetAvailableDeque` (`RemoveDisallowedRelicsFrom
    /// Deques` + Common→Uncommon→Rare walk); no fallback vec — an
    /// exhausted bag returns `None` (C# `FallbackRelic` = Circlet, not
    /// modeled, consistent with the player-bag pop).
    pub fn pop_relic_from_shared_grab_bag(&mut self, rarity: RelicRarity) -> Option<RelicId> {
        let owned: std::collections::BTreeSet<RelicId> = self.relics.iter().copied().collect();
        for &candidate_rarity in rarity.fallback_order() {
            let Some(bucket) = self
                .shared_relic_grab_bag_by_rarity
                .get_mut(&candidate_rarity)
            else {
                continue;
            };
            if let Some(idx) = bucket.iter().position(|id| !owned.contains(id)) {
                return Some(bucket.remove(idx));
            }
        }
        None
    }

    /// Pop the LAST eligible relic from the per-rarity grab bag, walking
    /// the same [`RelicRarity::fallback_order`] chain as
    /// [`Self::pop_relic_from_grab_bag`] but pulling from the BACK of
    /// each bucket. Mirrors C# `RelicGrabBag.PullFromBack` — the path
    /// `MerchantRelicEntry.FillSlot` takes via
    /// `RelicFactory.PullNextRelicFromBack`, as opposed to the
    /// front-pull used for reward relics.
    ///
    /// `filter` is applied on top of the owned-exclusion, mirroring C#'s
    /// `r => r.IsAllowedInShops` predicate (pass
    /// [`crate::shop::is_relic_allowed_in_shops`] for shop slots, or
    /// `|_| true` for an unfiltered back-pull). Returns the picked id
    /// together with the rarity bucket it came from — that bucket rarity
    /// is the relic's own `Rarity`, which drives `MerchantCost` — or
    /// `None` when the chain + fallback are exhausted of eligible
    /// relics.
    ///
    /// Like [`Self::pop_relic_from_grab_bag`], this omits C#'s
    /// `IsAllowed(runState)` prerequisite gate (no relic-prerequisite
    /// model at this pin). Bead `sts2-rs-jenwp` (M4 shop relic
    /// identity).
    pub fn pop_relic_from_grab_bag_back(
        &mut self,
        rarity: RelicRarity,
        filter: impl Fn(RelicId) -> bool,
    ) -> Option<(RelicId, RelicRarity)> {
        let owned: std::collections::BTreeSet<RelicId> = self.relics.iter().copied().collect();
        for &candidate_rarity in rarity.fallback_order() {
            let Some(bucket) = self.relic_grab_bag_by_rarity.get_mut(&candidate_rarity) else {
                continue;
            };
            if let Some(idx) = bucket
                .iter()
                .rposition(|id| !owned.contains(id) && filter(*id))
            {
                let picked = bucket.remove(idx);
                self.relic_grab_bag.retain(|&r| r != picked);
                self.remove_relic_from_shared_grab_bag(picked);
                return Some((picked, candidate_rarity));
            }
        }
        if let Some(idx) = self
            .relic_grab_bag_fallback
            .iter()
            .rposition(|id| !owned.contains(id) && filter(*id))
        {
            let picked = self.relic_grab_bag_fallback.remove(idx);
            self.relic_grab_bag.retain(|&r| r != picked);
            self.remove_relic_from_shared_grab_bag(picked);
            return Some((picked, rarity));
        }
        None
    }

    /// Strip `relic_id` from every relic-reward pool. Mirrors Python
    /// `Player.remove_relic_from_grab_bag` (`run_state.py:295-302`).
    /// Called when a relic is granted via reward, event, or shop so
    /// duplicates can't surface later.
    ///
    /// Removes ALL occurrences (Python uses `while ... remove`); the
    /// boss pool is included so a player who picks up a boss relic
    /// outside the BOSS_RELIC phase can't be offered it again.
    ///
    /// Bead `sts2-rs-597ws` (P9.1.f). Extended in `sts2-rs-l9vkh`
    /// (P9.2.r) to also strip from the per-rarity buckets.
    pub fn remove_relic_from_grab_bag(&mut self, relic_id: RelicId) {
        self.relic_grab_bag.retain(|&r| r != relic_id);
        for bucket in self.relic_grab_bag_by_rarity.values_mut() {
            bucket.retain(|&r| r != relic_id);
        }
        self.relic_grab_bag_fallback.retain(|&r| r != relic_id);
        self.boss_relic_pool.retain(|&r| r != relic_id);
        // Keep the SHARED bag in lockstep too — C# removes from both the
        // player and shared bags on obtain (`RelicCmd.cs:29-30`).
        self.remove_relic_from_shared_grab_bag(relic_id);
    }

    /// Offer up to `count` distinct boss-relic choices from
    /// [`Self::boss_relic_pool`]. Mirrors the BOSS_RELIC phase in
    /// Python `run_manager.py:685-694`: walk the pre-shuffled boss
    /// pool, take the first `count` entries that aren't already
    /// owned, return them in pool order.
    ///
    /// The pool is **not** drained here — drain happens when the
    /// player picks (via [`Self::remove_relic_from_grab_bag`]).
    /// Skipping the boss room consumes nothing.
    ///
    /// Bead `sts2-rs-597ws` (P9.1.f).
    #[must_use]
    pub fn offer_boss_relic_choice(&self, count: usize) -> Vec<RelicId> {
        self.boss_relic_pool
            .iter()
            .copied()
            .filter(|id| !self.relics.contains(id))
            .take(count)
            .collect()
    }

    /// Roll the 3 boss-relic offers for the NEXT act's Ancient event —
    /// the C#-faithful boss-relic surface (task #19, bead `sts2-rs-40ikw`).
    ///
    /// Boss relics come from the Ancient event that opens the act the player
    /// is about to enter, so this reads `generated_rooms[act_index]` (the
    /// next act; `act_index` is 1-based, so its value is the next act's
    /// 0-based index). The selected ancient (captured at room-gen) drives a
    /// per-event RNG seeded per `EventModel.cs:193`, then its
    /// `GenerateInitialOptions` port in [`crate::boss_ancient_offers`].
    ///
    /// Returns `[]` when no ancient is routed (raw-core runs without
    /// `generate_all_rooms`, or the Neow act) — callers fall back to the
    /// flat boss pool. Pickup side effects are applied at the pick site.
    #[must_use]
    pub fn roll_boss_relic_offers(&self) -> Vec<RelicId> {
        use crate::run_room_gen::AncientId;
        // `act_index` (1-based) names the act whose boss just died; the
        // Ancient belongs to the NEXT act, whose 0-based index equals the
        // 1-based `act_index` value.
        let next_act = self.act_index as usize;
        let Some(gen) = self.generated_rooms.get(next_act) else {
            return Vec::new();
        };
        let Some(ancient) = gen.ancient else {
            return Vec::new();
        };
        if ancient == AncientId::Neow {
            return Vec::new();
        }
        let cond = self.compute_ancient_conditions(next_act as i32);
        let mut rng = crate::boss_ancient_offers::event_rng(self.rng.seed, ancient);
        crate::boss_ancient_offers::roll_offers(ancient, &mut rng, &cond)
    }

    /// Deck/run predicates the Ancient `GenerateInitialOptions` methods read.
    /// `act_index_0based` is the next act's `CurrentActIndex` (Darv gate).
    fn compute_ancient_conditions(
        &self,
        act_index_0based: i32,
    ) -> crate::boss_ancient_offers::AncientConditions {
        use crate::cards::{CardTag, CardType};
        // Instinct: Attack-type, not already enchanted (Instinct is
        // non-stackable, so any existing enchantment blocks).
        let instinct = self
            .master_deck
            .iter()
            .filter(|c| c.card_type == CardType::Attack && c.enchantments.is_empty())
            .count();
        // Swift: any playable card type (Attack/Skill/Power), not enchanted.
        // The C# rule excludes Deck-pile Unplayable cards; Curse/Status are
        // the unplayable types here, so filtering to playable types matches.
        let swift = self
            .master_deck
            .iter()
            .filter(|c| {
                matches!(
                    c.card_type,
                    CardType::Attack | CardType::Skill | CardType::Power
                ) && c.enchantments.is_empty()
            })
            .count();
        // Goopy: base.CanEnchant && Tags.Contains(Defend). The Defend
        // CardTag is not populated on card specs at this pin (see
        // `relics/neow_relics.rs`), so this is ~always 0 → Pael's
        // conditional PaelsClaw is currently never added. Documented
        // divergence (contained to Pael's 2nd offer, isolated RNG).
        let goopy = self
            .master_deck
            .iter()
            .filter(|c| c.tags.contains(CardTag::Defend) && c.enchantments.is_empty())
            .count();
        // Tezcatara: `Deck.Cards.Any(c => c.Tags.Contains(Strike) && c.Rarity
        // == Basic)` (`Tezcatara.cs:191`). Basic rarity == rank 1 (same
        // lookup as `amalgamator_allowed`); the Strike tag IS populated on
        // master-deck cards.
        let rank = crate::lookup_card_rarity_rank();
        let tezcatara_basic_strike = self
            .master_deck
            .iter()
            .any(|c| c.tags.contains(CardTag::Strike) && rank.map_or(true, |f| f(c.card_id) == 1));
        crate::boss_ancient_offers::AncientConditions {
            act_index: act_index_0based,
            goopy_eligible_count: goopy,
            removable_count: self.removable_deck_card_count(),
            // Ironclad never holds an event pet at this pin.
            has_event_pet: false,
            instinct_eligible_count: instinct,
            swift_eligible_count: swift,
            // Headless `UnlockState.all` exposes all 5 characters; the
            // SeaGlass pick draws from the 4 that aren't the player's.
            num_other_characters: 4,
            // TouchOfOrobas's starter relic always exists; ArchaicTooth's
            // transcendence starter card is assumed present (Ironclad). The
            // C# LOCKED placeholder is therefore unreachable.
            orobas_pool3: vec![RelicId::TouchOfOrobas, RelicId::ArchaicTooth],
            // Standard runs carry no deck-clearing modifier.
            pandoras_allowed: true,
            tezcatara_basic_strike,
        }
    }

    /// `true` if any relic-reward pool still has an entry the player
    /// doesn't yet own. Mirrors Python `Player.has_available_relics`
    /// (`run_state.py:273-282`).
    ///
    /// Excludes the boss pool (Python checks `relic_grab_bag_by_rarity`
    /// + `relic_grab_bag_fallback`, not boss). Used by the reward
    /// dispatcher to decide whether to surface a relic-reward choice
    /// after combat.
    ///
    /// Bead `sts2-rs-597ws` (P9.1.f).
    #[must_use]
    pub fn has_available_relics(&self) -> bool {
        let owned: std::collections::BTreeSet<RelicId> = self.relics.iter().copied().collect();
        self.relic_grab_bag.iter().any(|id| !owned.contains(id))
            || self
                .relic_grab_bag_fallback
                .iter()
                .any(|id| !owned.contains(id))
    }

    pub fn generate_combat_card_rewards(
        &mut self,
        opts: CardRewardGenerationOptions,
    ) -> Vec<(crate::card_rewards::RewardRarity, bool)> {
        // Capture the upgrade-roll flag before the (Clone) opts is
        // consumed by `generate_card_reward_rarities`.
        let roll_upgrade = opts.roll_upgrade;
        // C# `RollForUpgrade` scales by `RunState.CurrentActIndex`, which is
        // 0-based (act 1 → 0 → 0% upgrade chance). Rust `self.act_index` is
        // 1-based, so convert to the 0-based form C# uses. (sts2-rs-jenwp M4)
        let act_index = self.act_index.saturating_sub(1);
        let ascension_level = self.card_rarity_odds.ascension_level;
        let rarities = self.generate_card_reward_rarities(opts);
        // Parity: Python uses `run_state.rng.rewards` for the
        // per-rarity upgrade roll (`run/rewards.py:222-225`). Bead
        // `sts2-rs-7e0ek` migrated this off the Xoshiro shim.
        rarities
            .into_iter()
            .map(|rarity| {
                let upgraded = roll_upgrade
                    && crate::card_rewards::roll_for_upgrade_with_dotnet(
                        &mut self.rng.rewards,
                        rarity,
                        act_index,
                        ascension_level,
                        crate::card_rewards::combat_card_upgrade_base_chance(),
                    );
                (rarity, upgraded)
            })
            .collect()
    }

    /// Narrow the candidate [`RoomType`] set for an unknown (`?`) room
    /// by walking the `modify_unknown_map_point_room_types` channel.
    /// Called from the (future) unknown-room resolver in the run/map
    /// layer before it rolls the actual room. Mirrors Python's
    /// `for relic in self.relics: room_types = relic.modify_unknown_map_point_room_types(...)`
    /// in `sts2_env/run/run_state.py::RunState._resolve_unknown_room`.
    /// JuzuBracelet (the canonical listener) drops
    /// [`RoomType::Monster`] from the set pre-act-3-treasure.
    /// Bead `sts2-rs-vuxmm`.
    pub fn modify_unknown_map_point_room_types(
        &mut self,
        mut room_types: std::collections::BTreeSet<RoomType>,
    ) -> std::collections::BTreeSet<RoomType> {
        let _ =
            crate::run_effects::dispatch_modify_unknown_map_point_room_types(self, &mut room_types);
        room_types
    }

    /// Walk the `modify_generated_map` channel against a freshly
    /// generated [`crate::map::ActMap`], letting listeners replace or
    /// mutate the layout. Called from the map-generation site
    /// (future) after the base generator runs. Mirrors Python's
    /// `for relic in self.relics: act_map = relic.modify_generated_map(owner, self, act_map, act_index)`
    /// from `sts2_env/run/run_state.py`'s map-build path. GoldenCompass
    /// (the canonical listener) replaces the act's map with a fixed
    /// golden-path layout on `after_obtained`
    /// (`sts2_env/relics/shop_event.py:2082-2117`).
    ///
    /// `act_index` is 1-indexed to match
    /// [`Self::act_index`]. Both `modify_generated_map` (early) and
    /// `modify_generated_map_late` listeners fire in order: the early
    /// channel can swap the entire layout (GoldenCompass), the late
    /// channel picks coords off the post-swap layout (FurCoat). Beads
    /// `sts2-rs-fchti` (early channel) + `sts2-rs-eboqq` (late channel).
    pub fn modify_generated_map(&mut self, act_map: &mut crate::map::ActMap, act_index: u8) {
        let _ = crate::run_effects::dispatch_modify_generated_map(self, act_map, act_index);
        let _ = crate::run_effects::dispatch_modify_generated_map_late(self, act_map, act_index);
    }

    /// Immediate-regen primitive for the current act's map. Mirrors
    /// Python `RunState.generate_map(self)` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1547-1575`:
    ///
    /// 1. Look up `num_rooms` from `self.acts[act_index - 1]`.
    /// 2. Roll a fresh [`crate::map::ActMap`] via
    ///    [`crate::map_generator::generate_act_map`] using the
    ///    per-act named RNG stream
    ///    ([`crate::run_rng_set::RunRngSet::map_rng`] with the
    ///    0-based act index).
    /// 3. Walk the [`modify_generated_map`] channel
    ///    (GoldenCompass-class layout swaps) followed by
    ///    [`modify_generated_map_late`] (FurCoat-class post-swap
    ///    decoration).
    /// 4. Store the result on [`Self::current_act_map`].
    ///
    /// Closes the in-act regen gap that bead `sts2-rs-k2nuu` flagged:
    /// pickup-time listeners (Python: `GoldenCompass.after_obtained`)
    /// can now force immediate re-roll of the current act's map by
    /// calling `rs.generate_map()` themselves, rather than waiting
    /// for the next driver-side regen at act-advance.
    ///
    /// `replace_treasure_with_elite = false` and
    /// `num_elites_override = None` match Python's default-shape
    /// call. Bead `sts2-rs-4aunm`.
    pub fn generate_map(&mut self) {
        let act_idx_1_based = self.act_index;
        let act_idx_0_based = i32::from(act_idx_1_based).saturating_sub(1);
        let num_rooms = self
            .acts
            .get(act_idx_0_based as usize)
            .map_or(crate::acts::DEFAULT_ACT_NUM_ROOMS, |a| {
                i32::from(a.num_rooms)
            });
        let ascension_level = i32::from(self.ascension_level);
        let mut act_map = {
            let rng = self.rng.map_rng(act_idx_0_based);
            crate::map_generator::generate_act_map(
                num_rooms,
                rng,
                ascension_level,
                false,
                i32::from(act_idx_1_based),
                None,
            )
        };
        self.modify_generated_map(&mut act_map, act_idx_1_based);
        // A10 DoubleBoss: the FINAL act carries a second boss node directly
        // above the boss (C# threads `runState.Act.HasSecondBoss` into the
        // `StandardActMap` ctor, `StandardActMap.cs:96`; only the last act
        // ever has one, `RunManager.cs:501`). Attached post-generation —
        // consumes no RNG. Bead sts2-rs-40ikw.2.
        if self.has_double_boss && self.is_last_act() {
            act_map.attach_second_boss();
        }
        self.current_act_map = Some(act_map);
    }

    /// `true` when [`Self::act_index`] is the run's final act (the only
    /// act that can carry an A10 second boss — C# `RunManager.cs:501`
    /// gates on `i == State.Acts.Count - 1`). Falls back to act 3 when
    /// [`Self::acts`] is empty (raw-core test paths), matching the
    /// run-over gate in `RunManager::exit_combat_and_transition`.
    #[must_use]
    pub fn is_last_act(&self) -> bool {
        if self.acts.is_empty() {
            self.act_index >= 3
        } else {
            usize::from(self.act_index) >= self.acts.len()
        }
    }

    /// A10 DoubleBoss: `true` when the player stands on the FIRST boss
    /// node of a map that has a second boss — i.e. one more boss fight
    /// remains before the act (and run) can end. Mirrors the C# gate
    /// `Map.SecondBossMapPoint != null && CurrentMapCoord ==
    /// Map.BossMapPoint.coord` (`NRewardsScreen.cs:451`, which routes
    /// the terminal boss-rewards screen back to map travel instead of
    /// the act change). Bead sts2-rs-40ikw.2.
    #[must_use]
    pub fn awaiting_second_boss(&self) -> bool {
        let Some(map) = self.current_act_map.as_ref() else {
            return false;
        };
        if map.second_boss.is_none() {
            return false;
        }
        let Some(boss_idx) = map.boss else {
            return false;
        };
        self.current_room.and_then(|r| r.map_coord) == Some(map.point(boss_idx).coord)
    }

    /// A10 DoubleBoss: `true` when the player stands on the SECOND boss
    /// node. The boss-room encounter resolver uses this as the C#
    /// `RoomSet.NextBossEncounter` cursor (`RoomSet.cs:70-79`: return
    /// `SecondBoss` once `bossEncountersVisited != 0` — entry order
    /// guarantees the second visit is the second-boss node, cf.
    /// `CombatManager.cs:801` keying the same check off the map coord).
    /// Bead sts2-rs-40ikw.2.
    #[must_use]
    pub fn at_second_boss_point(&self) -> bool {
        let Some(map) = self.current_act_map.as_ref() else {
            return false;
        };
        let Some(sb_idx) = map.second_boss else {
            return false;
        };
        self.current_room.and_then(|r| r.map_coord) == Some(map.point(sb_idx).coord)
    }

    /// Fire the `modify_card_being_added_to_deck` channel for `card`
    /// (typically called by a future port of
    /// `RunState.add_card_instance_to_deck` before the card is
    /// appended). Listeners may mutate `card` in place. Mirrors
    /// Python's `for relic in self.relics: card =
    /// relic.modify_card_being_added_to_deck(self, card)`. Bead
    /// `sts2-rs-9wx7`.
    pub fn modify_card_being_added_to_deck(&mut self, card: &mut Card) {
        let _ = crate::run_effects::dispatch_modify_card_being_added_to_deck(self, card);
    }

    /// Fire the `modify_merchant_card_creation_results` channel for
    /// `card`. Listeners may enchant or otherwise mutate the merchant
    /// inventory item before it's offered. `is_colorless` mirrors the
    /// Python keyword arg. Bead `sts2-rs-9wx7`.
    pub fn modify_merchant_card_creation_results(&mut self, card: &mut Card, is_colorless: bool) {
        let _ = crate::run_effects::dispatch_modify_merchant_card_creation_results(
            self,
            card,
            is_colorless,
        );
    }

    /// Offer combat / event rewards for the supplied [`RoomType`].
    /// Walks [`crate::run_effects::dispatch_modify_rewards`] so
    /// listeners (e.g. `WongosMysteryTicket`) can append entries.
    /// Mirrors Python's reward fan-out invoking each relic's
    /// `modify_rewards`. Bead `sts2-rs-9wx7`.
    pub fn offer_rewards(&mut self, room_type: RoomType, mut rewards: Vec<Reward>) -> Vec<Reward> {
        let _ = crate::run_effects::dispatch_modify_rewards(self, &mut rewards, room_type);
        rewards
    }

    /// Roll for a post-combat potion drop. Mirrors the Python pin
    /// `sts2-rl-agent@01c094d:sts2_env/run/reward_objects.py:968-978`:
    ///
    /// 1. Walk
    ///    [`crate::run_effects::dispatch_should_force_potion_reward`]
    ///    to compute the `force` flag (any listener — e.g.
    ///    [`crate::RelicId::WhiteBeastStatue`] — returning `true`
    ///    short-circuits to a forced drop).
    /// 2. Call `potion_reward_odds.roll(rng, is_elite, force)` over
    ///    the `rng.rewards` named stream. `is_elite` is computed
    ///    from `room_type == RoomType::Elite` (Python uses the same
    ///    explicit equality check).
    ///
    /// Returns `true` when a potion should be added to the reward
    /// fan-out. The helper does NOT push the reward itself — leaves
    /// the reward-vec composition to the caller so the combat-end
    /// reward generator can order Gold / Card / Relic / Potion in
    /// Python's canonical sequence. Callers push
    /// [`Reward::Potion(PotionReward::empty())`] on true.
    ///
    /// Bead `sts2-rs-vk9av`.
    pub fn roll_potion_reward(&mut self, room_type: RoomType) -> bool {
        let force = crate::run_effects::dispatch_should_force_potion_reward(self, room_type);
        let is_elite = room_type == RoomType::Elite;
        self.potion_reward_odds
            .roll(&mut self.rng.rewards, is_elite, force)
    }

    /// Add `delta` to [`Self::gold`]. Mirrors Python
    /// `RunState.gain_gold(amount)`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:120`).
    ///
    /// On positive `delta` the call runs the
    /// `should_gain_gold` → credit → `on_gold_gained` pipeline
    /// (bead `sts2-rs-wag75`). Any `should_gain_gold` listener setting
    /// its `cancel: &mut bool` out-parameter aborts the entire gain
    /// without crediting (matches Python's "return `False` ⇒ skip the
    /// `self.gold += amount` line").
    ///
    /// Negative `delta` (spend / loss convenience until the dedicated
    /// `lose_gold` surface lands) bypasses both channels — Python's
    /// `gain_gold` has the same `if amount <= 0: return` guard at line
    /// 121, so the relic hooks never fire on debits. Bead `sts2-rs-xqvn`
    /// (initial surface), `sts2-rs-wag75` (gain-hook pipeline).
    pub fn gain_gold(&mut self, delta: i32) {
        // Bead `sts2-rs-w9vpp`: saturating_add at both the negative-delta
        // short-circuit and the post-`should_gain_gold` credit. No
        // semantics change in the normal range; saturates at i32::MAX /
        // MIN rather than wrapping under stacked relic listeners or a
        // future event-reward fixture with a degenerate amount.
        if delta <= 0 {
            self.gold = self.gold.saturating_add(delta);
            return;
        }
        if !crate::run_effects::dispatch_should_gain_gold(self, delta) {
            return;
        }
        self.gold = self.gold.saturating_add(delta);
        let _ = crate::run_effects::dispatch_on_gold_gained(self, delta);
    }

    /// Offer a potion reward through the `should_procure_potion` veto
    /// channel. Returns `true` and pushes a [`Reward::Potion`] when the
    /// channel allows, `false` when any relic listener vetoes (e.g.
    /// [`crate::RelicId::Sozu`]).
    ///
    /// Mirrors Python `RunState.procure_potion`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:720-732`).
    /// The Rust port routes the reward through `pending_rewards` rather
    /// than directly into `player.potions`; the drain logic that maps
    /// `Reward::Potion` → `PlayerCombatState.potions.push(...)` is
    /// orthogonal and will land alongside the larger reward-drain pass.
    /// The veto is what matters today: Sozu must prevent the reward
    /// from ever entering the pending queue.
    ///
    /// Bead `sts2-rs-70anj.3.2`.
    pub fn procure_potion(&mut self, potion: PotionReward) -> bool {
        if !crate::run_effects::dispatch_should_procure_potion(self) {
            return false;
        }
        self.pending_rewards.push(Reward::Potion(potion));
        true
    }

    /// Roll the post-combat gold drop for `room_type` and append it
    /// to [`Self::pending_rewards`] as a [`Reward::Gold`]. Returns
    /// `Some(amount)` when an entry was appended, `None` when the
    /// room type carries no default gold drop (Treasure / Event /
    /// Shop / Rest / Unknown) or when `gold_proportion <= 0.0`.
    ///
    /// Mirrors Python
    /// `sts2-rl-agent@01c094d:sts2_env/run/reward_objects.py:954-967`
    /// (per-room gold-reward append in `RewardsSet.generate_default_rewards`):
    /// - Lookup the `(low, high)` tuple from
    ///   `ENCOUNTER_GOLD_REWARD_RANGES` (`reward_objects.py:874-878`).
    /// - Apply the A3 Poverty multiplier when
    ///   `ascension_level >= POVERTY_ASCENSION_LEVEL`
    ///   (`reward_objects.py:879-880`).
    /// - Apply `gold_proportion` for `Monster`-room fractional drops
    ///   (Python keeps the field on the room; the Rust port lets the
    ///   caller pass it directly to keep `RunState` content-agnostic).
    /// - Roll via the **rewards** RNG stream
    ///   `rng.rewards.next_int(low, high)` (inclusive, matching
    ///   Python's dotnet `Rng.next_int`).
    ///
    /// Python carries `(min_gold, max_gold)` on `GoldReward` and
    /// rolls at `populate()` time; the Rust [`Reward::Gold`] POD is
    /// flat (`amount: i32` only), so the roll folds into this call.
    /// Functionally equivalent — reward-drain code sees a pre-rolled
    /// flat amount.
    ///
    /// Rounding: C# truncates the Poverty multiply (`(int)num`,
    /// `EncounterModel.cs:52-57`) and banker's-rounds the proportion
    /// multiply (`Math.Round`, `RewardsSet.cs:170`); the Rust port
    /// mirrors both (`trunc` then `round_ties_even`). The interval
    /// bounds are clamped so `low <= high` after any combined
    /// Poverty × proportion shrink (a zero-width interval still
    /// rolls — `next_int(n, n)` returns `n`).
    ///
    /// Bead `sts2-rs-gccz5` (P9.2.q). Audit ref:
    /// `docs/audits/2026-05-22-full-run-sim-gap-audit.md` §2.5 row 4.
    pub fn append_encounter_gold_drop(
        &mut self,
        room_type: RoomType,
        gold_proportion: f64,
    ) -> Option<i32> {
        let amount = self.roll_encounter_gold(room_type, gold_proportion)?;
        self.pending_rewards.push(Reward::Gold { amount });
        Some(amount)
    }

    /// Roll the encounter gold value off the `rewards` stream WITHOUT
    /// queueing a [`Reward::Gold`] — the pure roll half of
    /// [`Self::append_encounter_gold_drop`], extracted so the live
    /// combat-end reward path (`RunManager::exit_combat_and_transition`)
    /// can roll the value in C#'s `RewardsSet` order and apply it
    /// immediately via [`Self::gain_gold`] (matching C#
    /// `GoldReward.Populate` → `PlayerCmd.GainGold`, which auto-claims
    /// gold on the rewards screen and runs the gain hooks).
    ///
    /// `gold_proportion` (the flee/escape shrink) is consulted ONLY for
    /// Monster rooms. C# `RewardsSet.GenerateRewardsFor`
    /// (`RewardsSet.cs:167-182`) adds the Monster `GoldReward` behind an
    /// `if (GoldProportion > 0f)` guard and scales its bounds by
    /// `GoldProportion`; the `Elite` and `Boss` arms ALWAYS add a
    /// `GoldReward` with the RAW encounter bounds and never look at
    /// `GoldProportion`. So for a Monster room this returns `None`
    /// (consuming NO RNG) when `gold_proportion <= 0`, but an Elite/Boss
    /// room always rolls regardless of `gold_proportion` — applying the
    /// shrink (or short-circuiting) on Elite/Boss both mis-values the reward
    /// AND desyncs the `rewards` RNG stream. Bead `sts2-rs-jenwp` (M4 live
    /// reward stream).
    pub fn roll_encounter_gold(
        &mut self,
        room_type: RoomType,
        gold_proportion: f64,
    ) -> Option<i32> {
        let is_monster = room_type == RoomType::Monster;
        if is_monster && gold_proportion <= 0.0 {
            return None;
        }
        let (low, high) = match room_type {
            RoomType::Monster => (ENCOUNTER_GOLD_LOW_MONSTER, ENCOUNTER_GOLD_HIGH_MONSTER),
            RoomType::Elite => (ENCOUNTER_GOLD_LOW_ELITE, ENCOUNTER_GOLD_HIGH_ELITE),
            RoomType::Boss => (ENCOUNTER_GOLD_LOW_BOSS, ENCOUNTER_GOLD_HIGH_BOSS),
            _ => return None,
        };
        // C# applies Poverty inside `EncounterModel.Min/MaxGoldReward`
        // (`EncounterModel.cs:52-57` / `:71-76`): `num *= 0.75; return
        // (int)num;` — TRUNCATION per bound — and only afterwards does
        // `RewardsSet.cs:170` scale by `GoldProportion` with
        // `Math.Round` (banker's). So at A3 the Monster low bound is
        // `(int)(10 * 0.75) = 7` (not banker's 8) and the Elite high
        // bound `(int)(45 * 0.75) = 33` (not 34). The Python pin used
        // banker's-round here; the C# decomp wins (2026-06-12 ascension
        // audit, bead sts2-rs-40ikw.2).
        let (mut low_f, mut high_f) = (f64::from(low), f64::from(high));
        if self.ascension_level >= POVERTY_ASCENSION_LEVEL {
            low_f = (low_f * POVERTY_ASCENSION_GOLD_MULTIPLIER).trunc();
            high_f = (high_f * POVERTY_ASCENSION_GOLD_MULTIPLIER).trunc();
        }
        // Only Monster rooms scale by `GoldProportion` (C# Elite/Boss arms use
        // raw bounds — see the method doc-comment).
        if is_monster {
            low_f = (low_f * gold_proportion).round_ties_even();
            high_f = (high_f * gold_proportion).round_ties_even();
        }
        let low_i = low_f as i32;
        let high_i = high_f as i32;
        let (low_i, high_i) = if low_i <= high_i {
            (low_i, high_i)
        } else {
            (high_i, low_i)
        };
        if high_i <= 0 {
            return None;
        }
        Some(self.rng.rewards.next_int(low_i, high_i))
    }

    /// Duplicate one random card from [`Self::master_deck`] and append
    /// the clone back to the deck. Mirrors Python
    /// `RunState.duplicate_card_from_deck`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_state.py:449`) on the
    /// headless / agent path: in Python, `request_deck_choice` is the
    /// interactive intercept; when it returns `False` (no UI), the
    /// fallback is `candidates[0].clone(...)`. The Rust port elects
    /// the rewards-rng-randomized analog because (a) the Python
    /// `candidates[0]` is just "must pick something" stub semantics
    /// without a real UI, and (b) MCTS rollouts deserve a less
    /// pathological default than always-duplicate-Strike#1.
    ///
    /// Filter: STS2's `card.card_type != CardType.QUEST`. The Rust
    /// port has no Quest CardType, so all deck cards qualify today.
    ///
    /// Returns `true` if a duplicate was appended, `false` if the deck
    /// was empty. Bead `sts2-rs-70anj.3.10`.
    pub fn duplicate_card_from_deck(&mut self) -> bool {
        if self.master_deck.is_empty() {
            return false;
        }
        // Bead `sts2-rs-hm2eb` (943w7 Stage 3): draw from the dotnet
        // `RunRngSet` reward stream instead of the legacy Xoshiro
        // `rewards_rng()` shim. `next_int_exclusive(0, len)` is the
        // C#-faithful `[0, len)` index draw (one stream counter tick).
        let len = self.master_deck.len() as i32;
        let idx = self.rng.rewards.next_int_exclusive(0, len) as usize;
        let clone = self.master_deck[idx];
        self.master_deck.push(clone);
        true
    }

    /// DEAD / obsolete (TODO: remove with `CombatState::pending_run_gold_loss`).
    /// This drains `combat.pending_run_gold_loss`, but that accumulator
    /// is no longer written: DEBT's turn-end gold loss now mutates the
    /// live `combat.gold` directly (C# `Debt.OnTurnEndInHand` ->
    /// `PlayerCmd.LoseGold`), and `exit_combat` reconciles run gold via
    /// `rs.gold = combat.gold`. This fn has NO production caller — only
    /// the unit tests below reference it. Kept temporarily to keep the
    /// fix diff small; safe to delete alongside the field.
    pub fn apply_pending_combat_gold_loss(&mut self, combat: &mut crate::CombatState) -> i32 {
        let intent = combat.pending_run_gold_loss;
        combat.pending_run_gold_loss = 0;
        if intent <= 0 {
            return 0;
        }
        let applied = intent.min(self.gold);
        self.gain_gold(-applied);
        applied
    }

    /// Decrement [`Self::current_hp`] by `amount` (clamped to `0`),
    /// returning the actual HP lost. Broadcasts through
    /// [`crate::run_effects::dispatch_after_max_hp_changed`] with
    /// `delta == 0` and `current_hp_delta == -applied` (mirroring
    /// the sign convention used by [`Self::heal`]).
    ///
    /// Mirrors Python `RunState.lose_hp(amount)` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:115`. Negative
    /// or zero `amount` is a no-op and the dispatcher does NOT fire
    /// (Python's `lose_hp(0)` is silently a no-op too — distinct from
    /// `heal(0)`'s zero-delta event because no HP actually moved).
    ///
    /// Bead `sts2-rs-jvk3` (P7-surf — feeds `FragrantMushroom`'s
    /// on-obtain 15-damage clause).
    pub fn lose_hp(&mut self, amount: i32) -> i32 {
        if amount <= 0 {
            return 0;
        }
        // C# routes out-of-combat event/relic damage through the full
        // `CreatureCmd.Damage` pipeline, so TungstenRod's
        // `ModifyHpLostAfterOsty` (−1 per HP loss on the owner,
        // TungstenRod.cs:19-26) applies to event digs and bridge holds
        // too — a ColossalFlower dig costs 4, not 5, with the Rod
        // (shadow census v9 seed 1046). The combat side folds this in
        // `dispatch_modify_hp_lost`; mirror the single relic here since
        // RunState has no modify-hp-lost bus.
        let amount = if self.owns_active_relic(crate::RelicId::TungstenRod) {
            amount - 1
        } else {
            amount
        };
        if amount <= 0 {
            return 0;
        }
        let before = self.current_hp;
        let after = self.current_hp.saturating_sub(amount).max(0);
        self.current_hp = after;
        let applied = before - after;
        let ctx = crate::run_effects::MaxHpChangeRunCtx {
            delta: 0,
            current_hp_delta: -applied,
        };
        let _ = crate::run_effects::dispatch_after_max_hp_changed(self, &ctx);
        applied
    }

    /// Take `amount` HP of damage outside of combat. Alias for
    /// [`Self::lose_hp`]. Mirrors Python
    /// `RunState.take_damage(amount)` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:324`, which is
    /// literally `return self.lose_hp(amount)`. Bead `sts2-rs-jvk3`
    /// (P7-surf — feeds `FragrantMushroom`'s on-obtain 15-damage
    /// clause and any future out-of-combat damage surface, e.g.
    /// event-room HP toll rewards).
    pub fn take_damage(&mut self, amount: i32) -> i32 {
        self.lose_hp(amount)
    }

    /// Replace the first occurrence of `current` in [`Self::relics`]
    /// with `replacement` and return `true` on success. Returns
    /// `false` (and leaves the vec untouched) when `current` is not
    /// present.
    ///
    /// Mirrors Python `RunState.transform_relic(current, new_id)` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:988-1002`.
    /// The Python implementation also swaps the parallel
    /// `relic_objects` entry; the Rust [`RunState`] does not yet
    /// materialize relic objects (only ids), so the swap reduces to a
    /// single `Vec` mutation. No hook channel fires on transform at
    /// the Python pin — the new relic's `after_obtained` is
    /// intentionally NOT invoked.
    ///
    /// Bead `sts2-rs-jvk3` (P7-surf — feeds `SwordOfStone`'s
    /// `after_combat_victory` swap-to-`SwordOfJade` clause).
    pub fn transform_relic(&mut self, current: RelicId, replacement: RelicId) -> bool {
        if let Some(idx) = self.relics.iter().position(|r| *r == current) {
            self.relics[idx] = replacement;
            true
        } else {
            false
        }
    }

    /// Replace the rewards-RNG seed (bead `sts2-rs-7krx0`). Callers
    /// initializing a fresh run from a session seed thread it here.
    #[must_use]
    pub fn with_rewards_seed(mut self, seed: u64) -> Self {
        self.rewards_rng_seed = seed;
        self
    }

    /// Vend a fresh `Xoshiro256PlusPlus` seeded from
    /// [`Self::rewards_rng_seed`] and advance the stored seed so the
    /// next call draws an independent sequence.
    ///
    /// **DEPRECATED for production (bead `sts2-rs-hm2eb`, 943w7 Stage 3):**
    /// every production reward draw (`duplicate_card_from_deck`,
    /// `upgrade_random_cards`, the colorless/`spec_index` samplers, and the
    /// Astrolabe/WingCharm shop-relic listeners) now draws from the dotnet
    /// [`RunRngSet`] `rewards` named stream (`self.rng.rewards`) instead of
    /// this Xoshiro shim. This method + [`Self::rewards_rng_seed`] +
    /// [`Self::with_rewards_seed`] are retained ONLY for the test surface
    /// (the seed-defaults golden test + `with_rewards_seed`-based fixtures);
    /// fully retiring the `u64` shim + its test seeding is a follow-up.
    /// (The C# decomp has no literal `"CardRewardRng"` stream — run RNG
    /// streams are `SnakeCase(RunRngType)`; the player-scoped `rewards`
    /// stream is the faithful Rust analog.)
    ///
    /// Bead `sts2-rs-7krx0`: the run-side rewards RNG state lives as a
    /// single `u64` (PartialEq-friendly, since the `Xoshiro` struct
    /// itself isn't `Eq`). Each call seeds a local generator with the
    /// current state, then bumps the state by drawing one `u64` from
    /// that generator. Deterministic + reproducible — back-to-back
    /// calls with the same starting seed produce the same sequence of
    /// vended generators.
    pub fn rewards_rng(&mut self) -> Xoshiro256PlusPlus {
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(self.rewards_rng_seed);
        self.rewards_rng_seed = rng.next_u64();
        rng
    }

    /// Upgrade up to `count` random non-upgraded cards in the run's
    /// master deck. Returns the number of cards actually upgraded
    /// (`<= count`; less when fewer than `count` cards are eligible).
    ///
    /// Mirrors Python `RunState.upgrade_random_cards(card_type, count)`
    /// at `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:709-718`:
    /// build a candidate list of non-upgraded deck cards, shuffle via
    /// the rewards RNG stream, and upgrade the first `count`.
    ///
    /// Bead `sts2-rs-7krx0` (replaces the bead-`sts2-rs-jvk3` surface
    /// stub that returned 0). Wired against the new
    /// [`Self::master_deck`] and [`Self::rewards_rng`] surfaces.
    pub fn upgrade_random_cards(&mut self, count: usize) -> usize {
        if count == 0 {
            return 0;
        }
        // Candidate set: upgradable master-deck indices (C# IsUpgradable —
        // excludes non-upgradable Curse/Status, not merely already-upgraded).
        let mut candidates: Vec<usize> = self
            .master_deck
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.is_upgradable().then_some(i))
            .collect();
        if candidates.is_empty() {
            return 0;
        }
        // Interactive mode (opt-in host, e.g. the workbench / `run_parity`
        // lockstep): don't upgrade anything now — queue the picks so the run
        // loop can present a deck card_select and let the player choose which
        // cards to upgrade, matching C# `Pomander.AfterObtained` →
        // `CardSelectCmd.FromDeckForUpgrade`. Consumes NO rewards RNG (the
        // interactive C# path does no shuffle — the player picks). Returns 0
        // upgraded-now; the run loop drains the queue via `UpgradeDeckCard`
        // actions. Default (parity) mode falls through to the random
        // rewards-stream auto-upgrade the Python pin uses. Bead
        // `sts2-rs-1hiht`.
        if self.interactive_card_upgrade {
            let take = count.min(candidates.len());
            self.pending_deck_upgrades += u32::try_from(take).unwrap_or(u32::MAX);
            return 0;
        }
        // Partial Fisher-Yates: only shuffle the first `take` slots so
        // the cost is `O(take)` rather than `O(candidates.len())` —
        // matches the Python reference shape (`rng.shuffle(...); take[:n]`).
        // Bead `sts2-rs-hm2eb` (943w7 Stage 3): partial Fisher-Yates off
        // the dotnet `RunRngSet` reward stream (was the legacy Xoshiro
        // `rewards_rng()` shim).
        let rng = &mut self.rng.rewards;
        let take = count.min(candidates.len());
        for i in 0..take {
            let len_remaining = (candidates.len() - i) as i32;
            // Draw an index in `[i, candidates.len())` and swap.
            let j = i + rng.next_int_exclusive(0, len_remaining) as usize;
            candidates.swap(i, j);
        }
        // Flip the upgraded flag on the chosen indices. `card.upgrade()`
        // is idempotent and returns true iff it flipped — used as the
        // success counter so a future race / pre-upgraded subset can't
        // overstate the result.
        let mut upgraded = 0_usize;
        for &deck_idx in candidates.iter().take(take) {
            if self.master_deck[deck_idx].upgrade() {
                upgraded += 1;
            }
        }
        upgraded
    }

    /// Downgrade up to `count` random upgraded cards in the run's master
    /// deck. Returns the number actually downgraded (`<= count`; fewer
    /// when the deck holds fewer than `count` upgraded cards). The
    /// mirror of [`Self::upgrade_random_cards`]: same rewards RNG
    /// stream + partial Fisher-Yates, but the candidate set is the
    /// *upgraded* deck indices and each pick is reverted via
    /// [`crate::cards::Card::downgrade`].
    ///
    /// Used by the `Reflections` event (`downgrade 2, upgrade 4`).
    pub fn downgrade_random_cards(&mut self, count: usize) -> usize {
        if count == 0 {
            return 0;
        }
        let mut candidates: Vec<usize> = self
            .master_deck
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.upgraded.then_some(i))
            .collect();
        if candidates.is_empty() {
            return 0;
        }
        let rng = &mut self.rng.rewards;
        let take = count.min(candidates.len());
        for i in 0..take {
            let len_remaining = (candidates.len() - i) as i32;
            let j = i + rng.next_int_exclusive(0, len_remaining) as usize;
            candidates.swap(i, j);
        }
        let mut downgraded = 0_usize;
        for &deck_idx in candidates.iter().take(take) {
            if self.master_deck[deck_idx].downgrade() {
                downgraded += 1;
            }
        }
        downgraded
    }

    /// Upgrade up to `count` random non-upgraded cards of the given
    /// [`CardType`] in the master deck. Returns the number of cards
    /// actually upgraded (`<= count`; less when fewer than `count`
    /// matching cards are eligible).
    ///
    /// Mirrors Python `Creature.upgrade_random_cards(card_type, count,
    /// rng=run_state.rng.niche)` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/creature.py` — the
    /// caller-supplied stream is `RunState.rng.niche`, distinct from
    /// the legacy `rewards_rng` used by the untyped
    /// [`Self::upgrade_random_cards`] (kept on the rewards stream for
    /// backwards-compat with bead `sts2-rs-7krx0` callers).
    ///
    /// Used by the `WarPaint` / `Whetstone` event-reward relics via
    /// the run-side after-obtained channel (see bead `sts2-rs-tb36q`).
    /// Bead `sts2-rs-tb36q`.
    pub fn upgrade_random_cards_of_type(
        &mut self,
        card_type: crate::cards::CardType,
        count: usize,
    ) -> usize {
        self.upgrade_random_cards_stable_inner(count, Some(card_type), None)
    }

    /// Upgrade up to `count` random upgradable cards of ANY type in the
    /// master deck, IMMEDIATELY (never interactive). Returns the number of
    /// cards actually upgraded (`<= count`).
    ///
    /// This is the all-types sibling of
    /// [`Self::upgrade_random_cards_of_type`] and the faithful port of every
    /// C# effect shaped as
    /// `Deck.Cards.Where(IsUpgradable).StableShuffle(Rng.Niche).Take(n)` →
    /// `CardCmd.Upgrade` — the SandCastle / WarHammer / FragrantMushroom
    /// relics and the BattlewornDummy / DoorsOfLightAndDark / Reflections
    /// events. Those effects upgrade RANDOM cards with no player choice, so —
    /// unlike the interactive-aware [`Self::upgrade_random_cards`] — this
    /// NEVER queues a `pending_deck_upgrades` player card_select even when
    /// `interactive_card_upgrade` is set. Honoring the flag here wrongly
    /// presents a "pick N cards to upgrade" screen (the workbench symptom)
    /// and, in C#-vs-Rust lockstep, would pick different cards than the C#
    /// niche shuffle. Only the genuine `CardSelectCmd.FromDeckForUpgrade`
    /// effects (Pomander / YummyCookie relics, AromaOfChaos / SapphireSeed
    /// events) route through `upgrade_random_cards`.
    pub fn upgrade_random_cards_stable(&mut self, count: usize) -> usize {
        self.upgrade_random_cards_stable_inner(count, None, None)
    }

    /// Like [`Self::upgrade_random_cards_stable`] but draws the `StableShuffle`
    /// from a caller-supplied **event-local** RNG stream instead of the run-wide
    /// `Rng.Niche`. v0.107.1 migrated the `BattlewornDummy` /
    /// `DoorsOfLightAndDark` upgrade draws from `RunState.Rng.Niche` to the
    /// event's own `base.Rng` (`BattlewornDummy.cs:97`, `DoorsOfLightAndDark.cs:28`
    /// — the `StableShuffle(base.Rng)` change vs v0.103.2's
    /// `StableShuffle(base.Owner.RunState.Rng.Niche)`); the relic callers
    /// (`WarPaint` / `SandCastle` / …) still draw `Rng.Niche` and keep using
    /// [`Self::upgrade_random_cards_stable`]. The caller builds the event RNG
    /// via the content-side `event_rng(state, entry)` helper (same derivation as
    /// [`crate::dotnet_rng::Rng::with_name`]). Never interactive.
    pub fn upgrade_random_cards_stable_with_rng(
        &mut self,
        count: usize,
        event_rng: &mut crate::dotnet_rng::Rng,
    ) -> usize {
        self.upgrade_random_cards_stable_inner(count, None, Some(event_rng))
    }

    /// Shared body for the `StableShuffle`-based upgrades. `type_filter`
    /// restricts candidates to one [`crate::cards::CardType`] (the
    /// `_of_type` callers); `None` allows every upgradable card (the
    /// all-types callers). The shuffle reads `Rng.Niche` by default; when
    /// `event_rng` is `Some`, it reads that event-local stream instead (the
    /// v0.107.1 `BattlewornDummy` / `DoorsOfLightAndDark` migration). Always
    /// upgrades immediately — never interactive.
    fn upgrade_random_cards_stable_inner(
        &mut self,
        count: usize,
        type_filter: Option<crate::cards::CardType>,
        event_rng: Option<&mut crate::dotnet_rng::Rng>,
    ) -> usize {
        if count == 0 {
            return 0;
        }
        let candidates: Vec<usize> = self
            .master_deck
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                if !c.is_upgradable() {
                    return None;
                }
                match type_filter {
                    Some(t) if c.card_type != t => None,
                    _ => Some(i),
                }
            })
            .collect();
        if candidates.is_empty() {
            return 0;
        }
        // C# `WarPaint.AfterObtained` (`Relics/WarPaint.cs:20-27`):
        //   Deck.Cards.Where(Skill && IsUpgradable)
        //       .ToList().StableShuffle(Rng.Niche).Take(2) -> Upgrade each.
        // `StableShuffle` (`ListExtensions.cs:10-19`) = `Sort()` by
        // `CardModel.CompareTo` (Entry ordinal, then `CurrentUpgradeLevel`;
        // `CardModel.cs:1745-1767` -> `AbstractModel.cs:64-74` -> `ModelId.cs:43-51`
        // ordinal Entry) THEN `UnstableShuffle` on `Rng.Niche`. The prior
        // port followed the Python pin (`rng.niche.shuffle`) and OMITTED the
        // pre-sort, so identical Fisher-Yates draws landed on different cards
        // whenever the candidate cards were not already in sorted Entry order
        // in the deck. Snapshot the (Entry name, upgraded) keys first so the
        // comparator does not borrow `self.master_deck` while `self.rng` is
        // mutably borrowed; then route through the faithful .NET introsort via
        // `stable_shuffle_by` (mirrors `power_cell.rs` / `uncommon.rs` /
        // `catastrophe.rs`, bead sts2-rs-5aelq).
        let keyed: Vec<(&'static str, u8)> = candidates
            .iter()
            .map(|&i| {
                let c = &self.master_deck[i];
                (c.card_id.to_python_name(), u8::from(c.upgraded))
            })
            .collect();
        // `candidates` and `keyed` are parallel (built in the same deck
        // order); sort positional indices into them by the C# key, then map
        // back to the deck indices.
        let mut order: Vec<usize> = (0..candidates.len()).collect();
        let cmp = |a: usize, b: usize| {
            keyed[a]
                .0
                .cmp(keyed[b].0)
                .then_with(|| keyed[a].1.cmp(&keyed[b].1))
        };
        match event_rng {
            // v0.107.1: BattlewornDummy / DoorsOfLightAndDark draw the shuffle
            // from the event's own `base.Rng` stream rather than `Rng.Niche`.
            Some(rng) => rng.stable_shuffle_by(order.as_mut_slice(), &cmp),
            None => self.rng.niche.stable_shuffle_by(order.as_mut_slice(), &cmp),
        }
        let candidates: Vec<usize> = order.into_iter().map(|i| candidates[i]).collect();
        let take = count.min(candidates.len());
        let mut upgraded = 0_usize;
        for &deck_idx in candidates.iter().take(take) {
            if self.master_deck[deck_idx].upgrade() {
                upgraded += 1;
            }
        }
        upgraded
    }

    /// Transform up to `count` random cards in the master deck —
    /// remove them and substitute the caller-supplied
    /// `replacements`. Returns the number of cards actually
    /// transformed (`<= count`; less when the deck has fewer than
    /// `count` cards OR when `replacements` is shorter than `count`).
    ///
    /// Mirrors Python `Creature.transform_cards(card_count)` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/creature.py` — pick N
    /// random deck slots via `run_state.rng.niche`, remove the
    /// originals, and slot in `card_count` freshly-rolled
    /// character-pool cards. The Python pin generates the pool
    /// replacements inline; the Rust port keeps RunState
    /// content-agnostic and accepts the resolved `replacements`
    /// from the caller (the character-pool sampler lives in
    /// `sts2-content`, mirroring the established
    /// [`Self::populate_relic_grab_bag`] / [`Self::offer_card_reward`]
    /// idiom).
    ///
    /// Selection uses the **niche** stream (matches the typed
    /// upgrade-cards primitive in bead `sts2-rs-tb36q`). Indices
    /// are picked via the dotnet Rng's `shuffle` to match Python's
    /// `rng.niche.shuffle(...)` shape.
    ///
    /// Bead `sts2-rs-aw73x`.
    pub fn transform_cards(&mut self, count: usize, replacements: Vec<Card>) -> usize {
        if count == 0 || self.master_deck.is_empty() {
            return 0;
        }
        let take = count.min(self.master_deck.len()).min(replacements.len());
        if take == 0 {
            return 0;
        }
        let mut indices: Vec<usize> = (0..self.master_deck.len()).collect();
        self.rng.niche.shuffle(&mut indices);
        // Slice the first `take` of the shuffled index list. Sort
        // descending so we can remove in-place without shifting the
        // earlier indices we still plan to remove.
        let mut chosen: Vec<usize> = indices.into_iter().take(take).collect();
        chosen.sort_unstable_by(|a, b| b.cmp(a));
        for &idx in &chosen {
            self.master_deck.remove(idx);
        }
        // Append the resolved replacements at the deck tail. Python
        // re-emits the replacements at the deck tail too; pile order
        // is non-load-bearing for parity (combat shuffle randomizes).
        for card in replacements.into_iter().take(take) {
            self.master_deck.push(card);
        }
        take
    }

    /// Transform a caller-specified set of master-deck slots,
    /// replacing each in-place with the matching entry from
    /// `replacements`. Returns the number of cards actually
    /// transformed (`<= indices.len()`; less when `indices` and
    /// `replacements` differ in length, or when any index is
    /// out-of-bounds against the current `master_deck`).
    ///
    /// Mirrors Python
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py::transform_specific_cards`,
    /// which delegates to `_transform_selected_cards` and ultimately
    /// `_apply_card_replacement`: each replacement is written onto
    /// the existing `CardInstance` in-place so the card's position
    /// in the deck (and, for combat consumers, its identity) is
    /// preserved.
    ///
    /// **Deck order (run-RNG parity, NOT a no-op):** C#
    /// `CardCmd.Transform` (`MegaCrit.Sts2.Core.Commands/CardCmd.cs:268`),
    /// for a `PileType.Deck` original, **removes the original from its
    /// slot and `AddInternal(replacement)` — appends the replacement to
    /// the END of the deck list** (default `index = -1`), not back into
    /// the original slot (the original-slot insert is the *combat-pile*
    /// branch). Multiple transforms are applied in ascending
    /// original-index order (C# `PileIndexSort`: same pile type ⇒ sort by
    /// the recorded pile index). The master deck is a plain insertion-order
    /// `List<CardModel>` (no PileIndex re-sort), and combat start feeds
    /// exactly that order into the Fisher-Yates shuffle
    /// (`Player.PopulateCombatState` → `Deck.Cards.ToList()` →
    /// `UnstableShuffle`). So in-place replacement (the prior port) matched
    /// the deck *multiset* but produced the wrong shuffle *input order* —
    /// every post-transform combat dealt a different opening hand than C#.
    /// Appending to the tail makes the whole-run lockstep agree.
    ///
    /// Why a separate primitive (vs. reusing [`Self::transform_cards`]):
    /// `transform_cards` picks random slots from the entire master
    /// deck via the `niche` RNG stream, removes them, and appends
    /// freshly-rolled replacements at the tail. Several shop-event
    /// relics (`PandorasBox` "transform all basic Strikes/Defends",
    /// the `transform_starter_card` family, the deck-choice
    /// `_transform_selected_cards` resolver, etc.) need to transform
    /// a **specific** caller-chosen subset. This primitive takes the
    /// caller's chosen indices + replacements and applies the C# deck
    /// remove-then-append ordering above.
    ///
    /// RunState stays content-agnostic — replacements come from the
    /// content-side character-pool sampler (same idiom as
    /// [`Self::transform_cards`], `populate_relic_grab_bag`, …).
    ///
    /// Out-of-bounds indices are skipped (they don't abort the whole
    /// call). Duplicate indices transform the same slot once (the slot
    /// is removed on first hit; the repeat is a no-op miss).
    ///
    /// Bead `sts2-rs-ab8uo` (P9.3.a); deck-order fix for run-RNG parity.
    pub fn transform_specific_cards(
        &mut self,
        indices: &[usize],
        replacements: Vec<Card>,
    ) -> usize {
        let take = indices.len().min(replacements.len());
        if take == 0 {
            return 0;
        }
        let deck_len = self.master_deck.len();
        // Pair each in-bounds index with its replacement, then apply in
        // ascending original-index order (C# `PileIndexSort` for the Deck
        // pile). Removing earlier slots shifts later ones down by a constant
        // for the run, so the *relative* append order equals the
        // recorded-pile-index order C# sorts on.
        let mut pairs: Vec<(usize, Card)> = Vec::with_capacity(take);
        for (i, &deck_idx) in indices.iter().take(take).enumerate() {
            if deck_idx >= deck_len {
                continue;
            }
            // Duplicate index: keep the last replacement (matching the prior
            // in-place last-write-wins) so deck length stays invariant.
            if let Some(slot) = pairs.iter_mut().find(|(idx, _)| *idx == deck_idx) {
                slot.1 = replacements[i];
            } else {
                pairs.push((deck_idx, replacements[i]));
            }
        }
        if pairs.is_empty() {
            return 0;
        }
        pairs.sort_by_key(|&(deck_idx, _)| deck_idx);
        let transformed = pairs.len();
        // Remove originals from the tail end first so the lower indices stay
        // valid as we go.
        for &(deck_idx, _) in pairs.iter().rev() {
            self.master_deck.remove(deck_idx);
        }
        // Append replacements in ascending original-index order.
        for (_, replacement) in pairs {
            self.master_deck.push(replacement);
        }
        transformed
    }

    /// Max number of `WingedBoots` "ignore-paths" map moves. Mirrors
    /// the relic's `Rooms` DynamicVar (`BaseValue = 3`) read by C#
    /// `RunSimulator.WingedBootsMaxUses`. Hardcoded because the Rust
    /// port does not model per-relic DynamicVars; the value is fixed
    /// in the C# content at this pin.
    pub const WINGED_BOOTS_MAX_USES: u32 = 3;

    /// `true` iff the player owns [`crate::RelicId::WingedBoots`] and
    /// has at least one "ignore-paths" use remaining. While this holds
    /// the map-choice surface offers every grid-live next-row node
    /// (not just connected children) and an off-path move is legal.
    /// Mirrors the C# headless gate `WingedBootsMaxUses() > 0 &&
    /// _wingedBootsOffPathUsed < bootsMax`.
    #[must_use]
    pub fn winged_boots_uses_remaining(&self) -> bool {
        self.owns_active_relic(crate::RelicId::WingedBoots)
            && self.winged_boots_off_path_used < Self::WINGED_BOOTS_MAX_USES
    }

    /// Record one `WingedBoots` off-path map move (the chosen node was
    /// not a connected child of the current node). Saturating so a
    /// stray double-call can never wrap. Mirrors C# `DoMapSelect`'s
    /// `_wingedBootsOffPathUsed++` guarded by `IsOffPathChoice`.
    pub fn winged_boots_record_off_path(&mut self) {
        self.winged_boots_off_path_used = self.winged_boots_off_path_used.saturating_add(1);
    }

    /// Open an interactive transform-cards prompt: the player picks
    /// exactly `count` cards from the current master deck. Returns
    /// `true` when the prompt was opened, `false` when:
    /// - `count == 0`,
    /// - the master deck has fewer than `count` cards, OR
    /// - a `pending_choice` is already open (see
    ///   [`Self::request_card_choice`]).
    ///
    /// Mirrors Python `Creature.offer_transform_cards_reward(count)`
    /// at `sts2-rl-agent@01c094d:sts2_env/core/creature.py` — opens
    /// a [`crate::ChoiceKind::CardSelect`] prompt with
    /// `min_select = max_select = count`, `can_skip = false`. The
    /// caller resolves via [`Self::resolve_pending_card_choice`] and
    /// then invokes [`Self::transform_cards`] with the
    /// content-side character-pool replacements (this method is the
    /// prompt-opener only — content-aware resolution is the
    /// consumer's job).
    ///
    /// `card_uid` and `source_pile` on each
    /// [`crate::ChoiceOption`] are `None`: master-deck cards have no
    /// in-combat [`crate::CardUid`] and no
    /// [`crate::cards::PileKind`] (the master deck is a run-side
    /// construct, not a combat pile).
    ///
    /// Bead `sts2-rs-aw73x` (depends on `sts2-rs-70anj.3.13` for
    /// the underlying `request_card_choice` infra, already landed).
    pub fn offer_transform_cards_reward(&mut self, count: usize) -> bool {
        if count == 0 || self.master_deck.len() < count {
            return false;
        }
        // Interactive mode (opt-in host: the workbench / run_parity
        // lockstep): queue the picks so the run loop surfaces a deck
        // card_select, mirroring C# `CardSelectCmd.FromDeckForTransformation`
        // — the replacement is rolled only AFTER the pick (`Rng.Niche`), so
        // queueing consumes no RNG. The legacy `pending_choice` prompt below
        // is only consumed by the rest-site Smith surface and strands at
        // any other phase (e.g. Neow), so opted-in hosts route here.
        if self.interactive_card_transform {
            for _ in 0..count {
                self.pending_deck_transforms
                    .push(PendingDeckTransform::default());
            }
            return true;
        }
        let Ok(count_u8) = u8::try_from(count) else {
            return false;
        };
        let options: Vec<crate::ChoiceOption> = self
            .master_deck
            .iter()
            .map(|c| crate::ChoiceOption {
                card_id: c.card_id,
                upgraded: c.upgraded,
                card_uid: None,
                source_pile: None,
            })
            .collect();
        self.request_card_choice(
            crate::ChoiceKind::CardSelect,
            options,
            count_u8,
            count_u8,
            false,
        )
    }

    /// Remove up to `count` cards from the run's master deck. Returns
    /// the number actually removed (`<= count`; less when the deck has
    /// fewer than `count` removable cards).
    ///
    /// Mirrors Python `RunState.remove_cards_from_deck(count, cards=None)`
    /// at `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:497-518`:
    /// take the first `count` from the candidate list (which is the
    /// deck in order, with any post-filter applied). Without
    /// a `request_deck_choice` interactive path, the function takes
    /// the head-N branch unconditionally.
    ///
    /// At the current pin Rust has no `CardType::Quest` discriminant
    /// and no per-card `is_removable` predicate — every master-deck
    /// entry is treated as removable, so the candidate set is the
    /// deck verbatim. Matches the Python reference shape for non-Quest
    /// decks (the only kind this port models).
    ///
    /// Bead `sts2-rs-t762n`.
    pub fn remove_cards_from_deck(&mut self, count: usize) -> usize {
        if count == 0 || self.master_deck.is_empty() {
            return 0;
        }
        // Interactive mode (opt-in host, e.g. the workbench): don't remove
        // anything now — queue the picks so the run loop can present a
        // deck card_select and let the player choose which cards to remove,
        // matching C#. Returns 0 removed-now; the run loop drains the queue
        // via `RemoveDeckCard` actions. Default (parity) mode falls through
        // to the front-of-deck auto-removal the Python pin uses.
        if self.interactive_card_removal {
            let take = count.min(self.master_deck.len());
            self.pending_deck_removals += u32::try_from(take).unwrap_or(u32::MAX);
            return 0;
        }
        let take = count.min(self.master_deck.len());
        self.master_deck.drain(0..take).count()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Bead `sts2-rs-70anj.3.16`: `RunState::act_index` defaults to 1
    /// (Act 1) and `advance_act` walks 1 → 2 → 3 then clamps. Mirrors
    /// the Python contract `RunState.act_index` reads. Listeners gated
    /// on act (LavaRock post-Act-1 boss, PumpkinCandle, FurCoat) rely
    /// on both the default and the clamp.
    #[test]
    fn act_index_default_and_advance() {
        let mut rs = RunState::default();
        assert_eq!(rs.act_index(), 1);
        rs.advance_act();
        assert_eq!(rs.act_index(), 2);
        rs.advance_act();
        assert_eq!(rs.act_index(), 3);
        rs.advance_act();
        assert_eq!(rs.act_index(), 3, "clamped at Act 3");
    }

    // ── Bead sts2-rs-2c0ze: map-traversal trackers (audit §2.2) ─────────

    /// Fresh `RunState` has all map-traversal trackers at zero/empty,
    /// matching Python's defaults at `run_state.py:1393-1400`.
    #[test]
    fn map_traversal_trackers_default_empty() {
        let rs = RunState::default();
        assert!(rs.visited_map_coords.is_empty());
        assert!(rs.map_point_history.is_empty());
        assert_eq!(rs.act_floor, 0);
        assert_eq!(rs.total_floor, 0);
        assert!(!rs.rooms_generated);
        assert!(rs.visited_event_ids.is_empty());
    }

    /// `add_visited_coord` registers a fresh visit: pushes the coord,
    /// sets `act_floor = row + 1`, bumps `total_floor`, and (when both
    /// filters are supplied) appends a history entry.
    #[test]
    fn add_visited_coord_fresh_visit_bumps_counters_and_logs_history() {
        use crate::map::{MapCoord, MapPointType};
        let mut rs = RunState::default();
        let coord = MapCoord::new(2, 4);
        let novel =
            rs.add_visited_coord(coord, Some(MapPointType::Monster), Some(RoomType::Monster));
        assert!(novel, "first visit must return true");
        assert_eq!(rs.visited_map_coords, vec![coord]);
        assert_eq!(rs.act_floor, 5, "act_floor == row + 1");
        assert_eq!(rs.total_floor, 1);
        assert_eq!(rs.map_point_history.len(), 1);
        assert_eq!(
            rs.map_point_history[0].map_point_type,
            MapPointType::Monster
        );
        assert_eq!(rs.map_point_history[0].room_type, RoomType::Monster);
    }

    /// Repeat visit to the same coord is a no-op — Python's
    /// `if coord in self.visited_map_coords: return False` guard.
    #[test]
    fn add_visited_coord_repeat_visit_is_noop() {
        use crate::map::{MapCoord, MapPointType};
        let mut rs = RunState::default();
        let coord = MapCoord::new(1, 2);
        rs.add_visited_coord(coord, Some(MapPointType::Monster), Some(RoomType::Monster));
        let novel = rs.add_visited_coord(
            coord,
            Some(MapPointType::RestSite),
            Some(RoomType::RestSite),
        );
        assert!(!novel, "repeat visit must return false");
        assert_eq!(rs.visited_map_coords.len(), 1);
        assert_eq!(rs.total_floor, 1, "total_floor must not double-count");
        assert_eq!(rs.act_floor, 3);
        assert_eq!(rs.map_point_history.len(), 1, "no second history entry");
    }

    /// Missing `map_point_type` OR `room_type` suppresses the history
    /// append but still bumps the floor counters.
    #[test]
    fn add_visited_coord_without_types_skips_history_append() {
        use crate::map::MapCoord;
        let mut rs = RunState::default();
        let novel = rs.add_visited_coord(MapCoord::new(0, 0), None, None);
        assert!(novel);
        assert_eq!(rs.total_floor, 1);
        assert!(rs.map_point_history.is_empty());
    }

    /// `advance_act` clears the per-act trackers but preserves the
    /// run-scoped ones — matches Python `enter_act` (`run_state.py:1577-1583`).
    #[test]
    fn advance_act_clears_per_act_state_preserves_run_state() {
        use crate::map::{MapCoord, MapPointType};
        let mut rs = RunState::default();
        rs.add_visited_coord(
            MapCoord::new(0, 2),
            Some(MapPointType::Monster),
            Some(RoomType::Monster),
        );
        rs.add_visited_coord(
            MapCoord::new(0, 5),
            Some(MapPointType::Elite),
            Some(RoomType::Elite),
        );
        assert_eq!(rs.total_floor, 2);
        rs.advance_act();
        assert!(rs.visited_map_coords.is_empty(), "per-act: cleared");
        assert_eq!(rs.act_floor, 0, "per-act: reset");
        assert_eq!(rs.total_floor, 2, "run-scoped: preserved");
        assert_eq!(rs.map_point_history.len(), 2, "run-scoped: preserved");
    }

    // ── map-relics parity: shop pull must mirror SharedRelicGrabBag.Remove ─

    /// A back-pull from the PLAYER grab bag (the shop-stocking path,
    /// `MerchantRelicEntry.FillSlot` → `RelicFactory.PullNextRelicFromBack`
    /// → `SharedRelicGrabBag.Remove`, `RelicFactory.cs:45`) must also strip
    /// the relic from the SHARED bag, so a later treasure-room pull
    /// (`pop_relic_from_shared_grab_bag`) skips it. Regression for the
    /// Defect seed=1019 divergence where a shop-offered (unbought) relic
    /// stayed at the front of Rust's shared rare bucket and shifted the
    /// treasure pull.
    #[test]
    fn player_bag_back_pull_strips_shared_bag() {
        let mut rs = RunState::default();
        // Same id in both bags' rare bucket; shared bag has a second id
        // behind it that the treasure pull should land on after removal.
        rs.relic_grab_bag_by_rarity
            .insert(RelicRarity::Rare, vec![RelicId::GamblingChip]);
        rs.relic_grab_bag.push(RelicId::GamblingChip);
        rs.shared_relic_grab_bag_by_rarity.insert(
            RelicRarity::Rare,
            vec![RelicId::GamblingChip, RelicId::IntimidatingHelmet],
        );

        // Shop stocks GamblingChip via a back-pull (not bought, not owned).
        let picked = rs.pop_relic_from_grab_bag_back(RelicRarity::Rare, |_| true);
        assert_eq!(picked, Some((RelicId::GamblingChip, RelicRarity::Rare)));
        // The shared bag must no longer front GamblingChip.
        assert_eq!(
            rs.shared_relic_grab_bag_by_rarity[&RelicRarity::Rare],
            vec![RelicId::IntimidatingHelmet],
        );

        // Treasure-room pull now lands on the next entry, as in C#.
        let treasure = rs.pop_relic_from_shared_grab_bag(RelicRarity::Rare);
        assert_eq!(treasure, Some(RelicId::IntimidatingHelmet));
    }

    /// Obtaining a relic strips it from the shared bag too
    /// (`RelicCmd.cs:29-30`).
    #[test]
    fn obtain_relic_strips_shared_bag() {
        let mut rs = RunState::default();
        rs.shared_relic_grab_bag_by_rarity.insert(
            RelicRarity::Rare,
            vec![RelicId::GamblingChip, RelicId::IntimidatingHelmet],
        );
        rs.obtain_relic(RelicId::GamblingChip);
        assert_eq!(
            rs.shared_relic_grab_bag_by_rarity[&RelicRarity::Rare],
            vec![RelicId::IntimidatingHelmet],
        );
    }

    // ── Bead sts2-rs-4aunm: RunState::generate_map ──────────────────────

    #[test]
    fn map_and_ascension_defaults() {
        let rs = RunState::default();
        assert!(rs.current_act_map.is_none());
        assert_eq!(rs.ascension_level, 0);
    }

    #[test]
    fn generate_map_populates_current_act_map_with_act1_room_count() {
        let mut rs = RunState::default();
        assert_eq!(rs.act_index, 1);
        rs.generate_map();
        let act_map = rs.current_act_map.as_ref().expect("map populated");
        // map_length tracks the boss row: num_rooms (15) + 1.
        assert_eq!(act_map.map_length, 16);
        assert!(act_map.start.is_some(), "generator must place start");
        assert!(act_map.boss.is_some(), "generator must place boss");
    }

    #[test]
    fn generate_map_overwrites_prior_map() {
        let mut rs = RunState::default();
        rs.generate_map();
        assert!(rs.current_act_map.is_some());
        let first_point_count = rs.current_act_map.as_ref().unwrap().points.len();
        rs.generate_map();
        let second_point_count = rs.current_act_map.as_ref().unwrap().points.len();
        assert!(first_point_count > 0);
        assert!(second_point_count > 0);
    }

    /// `generate_map()` fires the `modify_generated_map` channel.
    /// We register a one-shot listener that records the fire and
    /// mutates the in-flight map; post-call we verify both signals.
    #[test]
    fn generate_map_fires_modify_generated_map_channel() {
        use crate::map::MapCoord;
        use crate::relics::RelicId;
        use crate::run_effects::{register_modify_generated_map, ModifyGeneratedMapRunFn};
        use std::sync::atomic::{AtomicUsize, Ordering};

        static FIRES: AtomicUsize = AtomicUsize::new(0);

        fn handler(
            _rs: &mut RunState,
            act_map: &mut crate::map::ActMap,
            _act: u8,
        ) -> crate::card_effects::EffectResult {
            FIRES.fetch_add(1, Ordering::SeqCst);
            act_map
                .marked_rooms
                .entry(MapCoord::new(99, 99))
                .or_default()
                .insert(RelicId::EmptyCage);
            crate::card_effects::EffectResult::Resolved
        }

        register_modify_generated_map(RelicId::EmptyCage, handler as ModifyGeneratedMapRunFn);

        let baseline = FIRES.load(Ordering::SeqCst);
        let mut rs = RunState::default();
        rs.generate_map();
        assert!(
            FIRES.load(Ordering::SeqCst) > baseline,
            "modify_generated_map listener must fire during generate_map"
        );
        assert!(
            rs.current_act_map
                .as_ref()
                .unwrap()
                .marked_rooms
                .contains_key(&MapCoord::new(99, 99)),
            "post-generate map must carry the listener's mutation"
        );
    }

    // ── Bead sts2-rs-2mjei: enter_room auto-invocation ──────────────────

    /// `Room::at` carries map context that triggers `add_visited_coord`
    /// on `enter_room` — bumps act_floor, total_floor, pushes onto
    /// visited_map_coords, and appends a history entry. Pins the
    /// auto-invocation contract end-to-end.
    #[test]
    fn enter_room_with_map_context_auto_bumps_trackers() {
        use crate::map::{MapCoord, MapPointType};
        let mut rs = RunState::default();
        let coord = MapCoord::new(3, 6);
        let room = Room::at(coord, MapPointType::Monster, RoomType::Monster);
        rs.enter_room(room);

        assert_eq!(rs.visited_map_coords, vec![coord]);
        assert_eq!(rs.act_floor, 7, "act_floor = row + 1");
        assert_eq!(rs.total_floor, 1);
        assert_eq!(rs.map_point_history.len(), 1);
        assert_eq!(
            rs.map_point_history[0].map_point_type,
            MapPointType::Monster
        );
        assert_eq!(rs.map_point_history[0].room_type, RoomType::Monster);
        assert_eq!(rs.current_room, Some(room));
    }

    /// Coord-less rooms (the legacy `Room::new` API) preserve original
    /// behaviour — no tracker mutation, just `current_room` + the
    /// `after_room_entered` channel.
    #[test]
    fn enter_room_without_map_context_preserves_legacy_behaviour() {
        let mut rs = RunState::default();
        rs.enter_room(Room::new(RoomType::Monster));
        assert!(rs.visited_map_coords.is_empty());
        assert_eq!(rs.act_floor, 0);
        assert_eq!(rs.total_floor, 0);
        assert!(rs.map_point_history.is_empty());
        assert_eq!(
            rs.current_room.map(|r| r.room_type),
            Some(RoomType::Monster)
        );
    }

    /// Repeat-visit semantics flow through from `add_visited_coord`:
    /// re-entering the same map coord does not double-count.
    #[test]
    fn enter_room_at_same_coord_twice_does_not_double_count() {
        use crate::map::{MapCoord, MapPointType};
        let mut rs = RunState::default();
        let coord = MapCoord::new(2, 1);
        rs.enter_room(Room::at(coord, MapPointType::Monster, RoomType::Monster));
        rs.enter_room(Room::at(coord, MapPointType::Monster, RoomType::Monster));
        assert_eq!(rs.visited_map_coords.len(), 1);
        assert_eq!(rs.total_floor, 1);
        assert_eq!(
            rs.map_point_history.len(),
            1,
            "history append is gated on novelty, same as Python"
        );
    }

    /// Unknown-room flow: a `?` node resolved to `RoomType::Monster`
    /// keeps `map_point_type = Unknown` in the history, mirroring
    /// Python's "audit-trail distinguishes Monster-via-? from
    /// Monster-via-M".
    #[test]
    fn enter_room_unknown_node_preserves_node_type_in_history() {
        use crate::map::{MapCoord, MapPointType};
        let mut rs = RunState::default();
        let coord = MapCoord::new(0, 4);
        rs.enter_room(Room::at(coord, MapPointType::Unknown, RoomType::Monster));
        let entry = rs.map_point_history[0];
        assert_eq!(entry.map_point_type, MapPointType::Unknown);
        assert_eq!(entry.room_type, RoomType::Monster);
    }

    /// `count_map_point_history_entries` filters by room_type,
    /// map_point_type, both, or neither (Python keyword-only filters
    /// flattened to `Option`).
    #[test]
    fn count_map_point_history_entries_filter_table() {
        use crate::map::MapPointType;
        let mut rs = RunState::default();
        rs.append_to_map_point_history(MapPointType::Monster, RoomType::Monster);
        rs.append_to_map_point_history(MapPointType::Unknown, RoomType::Monster);
        rs.append_to_map_point_history(MapPointType::RestSite, RoomType::RestSite);

        assert_eq!(rs.count_map_point_history_entries(None, None), 3);
        assert_eq!(
            rs.count_map_point_history_entries(Some(RoomType::Monster), None),
            2,
            "two monster rooms (one via M node, one via ? node)"
        );
        assert_eq!(
            rs.count_map_point_history_entries(None, Some(MapPointType::Unknown)),
            1
        );
        assert_eq!(
            rs.count_map_point_history_entries(
                Some(RoomType::Monster),
                Some(MapPointType::Monster)
            ),
            1,
            "intersection filter — only the M-node monster room"
        );
    }

    /// `RunState::default()` produces an empty `pending_rewards` queue
    /// — pins the contract downstream relic-listener tests rely on.
    #[test]
    fn pending_rewards_default_empty() {
        let rs = RunState::default();
        assert!(rs.pending_rewards.is_empty());

        let rs = RunState::new();
        assert!(rs.pending_rewards.is_empty());
    }

    /// Bead `sts2-rs-nfoh0`: `add_card_removal_reward` stages one
    /// `Reward::RemoveCard { count }`, and is a no-op for `count == 0`.
    #[test]
    fn add_card_removal_reward_stages_and_skips_zero() {
        let mut rs = RunState::default();
        rs.add_card_removal_reward(0);
        assert!(rs.pending_rewards.is_empty(), "count == 0 stages nothing");

        rs.add_card_removal_reward(3);
        assert_eq!(rs.pending_rewards, vec![Reward::RemoveCard { count: 3 }]);
    }

    /// Bead `sts2-rs-jyfvu` (P9.1.e thin slice):
    /// `RunState::generate_card_reward_rarities` rolls `num_cards`
    /// rarities, honors `opts.num_cards`, and advances the pity
    /// counter via [`CardRarityOdds::roll`].
    #[test]
    fn generate_card_reward_rarities_yields_num_cards_entries() {
        let mut rs = RunState::default().with_rewards_seed(0xC0FFEE);
        let opts = CardRewardGenerationOptions {
            num_cards: 5,
            ..CardRewardGenerationOptions::default()
        };
        let rarities = rs.generate_card_reward_rarities(opts);
        assert_eq!(rarities.len(), 5);
    }

    // -- Pael cluster trackers (bead `sts2-rs-70anj.3.20`) -------------

    #[test]
    fn pael_state_defaults_to_zero() {
        let rs = RunState::default();
        assert_eq!(rs.pael_pet_level(), 0);
        assert_eq!(rs.pael_sacrificed_count(), 0);
        assert_eq!(rs.pael_fed_count(), 0);
    }

    #[test]
    fn pael_level_up_accumulates_and_saturates() {
        let mut rs = RunState::default();
        rs.pael_level_up(3);
        assert_eq!(rs.pael_pet_level(), 3);
        rs.pael_level_up(2);
        assert_eq!(rs.pael_pet_level(), 5);
        rs.pael_state.level = u32::MAX - 1;
        rs.pael_level_up(10);
        assert_eq!(rs.pael_pet_level(), u32::MAX, "saturates at u32::MAX");
    }

    #[test]
    fn pael_record_sacrifice_returns_running_count() {
        let mut rs = RunState::default();
        assert_eq!(rs.pael_record_sacrifice(), 1);
        assert_eq!(rs.pael_record_sacrifice(), 2);
        assert_eq!(rs.pael_record_sacrifice(), 3);
        assert_eq!(rs.pael_sacrificed_count(), 3);
    }

    #[test]
    fn pael_record_feed_returns_running_count_independent_of_level() {
        let mut rs = RunState::default();
        // Feeds and levels track separately — feeding 5x does NOT
        // automatically level the pet up.
        for _ in 0..5 {
            let _ = rs.pael_record_feed();
        }
        assert_eq!(rs.pael_fed_count(), 5);
        assert_eq!(rs.pael_pet_level(), 0, "feeds don't auto-level");
    }

    #[test]
    fn pael_state_field_round_trips() {
        // Confirm the three trackers are individually observable via
        // the accessors after the mutators bump them — i.e. there is
        // no field-name typo or off-by-one in the path between mutator
        // → struct field → accessor.
        let mut rs = RunState::default();
        rs.pael_level_up(7);
        for _ in 0..11 {
            let _ = rs.pael_record_sacrifice();
        }
        for _ in 0..13 {
            let _ = rs.pael_record_feed();
        }
        assert_eq!(rs.pael_pet_level(), 7);
        assert_eq!(rs.pael_sacrificed_count(), 11);
        assert_eq!(rs.pael_fed_count(), 13);
    }

    /// Bead `sts2-rs-597ws` (P9.1.f): `populate_relic_grab_bag` lands
    /// the three pools verbatim onto the matching `RunState` fields.
    #[test]
    fn populate_relic_grab_bag_stores_supplied_pools() {
        let mut rs = RunState::default();
        rs.populate_relic_grab_bag(
            vec![RelicId::Anchor, RelicId::Pendulum],
            vec![RelicId::Lantern],
            vec![RelicId::BlackBlood],
        );
        assert_eq!(rs.relic_grab_bag, vec![RelicId::Anchor, RelicId::Pendulum]);
        assert_eq!(rs.relic_grab_bag_fallback, vec![RelicId::Lantern]);
        assert_eq!(rs.boss_relic_pool, vec![RelicId::BlackBlood]);
    }

    /// Bead `sts2-rs-597ws` (P9.1.f): `remove_relic_from_grab_bag`
    /// strips ALL occurrences from every pool (Python contract).
    #[test]
    fn remove_relic_from_grab_bag_strips_every_pool() {
        let mut rs = RunState::default();
        rs.populate_relic_grab_bag(
            vec![RelicId::Anchor, RelicId::Pendulum, RelicId::Anchor],
            vec![RelicId::Anchor, RelicId::Lantern],
            vec![RelicId::Anchor, RelicId::BlackBlood],
        );
        rs.remove_relic_from_grab_bag(RelicId::Anchor);
        assert_eq!(rs.relic_grab_bag, vec![RelicId::Pendulum]);
        assert_eq!(rs.relic_grab_bag_fallback, vec![RelicId::Lantern]);
        assert_eq!(rs.boss_relic_pool, vec![RelicId::BlackBlood]);
    }

    /// Bead `sts2-rs-597ws` (P9.1.f): `offer_boss_relic_choice` returns
    /// up to `count` entries from `boss_relic_pool`, skipping any the
    /// player already owns, in pool order.
    #[test]
    fn offer_boss_relic_choice_skips_owned_returns_up_to_count() {
        // Player already owns Anchor.
        let mut rs = RunState {
            relics: vec![RelicId::Anchor],
            ..RunState::default()
        };
        rs.populate_relic_grab_bag(
            vec![],
            vec![],
            vec![
                RelicId::Anchor, // skipped (owned)
                RelicId::BlackBlood,
                RelicId::Pendulum,
                RelicId::Lantern,
            ],
        );
        let offered = rs.offer_boss_relic_choice(3);
        assert_eq!(
            offered,
            vec![RelicId::BlackBlood, RelicId::Pendulum, RelicId::Lantern]
        );
        // Offering does NOT drain the pool.
        assert_eq!(rs.boss_relic_pool.len(), 4);
    }

    /// Bead `sts2-rs-597ws` (P9.1.f): when boss pool has fewer
    /// non-owned entries than `count`, the offer is short.
    #[test]
    fn offer_boss_relic_choice_short_when_pool_exhausted() {
        let mut rs = RunState {
            relics: vec![RelicId::Anchor],
            ..RunState::default()
        };
        rs.populate_relic_grab_bag(vec![], vec![], vec![RelicId::Anchor, RelicId::Pendulum]);
        let offered = rs.offer_boss_relic_choice(3);
        assert_eq!(offered, vec![RelicId::Pendulum]);
    }

    /// Bead `sts2-rs-597ws` (P9.1.f): `has_available_relics` is true
    /// when grab_bag or fallback has any non-owned entry; false when
    /// all are owned or both are empty.
    #[test]
    fn has_available_relics_checks_grab_bag_and_fallback() {
        let mut rs = RunState::default();
        assert!(!rs.has_available_relics(), "empty pools → false");

        rs.populate_relic_grab_bag(vec![RelicId::Anchor], vec![], vec![]);
        assert!(rs.has_available_relics());

        // After picking Anchor, has_available is false again.
        rs.relics = vec![RelicId::Anchor];
        assert!(!rs.has_available_relics());

        // Fallback alone is enough.
        rs.populate_relic_grab_bag(vec![RelicId::Anchor], vec![RelicId::Pendulum], vec![]);
        assert!(rs.has_available_relics());

        // Boss pool does NOT count toward has_available_relics
        // (matches Python contract).
        rs.populate_relic_grab_bag(vec![], vec![], vec![RelicId::BlackBlood]);
        assert!(!rs.has_available_relics());
    }

    // -- l9vkh: per-rarity grab-bag buckets + pop primitive ----------

    fn make_grab_bag_buckets() -> BTreeMap<RelicRarity, Vec<RelicId>> {
        let mut m = BTreeMap::new();
        m.insert(
            RelicRarity::Common,
            vec![RelicId::Anchor, RelicId::Pendulum],
        );
        m.insert(RelicRarity::Uncommon, vec![RelicId::Lantern]);
        m.insert(RelicRarity::Rare, vec![RelicId::BlackBlood]);
        m.insert(RelicRarity::Shop, vec![RelicId::DivineRight]);
        m
    }

    /// Bead `sts2-rs-l9vkh`: per-rarity populator stores buckets and
    /// rebuilds the flat view in (Common → Uncommon → Rare → Shop)
    /// order.
    #[test]
    fn populate_by_rarity_rebuilds_flat_view_in_iteration_order() {
        let mut rs = RunState::default();
        rs.populate_relic_grab_bag_by_rarity(
            make_grab_bag_buckets(),
            vec![RelicId::CrackedCore],
            vec![RelicId::BoundPhylactery],
        );
        assert_eq!(
            rs.relic_grab_bag,
            vec![
                RelicId::Anchor,
                RelicId::Pendulum,
                RelicId::Lantern,
                RelicId::BlackBlood,
                RelicId::DivineRight,
            ],
        );
        assert_eq!(rs.relic_grab_bag_fallback, vec![RelicId::CrackedCore]);
        assert_eq!(rs.boss_relic_pool, vec![RelicId::BoundPhylactery]);
        assert_eq!(
            rs.relic_grab_bag_by_rarity.get(&RelicRarity::Common),
            Some(&vec![RelicId::Anchor, RelicId::Pendulum]),
        );
    }

    /// Bead `sts2-rs-l9vkh`: `pop_relic_from_grab_bag(rarity)` drains
    /// the rolled bucket front-first when entries are available.
    #[test]
    fn pop_relic_drains_from_rolled_bucket_front() {
        let mut rs = RunState::default();
        rs.populate_relic_grab_bag_by_rarity(make_grab_bag_buckets(), vec![], vec![]);
        let picked = rs.pop_relic_from_grab_bag(RelicRarity::Uncommon);
        assert_eq!(picked, Some(RelicId::Lantern));
        assert!(rs.relic_grab_bag_by_rarity[&RelicRarity::Uncommon].is_empty());
        // Flat view also pruned.
        assert!(!rs.relic_grab_bag.contains(&RelicId::Lantern));
    }

    /// Bead `sts2-rs-l9vkh`: when the rolled bucket is empty, the
    /// fallback chain walks down to lower-tier siblings. Common-roll
    /// with empty Common bucket → Uncommon → Rare.
    #[test]
    fn pop_relic_walks_fallback_chain_when_bucket_empty() {
        let mut rs = RunState::default();
        let mut buckets = BTreeMap::new();
        buckets.insert(RelicRarity::Common, vec![]);
        buckets.insert(RelicRarity::Uncommon, vec![]);
        buckets.insert(RelicRarity::Rare, vec![RelicId::BlackBlood]);
        rs.populate_relic_grab_bag_by_rarity(buckets, vec![], vec![]);
        let picked = rs.pop_relic_from_grab_bag(RelicRarity::Common);
        assert_eq!(picked, Some(RelicId::BlackBlood));
    }

    /// Bead `sts2-rs-l9vkh`: owned relics are skipped while walking
    /// the rolled bucket — the pop returns the first non-owned entry.
    #[test]
    fn pop_relic_skips_owned_within_bucket() {
        let mut rs = RunState::default();
        rs.populate_relic_grab_bag_by_rarity(make_grab_bag_buckets(), vec![], vec![]);
        rs.relics = vec![RelicId::Anchor];
        let picked = rs.pop_relic_from_grab_bag(RelicRarity::Common);
        assert_eq!(picked, Some(RelicId::Pendulum));
        // Anchor remained in the bucket since pop skips owned, not
        // drains owned.
        assert_eq!(
            rs.relic_grab_bag_by_rarity[&RelicRarity::Common],
            vec![RelicId::Anchor],
        );
    }

    /// Bead `sts2-rs-l9vkh`: when every bucket in the fallback chain
    /// is exhausted of non-owned entries, the dedicated fallback list
    /// engages. Then when both are exhausted, returns `None`.
    #[test]
    fn pop_relic_falls_back_to_fallback_list_then_none() {
        let mut rs = RunState::default();
        let mut buckets = BTreeMap::new();
        buckets.insert(RelicRarity::Rare, vec![]);
        rs.populate_relic_grab_bag_by_rarity(buckets, vec![RelicId::CrackedCore], vec![]);
        let picked = rs.pop_relic_from_grab_bag(RelicRarity::Rare);
        assert_eq!(picked, Some(RelicId::CrackedCore));
        assert!(rs.relic_grab_bag_fallback.is_empty());
        // Now everything is gone.
        let next = rs.pop_relic_from_grab_bag(RelicRarity::Rare);
        assert_eq!(next, None);
    }

    /// Bead `sts2-rs-l9vkh`: Shop-roll fallback chain is
    /// `[Shop, Common, Uncommon, Rare]` per Python
    /// `_relic_grab_bag_order(SHOP)`.
    #[test]
    fn pop_relic_shop_fallback_chain_matches_python() {
        let mut rs = RunState::default();
        let mut buckets = BTreeMap::new();
        buckets.insert(RelicRarity::Shop, vec![]);
        buckets.insert(RelicRarity::Common, vec![RelicId::Anchor]);
        rs.populate_relic_grab_bag_by_rarity(buckets, vec![], vec![]);
        let picked = rs.pop_relic_from_grab_bag(RelicRarity::Shop);
        assert_eq!(picked, Some(RelicId::Anchor));
    }

    /// Bead `sts2-rs-l9vkh`: `remove_relic_from_grab_bag` also strips
    /// from the per-rarity buckets (extended in P9.2.r).
    #[test]
    fn remove_relic_strips_from_per_rarity_buckets() {
        let mut rs = RunState::default();
        rs.populate_relic_grab_bag_by_rarity(make_grab_bag_buckets(), vec![], vec![]);
        rs.remove_relic_from_grab_bag(RelicId::Lantern);
        assert!(rs.relic_grab_bag_by_rarity[&RelicRarity::Uncommon].is_empty());
        assert!(!rs.relic_grab_bag.contains(&RelicId::Lantern));
    }

    /// Bead `sts2-rs-l9vkh`: `RelicRarity::fallback_order` mirrors
    /// Python's `_relic_grab_bag_order` exactly for all four bucket
    /// variants.
    #[test]
    fn relic_rarity_fallback_order_matches_python() {
        assert_eq!(
            RelicRarity::Common.fallback_order(),
            &[
                RelicRarity::Common,
                RelicRarity::Uncommon,
                RelicRarity::Rare
            ],
        );
        assert_eq!(
            RelicRarity::Uncommon.fallback_order(),
            &[RelicRarity::Uncommon, RelicRarity::Rare],
        );
        assert_eq!(RelicRarity::Rare.fallback_order(), &[RelicRarity::Rare]);
        assert_eq!(
            RelicRarity::Shop.fallback_order(),
            &[
                RelicRarity::Shop,
                RelicRarity::Common,
                RelicRarity::Uncommon,
                RelicRarity::Rare,
            ],
        );
    }

    /// Bead `sts2-rs-qwp1g` (P9.2.d): `Rest` option applies the
    /// 30%-of-max-HP heal through `rest_site_heal`. Returns the
    /// post-clamp delta in `Rested.healed`.
    ///
    /// The expected heal is computed by folding the
    /// `modify_rest_site_heal_amount` channel onto the 30%-of-max-HP
    /// base so this test is robust against process-global registry
    /// pollution from sibling tests that register additive listeners
    /// against `FakeAnchor` / `FakeBloodVial` etc. (see
    /// `run_effects::tests::modify_rest_site_heal_amount_*`). Without
    /// this guard, when the registry has accumulated `+15 + 3 = +18`
    /// of polluting listeners, the heal jumps from 30 → 48 and breaks
    /// this test in deterministic full-suite runs.
    #[test]
    fn do_rest_site_action_rest_applies_heal_through_channel() {
        let mut rs = RunState {
            max_hp: 100,
            current_hp: 10,
            ..RunState::default()
        };
        let base = 30; // 30 % of max_hp(100).
        let expected = rs.compute_rest_site_heal_amount(base).max(0);
        let outcome = rs.do_rest_site_action(RestSiteOption::Rest);
        let clamped = expected.min(100 - 10);
        assert_eq!(outcome, RestSiteActionOutcome::Rested { healed: clamped },);
        assert_eq!(rs.current_hp, 10 + clamped);
    }

    /// Bead `sts2-rs-qwp1g`: heal clamps to max_hp.
    #[test]
    fn do_rest_site_action_rest_clamps_at_max_hp() {
        let mut rs = RunState {
            max_hp: 80,
            current_hp: 70,
            ..RunState::default()
        };
        let outcome = rs.do_rest_site_action(RestSiteOption::Rest);
        // 30 % of 80 = 24; 70 + 24 = 94, clamped to 80 → healed = 10.
        assert_eq!(outcome, RestSiteActionOutcome::Rested { healed: 10 });
        assert_eq!(rs.current_hp, 80);
    }

    /// Bead `sts2-rs-qwp1g`: Smith with one upgradable card
    /// auto-resolves and the master_deck entry's `upgraded` flag flips.
    #[test]
    fn do_rest_site_action_smith_single_card_auto_upgrades() {
        use crate::cards::Card;
        let mut rs = RunState::default();
        // Master deck: one upgradable card.
        rs.master_deck.push(Card {
            card_id: CardId::Bash,
            upgraded: false,
            ..Card::pod_defaults()
        });
        let outcome = rs.do_rest_site_action(RestSiteOption::Smith { enabled: true });
        assert_eq!(
            outcome,
            RestSiteActionOutcome::SmithUpgraded {
                card_id: CardId::Bash
            }
        );
        assert!(rs.master_deck[0].upgraded);
        assert!(rs.pending_choice.is_none());
    }

    /// Bead `sts2-rs-qwp1g`: Smith with multiple upgradable cards
    /// opens the pending_choice slot for the agent to resolve.
    #[test]
    fn do_rest_site_action_smith_multi_opens_pending_choice() {
        use crate::cards::Card;
        let mut rs = RunState::default();
        for id in [CardId::Bash, CardId::PommelStrike, CardId::Anger] {
            rs.master_deck.push(Card {
                card_id: id,
                upgraded: false,
                ..Card::pod_defaults()
            });
        }
        let outcome = rs.do_rest_site_action(RestSiteOption::Smith { enabled: true });
        assert_eq!(outcome, RestSiteActionOutcome::SmithAwaitingChoice);
        let pc = rs.pending_choice.as_ref().expect("slot opened");
        assert_eq!(pc.options.len(), 3);
        assert_eq!(pc.min_select, 1);
        assert_eq!(pc.max_select, 1);
    }

    /// Bead `sts2-rs-qwp1g`: disabled options no-op.
    #[test]
    fn do_rest_site_action_disabled_option_is_noop() {
        let mut rs = RunState::default();
        let hp_before = rs.current_hp;
        let outcome = rs.do_rest_site_action(RestSiteOption::Smith { enabled: false });
        assert_eq!(outcome, RestSiteActionOutcome::Disabled);
        assert_eq!(rs.current_hp, hp_before);
    }

    /// Bead `sts2-rs-qwp1g`: deferred variants return `Unsupported`
    /// without state mutation.
    #[test]
    fn do_rest_site_action_deferred_variants_return_unsupported() {
        let mut rs = RunState::default();
        let hp_before = rs.current_hp;
        // Hatch is no longer deferred (bead `sts2-rs-RP24`) — it obtains
        // Byrdpip; see `do_rest_site_action_hatch_obtains_byrdpip`.
        let outcomes = [
            rs.do_rest_site_action(RestSiteOption::Dig),
            rs.do_rest_site_action(RestSiteOption::Clone),
            rs.do_rest_site_action(RestSiteOption::Mend),
        ];
        for outcome in outcomes {
            assert_eq!(outcome, RestSiteActionOutcome::Unsupported);
        }
        assert_eq!(rs.current_hp, hp_before);
    }

    /// Bead `sts2-rs-70anj.3.13`: a request with non-empty options
    /// opens the pending-choice slot; the stored payload mirrors the
    /// caller's args (with `min`/`max` clamp).
    #[test]
    fn request_card_choice_opens_pending_slot() {
        use crate::cards::CardId;
        use crate::{ChoiceKind, ChoiceOption};
        let mut rs = RunState::default();
        let opts = vec![
            ChoiceOption {
                card_id: CardId::StrikeIronclad,
                upgraded: false,
                card_uid: None,
                source_pile: None,
            },
            ChoiceOption {
                card_id: CardId::DefendIronclad,
                upgraded: false,
                card_uid: None,
                source_pile: None,
            },
            ChoiceOption {
                card_id: CardId::Bash,
                upgraded: false,
                card_uid: None,
                source_pile: None,
            },
        ];
        let opened = rs.request_card_choice(ChoiceKind::CardSelect, opts, 1, 1, false);
        assert!(opened);
        let pc = rs.pending_choice.as_ref().expect("slot opened");
        assert_eq!(pc.options.len(), 3);
        assert_eq!(pc.min_select, 1);
        assert_eq!(pc.max_select, 1);
        assert!(!pc.can_skip);
    }

    /// Bead `sts2-rs-70anj.3.13`: empty options and double-open both
    /// return `false` (Python guards).
    #[test]
    fn request_card_choice_rejects_empty_and_double_open() {
        use crate::cards::CardId;
        use crate::{ChoiceKind, ChoiceOption};
        let mut rs = RunState::default();
        assert!(!rs.request_card_choice(ChoiceKind::CardSelect, vec![], 1, 1, false));
        assert!(rs.pending_choice.is_none());

        let opts = vec![ChoiceOption {
            card_id: CardId::StrikeIronclad,
            upgraded: false,
            card_uid: None,
            source_pile: None,
        }];
        assert!(rs.request_card_choice(ChoiceKind::CardSelect, opts.clone(), 1, 1, false));
        // Second open is rejected.
        assert!(!rs.request_card_choice(ChoiceKind::CardSelect, opts, 1, 1, false));
    }

    /// Bead `sts2-rs-70anj.3.13`: `resolve_pending_card_choice` returns
    /// the picks in caller-supplied order and clears the slot. Bad
    /// indices / duplicates / out-of-range counts return `None`
    /// without clearing.
    #[test]
    fn resolve_pending_card_choice_clears_slot_on_valid_pick() {
        use crate::cards::CardId;
        use crate::{ChoiceKind, ChoiceOption};
        let mut rs = RunState::default();
        let opts = vec![
            ChoiceOption {
                card_id: CardId::StrikeIronclad,
                upgraded: false,
                card_uid: None,
                source_pile: None,
            },
            ChoiceOption {
                card_id: CardId::Bash,
                upgraded: true,
                card_uid: None,
                source_pile: None,
            },
            ChoiceOption {
                card_id: CardId::DefendIronclad,
                upgraded: false,
                card_uid: None,
                source_pile: None,
            },
        ];
        assert!(rs.request_card_choice(ChoiceKind::CardSelect, opts, 1, 1, false));

        // Out-of-range → None, slot still open.
        assert!(rs.resolve_pending_card_choice(&[7]).is_none());
        assert!(rs.pending_choice.is_some());

        // Valid pick → Some(chosen), slot cleared.
        let chosen = rs.resolve_pending_card_choice(&[1]).expect("valid");
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].card_id, CardId::Bash);
        assert!(chosen[0].upgraded);
        assert!(rs.pending_choice.is_none());
    }

    /// Bead `sts2-rs-70anj.3.13`: multi-select honors min/max range +
    /// rejects duplicate indices.
    #[test]
    fn resolve_pending_card_choice_multi_select_range_and_dedup() {
        use crate::cards::CardId;
        use crate::{ChoiceKind, ChoiceOption};
        let mut rs = RunState::default();
        let opts: Vec<ChoiceOption> =
            [CardId::StrikeIronclad, CardId::Bash, CardId::DefendIronclad]
                .iter()
                .map(|&id| ChoiceOption {
                    card_id: id,
                    upgraded: false,
                    card_uid: None,
                    source_pile: None,
                })
                .collect();
        // min=2, max=3.
        assert!(rs.request_card_choice(ChoiceKind::CardSelect, opts, 2, 3, false));

        // Count below min.
        assert!(rs.resolve_pending_card_choice(&[0]).is_none());
        // Duplicate indices.
        assert!(rs.resolve_pending_card_choice(&[0, 0]).is_none());
        // Valid 2-pick.
        let chosen = rs
            .resolve_pending_card_choice(&[2, 0])
            .expect("valid 2-pick");
        assert_eq!(chosen.len(), 2);
        assert_eq!(chosen[0].card_id, CardId::DefendIronclad);
        assert_eq!(chosen[1].card_id, CardId::StrikeIronclad);
    }

    /// Bead `sts2-rs-70anj.3.13`: `skip_pending_card_choice` clears the
    /// slot only when `can_skip` is true.
    #[test]
    fn skip_pending_card_choice_honors_can_skip() {
        use crate::cards::CardId;
        use crate::{ChoiceKind, ChoiceOption};
        let mut rs = RunState::default();
        // No open prompt → skip returns false.
        assert!(!rs.skip_pending_card_choice());

        let opts = vec![ChoiceOption {
            card_id: CardId::StrikeIronclad,
            upgraded: false,
            card_uid: None,
            source_pile: None,
        }];

        // can_skip = false → skip returns false, slot intact.
        rs.request_card_choice(ChoiceKind::CardSelect, opts.clone(), 1, 1, false);
        assert!(!rs.skip_pending_card_choice());
        assert!(rs.pending_choice.is_some());

        // Resolve to clear, then re-open with can_skip = true.
        let _ = rs.resolve_pending_card_choice(&[0]);
        rs.request_card_choice(ChoiceKind::CardSelect, opts, 1, 1, true);
        assert!(rs.skip_pending_card_choice());
        assert!(rs.pending_choice.is_none());
    }

    /// Bead `sts2-rs-jyfvu` (P9.1.e thin slice): boss context rolls
    /// always produce `RewardRarity::Rare` regardless of pity state.
    #[test]
    fn generate_card_reward_rarities_boss_always_rare() {
        let mut rs = RunState::default().with_rewards_seed(0xDEAD_BEEF);
        let opts = CardRewardGenerationOptions {
            context: "boss",
            num_cards: 4,
            ..CardRewardGenerationOptions::default()
        };
        let rarities = rs.generate_card_reward_rarities(opts);
        assert_eq!(rarities.len(), 4);
        for r in rarities {
            assert_eq!(r, crate::card_rewards::RewardRarity::Rare);
        }
    }

    /// Bead `sts2-rs-jyfvu` (P9.1.e thin slice):
    /// `generate_combat_card_rewards` pairs each rarity with an
    /// upgrade flag, and `roll_upgrade=false` forces all flags to
    /// false (SilverCrucible's rewards-leg contract).
    #[test]
    fn generate_combat_card_rewards_roll_upgrade_false_disables_upgrades() {
        let mut rs = RunState::default().with_rewards_seed(0xFEED_FACE);
        rs.act_index = 3; // would otherwise force a high upgrade rate
        let opts = CardRewardGenerationOptions {
            num_cards: 8,
            roll_upgrade: false,
            ..CardRewardGenerationOptions::default()
        };
        let entries = rs.generate_combat_card_rewards(opts);
        assert_eq!(entries.len(), 8);
        for (_rarity, upgraded) in entries {
            assert!(!upgraded);
        }
    }

    /// Bead `sts2-rs-ib04`: `PotionReward::empty()` mirrors Python's
    /// `PotionReward(player_id)` with no pre-rolled `potion_id` —
    /// `potion_kind` defaults to `None`, signalling "dispatcher rolls
    /// at drain time".
    #[test]
    fn potion_reward_empty_has_no_pre_rolled_kind() {
        let r = PotionReward::empty();
        assert!(r.potion_kind.is_none());
    }

    /// `PotionReward::with_kind(...)` round-trips its argument —
    /// pre-rolled rewards (rare today; will be used by relic bodies
    /// that grant a *specific* potion).
    #[test]
    fn potion_reward_with_kind_round_trips() {
        let r = PotionReward::with_kind(crate::potions::PotionKind::FirePotion);
        assert_eq!(r.potion_kind, Some(crate::potions::PotionKind::FirePotion));
    }

    /// `Reward::Potion` variant can be pushed onto `pending_rewards`
    /// and matched. Acts as the integration sentinel for events that
    /// swap their `Reward::Card(empty)` placeholders to
    /// `Reward::Potion(PotionReward::empty())`.
    #[test]
    fn reward_potion_round_trips_through_pending_rewards() {
        let mut rs = RunState::new();
        rs.pending_rewards
            .push(Reward::Potion(PotionReward::empty()));
        assert_eq!(rs.pending_rewards.len(), 1);
        let Reward::Potion(reward) = &rs.pending_rewards[0] else {
            panic!("expected Reward::Potion");
        };
        assert!(reward.potion_kind.is_none());
    }

    /// `Room::new(RoomType::RestSite)` round-trips its `room_type` —
    /// the field is the canonical query path `after_room_entered`
    /// listeners use to gate behaviour.
    #[test]
    fn room_new_round_trips_room_type() {
        let r = Room::new(RoomType::RestSite);
        assert_eq!(r.room_type, RoomType::RestSite);
    }

    /// Sanity guard: `RoomType` variants match the Python pin count
    /// + ordering (the seven Python variants plus the Rust-side
    /// `Unknown` catch-all).
    #[test]
    fn room_type_variant_count_matches_python_pin() {
        // If the Python pin grows a new RoomType, bump this test.
        // Order-pinned variants (Python parity): Monster, Elite, Boss,
        // Shop, RestSite, Treasure, Event.
        let variants = [
            RoomType::Unknown,
            RoomType::Monster,
            RoomType::Elite,
            RoomType::Boss,
            RoomType::Shop,
            RoomType::RestSite,
            RoomType::Treasure,
            RoomType::Event,
        ];
        assert_eq!(variants.len(), 8);
    }

    /// `Reward` variant constructors are equality-comparable — used
    /// by the `modify_rewards` hook bus tests + downstream parity
    /// fixture diffs.
    #[test]
    fn reward_variants_equality() {
        let a = Reward::Card(CardReward::from_options(vec![CardId::StrikeIronclad]));
        let b = Reward::Card(CardReward::from_options(vec![CardId::StrikeIronclad]));
        assert_eq!(a, b);

        let g1 = Reward::Gold { amount: 25 };
        let g2 = Reward::Gold { amount: 25 };
        assert_eq!(g1, g2);
    }

    /// Bead `sts2-rs-a8l8h`: offer_relic_rewards with count=0 must
    /// leave relic_rewards unchanged. A regression off-by-oning the
    /// loop bound (e.g. `1..=count` or `0..=count`) would silently
    /// push a phantom reward on every count=0 call. The sibling
    /// offer_relic_rewards_appends_to_queue test only covers count=3.
    #[test]
    fn offer_relic_rewards_count_zero_is_noop() {
        let mut rs = RunState::new();
        assert!(rs.relic_rewards.is_empty());
        rs.offer_relic_rewards(RelicId::BigMushroom, 0);
        assert!(
            rs.relic_rewards.is_empty(),
            "count=0 must not push any RelicReward"
        );
        // Calling count=0 after a populated push must also be inert.
        rs.offer_relic_rewards(RelicId::BigMushroom, 2);
        assert_eq!(rs.relic_rewards.len(), 2);
        rs.offer_relic_rewards(RelicId::SwordOfStone, 0);
        assert_eq!(
            rs.relic_rewards.len(),
            2,
            "count=0 after a populated queue must not grow the queue"
        );
    }

    /// `RunState::offer_relic_rewards` appends one `RelicReward` per
    /// `count` to the dedicated `relic_rewards` queue (bead
    /// `sts2-rs-bms`). Pins the Maw-Bank / `WongosMysteryTicket`
    /// retrofit contract.
    #[test]
    fn offer_relic_rewards_appends_to_queue() {
        let mut rs = RunState::new();
        assert!(rs.relic_rewards.is_empty());

        rs.offer_relic_rewards(RelicId::BigMushroom, 3);
        assert_eq!(rs.relic_rewards.len(), 3);
        assert!(rs
            .relic_rewards
            .iter()
            .all(|r| r.relic_id == RelicId::BigMushroom));

        // pending_rewards must remain untouched — the two queues are
        // independent surfaces.
        assert!(rs.pending_rewards.is_empty());
    }

    /// `RunState::accept_relic_reward(idx)` consumes the matching
    /// `relic_rewards` entry, pushes its id onto `relics`, and fires
    /// `run_effects::dispatch_after_obtained`. Sentinel test for bead
    /// `sts2-rs-zq8` (j87-b).
    ///
    /// Uses a process-static counter to verify the listener fired —
    /// the listener registry is global, so this test counts deltas
    /// rather than absolute values to stay robust against parallel
    /// `run_effects` tests.
    #[test]
    fn accept_relic_reward_fires_after_obtained_listener() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{
            dispatch_after_obtained, register_after_obtained, AfterObtainedRunFn,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        fn listener(_rs: &mut RunState) -> EffectResult {
            COUNTER.fetch_add(1, Ordering::SeqCst);
            EffectResult::Resolved
        }

        register_after_obtained(RelicId::Akabeko, listener as AfterObtainedRunFn);

        // Seed the reward queue + capture baseline (other tests may
        // have already registered listeners that fire here too).
        let mut rs = RunState::new();
        rs.offer_relic_rewards(RelicId::Akabeko, 1);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert!(rs.relics.is_empty());

        let baseline = COUNTER.load(Ordering::SeqCst);
        let acquired = rs.accept_relic_reward(0);

        // Reward consumed, relic recorded.
        assert_eq!(acquired, Some(RelicId::Akabeko));
        assert!(rs.relic_rewards.is_empty());
        assert_eq!(rs.relics, vec![RelicId::Akabeko]);

        // Listener fired at least once on top of any baseline.
        assert!(COUNTER.load(Ordering::SeqCst) > baseline);

        // Direct dispatch without a real acquisition also walks the
        // registry — confirms the listener stays subscribed.
        let baseline2 = COUNTER.load(Ordering::SeqCst);
        let _ = dispatch_after_obtained(&mut rs);
        assert!(COUNTER.load(Ordering::SeqCst) > baseline2);
    }

    /// `sts2-rs-pfiwm`: obtaining a countdown relic seeds its run-side
    /// counter from `RelicKind::initial_counter()` at pickup (EmberTea
    /// starts at 5), so a run-side reader sees the real charge before
    /// any combat.
    #[test]
    fn obtain_relic_seeds_countdown_counter_at_pickup() {
        let mut rs = RunState::new();
        rs.obtain_relic(RelicId::EmberTea);
        assert_eq!(
            rs.relic_counters.get(&RelicId::EmberTea),
            Some(&5),
            "EmberTea counter seeded to 5 at pickup"
        );
    }

    /// `sts2-rs-pfiwm`: relics with a zero initial counter stay absent
    /// from the map (kept sparse), and pickup never clobbers an
    /// already-recorded counter.
    #[test]
    fn obtain_relic_leaves_counterless_relics_absent_and_preserves_existing() {
        let mut rs = RunState::new();
        rs.obtain_relic(RelicId::BurningBlood);
        assert!(
            !rs.relic_counters.contains_key(&RelicId::BurningBlood),
            "zero-initial-counter relic stays absent"
        );

        // A pre-recorded counter (e.g. mid-run state) is not clobbered.
        rs.relic_counters.insert(RelicId::SwordOfStone, 2);
        rs.obtain_relic(RelicId::SwordOfStone);
        assert_eq!(
            rs.relic_counters.get(&RelicId::SwordOfStone),
            Some(&2),
            "existing counter preserved (or_insert, not overwrite)"
        );
    }

    /// Bead `sts2-rs-w785o`: `defer_followup_rewards` defaults to
    /// `false` on a fresh `RunState`, matching Python pin
    /// `run_state.py:1415`.
    #[test]
    fn defer_followup_rewards_defaults_false() {
        let rs = RunState::new();
        assert!(!rs.defer_followup_rewards);
    }

    /// Bead `sts2-rs-w785o`: `with_deferred_followup_rewards` flips the
    /// flag to `true` for the duration of the closure and restores the
    /// prior value on exit (mirrors Python's
    /// `_deferred_followup_rewards` context manager at
    /// `run_manager.py:418-424`).
    #[test]
    fn with_deferred_followup_rewards_sets_and_restores() {
        let mut rs = RunState::new();
        assert!(!rs.defer_followup_rewards);
        let mut observed = false;
        rs.with_deferred_followup_rewards(|inner| {
            observed = inner.defer_followup_rewards;
        });
        assert!(observed, "flag visible inside the closure");
        assert!(!rs.defer_followup_rewards, "restored after closure");

        // Nested call must restore to the prior `true`, not to the
        // hard-coded default `false`.
        rs.defer_followup_rewards = true;
        let mut observed_nested = false;
        rs.with_deferred_followup_rewards(|inner| {
            observed_nested = inner.defer_followup_rewards;
        });
        assert!(observed_nested);
        assert!(
            rs.defer_followup_rewards,
            "nested call must restore to prior `true`"
        );
    }

    /// Bead `sts2-rs-w785o`: redeeming a `RelicReward` via
    /// `accept_relic_reward` must run `dispatch_after_obtained` under
    /// `defer_followup_rewards == true`. The flag must be cleared on
    /// return.
    #[test]
    fn accept_relic_reward_sets_defer_flag_during_dispatch() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{register_after_obtained, AfterObtainedRunFn};
        use std::sync::atomic::{AtomicBool, Ordering};

        static SAW_DEFERRED: AtomicBool = AtomicBool::new(false);
        fn listener(rs: &mut RunState) -> EffectResult {
            if rs.relics.last() == Some(&RelicId::TinyMailbox) && rs.defer_followup_rewards {
                SAW_DEFERRED.store(true, Ordering::SeqCst);
            }
            EffectResult::Resolved
        }
        register_after_obtained(RelicId::TinyMailbox, listener as AfterObtainedRunFn);

        let mut rs = RunState::new();
        rs.offer_relic_rewards(RelicId::TinyMailbox, 1);
        assert!(!rs.defer_followup_rewards);
        let _ = rs.accept_relic_reward(0);
        assert!(
            SAW_DEFERRED.load(Ordering::SeqCst),
            "listener must observe defer_followup_rewards == true"
        );
        assert!(
            !rs.defer_followup_rewards,
            "flag must be restored after accept_relic_reward returns"
        );
    }

    /// `accept_relic_reward(idx)` returns `None` when `idx` is out of
    /// bounds and leaves the queue + relic list untouched.
    #[test]
    fn accept_relic_reward_out_of_bounds_returns_none() {
        let mut rs = RunState::new();
        rs.offer_relic_rewards(RelicId::Akabeko, 2);

        let acquired = rs.accept_relic_reward(99);
        assert_eq!(acquired, None);
        assert_eq!(rs.relic_rewards.len(), 2);
        assert!(rs.relics.is_empty());
    }

    /// `RunState::enter_room(room)` sets `current_room` and fires
    /// `run_effects::dispatch_after_room_entered` with the entered
    /// `RoomType`. Sentinel test for bead `sts2-rs-ouv` (j87-c).
    #[test]
    fn enter_room_fires_after_room_entered_listener() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{
            dispatch_after_room_entered, register_after_room_entered, AfterRoomEnteredRunFn,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Two counters: total firings + RestSite-only firings. The
        // second exercises the per-listener `RoomType` gate that real
        // listeners (DreamCatcher, FakeVenerableTeaSet) rely on.
        static TOTAL: AtomicUsize = AtomicUsize::new(0);
        static REST_SITE_ONLY: AtomicUsize = AtomicUsize::new(0);

        fn listener_total(_rs: &mut RunState, _room_type: RoomType) -> EffectResult {
            TOTAL.fetch_add(1, Ordering::SeqCst);
            EffectResult::Resolved
        }
        fn listener_rest_site(_rs: &mut RunState, room_type: RoomType) -> EffectResult {
            if room_type == RoomType::RestSite {
                REST_SITE_ONLY.fetch_add(1, Ordering::SeqCst);
                EffectResult::Resolved
            } else {
                EffectResult::NotImplemented
            }
        }

        register_after_room_entered(RelicId::MawBank, listener_total as AfterRoomEnteredRunFn);
        register_after_room_entered(
            RelicId::DreamCatcher,
            listener_rest_site as AfterRoomEnteredRunFn,
        );

        let mut rs = RunState::new();
        assert!(rs.current_room.is_none());

        let baseline_total = TOTAL.load(Ordering::SeqCst);
        let baseline_rest = REST_SITE_ONLY.load(Ordering::SeqCst);

        // Enter a Monster room — total fires, rest-site doesn't.
        rs.enter_room(Room::new(RoomType::Monster));
        assert_eq!(
            rs.current_room.map(|r| r.room_type),
            Some(RoomType::Monster)
        );
        assert!(TOTAL.load(Ordering::SeqCst) > baseline_total);
        assert_eq!(REST_SITE_ONLY.load(Ordering::SeqCst), baseline_rest);

        // Enter a RestSite — both fire.
        let baseline_total = TOTAL.load(Ordering::SeqCst);
        let baseline_rest = REST_SITE_ONLY.load(Ordering::SeqCst);
        rs.enter_room(Room::new(RoomType::RestSite));
        assert_eq!(
            rs.current_room.map(|r| r.room_type),
            Some(RoomType::RestSite)
        );
        assert!(TOTAL.load(Ordering::SeqCst) > baseline_total);
        assert!(REST_SITE_ONLY.load(Ordering::SeqCst) > baseline_rest);

        // Direct dispatch with explicit room_type also walks the
        // registry — confirms listeners stay subscribed.
        let baseline_total = TOTAL.load(Ordering::SeqCst);
        let _ = dispatch_after_room_entered(&mut rs, RoomType::Shop);
        assert!(TOTAL.load(Ordering::SeqCst) > baseline_total);
    }

    /// `RunState::enter_room` is idempotent at the state level (sets
    /// `current_room` to the same value) but NOT at the dispatcher
    /// level: per the doc-comment at runstate.rs:437–440, "calling
    /// `enter_room` twice with the same value still fires the
    /// dispatcher both times." Pins this contract — a future
    /// drive-by adding "skip dispatch when current_room == new_room"
    /// optimization would break listeners like `MawBank` that count
    /// every room transition as a discrete event. Bead
    /// `sts2-rs-wvgvb` (test-coverage for j87-c room transition
    /// surface).
    #[test]
    fn enter_room_re_entry_fires_dispatcher_twice() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{register_after_room_entered, AfterRoomEnteredRunFn};
        use std::sync::atomic::{AtomicUsize, Ordering};

        static CALLS: AtomicUsize = AtomicUsize::new(0);

        fn listener(_rs: &mut RunState, _rt: RoomType) -> EffectResult {
            CALLS.fetch_add(1, Ordering::SeqCst);
            EffectResult::Resolved
        }

        // Use a distinct RelicId from the sibling `enter_room_fires_`
        // test so its registry slot is independent of test ordering.
        register_after_room_entered(RelicId::PrayerWheel, listener as AfterRoomEnteredRunFn);

        let mut rs = RunState::new();
        let baseline = CALLS.load(Ordering::SeqCst);

        rs.enter_room(Room::new(RoomType::Monster));
        let after_first = CALLS.load(Ordering::SeqCst);
        rs.enter_room(Room::new(RoomType::Monster));
        let after_second = CALLS.load(Ordering::SeqCst);

        assert!(
            after_first > baseline,
            "first enter_room must fire (got {after_first} vs baseline {baseline})"
        );
        assert!(
            after_second > after_first,
            "re-entering the SAME RoomType must fire dispatcher again \
             (got {after_second} after second call vs {after_first} after first); \
             pins the docstring's per-transition-is-an-event contract"
        );
        assert_eq!(
            rs.current_room.map(|r| r.room_type),
            Some(RoomType::Monster)
        );
    }

    /// `RunState::enter_event_room(event_id)` sets `current_event`
    /// AND routes through `enter_room(Room::new(RoomType::Event))`,
    /// firing the `after_room_entered` hook with `RoomType::Event`.
    /// Pins both: the `current_event` state update AND the dispatcher
    /// route-through. Bead `sts2-rs-wvgvb` (test-coverage for j87-c +
    /// 6ynt.4.1 event-room surfaces).
    #[test]
    fn enter_event_room_sets_current_event_and_fires_event_type() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{register_after_room_entered, AfterRoomEnteredRunFn};
        use std::sync::atomic::{AtomicUsize, Ordering};

        static EVENT_CALLS: AtomicUsize = AtomicUsize::new(0);

        fn event_listener(_rs: &mut RunState, room_type: RoomType) -> EffectResult {
            if room_type == RoomType::Event {
                EVENT_CALLS.fetch_add(1, Ordering::SeqCst);
                EffectResult::Resolved
            } else {
                EffectResult::NotImplemented
            }
        }

        // Distinct RelicId again so registry slot isn't shared with
        // sibling tests under shuffled test ordering.
        register_after_room_entered(RelicId::WingCharm, event_listener as AfterRoomEnteredRunFn);

        let mut rs = RunState::new();
        assert!(
            rs.current_event.is_none(),
            "freshly-constructed RunState has no active event"
        );
        let baseline = EVENT_CALLS.load(Ordering::SeqCst);

        rs.enter_event_room(crate::events::EventId::LostWisp);

        assert_eq!(
            rs.current_event,
            Some(crate::events::EventId::LostWisp),
            "enter_event_room must set current_event to the supplied id"
        );
        assert_eq!(
            rs.current_room.map(|r| r.room_type),
            Some(RoomType::Event),
            "enter_event_room must route through enter_room with RoomType::Event"
        );
        assert!(
            EVENT_CALLS.load(Ordering::SeqCst) > baseline,
            "after_room_entered listener gated on RoomType::Event must fire"
        );
    }

    /// Entering an event room from a map node preserves the standing
    /// room's `map_coord` (instead of wiping it to `None`), so the next
    /// `MapChoice` reads the real position rather than falling back to
    /// the start row. Regression for the "every event teleports you back
    /// to row 1" bug. Bead `sts2-rs-26qg3` follow-up.
    #[test]
    fn enter_event_room_preserves_standing_map_coord() {
        use crate::map::{MapCoord, MapPointType};

        let mut rs = RunState::new();
        // Stand on a coord-bearing map node (as `do_map_move` does on arrival).
        let coord = MapCoord::new(2, 2);
        rs.enter_room(Room::at(coord, MapPointType::Unknown, RoomType::Event));
        assert_eq!(rs.current_room.and_then(|r| r.map_coord), Some(coord));
        let visited_before = rs.visited_map_coords.len();

        rs.enter_event_room(crate::events::EventId::LostWisp);

        assert_eq!(
            rs.current_room.and_then(|r| r.map_coord),
            Some(coord),
            "event-room entry must keep the coord of the node we arrived at",
        );
        assert_eq!(rs.current_room.map(|r| r.room_type), Some(RoomType::Event),);
        assert_eq!(
            rs.visited_map_coords.len(),
            visited_before,
            "re-entering the same coord must not double-count the visit",
        );
    }

    /// `enter_event_room` with no standing room (Neow at run start) still
    /// works and leaves a coord-less Event room — the original behaviour.
    #[test]
    fn enter_event_room_without_standing_room_is_coordless() {
        let mut rs = RunState::new();
        assert!(rs.current_room.is_none());
        rs.enter_event_room(crate::events::EventId::Neow);
        assert_eq!(rs.current_room.map(|r| r.room_type), Some(RoomType::Event));
        assert_eq!(rs.current_room.and_then(|r| r.map_coord), None);
    }

    /// `RunState::default()` seeds HP/gold to Python pin
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:38-40`
    /// Bead `sts2-rs-3knr4`: bead 7krx0 added `master_deck` and
    /// `rewards_rng_seed` to RunState. The Default impl initializes
    /// both (empty vec, seed=0). No test pinned these defaults — a
    /// regression that pre-populated master_deck or seeded the RNG
    /// non-zero would silently corrupt every test constructing
    /// RunState::new() without setting these fields.
    #[test]
    fn run_state_default_master_deck_and_rewards_rng_seed() {
        let rs = RunState::default();
        assert!(rs.master_deck.is_empty(), "master_deck starts empty");
        assert_eq!(rs.rewards_rng_seed, 0, "rewards_rng_seed starts at 0");
        // Bead `sts2-rs-w0zbf`: relic_counters BTreeMap defaults
        // empty — sibling fill that the original 3knr4 test missed.
        assert!(rs.relic_counters.is_empty(), "relic_counters starts empty");
    }

    /// (`max_hp = 80`, `current_hp = 80`, `gold = 99`). Sentinel for
    /// bead `sts2-rs-xqvn`.
    #[test]
    fn default_hp_and_gold_match_python_pin() {
        let rs = RunState::default();
        assert_eq!(rs.max_hp, 80);
        assert_eq!(rs.current_hp, 80);
        assert_eq!(rs.gold, 99);
    }

    /// `RunState::gain_max_hp(delta)` bumps both `max_hp` and
    /// `current_hp` by `delta` (matches Python
    /// `RunState.gain_max_hp` at `run_state.py:308`).
    #[test]
    fn gain_max_hp_bumps_both_max_and_current() {
        let mut rs = RunState::new();
        rs.gain_max_hp(6);
        assert_eq!(rs.max_hp, 86);
        assert_eq!(rs.current_hp, 86);

        // Negative delta moves BOTH fields by the full delta — this is
        // the raw Python-pin primitive. Production max-HP LOSS must go
        // through `lose_max_hp` instead (C#-faithful clamp); this leg only
        // pins `gain_max_hp`'s own documented arithmetic.
        rs.gain_max_hp(-10);
        assert_eq!(rs.max_hp, 76);
        assert_eq!(rs.current_hp, 76);
    }

    /// `RunState::lose_max_hp(amount)` mirrors C# `CreatureCmd.LoseMaxHp`:
    /// it reduces `current_hp` ONLY when the new max falls below it (a
    /// clamp-down), never by the full delta. This is the bug-fix surface
    /// for the ascension max-HP-loss parity divergence (LeafyPoultice /
    /// DistinguishedCape at A2+ left the player too low because the old
    /// `gain_max_hp(-amount)` over-subtracted current HP).
    #[test]
    fn lose_max_hp_clamps_current_only_below_new_max() {
        // current < max (the A2 WearyTraveler shape): losing max HP that
        // stays above current must NOT touch current.
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 64;
        rs.lose_max_hp(12); // new_max = 68 > current 64
        assert_eq!(rs.max_hp, 68);
        assert_eq!(
            rs.current_hp, 64,
            "current must be untouched when it fits under new max"
        );

        // current == max (the asc-0 shape): new_max < current, so current
        // clamps down to the new max (both engines always agreed here).
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 80;
        rs.lose_max_hp(12);
        assert_eq!(rs.max_hp, 68);
        assert_eq!(rs.current_hp, 68);

        // max floors at 1.
        let mut rs = RunState::new();
        rs.max_hp = 10;
        rs.current_hp = 10;
        rs.lose_max_hp(50);
        assert_eq!(rs.max_hp, 1);
        assert_eq!(rs.current_hp, 1);
    }

    /// `RunState::heal(amount)` clamps `current_hp` to `max_hp` and
    /// returns the actual delta applied (matches Python
    /// `RunState.heal` at `run_state.py:110-113`).
    /// Bead `sts2-rs-i562s`: heal(0) DOES fire after_max_hp_changed
    /// with current_hp_delta == 0 — the contract that lose_hp's
    /// doc-comment (line 695-698) explicitly contrasts against
    /// lose_hp(0)'s silent path. heal has no early-return on
    /// amount<=0 (runstate.rs:549). A regression that added one
    /// would silently break listeners depending on the zero-delta
    /// event (none today at pin 01c094d, but the contract is
    /// load-bearing for future Phase-7 listener arms).
    #[test]
    #[serial_test::serial(global_registry)]
    fn heal_zero_fires_dispatcher_with_zero_delta() {
        use crate::card_effects::EffectResult;
        use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
        use std::sync::Mutex;

        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        static LAST_CURRENT_DELTA: AtomicI32 = AtomicI32::new(i32::MIN);
        static GUARD: Mutex<()> = Mutex::new(());

        let _guard = GUARD.lock().unwrap();
        COUNTER.store(0, Ordering::SeqCst);
        LAST_CURRENT_DELTA.store(i32::MIN, Ordering::SeqCst);

        fn listener(
            rs: &mut RunState,
            ctx: &crate::run_effects::MaxHpChangeRunCtx,
        ) -> EffectResult {
            // Ownership gate — without this the listener fires for
            // every parallel test that calls heal/lose_hp/gain_max_hp,
            // racing this test's COUNTER assertion.
            if !rs.relics.contains(&RelicId::BigMushroom) {
                return EffectResult::NotImplemented;
            }
            COUNTER.fetch_add(1, Ordering::SeqCst);
            LAST_CURRENT_DELTA.store(ctx.current_hp_delta, Ordering::SeqCst);
            EffectResult::Resolved
        }
        crate::run_effects::register_after_max_hp_changed(
            RelicId::BigMushroom,
            listener as crate::run_effects::AfterMaxHpChangedRunFn,
        );

        let mut rs = RunState::new();
        rs.relics.push(RelicId::BigMushroom);
        // Start at full HP so heal(0) is meaningfully zero-delta.
        assert_eq!(rs.current_hp, rs.max_hp);

        let baseline = COUNTER.load(Ordering::SeqCst);
        let applied = rs.heal(0);
        assert_eq!(applied, 0, "no HP moved");
        assert_eq!(
            COUNTER.load(Ordering::SeqCst),
            baseline + 1,
            "heal(0) must fire after_max_hp_changed exactly once"
        );
        assert_eq!(
            LAST_CURRENT_DELTA.load(Ordering::SeqCst),
            0,
            "ctx.current_hp_delta carries the zero-delta signal"
        );
    }

    #[test]
    fn heal_clamps_to_max_hp_and_returns_applied_delta() {
        let mut rs = RunState::new();
        // Take a hit first so the heal has room to work.
        rs.current_hp = 50;

        let applied = rs.heal(20);
        assert_eq!(applied, 20);
        assert_eq!(rs.current_hp, 70);

        // Overheal clamps to max_hp; returned delta is the clamped amount.
        let applied = rs.heal(999);
        assert_eq!(applied, 10);
        assert_eq!(rs.current_hp, rs.max_hp);

        // Heal at full HP applies zero.
        let applied = rs.heal(5);
        assert_eq!(applied, 0);
        assert_eq!(rs.current_hp, rs.max_hp);
    }

    #[test]
    fn apply_ancient_event_heal_restores_to_full_at_base_ascension() {
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 30;
        rs.ascension_level = 0;
        let healed = rs.apply_ancient_event_heal();
        assert_eq!(healed, 50, "heals the full missing 50");
        assert_eq!(rs.current_hp, 80, "back to full between acts");
    }

    #[test]
    fn apply_ancient_event_heal_weary_traveler_recovers_80pct_missing() {
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 30;
        // A2 WearyTraveler: missing 50 → floor(50 * 0.8) = 40 → 70 HP.
        rs.ascension_level = WEARY_TRAVELER_ASCENSION_LEVEL;
        let healed = rs.apply_ancient_event_heal();
        assert_eq!(healed, 40);
        assert_eq!(rs.current_hp, 70);
    }

    #[test]
    fn apply_ancient_event_heal_weary_traveler_floors_odd_missing() {
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 23; // missing 57 → floor(57 * 0.8 = 45.6) = 45.
        rs.ascension_level = 2;
        let healed = rs.apply_ancient_event_heal();
        assert_eq!(healed, 45, "C# (int) truncates the decimal heal");
        assert_eq!(rs.current_hp, 68);
    }

    #[test]
    fn apply_ancient_event_heal_noop_when_already_full() {
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 80;
        rs.ascension_level = 2;
        assert_eq!(rs.apply_ancient_event_heal(), 0);
        assert_eq!(rs.current_hp, 80);
    }

    /// `RunState::gain_gold(delta)` adjusts the gold pile by `delta`.
    /// Negative `delta` is accepted as a convenience (Python's
    /// dedicated `lose_gold` surface isn't ported by this bead).
    #[test]
    fn gain_gold_adjusts_gold_pile() {
        let mut rs = RunState::new();
        assert_eq!(rs.gold, 99);

        rs.gain_gold(25);
        assert_eq!(rs.gold, 124);

        rs.gain_gold(-50);
        assert_eq!(rs.gold, 74);
    }

    /// Bead `sts2-rs-w9vpp`: `gain_gold` uses `saturating_add` so a
    /// degenerate stacked credit can't wrap `i32`.
    #[test]
    fn gain_gold_saturates_at_i32_max_on_overflow() {
        let mut rs = RunState::new();
        rs.gold = i32::MAX - 10;
        rs.gain_gold(1_000_000_000);
        assert_eq!(rs.gold, i32::MAX, "credit saturates at i32::MAX");
    }

    /// Bead `sts2-rs-w9vpp`: the negative-delta short-circuit also
    /// saturates (Python's path doesn't even run the `should_gain_gold`
    /// pipeline for non-positive deltas, so the only hardening surface
    /// is the raw `+=`).
    #[test]
    fn gain_gold_saturates_at_i32_min_on_underflow() {
        let mut rs = RunState::new();
        rs.gold = i32::MIN + 10;
        rs.gain_gold(-1_000_000_000);
        assert_eq!(rs.gold, i32::MIN, "debit saturates at i32::MIN");
    }

    // ── append_encounter_gold_drop — bead gccz5 (P9.2.q) ─────────

    fn last_gold_amount(rs: &RunState) -> Option<i32> {
        rs.pending_rewards.last().and_then(|r| match r {
            Reward::Gold { amount } => Some(*amount),
            _ => None,
        })
    }

    #[test]
    fn append_encounter_gold_drop_monster_rolls_in_range_10_to_20() {
        let mut rs = RunState {
            rng: crate::run_rng_set::RunRngSet::new(0x1234),
            ..RunState::default()
        };
        let amount = rs
            .append_encounter_gold_drop(RoomType::Monster, 1.0)
            .expect("Monster room appends");
        assert!(
            (ENCOUNTER_GOLD_LOW_MONSTER..=ENCOUNTER_GOLD_HIGH_MONSTER).contains(&amount),
            "rolled {amount} outside [10, 20]"
        );
        assert_eq!(last_gold_amount(&rs), Some(amount));
    }

    #[test]
    fn append_encounter_gold_drop_elite_rolls_in_range_35_to_45() {
        let mut rs = RunState {
            rng: crate::run_rng_set::RunRngSet::new(0xabcd),
            ..RunState::default()
        };
        let amount = rs
            .append_encounter_gold_drop(RoomType::Elite, 1.0)
            .expect("Elite room appends");
        assert!(
            (ENCOUNTER_GOLD_LOW_ELITE..=ENCOUNTER_GOLD_HIGH_ELITE).contains(&amount),
            "rolled {amount} outside [35, 45]"
        );
    }

    #[test]
    fn append_encounter_gold_drop_boss_rolls_exactly_100() {
        let mut rs = RunState {
            rng: crate::run_rng_set::RunRngSet::new(0xdead),
            ..RunState::default()
        };
        let amount = rs
            .append_encounter_gold_drop(RoomType::Boss, 1.0)
            .expect("Boss room appends");
        assert_eq!(amount, 100, "Boss range is the fixed point (100, 100)");
    }

    #[test]
    fn append_encounter_gold_drop_skips_non_combat_rooms() {
        let mut rs = RunState::new();
        for room_type in [
            RoomType::Treasure,
            RoomType::Shop,
            RoomType::RestSite,
            RoomType::Event,
            RoomType::Unknown,
        ] {
            assert!(
                rs.append_encounter_gold_drop(room_type, 1.0).is_none(),
                "non-combat room {room_type:?} must not append"
            );
        }
        assert!(rs.pending_rewards.is_empty());
    }

    #[test]
    fn append_encounter_gold_drop_zero_proportion_skips_append() {
        let mut rs = RunState::new();
        assert!(rs
            .append_encounter_gold_drop(RoomType::Monster, 0.0)
            .is_none());
        assert!(rs
            .append_encounter_gold_drop(RoomType::Monster, -0.5)
            .is_none());
        assert!(rs.pending_rewards.is_empty());
    }

    #[test]
    fn append_encounter_gold_drop_applies_poverty_multiplier_at_asc_3() {
        // C# `EncounterModel.MinGoldReward` truncates: Monster
        // (10, 20) × 0.75 = (7.5, 15.0) → `(int)` = (7, 15).
        let mut rs = RunState {
            rng: crate::run_rng_set::RunRngSet::new(0xc0ffee),
            ascension_level: 3,
            ..RunState::default()
        };
        let amount = rs
            .append_encounter_gold_drop(RoomType::Monster, 1.0)
            .expect("Monster room appends");
        assert!(
            (7..=15).contains(&amount),
            "Poverty-shrunk Monster range [7, 15] excludes {amount}"
        );
    }

    #[test]
    fn roll_encounter_gold_poverty_truncates_elite_bounds() {
        // C# Elite (35, 45) × 0.75 = (26.25, 33.75) → `(int)` = (26, 33).
        // Banker's-round would give 34 on the high bound — the audit
        // divergence this test pins (bead sts2-rs-40ikw.2). Sample many
        // rolls: every value must stay within [26, 33] and both bounds
        // must be reachable.
        let mut rs = RunState {
            rng: crate::run_rng_set::RunRngSet::new(0xdead),
            ascension_level: 3,
            ..RunState::default()
        };
        let mut seen_min = i32::MAX;
        let mut seen_max = i32::MIN;
        for _ in 0..400 {
            let amount = rs
                .roll_encounter_gold(RoomType::Elite, 1.0)
                .expect("Elite room rolls");
            assert!(
                (26..=33).contains(&amount),
                "Poverty Elite roll {amount} outside C# range [26, 33]"
            );
            seen_min = seen_min.min(amount);
            seen_max = seen_max.max(amount);
        }
        assert_eq!(seen_min, 26, "low bound reachable");
        assert_eq!(seen_max, 33, "high bound reachable (33, not banker's 34)");
    }

    #[test]
    fn gold_proportion_only_shrinks_monster_rooms() {
        // C# `RewardsSet.GenerateRewardsFor` (RewardsSet.cs:167-182): the
        // flee/escape `GoldProportion` is applied ONLY to the Monster arm; the
        // Elite/Boss arms always award the raw encounter bounds and never
        // consult it. A half-escaped combat must therefore halve Monster gold
        // but leave Elite/Boss gold at full value (and Elite/Boss must still
        // roll even when the proportion is 0 — they don't short-circuit).
        let new_rs = || RunState {
            rng: crate::run_rng_set::RunRngSet::new(0x901d),
            ..RunState::default()
        };

        // Monster (10..=20) × 0.5 → (5..=10).
        let mut rs = new_rs();
        for _ in 0..200 {
            let g = rs
                .roll_encounter_gold(RoomType::Monster, 0.5)
                .expect("monster rolls when proportion > 0");
            assert!(
                (5..=10).contains(&g),
                "monster gold {g} not shrunk to [5,10]"
            );
        }

        // Elite (35..=45) ignores the proportion entirely — full bounds.
        let mut rs = new_rs();
        for _ in 0..200 {
            let g = rs
                .roll_encounter_gold(RoomType::Elite, 0.5)
                .expect("elite always rolls");
            assert!(
                (35..=45).contains(&g),
                "elite gold {g} must ignore gold_proportion (raw [35,45])"
            );
        }

        // Boss ignores the proportion AND still rolls at proportion 0 (no
        // Monster-style short-circuit).
        let mut rs = new_rs();
        let g = rs
            .roll_encounter_gold(RoomType::Boss, 0.0)
            .expect("boss rolls even at gold_proportion 0");
        assert_eq!(g, 100, "boss raw bounds (100,100), proportion ignored");

        // Monster at proportion 0 short-circuits (no RNG consumed).
        let mut rs = new_rs();
        assert_eq!(rs.roll_encounter_gold(RoomType::Monster, 0.0), None);
    }

    #[test]
    fn append_encounter_gold_drop_poverty_inactive_below_asc_3() {
        let mut rs = RunState {
            rng: crate::run_rng_set::RunRngSet::new(0xc0ffee),
            ascension_level: 2,
            ..RunState::default()
        };
        let amount = rs
            .append_encounter_gold_drop(RoomType::Monster, 1.0)
            .expect("Monster room appends");
        // Asc 2 keeps the canonical (10, 20) range.
        assert!((10..=20).contains(&amount), "{amount} outside [10, 20]");
    }

    #[test]
    fn append_encounter_gold_drop_monster_proportion_scales_range() {
        // Monster (10, 20) × 0.5 = (5, 10).
        let mut rs = RunState {
            rng: crate::run_rng_set::RunRngSet::new(0xfeed),
            ..RunState::default()
        };
        let amount = rs
            .append_encounter_gold_drop(RoomType::Monster, 0.5)
            .expect("Monster room appends");
        assert!((5..=10).contains(&amount), "{amount} outside [5, 10]");
    }

    #[test]
    fn append_encounter_gold_drop_is_deterministic_under_same_seed() {
        let build = |seed: i32| {
            let mut rs = RunState {
                rng: crate::run_rng_set::RunRngSet::new(seed),
                ..RunState::default()
            };
            for _ in 0..5 {
                rs.append_encounter_gold_drop(RoomType::Monster, 1.0);
            }
            rs.pending_rewards
                .iter()
                .filter_map(|r| match r {
                    Reward::Gold { amount } => Some(*amount),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            build(0x77),
            build(0x77),
            "same seed → identical drop sequence"
        );
    }

    #[test]
    fn append_encounter_gold_drop_grows_pending_rewards_by_one() {
        let mut rs = RunState {
            rng: crate::run_rng_set::RunRngSet::new(0x55),
            ..RunState::default()
        };
        let before = rs.pending_rewards.len();
        let _ = rs.append_encounter_gold_drop(RoomType::Monster, 1.0);
        assert_eq!(rs.pending_rewards.len(), before + 1);
    }

    /// C# TungstenRod reduces every owner HP loss by 1 — including
    /// out-of-combat event damage, which C# routes through the full
    /// `CreatureCmd.Damage` pipeline (census v9 seed 1046: flower dig
    /// cs=4 rs=5).
    #[test]
    fn lose_hp_applies_tungsten_rod_reduction_out_of_combat() {
        let mut rs = RunState::new();
        rs.current_hp = 50;
        rs.max_hp = 80;
        rs.relics.push(crate::RelicId::TungstenRod);
        assert_eq!(rs.lose_hp(5), 4, "rod shaves 1 off the event damage");
        assert_eq!(rs.current_hp, 46);
        assert_eq!(rs.lose_hp(1), 0, "a 1-damage tick is fully absorbed");
        assert_eq!(rs.current_hp, 46);
    }

    /// C# `Player.AddPotionInternal` fills the FIRST empty physical slot
    /// (`_potionSlots.IndexOf(null)`) and `RemovePotionInternal` nulls the
    /// slot without compacting, so the visible belt is non-empty slots in
    /// SLOT order: hold [A@0, B@1], drink A, add C → [C, B], not [B, C]
    /// (shadow census v8 seeds 1012/1013/1028/1035/1044/1051).
    #[test]
    fn potion_belt_add_fills_first_empty_slot_in_slot_order() {
        use crate::potions::PotionKind;
        let mut rs = RunState::new();
        assert!(rs.add_potion_to_belt(PotionKind::FirePotion));
        assert!(rs.add_potion_to_belt(PotionKind::EnergyPotion));
        assert_eq!(
            rs.remove_potion_from_belt(0),
            PotionKind::FirePotion,
            "drink the slot-0 potion, leaving a hole"
        );
        assert!(rs.add_potion_to_belt(PotionKind::BlockPotion));
        assert_eq!(
            rs.potions,
            vec![PotionKind::BlockPotion, PotionKind::EnergyPotion],
            "new potion fills the slot-0 hole and lists FIRST"
        );
        // A third add fills slot 2 — belt full at the default 3-slot cap.
        assert!(rs.add_potion_to_belt(PotionKind::WeakPotion));
        assert!(!rs.add_potion_to_belt(PotionKind::FirePotion), "belt full");
        assert_eq!(
            rs.potions,
            vec![
                PotionKind::BlockPotion,
                PotionKind::EnergyPotion,
                PotionKind::WeakPotion,
            ]
        );
    }

    /// Bead `sts2-rs-dsgf9`: `RunState::gain_potion_slots(amount)`
    /// shifts `max_potion_slots` by `amount`, clamped at 0. Mirrors
    /// Python `run_state.py:316-317`. Default is 3 (Python pin
    /// `run_state.py:45`).
    #[test]
    fn gain_potion_slots_grows_and_clamps_at_zero() {
        let mut rs = RunState::new();
        assert_eq!(rs.max_potion_slots, 3, "default mirrors Python pin");

        // PotionBelt-style +2.
        rs.gain_potion_slots(2);
        assert_eq!(rs.max_potion_slots, 5);

        // AlchemicalCoffer-style +1 — no MAX_POTIONS cap here; the
        // combat-side hard cap lives in `PlayerCombatState.potions`.
        rs.gain_potion_slots(1);
        assert_eq!(rs.max_potion_slots, 6);

        // Sozu-style negative delta drops to zero, not negative.
        rs.gain_potion_slots(-99);
        assert_eq!(rs.max_potion_slots, 0, "clamp at zero, no underflow");

        // No-op deltas are safe.
        rs.gain_potion_slots(0);
        assert_eq!(rs.max_potion_slots, 0);
    }

    /// Bead `sts2-rs-flfyp`: apply_pending_combat_gold_loss with a
    /// negative intent. Per runstate.rs:677-686, the queue is zeroed
    /// BEFORE the `intent <= 0` early-return check (lines 679 + 680),
    /// so a negative intent silently clears the queue and returns 0
    /// without touching gold. A future drive-by reordering the
    /// zero-clear to AFTER the check would leak the negative value
    /// into the next drain attempt.
    #[test]
    fn apply_pending_combat_gold_loss_negative_intent_clears_queue_returns_zero() {
        let mut rs = RunState::new();
        let starting_gold = rs.gold;
        let mut combat = crate::CombatState {
            pending_run_gold_loss: -42,
            ..crate::CombatState::default()
        };
        let applied = rs.apply_pending_combat_gold_loss(&mut combat);
        assert_eq!(applied, 0, "negative intent returns 0");
        assert_eq!(rs.gold, starting_gold, "gold untouched");
        assert_eq!(
            combat.pending_run_gold_loss, 0,
            "queue drained even on negative intent (idempotency invariant)"
        );
    }

    /// Bead `sts2-rs-3bie`: DEBT-discarded gold-loss intent draining.
    /// Mirrors Python `combat.py:2403-2405`:
    /// `gold = max(0, gold - min(intent, gold))`.
    #[test]
    fn apply_pending_combat_gold_loss_clamps_at_current_gold() {
        let mut rs = RunState::new();
        rs.gold = 15;

        let mut combat = crate::CombatState {
            pending_run_gold_loss: 10,
            ..crate::CombatState::default()
        };

        let applied = rs.apply_pending_combat_gold_loss(&mut combat);

        assert_eq!(applied, 10, "loses exactly 10 when gold >= intent");
        assert_eq!(rs.gold, 5);
        assert_eq!(combat.pending_run_gold_loss, 0, "queue drained");

        // Second DEBT discard accumulates 10 more, then drain caps at
        // available 5 (Python's max(0, gold - min(intent, gold)) clamp).
        combat.pending_run_gold_loss = 10;
        let applied = rs.apply_pending_combat_gold_loss(&mut combat);
        assert_eq!(applied, 5, "clamps to remaining gold");
        assert_eq!(rs.gold, 0);
        assert_eq!(combat.pending_run_gold_loss, 0);

        // Third drain on zero intent is a no-op.
        let applied = rs.apply_pending_combat_gold_loss(&mut combat);
        assert_eq!(applied, 0);
        assert_eq!(rs.gold, 0);
    }

    /// Bead `sts2-rs-sj8ph` (1/2): `gain_potion_slots` is a pure
    /// mutator — it does NOT fire any run-side hook channel. The
    /// most-adjacent dispatcher (`after_max_hp_changed`) must stay
    /// untouched across the call. Pins the absence of a future
    /// drive-by `after_potion_slots_changed` channel that would
    /// diverge from Python pin `01c094d` (no such hook there).
    #[test]
    #[serial_test::serial(global_registry)]
    fn gain_potion_slots_fires_no_hooks() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{
            register_after_max_hp_changed, AfterMaxHpChangedRunFn, MaxHpChangeRunCtx,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn listener(rs: &mut RunState, _ctx: &MaxHpChangeRunCtx) -> EffectResult {
            if !rs.relics.contains(&RelicId::FakeBloodVial) {
                return EffectResult::NotImplemented;
            }
            CALLS.fetch_add(1, Ordering::SeqCst);
            EffectResult::Resolved
        }
        register_after_max_hp_changed(RelicId::FakeBloodVial, listener as AfterMaxHpChangedRunFn);

        let mut rs = RunState::new();
        rs.relics.push(RelicId::FakeBloodVial);
        let baseline = CALLS.load(Ordering::SeqCst);

        rs.gain_potion_slots(2);
        rs.gain_potion_slots(-1);
        rs.gain_potion_slots(0);

        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            baseline,
            "gain_potion_slots must not fire after_max_hp_changed"
        );
    }

    /// Bead `sts2-rs-sj8ph` (2/2): `saturating_add` semantics — a
    /// stacked positive delta near `i32::MAX` saturates rather than
    /// wrapping. The negative-clamp leg is already covered by
    /// `gain_potion_slots_grows_and_clamps_at_zero`.
    #[test]
    fn gain_potion_slots_saturates_at_i32_max() {
        let mut rs = RunState::new();
        rs.max_potion_slots = i32::MAX - 5;
        rs.gain_potion_slots(1_000_000);
        assert_eq!(
            rs.max_potion_slots,
            i32::MAX,
            "near-i32::MAX delta must saturate, not wrap"
        );
    }

    /// `RunState::gain_max_hp` and `RunState::heal` both fire
    /// `run_effects::dispatch_after_max_hp_changed`. The listener
    /// receives a `MaxHpChangeRunCtx` with `delta` reflecting `max_hp`
    /// movement and `current_hp_delta` reflecting `current_hp`
    /// movement. Sentinel for bead `sts2-rs-xqvn`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn after_max_hp_changed_channel_fires_on_gain_and_heal() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{
            register_after_max_hp_changed, AfterMaxHpChangedRunFn, MaxHpChangeRunCtx,
        };
        use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        static LAST_DELTA: AtomicI32 = AtomicI32::new(0);
        static LAST_CURRENT_DELTA: AtomicI32 = AtomicI32::new(0);

        fn listener(rs: &mut RunState, ctx: &MaxHpChangeRunCtx) -> EffectResult {
            if !rs.relics.contains(&RelicId::FakeMango) {
                return EffectResult::NotImplemented;
            }
            COUNTER.fetch_add(1, Ordering::SeqCst);
            LAST_DELTA.store(ctx.delta, Ordering::SeqCst);
            LAST_CURRENT_DELTA.store(ctx.current_hp_delta, Ordering::SeqCst);
            EffectResult::Resolved
        }

        register_after_max_hp_changed(RelicId::FakeMango, listener as AfterMaxHpChangedRunFn);

        let mut rs = RunState::new();
        rs.relics.push(RelicId::FakeMango);
        rs.current_hp = 50;

        let baseline = COUNTER.load(Ordering::SeqCst);

        // gain_max_hp(+7): both deltas == +7.
        rs.gain_max_hp(7);
        assert!(COUNTER.load(Ordering::SeqCst) > baseline);
        assert_eq!(LAST_DELTA.load(Ordering::SeqCst), 7);
        assert_eq!(LAST_CURRENT_DELTA.load(Ordering::SeqCst), 7);

        // heal: `delta == 0`, `current_hp_delta == applied`.
        let baseline = COUNTER.load(Ordering::SeqCst);
        let applied = rs.heal(10);
        assert!(COUNTER.load(Ordering::SeqCst) > baseline);
        assert_eq!(LAST_DELTA.load(Ordering::SeqCst), 0);
        assert_eq!(LAST_CURRENT_DELTA.load(Ordering::SeqCst), applied);
    }

    /// Bead `sts2-rs-g5ps4`: pin two extra `gain_max_hp` dispatcher
    /// contracts that the sibling channel test doesn't:
    ///
    /// 1. **Zero-delta still fires** — per the docstring at
    ///    runstate.rs:517-520, `gain_max_hp(0)` must still call
    ///    `dispatch_after_max_hp_changed` with both delta fields = 0.
    /// 2. **`saturating_add` semantics** — a delta near `i32::MAX`
    ///    must NOT panic; the ctx reports the *requested* delta,
    ///    while `max_hp` and `current_hp` saturate at `i32::MAX`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn gain_max_hp_zero_delta_fires_and_saturating_add_does_not_panic() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{
            register_after_max_hp_changed, AfterMaxHpChangedRunFn, MaxHpChangeRunCtx,
        };
        use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

        // Distinct statics from the sibling test so parallel runs don't
        // poison each other's counters.
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        static LAST_DELTA: AtomicI32 = AtomicI32::new(i32::MIN);
        static LAST_CURRENT_DELTA: AtomicI32 = AtomicI32::new(i32::MIN);

        fn listener(rs: &mut RunState, ctx: &MaxHpChangeRunCtx) -> EffectResult {
            // Ownership gate — see sibling `heal_zero_fires_…` test
            // for the cross-test race rationale.
            if !rs.relics.contains(&RelicId::FakeAnchor) {
                return EffectResult::NotImplemented;
            }
            COUNTER.fetch_add(1, Ordering::SeqCst);
            LAST_DELTA.store(ctx.delta, Ordering::SeqCst);
            LAST_CURRENT_DELTA.store(ctx.current_hp_delta, Ordering::SeqCst);
            EffectResult::Resolved
        }
        register_after_max_hp_changed(RelicId::FakeAnchor, listener as AfterMaxHpChangedRunFn);

        let mut rs = RunState::new();
        rs.relics.push(RelicId::FakeAnchor);
        rs.max_hp = 80;
        rs.current_hp = 50;

        // (1) zero-delta still fires.
        let baseline = COUNTER.load(Ordering::SeqCst);
        rs.gain_max_hp(0);
        assert!(
            COUNTER.load(Ordering::SeqCst) > baseline,
            "zero-delta gain_max_hp must still fire the dispatcher"
        );
        assert_eq!(LAST_DELTA.load(Ordering::SeqCst), 0);
        assert_eq!(LAST_CURRENT_DELTA.load(Ordering::SeqCst), 0);
        assert_eq!(rs.max_hp, 80, "no max_hp movement");
        assert_eq!(rs.current_hp, 50, "no current_hp movement");

        // (2) saturating_add — large positive delta saturates instead
        // of panicking. The ctx reports the requested delta verbatim.
        rs.max_hp = i32::MAX - 5;
        rs.current_hp = i32::MAX - 5;
        rs.gain_max_hp(1_000_000);
        assert_eq!(rs.max_hp, i32::MAX);
        assert_eq!(rs.current_hp, i32::MAX);
        assert_eq!(LAST_DELTA.load(Ordering::SeqCst), 1_000_000);
        assert_eq!(LAST_CURRENT_DELTA.load(Ordering::SeqCst), 1_000_000);
    }

    /// `RunState::lose_hp` decrements `current_hp` clamped to 0 and
    /// returns the actual delta lost. Bead `sts2-rs-jvk3`.
    #[test]
    fn lose_hp_decrements_and_clamps_to_zero() {
        let mut rs = RunState::new();
        rs.current_hp = 50;
        rs.max_hp = 80;

        // Plain decrement.
        let lost = rs.lose_hp(15);
        assert_eq!(lost, 15);
        assert_eq!(rs.current_hp, 35);

        // Clamp at zero on overrun.
        let lost = rs.lose_hp(100);
        assert_eq!(lost, 35);
        assert_eq!(rs.current_hp, 0);

        // Zero / negative is a no-op.
        let lost = rs.lose_hp(0);
        assert_eq!(lost, 0);
        let lost = rs.lose_hp(-5);
        assert_eq!(lost, 0);
        assert_eq!(rs.current_hp, 0);
    }

    /// `RunState::take_damage` is a thin alias for `lose_hp`
    /// (Python: `take_damage = lose_hp`). Bead `sts2-rs-jvk3`.
    #[test]
    fn take_damage_aliases_lose_hp() {
        let mut rs = RunState::new();
        rs.current_hp = 30;
        rs.max_hp = 80;
        let lost = rs.take_damage(15);
        assert_eq!(lost, 15);
        assert_eq!(rs.current_hp, 15);
    }

    /// `RunState::lose_hp` fires `after_max_hp_changed` with
    /// `delta == 0` and `current_hp_delta == -applied` (mirroring the
    /// sign convention used by `heal`). Bead `sts2-rs-jvk3`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn lose_hp_fires_after_max_hp_changed_with_negative_delta() {
        use crate::card_effects::EffectResult;
        use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
        use std::sync::Mutex;

        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        static LAST_DELTA: AtomicI32 = AtomicI32::new(i32::MIN);
        static LAST_CURRENT_DELTA: AtomicI32 = AtomicI32::new(i32::MIN);
        static GUARD: Mutex<()> = Mutex::new(());

        let _guard = GUARD.lock().unwrap();
        COUNTER.store(0, Ordering::SeqCst);
        LAST_DELTA.store(i32::MIN, Ordering::SeqCst);
        LAST_CURRENT_DELTA.store(i32::MIN, Ordering::SeqCst);

        fn listener(
            rs: &mut RunState,
            ctx: &crate::run_effects::MaxHpChangeRunCtx,
        ) -> EffectResult {
            if !rs.relics.contains(&RelicId::FragrantMushroom) {
                return EffectResult::NotImplemented;
            }
            COUNTER.fetch_add(1, Ordering::SeqCst);
            LAST_DELTA.store(ctx.delta, Ordering::SeqCst);
            LAST_CURRENT_DELTA.store(ctx.current_hp_delta, Ordering::SeqCst);
            EffectResult::Resolved
        }
        crate::run_effects::register_after_max_hp_changed(
            RelicId::FragrantMushroom,
            listener as crate::run_effects::AfterMaxHpChangedRunFn,
        );

        let mut rs = RunState::new();
        rs.relics.push(RelicId::FragrantMushroom);
        rs.current_hp = 50;
        rs.max_hp = 80;
        let lost = rs.lose_hp(15);
        assert_eq!(lost, 15);
        assert!(COUNTER.load(Ordering::SeqCst) > 0);
        assert_eq!(LAST_DELTA.load(Ordering::SeqCst), 0);
        assert_eq!(LAST_CURRENT_DELTA.load(Ordering::SeqCst), -15);

        // No-op (amount <= 0) does NOT fire the dispatcher.
        let baseline = COUNTER.load(Ordering::SeqCst);
        let lost = rs.lose_hp(0);
        assert_eq!(lost, 0);
        assert_eq!(COUNTER.load(Ordering::SeqCst), baseline);

        // Bead `sts2-rs-e55kd`: lose_hp(<0) also takes the early-return
        // branch (amount <= 0) and must NOT fire the dispatcher. The
        // existing baseline above asserts lose_hp(0) silence; the
        // negative path is the sibling miss.
        let lost = rs.lose_hp(-42);
        assert_eq!(lost, 0, "negative amount returns 0");
        assert_eq!(
            COUNTER.load(Ordering::SeqCst),
            baseline,
            "negative amount must not fire after_max_hp_changed"
        );
    }

    /// `RunState::transform_relic` replaces the first matching id and
    /// returns `true`; absent-id is a no-op returning `false`. Bead
    /// `sts2-rs-jvk3`.
    #[test]
    fn transform_relic_replaces_first_match() {
        let mut rs = RunState::new();
        rs.relics.push(RelicId::SwordOfStone);
        rs.relics.push(RelicId::BigMushroom);
        rs.relics.push(RelicId::SwordOfStone); // duplicate — should NOT be touched

        let ok = rs.transform_relic(RelicId::SwordOfStone, RelicId::SwordOfJade);
        assert!(ok);
        assert_eq!(
            rs.relics,
            vec![
                RelicId::SwordOfJade,
                RelicId::BigMushroom,
                RelicId::SwordOfStone,
            ]
        );

        // Absent id leaves the vec untouched.
        let ok = rs.transform_relic(RelicId::FakeAnchor, RelicId::FakeMango);
        assert!(!ok);
        assert_eq!(
            rs.relics,
            vec![
                RelicId::SwordOfJade,
                RelicId::BigMushroom,
                RelicId::SwordOfStone,
            ]
        );
    }

    /// `RunState::upgrade_random_cards` on an empty deck stays a
    /// no-op — same conservative-stub behavior as pre-`7krx0`, but
    /// driven by the deck being empty rather than the helper itself
    /// being a hardcoded `0`.
    #[test]
    fn upgrade_random_cards_empty_deck_returns_zero() {
        let mut rs = RunState::new();
        assert_eq!(rs.upgrade_random_cards(0), 0);
        assert_eq!(rs.upgrade_random_cards(3), 0);
        assert_eq!(rs.upgrade_random_cards(99), 0);
    }

    /// Bead `sts2-rs-5r3f7`: `enter_event_room(id)` sets
    /// `current_event = Some(id)` and advances room state to
    /// `RoomType::Event`. Exercises the small (3-line) entry path
    /// that drives the P7 events framework's per-step bookkeeping.
    #[test]
    fn enter_event_room_sets_current_event_and_room_type() {
        let mut rs = RunState::new();
        assert!(rs.current_event.is_none(), "fresh RunState has no event");
        rs.enter_event_room(crate::events::EventId::LostWisp);
        assert_eq!(rs.current_event, Some(crate::events::EventId::LostWisp));
        assert_eq!(rs.current_room.map(|r| r.room_type), Some(RoomType::Event));
    }

    /// Bead `sts2-rs-5r3f7`: `exit_event_room()` returns the previously-
    /// active event id and clears `current_event`. A second call
    /// returns `None` (idempotent clear). Calling exit without first
    /// entering returns `None`.
    #[test]
    fn exit_event_room_clears_and_returns_previous_id() {
        let mut rs = RunState::new();
        assert_eq!(rs.exit_event_room(), None, "no-op when no event active");

        rs.enter_event_room(crate::events::EventId::LostWisp);
        assert_eq!(rs.exit_event_room(), Some(crate::events::EventId::LostWisp));
        assert!(rs.current_event.is_none());

        assert_eq!(rs.exit_event_room(), None, "second exit is idempotent");

        rs.enter_event_room(crate::events::EventId::TrashHeap);
        assert_eq!(
            rs.exit_event_room(),
            Some(crate::events::EventId::TrashHeap)
        );
    }

    /// Helper: build a Card POD with the given `card_id` and upgrade
    /// state, leaving every other field at the canonical default.
    fn mk_deck_card(card_id: crate::cards::CardId, upgraded: bool) -> Card {
        let mut c = Card::pod_defaults();
        c.card_id = card_id;
        c.upgraded = upgraded;
        c
    }

    /// `upgrade_random_cards` with a 5-card master deck of
    /// non-upgraded Strikes upgrades exactly `count` cards.
    /// Bead `sts2-rs-7krx0`.
    #[test]
    fn upgrade_random_cards_upgrades_exactly_n_when_enough_unupgraded() {
        let mut rs = RunState::new().with_rewards_seed(0xdead_beef);
        for _ in 0..5 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        }
        let upgraded = rs.upgrade_random_cards(3);
        assert_eq!(upgraded, 3, "exactly 3 upgrades reported");
        let n_upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(n_upgraded, 3, "exactly 3 cards now have upgraded=true");
    }

    /// `upgrade_random_cards` saturates at the candidate set when
    /// the deck has fewer eligible cards than `count`.
    #[test]
    fn upgrade_random_cards_saturates_at_unupgraded_count() {
        let mut rs = RunState::new();
        // 2 non-upgraded + 3 already-upgraded → only 2 are eligible.
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::DefendIronclad, false));
        for _ in 0..3 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, true));
        }
        let upgraded = rs.upgrade_random_cards(5);
        assert_eq!(upgraded, 2, "saturates at the 2 eligible cards");
        let n_upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(n_upgraded, 5, "all 5 cards now upgraded (2 fresh + 3 pre)");
    }

    /// Two RunStates with the same starting seed produce the same
    /// sequence of upgrades on identical decks. Locks the
    /// deterministic-rewards-RNG contract from bead `sts2-rs-7krx0`.
    #[test]
    fn upgrade_random_cards_is_deterministic_under_same_seed() {
        let build = || {
            let mut rs = RunState::new().with_rewards_seed(0x42);
            for i in 0..10 {
                let id = if i % 2 == 0 {
                    crate::cards::CardId::StrikeIronclad
                } else {
                    crate::cards::CardId::DefendIronclad
                };
                rs.master_deck.push(mk_deck_card(id, false));
            }
            rs
        };
        let mut a = build();
        let mut b = build();
        assert_eq!(a.upgrade_random_cards(4), 4);
        assert_eq!(b.upgrade_random_cards(4), 4);
        let a_set: Vec<bool> = a.master_deck.iter().map(|c| c.upgraded).collect();
        let b_set: Vec<bool> = b.master_deck.iter().map(|c| c.upgraded).collect();
        assert_eq!(a_set, b_set, "same starting seed → same chosen indices");
    }

    /// Bead `sts2-rs-tb36q`: typed variant — `upgrade_random_cards_of_type`
    /// only touches cards whose `card_type` matches the filter and
    /// uses the `niche` stream (not the legacy `rewards` stream).
    fn mk_deck_card_of_type(
        card_id: crate::cards::CardId,
        card_type: crate::cards::CardType,
        upgraded: bool,
    ) -> Card {
        let mut c = Card::pod_defaults();
        c.card_id = card_id;
        c.card_type = card_type;
        c.upgraded = upgraded;
        c
    }

    #[test]
    fn upgrade_random_cards_of_type_zero_count_is_noop() {
        let mut rs = RunState::new();
        rs.master_deck.push(mk_deck_card_of_type(
            crate::cards::CardId::StrikeIronclad,
            crate::cards::CardType::Attack,
            false,
        ));
        assert_eq!(
            rs.upgrade_random_cards_of_type(crate::cards::CardType::Attack, 0),
            0
        );
        assert!(!rs.master_deck[0].upgraded);
    }

    #[test]
    fn upgrade_random_cards_of_type_filters_by_card_type() {
        // 3 Skills + 3 Attacks, all non-upgraded. Asking for 2 Skills
        // must upgrade 2 Skills and leave every Attack untouched.
        let mut rs = RunState::new();
        for _ in 0..3 {
            rs.master_deck.push(mk_deck_card_of_type(
                crate::cards::CardId::DefendIronclad,
                crate::cards::CardType::Skill,
                false,
            ));
        }
        for _ in 0..3 {
            rs.master_deck.push(mk_deck_card_of_type(
                crate::cards::CardId::StrikeIronclad,
                crate::cards::CardType::Attack,
                false,
            ));
        }
        let upgraded = rs.upgrade_random_cards_of_type(crate::cards::CardType::Skill, 2);
        assert_eq!(upgraded, 2);
        let skills_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == crate::cards::CardType::Skill && c.upgraded)
            .count();
        assert_eq!(skills_upgraded, 2, "exactly 2 Skills upgraded");
        let attacks_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == crate::cards::CardType::Attack && c.upgraded)
            .count();
        assert_eq!(attacks_upgraded, 0, "no Attack was touched");
    }

    #[test]
    fn upgrade_random_cards_of_type_saturates_at_candidate_count() {
        // 2 non-upgraded Skills + 3 already-upgraded Skills + 4 Attacks.
        // Asking for 5 Skills upgrades only the 2 eligible Skills.
        let mut rs = RunState::new();
        for _ in 0..2 {
            rs.master_deck.push(mk_deck_card_of_type(
                crate::cards::CardId::DefendIronclad,
                crate::cards::CardType::Skill,
                false,
            ));
        }
        for _ in 0..3 {
            rs.master_deck.push(mk_deck_card_of_type(
                crate::cards::CardId::DefendIronclad,
                crate::cards::CardType::Skill,
                true,
            ));
        }
        for _ in 0..4 {
            rs.master_deck.push(mk_deck_card_of_type(
                crate::cards::CardId::StrikeIronclad,
                crate::cards::CardType::Attack,
                false,
            ));
        }
        let upgraded = rs.upgrade_random_cards_of_type(crate::cards::CardType::Skill, 5);
        assert_eq!(upgraded, 2);
        let total_skill_upgraded = rs
            .master_deck
            .iter()
            .filter(|c| c.card_type == crate::cards::CardType::Skill && c.upgraded)
            .count();
        assert_eq!(total_skill_upgraded, 5, "2 fresh + 3 pre-existing upgrades");
    }

    #[test]
    fn upgrade_random_cards_of_type_no_matching_card_type_returns_zero() {
        let mut rs = RunState::new();
        for _ in 0..3 {
            rs.master_deck.push(mk_deck_card_of_type(
                crate::cards::CardId::StrikeIronclad,
                crate::cards::CardType::Attack,
                false,
            ));
        }
        assert_eq!(
            rs.upgrade_random_cards_of_type(crate::cards::CardType::Skill, 2),
            0,
            "no Skill in deck → zero upgrades"
        );
        let any_upgraded = rs.master_deck.iter().any(|c| c.upgraded);
        assert!(!any_upgraded, "no Attack was touched either");
    }

    #[test]
    fn upgrade_random_cards_of_type_uses_niche_stream() {
        // Two runs with identical seed → identical chosen Skill subset.
        // Three runs with same seed should also leave `rewards`
        // untouched (the typed variant uses `niche`, not `rewards`).
        let build = || {
            let mut rs = RunState {
                rng: crate::run_rng_set::RunRngSet::new(0x42),
                ..RunState::default()
            };
            for i in 0..6 {
                let card_type = if i % 2 == 0 {
                    crate::cards::CardType::Skill
                } else {
                    crate::cards::CardType::Attack
                };
                rs.master_deck.push(mk_deck_card_of_type(
                    crate::cards::CardId::DefendIronclad,
                    card_type,
                    false,
                ));
            }
            rs
        };
        let mut a = build();
        let mut b = build();
        assert_eq!(
            a.upgrade_random_cards_of_type(crate::cards::CardType::Skill, 2),
            2
        );
        assert_eq!(
            b.upgrade_random_cards_of_type(crate::cards::CardType::Skill, 2),
            2
        );
        let a_set: Vec<bool> = a.master_deck.iter().map(|c| c.upgraded).collect();
        let b_set: Vec<bool> = b.master_deck.iter().map(|c| c.upgraded).collect();
        assert_eq!(
            a_set, b_set,
            "same seed → same chosen Skills via niche stream"
        );
    }

    /// C# `WarPaint`/`StableShuffle` invariant: the typed upgrade selection
    /// pre-sorts candidates by `CardModel.CompareTo` (Entry ordinal, then
    /// upgrade level) BEFORE the Fisher-Yates draw, so the chosen cards
    /// depend only on the *sorted* candidate multiset + the RNG, NOT on the
    /// raw master-deck ordering. Two decks with identical Skill multisets but
    /// different deck orderings must therefore upgrade the SAME card
    /// identities off the same niche seed. The pre-`stable_shuffle_by` port
    /// (plain `rng.niche.shuffle` on deck-order candidates) FAILED this — the
    /// Necrobinder seed=1011 WarPaint divergence (DEFEND vs AFTERLIFE).
    #[test]
    fn upgrade_random_cards_of_type_is_invariant_to_deck_order() {
        use crate::cards::CardId;
        // Three distinct-named Skills (ARMAMENTS < BATTLE_TRANCE <
        // BLOODLETTING ordinally) laid out in two different deck orders.
        let skills = [
            CardId::Armaments,
            CardId::BattleTrance,
            CardId::Bloodletting,
        ];
        let build = |order: &[CardId]| {
            let mut rs = RunState {
                rng: crate::run_rng_set::RunRngSet::new(0x1011),
                ..RunState::default()
            };
            for &id in order {
                rs.master_deck.push(mk_deck_card_of_type(
                    id,
                    crate::cards::CardType::Skill,
                    false,
                ));
            }
            rs
        };
        // Order A: sorted; Order B: reversed (worst case for "raw deck order").
        let mut a = build(&[skills[0], skills[1], skills[2]]);
        let mut b = build(&[skills[2], skills[1], skills[0]]);
        assert_eq!(
            a.upgrade_random_cards_of_type(crate::cards::CardType::Skill, 2),
            2
        );
        assert_eq!(
            b.upgrade_random_cards_of_type(crate::cards::CardType::Skill, 2),
            2
        );
        // Compare the chosen *card identities* (not deck positions): both
        // runs must upgrade the same two CardIds.
        let chosen = |rs: &RunState| -> std::collections::BTreeSet<CardId> {
            rs.master_deck
                .iter()
                .filter(|c| c.upgraded)
                .map(|c| c.card_id)
                .collect()
        };
        assert_eq!(
            chosen(&a),
            chosen(&b),
            "StableShuffle pre-sort makes the WarPaint pick independent of raw deck order"
        );
    }

    // ── transform_cards / offer_transform_cards_reward — bead aw73x ──

    #[test]
    fn transform_cards_zero_count_is_noop() {
        let mut rs = RunState::new();
        for _ in 0..3 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        }
        let before_len = rs.master_deck.len();
        assert_eq!(rs.transform_cards(0, vec![]), 0);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    #[test]
    fn transform_cards_empty_deck_returns_zero() {
        let mut rs = RunState::new();
        let replacement = mk_deck_card(crate::cards::CardId::DefendIronclad, false);
        assert_eq!(rs.transform_cards(3, vec![replacement]), 0);
        assert!(rs.master_deck.is_empty());
    }

    #[test]
    fn transform_cards_saturates_at_min_of_count_deck_replacements() {
        let mut rs = RunState::new();
        for _ in 0..5 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        }
        // Ask for 4 transforms but supply only 2 replacements → saturates at 2.
        let replacements = vec![
            mk_deck_card(crate::cards::CardId::DefendIronclad, false),
            mk_deck_card(crate::cards::CardId::DefendIronclad, false),
        ];
        let transformed = rs.transform_cards(4, replacements);
        assert_eq!(transformed, 2, "saturates at replacements.len()");
        assert_eq!(rs.master_deck.len(), 5, "deck size preserved (2 in, 2 out)");
        let defends = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == crate::cards::CardId::DefendIronclad)
            .count();
        assert_eq!(defends, 2);
    }

    #[test]
    fn transform_cards_removes_count_and_appends_replacements() {
        let mut rs = RunState::new();
        for _ in 0..6 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        }
        let replacements = vec![
            mk_deck_card(crate::cards::CardId::DefendIronclad, true),
            mk_deck_card(crate::cards::CardId::DefendIronclad, true),
            mk_deck_card(crate::cards::CardId::DefendIronclad, true),
        ];
        let transformed = rs.transform_cards(3, replacements);
        assert_eq!(transformed, 3);
        assert_eq!(rs.master_deck.len(), 6, "deck size invariant: 3 out, 3 in");
        let strike_count = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == crate::cards::CardId::StrikeIronclad)
            .count();
        let defend_count = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == crate::cards::CardId::DefendIronclad)
            .count();
        assert_eq!(strike_count, 3);
        assert_eq!(defend_count, 3);
    }

    #[test]
    fn transform_cards_is_deterministic_under_same_seed() {
        let build = || {
            let mut rs = RunState {
                rng: crate::run_rng_set::RunRngSet::new(0x42),
                ..RunState::default()
            };
            for i in 0..8 {
                let id = if i % 2 == 0 {
                    crate::cards::CardId::StrikeIronclad
                } else {
                    crate::cards::CardId::DefendIronclad
                };
                rs.master_deck.push(mk_deck_card(id, false));
            }
            rs
        };
        let mk_replacements = || {
            vec![
                mk_deck_card(crate::cards::CardId::AscendersBane, false),
                mk_deck_card(crate::cards::CardId::AscendersBane, false),
                mk_deck_card(crate::cards::CardId::AscendersBane, false),
            ]
        };
        let mut a = build();
        let mut b = build();
        assert_eq!(a.transform_cards(3, mk_replacements()), 3);
        assert_eq!(b.transform_cards(3, mk_replacements()), 3);
        let a_ids: Vec<_> = a.master_deck.iter().map(|c| c.card_id).collect();
        let b_ids: Vec<_> = b.master_deck.iter().map(|c| c.card_id).collect();
        assert_eq!(a_ids, b_ids, "same seed → identical post-transform deck");
    }

    // ── transform_specific_cards — bead ab8uo (P9.3.a) ──

    #[test]
    fn transform_specific_cards_empty_inputs_return_zero() {
        let mut rs = RunState::new();
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        assert_eq!(rs.transform_specific_cards(&[], vec![]), 0);
        assert_eq!(
            rs.transform_specific_cards(&[0], vec![]),
            0,
            "no replacements → no-op"
        );
        assert_eq!(
            rs.transform_specific_cards(
                &[],
                vec![mk_deck_card(crate::cards::CardId::DefendIronclad, false)]
            ),
            0,
            "no indices → no-op"
        );
    }

    #[test]
    fn transform_specific_cards_removes_original_and_appends_replacement_to_tail() {
        // C# `CardCmd.Transform` deck branch: remove the original from its
        // slot, append the replacement to the END of the deck.
        let mut rs = RunState::new();
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        let replacements = vec![mk_deck_card(crate::cards::CardId::DefendIronclad, true)];
        let n = rs.transform_specific_cards(&[1], replacements);
        assert_eq!(n, 1);
        assert_eq!(rs.master_deck.len(), 3, "deck length unchanged");
        // Original index-1 Strike removed; surviving Strikes keep order; the
        // Defend replacement lands at the tail (index 2).
        assert_eq!(
            rs.master_deck[0].card_id,
            crate::cards::CardId::StrikeIronclad
        );
        assert_eq!(
            rs.master_deck[1].card_id,
            crate::cards::CardId::StrikeIronclad
        );
        assert_eq!(
            rs.master_deck[2].card_id,
            crate::cards::CardId::DefendIronclad
        );
        assert!(
            rs.master_deck[2].upgraded,
            "replacement carries its own flags"
        );
    }

    #[test]
    fn transform_specific_cards_appends_in_ascending_index_order() {
        // Multiple transforms append in ascending original-index order
        // (C# `PileIndexSort`), regardless of the order indices are passed.
        let mut rs = RunState::new();
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::Bash, false));
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::DefendIronclad, false));
        // Pass indices out of order: 2 (Defend) then 0 (Strike). The tail
        // order must still be ascending-index: Strike-replacement (idx 0)
        // before Defend-replacement (idx 2).
        let replacements = vec![
            mk_deck_card(crate::cards::CardId::Anger, false), // for idx 2
            mk_deck_card(crate::cards::CardId::Headbutt, false), // for idx 0
        ];
        let n = rs.transform_specific_cards(&[2, 0], replacements);
        assert_eq!(n, 2);
        assert_eq!(rs.master_deck.len(), 3, "deck length unchanged");
        // Only Bash (original idx 1) survives in place at the head.
        assert_eq!(rs.master_deck[0].card_id, crate::cards::CardId::Bash);
        // Tail = replacements in ascending original-index order: idx-0's
        // Headbutt, then idx-2's Anger.
        assert_eq!(rs.master_deck[1].card_id, crate::cards::CardId::Headbutt);
        assert_eq!(rs.master_deck[2].card_id, crate::cards::CardId::Anger);
    }

    #[test]
    fn transform_specific_cards_saturates_at_min_of_indices_replacements() {
        let mut rs = RunState::new();
        for _ in 0..4 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        }
        // 3 indices, 2 replacements → take = 2 (transform indices 0 and 1).
        let replacements = vec![
            mk_deck_card(crate::cards::CardId::DefendIronclad, false),
            mk_deck_card(crate::cards::CardId::DefendIronclad, false),
        ];
        let n = rs.transform_specific_cards(&[0, 1, 2], replacements);
        assert_eq!(n, 2);
        assert_eq!(rs.master_deck.len(), 4, "deck length unchanged");
        // Originals 0 and 1 removed; the two untouched Strikes shift to the
        // head, the two Defend replacements append at the tail.
        assert_eq!(
            rs.master_deck[0].card_id,
            crate::cards::CardId::StrikeIronclad
        );
        assert_eq!(
            rs.master_deck[1].card_id,
            crate::cards::CardId::StrikeIronclad
        );
        assert_eq!(
            rs.master_deck[2].card_id,
            crate::cards::CardId::DefendIronclad
        );
        assert_eq!(
            rs.master_deck[3].card_id,
            crate::cards::CardId::DefendIronclad
        );
    }

    #[test]
    fn transform_specific_cards_skips_out_of_bounds_indices() {
        let mut rs = RunState::new();
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        let replacements = vec![
            mk_deck_card(crate::cards::CardId::DefendIronclad, false),
            mk_deck_card(crate::cards::CardId::DefendIronclad, false),
        ];
        // Index 5 is OOB; index 1 is valid.
        let n = rs.transform_specific_cards(&[5, 1], replacements);
        assert_eq!(n, 1, "only the in-bounds index was transformed");
        assert_eq!(
            rs.master_deck[0].card_id,
            crate::cards::CardId::StrikeIronclad
        );
        assert_eq!(
            rs.master_deck[1].card_id,
            crate::cards::CardId::DefendIronclad
        );
    }

    #[test]
    fn transform_specific_cards_does_not_advance_niche_rng() {
        // Mirrors Python: transform_specific_cards is content-agnostic
        // and does no sampling of its own, so it must not perturb the
        // niche RNG stream — callers control sampling externally.
        let mut rs = RunState {
            rng: crate::run_rng_set::RunRngSet::new(0x99),
            ..RunState::default()
        };
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        let counter_before = rs.rng.niche.counter();
        let replacements = vec![mk_deck_card(crate::cards::CardId::DefendIronclad, false)];
        let _ = rs.transform_specific_cards(&[0], replacements);
        let counter_after = rs.rng.niche.counter();
        assert_eq!(
            counter_before, counter_after,
            "transform_specific_cards must not advance the niche RNG"
        );
    }

    #[test]
    fn winged_boots_uses_remaining_tracks_ownership_and_charges() {
        let mut rs = RunState::new();
        // Not owned → no off-path moves regardless of counter.
        assert!(!rs.winged_boots_uses_remaining());
        rs.relics.push(crate::RelicId::WingedBoots);
        assert!(
            rs.winged_boots_uses_remaining(),
            "owned, 0 used → remaining"
        );
        // Spend all three uses; the fourth call sees none remaining.
        for i in 1..=RunState::WINGED_BOOTS_MAX_USES {
            assert!(rs.winged_boots_uses_remaining());
            rs.winged_boots_record_off_path();
            assert_eq!(rs.winged_boots_off_path_used, i);
        }
        assert!(
            !rs.winged_boots_uses_remaining(),
            "all {} uses spent → no off-path moves",
            RunState::WINGED_BOOTS_MAX_USES
        );
        // Saturating: extra records never wrap.
        rs.winged_boots_record_off_path();
        assert_eq!(
            rs.winged_boots_off_path_used,
            RunState::WINGED_BOOTS_MAX_USES + 1
        );
        assert!(!rs.winged_boots_uses_remaining());
    }

    #[test]
    fn offer_transform_cards_reward_opens_pending_choice() {
        let mut rs = RunState::new();
        for _ in 0..4 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        }
        assert!(rs.pending_choice.is_none());
        let opened = rs.offer_transform_cards_reward(2);
        assert!(opened, "prompt opened on a 4-card deck asking for 2");
        let pc = rs.pending_choice.as_ref().expect("pending choice set");
        assert_eq!(pc.kind, crate::ChoiceKind::CardSelect);
        assert_eq!(pc.options.len(), 4);
        assert_eq!(pc.min_select, 2);
        assert_eq!(pc.max_select, 2);
        assert!(!pc.can_skip, "transform reward is mandatory");
        // No source_pile / card_uid on master-deck options.
        for opt in &pc.options {
            assert!(opt.card_uid.is_none());
            assert!(opt.source_pile.is_none());
        }
    }

    #[test]
    fn offer_transform_cards_reward_rejects_when_deck_too_small() {
        let mut rs = RunState::new();
        rs.master_deck
            .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        assert!(
            !rs.offer_transform_cards_reward(2),
            "1-card deck cannot satisfy a 2-card transform reward"
        );
        assert!(rs.pending_choice.is_none());
    }

    #[test]
    fn offer_transform_cards_reward_rejects_zero_count() {
        let mut rs = RunState::new();
        for _ in 0..3 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        }
        assert!(!rs.offer_transform_cards_reward(0));
        assert!(rs.pending_choice.is_none());
    }

    #[test]
    fn offer_transform_cards_reward_rejects_double_open() {
        let mut rs = RunState::new();
        for _ in 0..3 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        }
        assert!(rs.offer_transform_cards_reward(1));
        assert!(
            !rs.offer_transform_cards_reward(1),
            "second open with a prompt already pending must be refused"
        );
    }

    /// Bead `sts2-rs-t762n`: `remove_cards_from_deck(0)` is a no-op
    /// on an empty deck and on a populated deck.
    #[test]
    fn remove_cards_from_deck_count_zero_is_noop() {
        let mut rs = RunState::new();
        assert_eq!(rs.remove_cards_from_deck(0), 0);
        for _ in 0..3 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        }
        let before_len = rs.master_deck.len();
        assert_eq!(rs.remove_cards_from_deck(0), 0);
        assert_eq!(rs.master_deck.len(), before_len);
    }

    /// Bead `sts2-rs-t762n`: `remove_cards_from_deck` on an empty deck
    /// returns 0 regardless of `count`.
    #[test]
    fn remove_cards_from_deck_empty_deck_returns_zero() {
        let mut rs = RunState::new();
        assert_eq!(rs.remove_cards_from_deck(5), 0);
        assert!(rs.master_deck.is_empty());
    }

    /// Bead `sts2-rs-t762n`: with a deck of N cards, removing `count`
    /// with `count <= N` removes exactly `count` and shrinks the deck
    /// by `count`. Returns the actual removed count.
    #[test]
    fn remove_cards_from_deck_removes_exactly_n_when_enough_cards() {
        let mut rs = RunState::new();
        for _ in 0..10 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        }
        let removed = rs.remove_cards_from_deck(4);
        assert_eq!(removed, 4);
        assert_eq!(rs.master_deck.len(), 6);
    }

    /// Bead `sts2-rs-t762n`: with a deck of N cards, requesting more
    /// than N removes all N and returns N (clamped).
    #[test]
    fn remove_cards_from_deck_clamps_to_deck_size() {
        let mut rs = RunState::new();
        for _ in 0..3 {
            rs.master_deck
                .push(mk_deck_card(crate::cards::CardId::StrikeIronclad, false));
        }
        let removed = rs.remove_cards_from_deck(10);
        assert_eq!(removed, 3);
        assert!(rs.master_deck.is_empty());
    }

    /// Bead `sts2-rs-vd9tm`: `gain_max_hp` saturates on i32 overflow rather
    /// than panicking (debug) or silently wrapping (release).
    #[test]
    fn gain_max_hp_saturates_on_overflow() {
        let mut rs = RunState::new();
        rs.gain_max_hp(i32::MAX);
        assert_eq!(rs.max_hp, i32::MAX);
        assert_eq!(rs.current_hp, i32::MAX);
    }

    /// Bead `sts2-rs-vd9tm`: `gain_max_hp` with a negative delta large enough
    /// to drive `max_hp` to zero or below floors at `1`, mirroring Python's
    /// `lose_max_hp`'s `max(max_hp, 1)` clamp.
    #[test]
    fn gain_max_hp_floors_max_hp_at_one() {
        let mut rs = RunState::new();
        assert_eq!(rs.max_hp, 80);
        rs.gain_max_hp(-200);
        assert_eq!(rs.max_hp, 1, "max_hp must floor at 1, not go negative");
    }

    /// Bead `sts2-rs-vd9tm`: `heal` saturates on extreme amounts and still
    /// respects the (clamped) `max_hp` ceiling.
    #[test]
    fn heal_saturates_and_clamps_to_max_hp() {
        let mut rs = RunState::new();
        rs.current_hp = 10;
        rs.max_hp = 80;
        let applied = rs.heal(i32::MAX);
        assert_eq!(rs.current_hp, 80, "heal clamped to max_hp");
        assert_eq!(applied, 70);
    }

    /// Bead `sts2-rs-vd9tm`: `lose_hp` saturates on extreme amounts and still
    /// floors at zero.
    #[test]
    fn lose_hp_saturates_and_floors_at_zero() {
        let mut rs = RunState::new();
        rs.current_hp = 10;
        let applied = rs.lose_hp(i32::MAX);
        assert_eq!(rs.current_hp, 0);
        assert_eq!(applied, 10);
    }

    /// Bead `sts2-rs-ekf4j`: `rest_site_options()` always emits Rest +
    /// Smith. Empty-deck Smith is disabled.
    #[test]
    fn rest_site_options_emits_rest_and_smith_by_default() {
        let rs = RunState::new();
        let opts = rs.rest_site_options();
        assert!(opts.iter().any(|o| matches!(o, RestSiteOption::Rest)));
        assert!(opts
            .iter()
            .any(|o| matches!(o, RestSiteOption::Smith { enabled: false })));
    }

    /// Bead `sts2-rs-RP24`: a `ByrdonisEgg` in the master deck surfaces
    /// the `Hatch` rest-site option (C# `ByrdonisEgg.TryModifyRestSiteOptions`).
    #[test]
    fn rest_site_options_offers_hatch_when_deck_holds_byrdonis_egg() {
        let mut rs = RunState::new();
        assert!(
            !rs.rest_site_options()
                .iter()
                .any(|o| matches!(o, RestSiteOption::Hatch)),
            "no Hatch without the egg"
        );
        let mut egg = Card::pod_defaults();
        egg.card_id = CardId::ByrdonisEgg;
        rs.master_deck.push(egg);
        assert!(
            rs.rest_site_options()
                .iter()
                .any(|o| matches!(o, RestSiteOption::Hatch)),
            "Hatch offered once the egg is in the deck"
        );
    }

    /// Bead `sts2-rs-RP24`: selecting `Hatch` obtains the `Byrdpip` relic
    /// (C# `HatchRestSiteOption.OnSelect`) and reports `Hatched`. The
    /// egg→`ByrdSwoop` transform itself is driven by Byrdpip's content-side
    /// `after_obtained` listener (covered in `sts2-content`).
    #[test]
    fn do_rest_site_action_hatch_obtains_byrdpip() {
        let mut rs = RunState::new();
        let mut egg = Card::pod_defaults();
        egg.card_id = CardId::ByrdonisEgg;
        rs.master_deck.push(egg);
        let outcome = rs.do_rest_site_action(RestSiteOption::Hatch);
        assert_eq!(outcome, RestSiteActionOutcome::Hatched);
        assert!(
            rs.relics.contains(&RelicId::Byrdpip),
            "Hatch grants the Byrdpip relic"
        );
    }

    /// `Smith.enabled` flips on once the deck has at least one
    /// non-upgraded card.
    #[test]
    fn rest_site_options_smith_enabled_when_unupgraded_card_present() {
        let mut rs = RunState::new();
        let mut card = Card::pod_defaults();
        card.upgraded = false;
        rs.master_deck.push(card);
        let opts = rs.rest_site_options();
        assert!(opts
            .iter()
            .any(|o| matches!(o, RestSiteOption::Smith { enabled: true })));
    }

    /// With no content registry installed — this crate never installs one —
    /// `removable_deck_card_count` counts EVERY master-deck entry, the same
    /// documented fallback `removal_offer_order` takes. Basic Strike is in
    /// the count on purpose: C# `CookRestSiteOption.cs:64` counts
    /// `c.IsRemovable`, and a basic Strike is removable. The registered
    /// behaviour (Eternal excluded, Ember'd basics excluded) is pinned in
    /// `sts2-content` where the registry exists.
    #[test]
    fn removable_deck_card_count_is_unfiltered_without_the_content_registry() {
        let mut rs = RunState::new();
        let mut basic = Card::pod_defaults();
        basic.card_id = CardId::StrikeIronclad;
        let mut other = Card::pod_defaults();
        other.card_id = CardId::Anger;
        rs.master_deck.extend([basic, other, other]);
        assert_eq!(rs.removable_deck_card_count(), 3);
    }

    /// `removal_offer_order` mirrors C# `FromDeckForRemoval`: sorts Curse-type
    /// cards to the FRONT (stable) so a curse sitting mid-deck does not shift
    /// every removal index. It does NOT filter basics (C# `IsRemovable` allows
    /// basic removal via Neow/events — only the shop predicate excludes them).
    #[test]
    fn removal_offer_order_fronts_curses_keeps_all_cards() {
        let mut rs = RunState::new();
        let mk = |id: CardId, ty: crate::cards::CardType| {
            let mut c = Card::pod_defaults();
            c.card_id = id;
            c.card_type = ty;
            c
        };
        use crate::cards::CardType;
        // deck: Strike, Decay(curse), Anger, Anger
        rs.master_deck.extend([
            mk(CardId::StrikeIronclad, CardType::Attack),
            mk(CardId::Decay, CardType::Curse),
            mk(CardId::Anger, CardType::Attack),
            mk(CardId::Anger, CardType::Attack),
        ]);
        // Decay (the curse) sorts to the front; all other cards (incl. the
        // basic Strike) keep deck order behind it.
        assert_eq!(rs.removal_offer_order(), vec![1, 0, 2, 3]);
    }

    /// `upgradable_deck_cards()` returns the `CardId`s of every
    /// non-upgraded entry; upgraded cards are filtered out. Bead
    /// `sts2-rs-70anj.3.14`.
    #[test]
    fn upgradable_deck_cards_filters_upgraded() {
        let mut rs = RunState::new();
        let mut anger = Card::pod_defaults();
        anger.card_id = CardId::Anger;
        let mut anger_up = Card::pod_defaults();
        anger_up.card_id = CardId::Anger;
        anger_up.upgraded = true;
        let mut strike = Card::pod_defaults();
        strike.card_id = CardId::StrikeIronclad;
        rs.master_deck.extend([anger, anger_up, strike]);
        let pool = rs.upgradable_deck_cards();
        assert_eq!(pool, vec![CardId::Anger, CardId::StrikeIronclad]);
    }

    /// `offer_upgrade_cards_reward(count, None)` pushes a
    /// `Reward::UpgradeCards` carrying the auto-derived eligible pool.
    /// Bead `sts2-rs-70anj.3.14`.
    #[test]
    fn offer_upgrade_cards_reward_auto_pool() {
        let mut rs = RunState::new();
        let mut anger = Card::pod_defaults();
        anger.card_id = CardId::Anger;
        let mut anger_up = Card::pod_defaults();
        anger_up.card_id = CardId::Anger;
        anger_up.upgraded = true;
        rs.master_deck.extend([anger, anger_up]);

        rs.offer_upgrade_cards_reward(1, None);
        assert_eq!(rs.pending_rewards.len(), 1);
        let Reward::UpgradeCards(r) = &rs.pending_rewards[0] else {
            panic!("expected Reward::UpgradeCards");
        };
        assert_eq!(r.count, 1);
        assert_eq!(r.options, vec![CardId::Anger]);
    }

    /// `offer_upgrade_cards_reward(count, Some(pool))` honours the
    /// caller-supplied pool verbatim — mirrors Python `cards=` kwarg
    /// used by `WarHammer` to pre-filter candidates. Bead
    /// `sts2-rs-70anj.3.14`.
    #[test]
    fn offer_upgrade_cards_reward_explicit_pool() {
        let mut rs = RunState::new();
        let pool = vec![CardId::Anger, CardId::Bash];
        rs.offer_upgrade_cards_reward(2, Some(pool.clone()));
        let Reward::UpgradeCards(r) = &rs.pending_rewards[0] else {
            panic!("expected Reward::UpgradeCards");
        };
        assert_eq!(r.count, 2);
        assert_eq!(r.options, pool);
    }

    /// `offer_upgrade_cards_reward(0, ...)` still queues a reward —
    /// matches Python's behaviour (caller decides whether to skip).
    /// Bead `sts2-rs-70anj.3.14`.
    #[test]
    fn offer_upgrade_cards_reward_count_zero_still_pushes() {
        let mut rs = RunState::new();
        rs.offer_upgrade_cards_reward(0, Some(vec![CardId::Anger]));
        assert_eq!(rs.pending_rewards.len(), 1);
    }

    /// Bead `sts2-rs-2v8y2`: `offer_enchant_cards_reward` pushes a
    /// fully-populated `Reward::EnchantCards` onto pending_rewards.
    #[test]
    fn offer_enchant_cards_reward_pushes_populated_reward() {
        use crate::cards::EnchantmentId;
        let mut rs = RunState::new();
        let pool = vec![CardId::Anger, CardId::Bash];
        rs.offer_enchant_cards_reward(EnchantmentId::Sharp, 3, 2, pool.clone(), Some(0));
        assert_eq!(rs.pending_rewards.len(), 1);
        let Reward::EnchantCards(r) = &rs.pending_rewards[0] else {
            panic!("expected Reward::EnchantCards");
        };
        assert_eq!(r.enchantment, EnchantmentId::Sharp);
        assert_eq!(r.amount, 3);
        assert_eq!(r.count, 2);
        assert_eq!(r.options, pool);
        assert_eq!(r.min_count, Some(0));
    }

    /// Bead `sts2-rs-2v8y2`: `min_count = None` round-trips (Python's
    /// "must pick exactly `count`" semantics).
    #[test]
    fn offer_enchant_cards_reward_no_min_count() {
        use crate::cards::EnchantmentId;
        let mut rs = RunState::new();
        rs.offer_enchant_cards_reward(EnchantmentId::Adroit, 1, 3, vec![CardId::Anger], None);
        let Reward::EnchantCards(r) = &rs.pending_rewards[0] else {
            panic!("expected Reward::EnchantCards");
        };
        assert_eq!(r.min_count, None);
    }

    /// `RestSiteOption::enabled()` round-trip.
    #[test]
    fn rest_site_option_enabled_round_trip() {
        assert!(RestSiteOption::Rest.enabled());
        assert!(RestSiteOption::Smith { enabled: true }.enabled());
        assert!(!RestSiteOption::Smith { enabled: false }.enabled());
        assert!(RestSiteOption::Cook { enabled: true }.enabled());
        assert!(!RestSiteOption::Cook { enabled: false }.enabled());
        assert!(RestSiteOption::Lift { lifts_done: 0 }.enabled());
        assert!(RestSiteOption::Lift { lifts_done: 2 }.enabled());
        assert!(!RestSiteOption::Lift { lifts_done: 3 }.enabled());
    }
}
