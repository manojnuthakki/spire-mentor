//! Shop (Merchant) inventory PODs + base price tables — bead
//! `sts2-rs-o0x66` (P9.2.c.1 bounded slice).
//!
//! Ports the data shape + price constants from
//! `sts2-rl-agent@01c094d:sts2_env/run/shop.py`. Pure POD + constants
//! only — no RNG, no inventory roll, no merchant-price-modifier
//! channel walk. Downstream sub-beads layer on:
//! - `64mq2`: `_enter_shop` inventory roll generator (uses these
//!   PODs + `RunRngSet.shops`).
//! - `rhcgq`: purchase action + sold-out tracking.
//! - `jbqw4`: remove-card slot + special pricing.
//! - `s8jgx`: umbrella P9.2.c shop pipeline.
//!
//! ## Local rarity mirrors
//!
//! Python uses `CardRarity` / `PotionRarity` / `RelicRarity` enums from
//! `sts2_env/core/enums.py`. Those Rust counterparts live in
//! `sts2-content` (`CardRarity`) and don't all exist yet (`PotionRarity`
//! isn't ported as an enum; `RelicRarity` lives in `sts2-content`).
//! Because the workspace dep direction is `content → core`, this module
//! defines local rarity-bucket enums for the price-table lookups:
//!
//! - [`ShopRelicRarity`] (Common / Uncommon / Rare / Shop) — the 4
//!   rarities Python's `RELIC_BASE_COSTS` indexes; `Boss` / `Starter`
//!   / `Event` / `Ancient` are unreachable from a merchant roll.
//! - [`ShopPotionRarity`] (Common / Uncommon / Rare) — the 3 rarities
//!   Python's `POTION_BASE_COSTS` indexes.
//!
//! For card-rarity, this module reuses
//! [`crate::card_rewards::RewardRarity`] (Common / Uncommon / Rare —
//! exactly the buckets Python's `CARD_BASE_COSTS` distinguishes; Basic
//! / Status / Curse / etc. fall to the default).

use crate::card_rewards::RewardRarity;

// ── Card pricing constants (Python `shop.py:27-36`) ──────────────────

/// Base card prices indexed by [`RewardRarity`]. Mirrors Python
/// `CARD_BASE_COSTS = {RARE: 150, UNCOMMON: 75}` with the implicit
/// default of [`CARD_DEFAULT_BASE_COST`] for COMMON.
pub const CARD_BASE_COST_RARE: i32 = 150;
/// Mirrors Python `CARD_BASE_COSTS[UNCOMMON] = 75`.
pub const CARD_BASE_COST_UNCOMMON: i32 = 75;
/// Mirrors Python `CARD_DEFAULT_BASE_COST = 50` (applied to COMMON
/// + any rarity not in the table).
pub const CARD_DEFAULT_BASE_COST: i32 = 50;

/// Mirrors Python `COLORLESS_CARD_PRICE_MULTIPLIER = 1.15`.
pub const COLORLESS_CARD_PRICE_MULTIPLIER: f64 = 1.15;

/// Mirrors Python `MERCHANT_CARD_PRICE_VARIANCE = (0.95, 1.05)`. The
/// inventory-roll generator (bead `sts2-rs-64mq2`) consumes this for
/// `rng.next_float_range(MIN, MAX)`.
pub const MERCHANT_CARD_PRICE_VARIANCE_MIN: f64 = 0.95;
/// See [`MERCHANT_CARD_PRICE_VARIANCE_MIN`].
pub const MERCHANT_CARD_PRICE_VARIANCE_MAX: f64 = 1.05;

/// Mirrors Python `MERCHANT_POTION_PRICE_VARIANCE = (0.95, 1.05)`.
pub const MERCHANT_POTION_PRICE_VARIANCE_MIN: f64 = 0.95;
/// See [`MERCHANT_POTION_PRICE_VARIANCE_MIN`].
pub const MERCHANT_POTION_PRICE_VARIANCE_MAX: f64 = 1.05;

/// Mirrors Python `MERCHANT_RELIC_PRICE_VARIANCE = (0.85, 1.15)` —
/// wider than card / potion variance.
pub const MERCHANT_RELIC_PRICE_VARIANCE_MIN: f64 = 0.85;
/// See [`MERCHANT_RELIC_PRICE_VARIANCE_MIN`].
pub const MERCHANT_RELIC_PRICE_VARIANCE_MAX: f64 = 1.15;

/// Mirrors Python `SALE_PRICE_DIVISOR = 2` — on-sale cards halve their
/// rolled price (integer-divide).
pub const SALE_PRICE_DIVISOR: i32 = 2;

// ── Potion pricing constants (Python `shop.py:38-42`) ────────────────

/// Mirrors Python `POTION_BASE_COSTS[RARE] = 100`.
pub const POTION_BASE_COST_RARE: i32 = 100;
/// Mirrors Python `POTION_BASE_COSTS[UNCOMMON] = 75`.
pub const POTION_BASE_COST_UNCOMMON: i32 = 75;
/// Mirrors Python `POTION_DEFAULT_BASE_COST = 50` (COMMON + any
/// rarity not in the table).
pub const POTION_DEFAULT_BASE_COST: i32 = 50;

// ── Relic pricing constants (Python `shop.py:111-117`) ───────────────

/// Mirrors Python `RELIC_BASE_COSTS[COMMON] = 150`.
pub const RELIC_BASE_COST_COMMON: i32 = 150;
/// Mirrors Python `RELIC_BASE_COSTS[UNCOMMON] = 250`.
pub const RELIC_BASE_COST_UNCOMMON: i32 = 250;
/// Mirrors Python `RELIC_BASE_COSTS[RARE] = 300`.
pub const RELIC_BASE_COST_RARE: i32 = 300;
/// Mirrors Python `RELIC_BASE_COSTS[SHOP] = 150`.
pub const RELIC_BASE_COST_SHOP: i32 = 150;
/// Mirrors Python `RELIC_DEFAULT_BASE_COST = 200`.
pub const RELIC_DEFAULT_BASE_COST: i32 = 200;

// ── Card-removal pricing (Python `shop.py:44-45`) ────────────────────

/// Mirrors Python `CARD_REMOVAL_BASE_COST = 75` — first removal in a
/// run costs 75 gold.
pub const CARD_REMOVAL_BASE_COST: i32 = 75;
/// Mirrors Python `CARD_REMOVAL_COST_INCREASE = 25` — each subsequent
/// removal adds 25 gold.
pub const CARD_REMOVAL_COST_INCREASE: i32 = 25;
/// Ascension level at which `Inflation` raises shop card-removal pricing.
/// Mirrors C# `AscensionLevel.Inflation` (enum discriminant 6) —
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Entities.Ascension/AscensionLevel.cs`.
pub const INFLATION_ASCENSION_LEVEL: u8 = 6;
/// Card-removal base cost at ascension >= 6. Mirrors C#
/// `MerchantCardRemovalEntry.BaseCost = GetValueIfAscension(Inflation, 100, 75)`.
pub const CARD_REMOVAL_BASE_COST_INFLATION: i32 = 100;
/// Per-removal increment at ascension >= 6. Mirrors C#
/// `MerchantCardRemovalEntry.PriceIncrease = GetValueIfAscension(Inflation, 50, 25)`.
pub const CARD_REMOVAL_COST_INCREASE_INFLATION: i32 = 50;

// ── Inventory shape constants (Python `shop.py:105-110`) ─────────────

/// Mirrors Python `SHOP_ENTRY_SOLD_OUT_PRICE = 999999` — sentinel
/// applied to an entry's `price` after it's purchased.
pub const SHOP_ENTRY_SOLD_OUT_PRICE: i32 = 999_999;

/// Mirrors Python `MERCHANT_CHARACTER_CARD_TYPES = ("Attack",
/// "Attack", "Skill", "Skill", "Power")` — the canonical 5-slot
/// composition of the character-card row at every merchant.
pub const MERCHANT_CHARACTER_CARD_TYPES: [crate::cards::CardType; 5] = [
    crate::cards::CardType::Attack,
    crate::cards::CardType::Attack,
    crate::cards::CardType::Skill,
    crate::cards::CardType::Skill,
    crate::cards::CardType::Power,
];

/// Mirrors Python `MERCHANT_ROLLED_RELIC_COUNT = 2` — number of
/// non-shop-pool relics offered at every merchant (the 3rd slot is
/// reserved for a [`ShopRelicRarity::Shop`]-pool relic).
pub const MERCHANT_ROLLED_RELIC_COUNT: usize = 2;

/// Mirrors Python `MERCHANT_POTION_COUNT = 3` — every merchant offers
/// exactly 3 potions.
pub const MERCHANT_POTION_COUNT: usize = 3;

/// Mirrors Python's `MERCHANT_COLORLESS_CARD_RARITIES = (UNCOMMON,
/// RARE)` — the 2 colorless cards in the inventory roll are one
/// [`RewardRarity::Uncommon`] and one [`RewardRarity::Rare`].
pub const MERCHANT_COLORLESS_CARD_RARITIES: [RewardRarity; 2] =
    [RewardRarity::Uncommon, RewardRarity::Rare];

// ── Local rarity mirrors ─────────────────────────────────────────────

/// Shop-relic rarity bucket. Mirrors the subset of Python's
/// `RelicRarity` (`sts2_env/core/enums.py::RelicRarity`) that
/// merchant rolls can produce. `Boss` / `Starter` / `Event` /
/// `Ancient` are unreachable from a merchant roll, so they are not
/// listed.
///
/// Lives in `sts2-core` rather than reusing
/// `sts2_content::relics::RelicRarity` because the workspace dep
/// direction is `content → core`. The `s8jgx`-chain
/// content-side roller will provide a `From` conversion when it lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ShopRelicRarity {
    /// `RelicRarity.COMMON`. Base price [`RELIC_BASE_COST_COMMON`].
    Common,
    /// `RelicRarity.UNCOMMON`. Base price [`RELIC_BASE_COST_UNCOMMON`].
    Uncommon,
    /// `RelicRarity.RARE`. Base price [`RELIC_BASE_COST_RARE`].
    Rare,
    /// `RelicRarity.SHOP`. Base price [`RELIC_BASE_COST_SHOP`].
    Shop,
}

/// Shop-potion rarity bucket. Mirrors Python's `PotionRarity`. Locally
/// defined for the same dep-direction reason as [`ShopRelicRarity`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ShopPotionRarity {
    /// `PotionRarity.COMMON`. Base price [`POTION_DEFAULT_BASE_COST`].
    Common,
    /// `PotionRarity.UNCOMMON`. Base price [`POTION_BASE_COST_UNCOMMON`].
    Uncommon,
    /// `PotionRarity.RARE`. Base price [`POTION_BASE_COST_RARE`].
    Rare,
}

// ── Pure price-table accessors ───────────────────────────────────────

/// Base price for a card of the given rarity. Mirrors Python
/// `card_base_cost(rarity)` (`shop.py:48-49`). `Uncommon` / `Rare`
/// have explicit entries; `Common` falls to [`CARD_DEFAULT_BASE_COST`].
#[must_use]
pub const fn card_base_cost(rarity: RewardRarity) -> i32 {
    match rarity {
        RewardRarity::Rare => CARD_BASE_COST_RARE,
        RewardRarity::Uncommon => CARD_BASE_COST_UNCOMMON,
        RewardRarity::Common => CARD_DEFAULT_BASE_COST,
    }
}

/// Base price for a potion of the given rarity. Mirrors Python
/// `POTION_BASE_COSTS.get(rarity, POTION_DEFAULT_BASE_COST)`.
#[must_use]
pub const fn potion_base_cost(rarity: ShopPotionRarity) -> i32 {
    match rarity {
        ShopPotionRarity::Rare => POTION_BASE_COST_RARE,
        ShopPotionRarity::Uncommon => POTION_BASE_COST_UNCOMMON,
        ShopPotionRarity::Common => POTION_DEFAULT_BASE_COST,
    }
}

/// Base price for a relic of the given rarity. Mirrors Python
/// `RELIC_BASE_COSTS.get(rarity, RELIC_DEFAULT_BASE_COST)`. The
/// `_DEFAULT_` fall-through path can only fire if a future rarity
/// variant lands without an entry — at the pin every reachable
/// merchant rarity has an explicit entry.
#[must_use]
pub const fn relic_base_cost(rarity: ShopRelicRarity) -> i32 {
    match rarity {
        ShopRelicRarity::Common => RELIC_BASE_COST_COMMON,
        ShopRelicRarity::Uncommon => RELIC_BASE_COST_UNCOMMON,
        ShopRelicRarity::Rare => RELIC_BASE_COST_RARE,
        ShopRelicRarity::Shop => RELIC_BASE_COST_SHOP,
    }
}

/// Card-removal cost as a function of removals already used in the run
/// and the run's ascension level. Mirrors C#
/// `MerchantCardRemovalEntry.CalcCost`: `BaseCost + PriceIncrease *
/// removalsUsed`, where A6 `Inflation` raises the base 75→100 and the
/// increment 25→50 (`GetValueIfAscension(Inflation, …)`). Below A6 this
/// matches the Python pin (`shop.py:74-76`): first removal = 75 gold,
/// each subsequent +25.
#[must_use]
pub const fn card_removal_cost(removals_used: u32, ascension: u8) -> i32 {
    let (base, increase) = if ascension >= INFLATION_ASCENSION_LEVEL {
        (
            CARD_REMOVAL_BASE_COST_INFLATION,
            CARD_REMOVAL_COST_INCREASE_INFLATION,
        )
    } else {
        (CARD_REMOVAL_BASE_COST, CARD_REMOVAL_COST_INCREASE)
    };
    base + increase * (removals_used as i32)
}

/// `true` iff `price` is below the sold-out sentinel. Mirrors Python
/// `is_shop_entry_available(entry) = entry.price < SHOP_ENTRY_SOLD_OUT_PRICE`.
#[must_use]
pub const fn is_shop_entry_available(price: i32) -> bool {
    price < SHOP_ENTRY_SOLD_OUT_PRICE
}

/// Fold the player's owned merchant-price relics onto a rolled base
/// merchant price, mirroring C# `MerchantEntry.Cost`
/// (`MegaCrit.Sts2.Core.Entities.Merchant/MerchantEntry.cs:19-30`): while
/// the player is in a `MerchantRoom`, `Hook.ModifyMerchantPrice` folds
/// every owner's modifier in `decimal`, then the getter truncates once
/// via `(int)`.
///
/// Only two relics override `ModifyMerchantPrice` in v0.107.1
/// (`grep -l ModifyMerchantPrice …Models.Relics`):
///
/// * `TheCourier` — `price * (1 - 20/100)` = `×0.8`
///   (`TheCourier.cs:19-26`).
/// * `MembershipCard` — `price * (Discount/100)` with `Discount = 50`,
///   i.e. `×0.5` (`MembershipCard.cs`).
///
/// Both factors are exact fractions (`4/5`, `1/2`), so an integer
/// numerator/denominator fold followed by one integer division
/// reproduces the C# decimal product's `(int)` truncation for any
/// non-negative price: `(int)(p * 0.8m) == p*4/5` and
/// `(int)(p * 0.5m) == p/2` (floor, since prices are `>= 0`), and the
/// multiplicative fold before a single truncation matches the hook walk
/// (`p*4/5*1/2 == (int)(p*0.4m)`). Order is irrelevant.
///
/// The sold-out sentinel ([`SHOP_ENTRY_SOLD_OUT_PRICE`]) is returned
/// unchanged so a discounted sentinel can never read as available.
#[must_use]
pub fn merchant_price_with_modifiers(relics: &[crate::relics::RelicId], base_price: i32) -> i32 {
    use crate::relics::RelicId;
    if base_price <= 0 || !is_shop_entry_available(base_price) {
        return base_price;
    }
    let mut num: i64 = 1;
    let mut den: i64 = 1;
    if relics.contains(&RelicId::TheCourier) {
        num *= 4;
        den *= 5;
    }
    if relics.contains(&RelicId::MembershipCard) {
        den *= 2;
    }
    ((i64::from(base_price) * num) / den) as i32
}

// ── Inventory PODs ───────────────────────────────────────────────────

/// One card-slot entry in the merchant inventory. Mirrors Python's
/// `@dataclass ShopCardEntry` (`shop.py:124-132`).
///
/// `card` is the concrete in-run [`crate::cards::Card`] instance
/// rolled at inventory generation; downstream purchase action moves
/// the card POD onto [`crate::runstate::RunState::master_deck`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShopCardEntry {
    /// Rolled rarity bucket the card came from. Mirrors Python
    /// `ShopCardEntry.rarity`.
    pub rarity: RewardRarity,
    /// Slot-template card type from
    /// [`MERCHANT_CHARACTER_CARD_TYPES`] (character row) or
    /// [`crate::cards::CardType::Status`] sentinel for the colorless
    /// row's free-form type. Mirrors Python `ShopCardEntry.card_type`.
    pub card_type: crate::cards::CardType,
    /// The rolled card. `None` until the inventory roll generator
    /// (bead `sts2-rs-64mq2`) fills it in; allows the POD slot to
    /// land before the roller.
    pub card: Option<crate::cards::Card>,
    /// `true` iff this entry comes from the colorless pool. Triggers
    /// the [`COLORLESS_CARD_PRICE_MULTIPLIER`] (×1.15) on the
    /// base price. Mirrors Python `ShopCardEntry.is_colorless`.
    pub is_colorless: bool,
    /// `true` iff the entry rolled on sale — price halves via
    /// [`SALE_PRICE_DIVISOR`]. Mirrors Python
    /// `ShopCardEntry.on_sale`.
    pub on_sale: bool,
    /// Final gold price. [`SHOP_ENTRY_SOLD_OUT_PRICE`] after
    /// purchase. Mirrors Python `ShopCardEntry.price`.
    pub price: i32,
}

/// One relic-slot entry. Mirrors Python's `@dataclass ShopRelicEntry`
/// (`shop.py:135-139`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShopRelicEntry {
    /// Rolled rarity bucket. Mirrors Python `ShopRelicEntry.relic_rarity`.
    pub relic_rarity: ShopRelicRarity,
    /// The specific relic. `None` until the inventory roll generator
    /// fills it in. Mirrors Python `ShopRelicEntry.relic_id`.
    pub relic_id: Option<crate::relics::RelicId>,
    /// Final gold price. Mirrors Python `ShopRelicEntry.price`.
    pub price: i32,
}

/// One potion-slot entry. Mirrors Python's `@dataclass
/// ShopPotionEntry` (`shop.py:142-146`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShopPotionEntry {
    /// Rolled rarity bucket. Mirrors Python `ShopPotionEntry.potion_rarity`.
    pub potion_rarity: ShopPotionRarity,
    /// The specific potion kind. `None` until the inventory roll
    /// generator fills it in. Mirrors Python
    /// `ShopPotionEntry.potion_id`.
    pub potion_kind: Option<crate::potions::PotionKind>,
    /// Final gold price. Mirrors Python `ShopPotionEntry.price`.
    pub price: i32,
}

/// Full merchant inventory. Mirrors Python's `@dataclass
/// ShopInventory` (`shop.py:149-158`).
///
/// Composition (per Python `MerchantInventory.cs`):
/// - 5 character cards (Attack/Attack/Skill/Skill/Power per
///   [`MERCHANT_CHARACTER_CARD_TYPES`]).
/// - 2 colorless cards ([`MERCHANT_COLORLESS_CARD_RARITIES`] =
///   one Uncommon, one Rare).
/// - 3 relics ([`MERCHANT_ROLLED_RELIC_COUNT`] = 2 normal-pool +
///   1 [`ShopRelicRarity::Shop`]-pool).
/// - 3 potions ([`MERCHANT_POTION_COUNT`]).
/// - 1 remove-card slot (`removal_cost` + `removal_used`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShopInventory {
    /// 5-card character row. Mirrors Python `ShopInventory.cards`.
    pub cards: Vec<ShopCardEntry>,
    /// 2-card colorless row. Mirrors Python
    /// `ShopInventory.colorless_cards`.
    pub colorless_cards: Vec<ShopCardEntry>,
    /// 3-relic row. Mirrors Python `ShopInventory.relics`.
    pub relics: Vec<ShopRelicEntry>,
    /// 3-potion row. Mirrors Python `ShopInventory.potions`.
    pub potions: Vec<ShopPotionEntry>,
    /// Current remove-card price. Initialized to
    /// [`CARD_REMOVAL_BASE_COST`] (75); recomputed via
    /// [`card_removal_cost`] after each use. Mirrors Python
    /// `ShopInventory.removal_cost`.
    pub removal_cost: i32,
    /// `true` iff the remove-card slot has been consumed this shop
    /// visit. Mirrors Python `ShopInventory.removal_used`.
    pub removal_used: bool,
}

impl ShopInventory {
    /// Construct an empty inventory with `removal_cost =
    /// CARD_REMOVAL_BASE_COST` and `removal_used = false`. All four
    /// vec slots start empty — the inventory-roll generator (bead
    /// `sts2-rs-64mq2`) fills them.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            cards: Vec::new(),
            colorless_cards: Vec::new(),
            relics: Vec::new(),
            potions: Vec::new(),
            removal_cost: CARD_REMOVAL_BASE_COST,
            removal_used: false,
        }
    }
}

// ── Rarity / price RNG rolls (bead `sts2-rs-64mq2`) ──────────────────

/// Set of relic IDs barred from the merchant relic pool. The
/// authoritative source is C# `RelicModel.IsAllowedInShops` (default
/// `true`, overridden to `false` on exactly these five relics:
/// `MegaCrit.Sts2.Core.Models.Relics/{TheCourier,OldCoin,LuckyFysh,
/// BowlerHat,AmethystAubergine}.cs`). The shop relic pull
/// ([`crate::runstate::RunState::pop_relic_from_grab_bag_back`]) filters
/// these out via [`is_relic_allowed_in_shops`]; they still occupy their
/// grab-bag deque positions (so the `up_front` shuffle permutation
/// matches C#) and are only skipped at pull time.
///
/// (The Python pin `shop.py:106` only listed `TheCourier`/`OldCoin`;
/// the other three are C#-only `IsAllowedInShops=false` overrides.)
pub const SHOP_BLACKLISTED_RELICS: &[&str] = &[
    "TheCourier",
    "OldCoin",
    "LuckyFysh",
    "BowlerHat",
    "AmethystAubergine",
];

/// C# `RelicModel.IsAllowedInShops` — `true` for every grab-bag relic
/// except the five [`SHOP_BLACKLISTED_RELICS`]. Used as the pull-time
/// filter for the merchant's back-of-bag relic slots
/// (`MerchantRelicEntry.FillSlot` → `PullNextRelicFromBack(... r =>
/// r.IsAllowedInShops)`).
#[must_use]
pub const fn is_relic_allowed_in_shops(id: crate::relics::RelicId) -> bool {
    use crate::relics::RelicId::{AmethystAubergine, BowlerHat, LuckyFysh, OldCoin, TheCourier};
    !matches!(
        id,
        TheCourier | OldCoin | LuckyFysh | BowlerHat | AmethystAubergine
    )
}

/// Roll a relic rarity from the merchant pool. Mirrors Python
/// `roll_relic_rarity(rng)` (`shop.py:81-88`): 50% Common, 33% Uncommon,
/// 17% Rare. Reads one `rng.next_float()` draw — matches Python's RNG
/// advancement exactly.
pub fn roll_relic_rarity(rng: &mut crate::dotnet_rng::Rng) -> ShopRelicRarity {
    let roll = rng.next_float(1.0);
    if roll < 0.50 {
        ShopRelicRarity::Common
    } else if roll < 0.83 {
        ShopRelicRarity::Uncommon
    } else {
        ShopRelicRarity::Rare
    }
}

/// Roll a potion rarity from the merchant pool. Mirrors Python
/// `roll_potion_rarity(rng)` (`shop.py:93-100`): 10% Rare, 25% Uncommon,
/// 65% Common (Python compares with `<=`, so the boundary draws go to
/// the higher rarity — preserved here).
pub fn roll_potion_rarity(rng: &mut crate::dotnet_rng::Rng) -> ShopPotionRarity {
    let roll = rng.next_float(1.0);
    if roll <= 0.10 {
        ShopPotionRarity::Rare
    } else if roll <= 0.35 {
        ShopPotionRarity::Uncommon
    } else {
        ShopPotionRarity::Common
    }
}

/// Rolled card price. Mirrors Python `card_price(rarity, rng,
/// is_colorless, on_sale)` (`shop.py:52-60`). Advances RNG by exactly
/// one `next_float_range` draw, after the colorless multiplier is
/// applied (matches Python's order of operations).
#[must_use]
pub fn card_price(
    rarity: RewardRarity,
    rng: &mut crate::dotnet_rng::Rng,
    is_colorless: bool,
    on_sale: bool,
) -> i32 {
    let mut base = f64::from(card_base_cost(rarity));
    if is_colorless {
        // C# `MerchantCardEntry.GetCost` applies the colorless multiplier via
        // `Mathf.RoundToInt(num * 1.15f)` = `(int)Math.Round(x)`, which is
        // banker's rounding (round-half-to-even). The colorless base lands on
        // an exact `.5` (rare: 150×1.15 = 172.5 → 172, NOT 173), so half-away
        // `f64::round` diverges by one here. Match Godot/.NET `RoundToInt`.
        base = (base * COLORLESS_CARD_PRICE_MULTIPLIER).round_ties_even();
    }
    let factor = rng.next_float_range(
        MERCHANT_CARD_PRICE_VARIANCE_MIN,
        MERCHANT_CARD_PRICE_VARIANCE_MAX,
    );
    // Final cost likewise via `Mathf.RoundToInt` (banker's). For colored cards
    // the `base × random_factor` product practically never hits an exact `.5`,
    // so this only matters for determinism with the colorless base above.
    let mut cost = (base * factor).round_ties_even() as i32;
    if on_sale {
        cost /= SALE_PRICE_DIVISOR;
    }
    cost
}

/// Rolled potion price. Mirrors Python `potion_price(rarity, rng)`
/// (`shop.py:63-66`).
#[must_use]
pub fn potion_price(rarity: ShopPotionRarity, rng: &mut crate::dotnet_rng::Rng) -> i32 {
    let base = f64::from(potion_base_cost(rarity));
    let factor = rng.next_float_range(
        MERCHANT_POTION_PRICE_VARIANCE_MIN,
        MERCHANT_POTION_PRICE_VARIANCE_MAX,
    );
    // C# `MerchantPotionEntry.CalcCost` rounds via `(int)Mathf.Round(...)`,
    // which is .NET `Math.Round` = round-half-to-even (banker's), NOT
    // half-away-from-zero. Match the card path above (`round_ties_even`).
    (base * factor).round_ties_even() as i32
}

/// Rolled relic price. Mirrors Python `relic_price(merchant_cost, rng)`
/// (`shop.py:69-71`). Wider variance band than card / potion (0.85-1.15
/// vs 0.95-1.05).
#[must_use]
pub fn relic_price(rarity: ShopRelicRarity, rng: &mut crate::dotnet_rng::Rng) -> i32 {
    let base = f64::from(relic_base_cost(rarity));
    let factor = rng.next_float_range(
        MERCHANT_RELIC_PRICE_VARIANCE_MIN,
        MERCHANT_RELIC_PRICE_VARIANCE_MAX,
    );
    // C# `MerchantRelicEntry.CalcCost` rounds via `(int)Math.Round((float)...)`
    // = round-half-to-even (banker's), NOT half-away-from-zero. Match the card
    // path above (`round_ties_even`).
    (base * factor).round_ties_even() as i32
}

// ── Inventory-roll generator (bead `sts2-rs-64mq2`) ──────────────────

/// Generate a fresh merchant inventory.
///
/// Mirrors Python's `generate_shop_inventory(run_state)`
/// (`shop.py:399-455`). Slot composition + RNG advancement order match
/// the Python sequence exactly:
///
/// 1. `sale_index = rng.next_int(0, len(card_types) - 1)` — picks which
///    character-card slot rolls on sale.
/// 2. For each of the 5 character cards: one
///    `roll_without_changing_odds` draw on `card_rarity_odds` (context =
///    Shop), then [`card_price`] (one `next_float_range` draw).
/// 3. For each of the 2 colorless cards (Uncommon, Rare): one
///    [`card_price`] draw with `is_colorless = true`.
/// 4. For each of the 2 rolled relics: one [`roll_relic_rarity`] draw,
///    then [`relic_price`].
/// 5. One Shop-rarity relic: just [`relic_price`].
/// 6. For each of the 3 potions: one [`roll_potion_rarity`] draw, then
///    [`potion_price`].
/// 7. `removal_cost = card_removal_cost(removals_used)`.
///
/// ## Scoped delivery
///
/// This generator fills the **RNG-driven price + rarity** shape. Three
/// pieces are intentionally `None` and deferred to downstream
/// content-side beads (umbrella `sts2-rs-s8jgx`):
///
/// - `ShopCardEntry.card`: the concrete [`crate::cards::Card`] pulled
///   from the character / colorless card pool.
/// - `ShopRelicEntry.relic_id`: the concrete relic pulled from the
///   merchant grab-bag (filtered by `SHOP_BLACKLISTED_RELICS`).
/// - `ShopPotionEntry.potion_kind`: the concrete potion pulled from the
///   normal-pool potion list.
///
/// The merchant-price-modifier channel walk (`modify_merchant_price`)
/// is deferred to bead `sts2-rs-rhcgq`.
#[must_use]
pub fn generate_shop_inventory(
    rng: &mut crate::dotnet_rng::Rng,
    card_rarity_odds: &crate::card_rewards::CardRarityOdds,
    removals_used: u32,
) -> ShopInventory {
    let mut inv = ShopInventory::empty();

    // Step 1: sale index. Python does `rng.next_int(0, len - 1)`
    // (inclusive on both ends, per the `_INCLUSIVE` semantics).
    let sale_index_high = (MERCHANT_CHARACTER_CARD_TYPES.len() - 1) as i32;
    let sale_index = rng.next_int(0, sale_index_high) as usize;

    // Step 2: character cards.
    for (i, &card_type) in MERCHANT_CHARACTER_CARD_TYPES.iter().enumerate() {
        let rarity = card_rarity_odds
            .roll_without_changing_odds_with_dotnet(rng, crate::card_rewards::RarityContext::Shop);
        let on_sale = i == sale_index;
        let price = card_price(rarity, rng, false, on_sale);
        inv.cards.push(ShopCardEntry {
            rarity,
            card_type,
            card: None,
            is_colorless: false,
            on_sale,
            price,
        });
    }

    // Step 3: colorless cards (one Uncommon, one Rare).
    for &rarity in &MERCHANT_COLORLESS_CARD_RARITIES {
        let price = card_price(rarity, rng, true, false);
        inv.colorless_cards.push(ShopCardEntry {
            rarity,
            // Python uses card_type = "Colorless" — the closest enum
            // bucket on the Rust side is `CardType::Status` (used as a
            // sentinel for "no character type"); the colorless flag
            // disambiguates. The downstream content-side roller
            // overwrites this when it fills `card`.
            card_type: crate::cards::CardType::Status,
            card: None,
            is_colorless: true,
            on_sale: false,
            price,
        });
    }

    // Step 4: rolled-rarity relics (2 slots).
    for _ in 0..MERCHANT_ROLLED_RELIC_COUNT {
        let relic_rarity = roll_relic_rarity(rng);
        let price = relic_price(relic_rarity, rng);
        inv.relics.push(ShopRelicEntry {
            relic_rarity,
            relic_id: None,
            price,
        });
    }
    // Step 5: shop-pool relic.
    let shop_relic_price = relic_price(ShopRelicRarity::Shop, rng);
    inv.relics.push(ShopRelicEntry {
        relic_rarity: ShopRelicRarity::Shop,
        relic_id: None,
        price: shop_relic_price,
    });

    // Step 6: potions.
    for _ in 0..MERCHANT_POTION_COUNT {
        let potion_rarity = roll_potion_rarity(rng);
        let price = potion_price(potion_rarity, rng);
        inv.potions.push(ShopPotionEntry {
            potion_rarity,
            potion_kind: None,
            price,
        });
    }

    // Step 7: card-removal slot.
    inv.removal_cost = card_removal_cost(removals_used, card_rarity_odds.ascension_level);
    inv.removal_used = false;

    inv
}

// ── Remove-card slot purchase (Python `run_manager.py:1557-1601, 1686-1694`) ──

/// Outcome of a [`purchase_card_removal`] attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurchaseCardRemovalResult {
    /// Removal succeeded. Carries the [`crate::Card`] that was popped
    /// out of `run_state.master_deck` and the gold actually debited
    /// (matches [`crate::RunState::gold`] decrement; saturates per
    /// [`crate::RunState::gain_gold`]'s contract).
    Removed { card: crate::Card, gold_spent: i32 },
    /// The slot was already consumed this shop visit
    /// (`inv.removal_used == true`).
    AlreadyUsed,
    /// The player can't afford `inv.removal_cost`.
    InsufficientGold,
    /// `deck_idx` is past the end of `run_state.master_deck`.
    InvalidDeckIndex,
    /// `master_deck[deck_idx]` carries the Eternal keyword, so C#
    /// `CardModel.IsRemovable` (`CardModel.cs:738`) refuses it. Basics are
    /// NOT in this arm: the shop removes them like anything else. An earlier
    /// revision of this comment claimed the opposite and cited Python
    /// `Card.is_removable`; the basics-excluding predicate it named is gone
    /// (2026-08-26), having cost every deployed champion the standard
    /// Strike-thinning line. Bead `sts2-rs-hrz2m`.
    NotRemovable,
}

/// Purchase the remove-card slot: debit gold, pop the chosen
/// `master_deck[deck_idx]`, bump the run's `card_shop_removals_used`
/// counter, mark the slot used, and re-price `inv.removal_cost` for
/// the next would-be use.
///
/// Mirrors Python
/// `RunManager._do_shop.remove_card` + `_finalize_shop_card_removal`
/// (`run_manager.py:1557-1601, 1686-1694`). The Python flow opens
/// an interactive `RemoveCardReward` choice; this Rust surface takes
/// the resolved index directly, deferring the choice/UI plumbing to
/// the downstream run-action layer.
///
/// Beads `sts2-rs-jbqw4` (P9.2.c.4) + `sts2-rs-hrz2m` (added
/// `NotRemovable` defense-in-depth — an Eternal card at `deck_idx`
/// rejects with [`PurchaseCardRemovalResult::NotRemovable`] before
/// any gold/state mutation).
/// The action surface in
/// [`crate::run_manager::RunManager::get_available_actions`] pre-filters
/// the enumeration as well so well-behaved callers never hit this arm.
pub fn purchase_card_removal(
    run_state: &mut crate::RunState,
    inv: &mut ShopInventory,
    deck_idx: usize,
) -> PurchaseCardRemovalResult {
    if inv.removal_used {
        return PurchaseCardRemovalResult::AlreadyUsed;
    }
    // `MerchantCardRemovalEntry : MerchantEntry`, so its `.Cost` getter also
    // folds `Hook.ModifyMerchantPrice` (Courier / MembershipCard) over the
    // rolled base; `inv.removal_cost` stays the undiscounted base.
    let cost = merchant_price_with_modifiers(&run_state.active_relics(), inv.removal_cost);
    if run_state.gold < cost {
        return PurchaseCardRemovalResult::InsufficientGold;
    }
    if deck_idx >= run_state.master_deck.len() {
        return PurchaseCardRemovalResult::InvalidDeckIndex;
    }
    // v0.107.1: the shop uses the generic `CardModel.IsRemovable` gate —
    // `!Keywords.Contains(CardKeyword.Eternal)` and nothing else, so basics
    // are fair game and only Eternal cards (AscendersBane, a
    // TezcatarasEmber'd Strike) are refused. Mirrored via the
    // content-registered predicate; absent registry (pure-core tests)
    // everything is removable.
    let removable = crate::lookup_card_removable();
    if !removable.map_or(true, |f| f(&run_state.master_deck[deck_idx])) {
        return PurchaseCardRemovalResult::NotRemovable;
    }
    let gold_before = run_state.gold;
    run_state.gain_gold(-cost);
    let gold_spent = gold_before - run_state.gold;
    let card = run_state.master_deck.remove(deck_idx);
    run_state.card_shop_removals_used = run_state.card_shop_removals_used.saturating_add(1);
    inv.removal_used = true;
    inv.removal_cost =
        card_removal_cost(run_state.card_shop_removals_used, run_state.ascension_level);
    PurchaseCardRemovalResult::Removed { card, gold_spent }
}

/// Identifies a single merchant slot for a post-purchase restock
/// ([`crate::run_manager::ContentCallbacks::restock_shop_entry`]).
///
/// The `Card` index is over the concatenated `cards ++ colorless_cards`
/// rows — the SAME index space [`buy_card`] takes — so
/// `do_shop_buy_card` can hand the just-bought index straight to the
/// restock callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShopSlot {
    /// A card slot (unified character-then-colorless index).
    Card(usize),
    /// A relic slot.
    Relic(usize),
    /// A potion slot.
    Potion(usize),
}

// ── Purchase actions (bead `sts2-rs-rhcgq`, P9.2.c.3) ────────────────

/// Outcome of a [`buy_card`] / [`buy_relic`] / [`buy_potion`] attempt.
///
/// Mirrors the descriptive branches Python `_buy_item` returns
/// (`sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:1496-1555`);
/// the Rust port collapses the dict-with-`"description"` form into a
/// typed enum so callers can match without string comparison.
/// `SoldOut` is surfaced separately from `CannotAfford` because the
/// [`SHOP_ENTRY_SOLD_OUT_PRICE`] sentinel is the only legal way an
/// in-bounds purchase can fail for a solvent player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShopPurchaseOutcome {
    /// Gold deducted, item moved onto the player, slot priced
    /// [`SHOP_ENTRY_SOLD_OUT_PRICE`].
    Bought,
    /// Player gold below entry price.
    CannotAfford,
    /// Index out of bounds for the relevant slot row.
    IndexOutOfBounds,
    /// Slot already sold (price `>= SHOP_ENTRY_SOLD_OUT_PRICE`).
    SoldOut,
    /// Inventory roll left this slot's concrete payload `None`
    /// (no `card` / `relic_id` / `potion_kind` filled). Mirrors the
    /// Python branch where the entry has no item to move — Python's
    /// 1496-1555 block silently no-ops on the deck push but still
    /// deducts gold; the Rust port refuses the buy to prevent
    /// gold-burn on a half-rolled inventory.
    SlotEmpty,
    /// Potion buy refused because the belt cannot hold it — either no
    /// free slot (C# `PotionProcureFailureReason.Space`) or a procure
    /// veto such as Sozu (`PotionProcureFailureReason.NotAllowed`). C#
    /// `MerchantPotionEntry.OnTryPurchase` runs `TryToProcure` BEFORE
    /// `LoseGold`, so a failed procure costs the player nothing and the
    /// slot stays stocked. No gold is deducted; the slot is not marked
    /// sold-out.
    PotionBeltFull,
}

/// Add a shop-bought card to the master deck the way C#
/// `CardPileCmd.Add(_, PileType.Deck)` does — firing BOTH deck-add hooks
/// that every merchant purchase (`MerchantCardEntry.OnTryPurchase`) routes
/// through:
///
/// * `Hook.ModifyCardBeingAddedToDeck` **pre-add** (`CardPileCmd.cs:502`) —
///   egg relics (`MoltenEgg`/`FrozenEgg`/`ToxicEgg`) upgrade a shop-bought
///   Attack/Skill/Power in place; `FresnelLens` enchants.
/// * `Hook.AfterCardChangedPiles` **post-add** (`CardPileCmd.cs:636/684`) —
///   `LuckyFysh.cs` grants `GoldVar(15)` per card that lands in the Deck.
///
/// `source` is [`crate::relic_effects::AddSource::ShopPurchase`]. Both hooks
/// are RunState-level and consume NEITHER the `Shops` nor `Rewards` RNG
/// stream, so merchant-inventory RNG alignment is unaffected. Shared by the
/// interactive [`buy_card`] and the `LordsParasol` [`auto_purchase_shop_inventory`]
/// path so the two deck-add sites are byte-identical.
fn push_shop_card_to_deck(run_state: &mut crate::runstate::RunState, mut card: crate::cards::Card) {
    run_state.modify_card_being_added_to_deck(&mut card);
    let ctx = crate::run_effects::CardAddedRunCtx {
        card_id: card.card_id,
        card_type: card.card_type,
        source: crate::relic_effects::AddSource::ShopPurchase,
    };
    run_state.master_deck.push(card);
    let _ = crate::run_effects::dispatch_on_card_added_to_deck(run_state, &ctx);
}

/// Buy from the combined character + colorless card row.
///
/// Mirrors Python `_buy_item("buy_card", index)`
/// (`sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:1496-1512`).
///
/// `index` is over `inv.cards + inv.colorless_cards`:
/// `0..cards.len()` addresses the 5-slot character row;
/// `cards.len()..cards.len()+colorless_cards.len()` addresses the
/// 2-slot colorless row. Matches Python's `all_cards = list(inv.cards)
/// + list(inv.colorless_cards)` concatenation order.
///
/// On success: deducts `entry.price` gold via
/// [`crate::runstate::RunState::gain_gold`], clones the entry's
/// [`crate::cards::Card`] onto `run_state.master_deck`, and sets the
/// entry's price to [`SHOP_ENTRY_SOLD_OUT_PRICE`].
///
/// The `_handle_post_shop_purchase` relic-side hook walk
/// (`run_manager.py:1696-1720` — `on_item_purchased` listeners +
/// `refill_shop_entry`) is intentionally deferred: this primitive
/// owns the gold + slot mutations only; the listener channel will be
/// layered on top in a follow-up bead (no listeners exist at the pin
/// today, so the deferral is observationally inert).
pub fn buy_card(
    run_state: &mut crate::runstate::RunState,
    inv: &mut ShopInventory,
    index: usize,
) -> ShopPurchaseOutcome {
    let character_len = inv.cards.len();
    let total = character_len + inv.colorless_cards.len();
    if index >= total {
        return ShopPurchaseOutcome::IndexOutOfBounds;
    }
    let entry = if index < character_len {
        &inv.cards[index]
    } else {
        &inv.colorless_cards[index - character_len]
    };
    if !is_shop_entry_available(entry.price) {
        return ShopPurchaseOutcome::SoldOut;
    }
    // C# `MerchantEntry.Cost` applies `Hook.ModifyMerchantPrice` (Courier /
    // MembershipCard) to the rolled base before both the affordability check
    // and `LoseGold`; the rolled base (`entry.price`) stays undiscounted so
    // re-reads and the sold-out mark are stable.
    let price = merchant_price_with_modifiers(&run_state.active_relics(), entry.price);
    if run_state.gold < price {
        return ShopPurchaseOutcome::CannotAfford;
    }
    let Some(card) = entry.card else {
        return ShopPurchaseOutcome::SlotEmpty;
    };
    // C# `MerchantCardEntry.OnTryPurchase` (v0.107.1:130) buys via
    // `CardPileCmd.Add(CreationResult.Card, PileType.Deck)` FIRST (firing both
    // deck-add hooks — see [`push_shop_card_to_deck`]), then
    // `PlayerCmd.LoseGold(Cost)`. Keep that order so `LuckyFysh`'s +15g lands
    // before the price debit (net-identical, but faithful).
    push_shop_card_to_deck(run_state, card);
    run_state.gain_gold(-price);
    let entry = if index < character_len {
        &mut inv.cards[index]
    } else {
        &mut inv.colorless_cards[index - character_len]
    };
    entry.price = SHOP_ENTRY_SOLD_OUT_PRICE;
    let _ = crate::run_effects::dispatch_on_item_purchased(run_state, price);
    ShopPurchaseOutcome::Bought
}

/// Buy from the relic row.
///
/// Mirrors Python `_buy_item("buy_relic", index)`
/// (`sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:1514-1540`).
///
/// On success: deducts `entry.price` gold, pushes `entry.relic_id`
/// onto `run_state.relics`, fires the `after_obtained` channel
/// (mirroring [`crate::runstate::RunState::accept_relic_reward`]'s
/// post-acquire dispatch — same listener surface, no separate
/// shop-relic hook channel exists at the pin), sets the entry's price
/// to [`SHOP_ENTRY_SOLD_OUT_PRICE`].
///
/// Python's `_deferred_followup_rewards` context manager + the
/// `pending_choice` / `pending_rewards` resume-phase plumbing are
/// orchestration-layer concerns deferred to the umbrella `s8jgx`
/// integration step; this primitive does the gold/slot/listener
/// surface only.
pub fn buy_relic(
    run_state: &mut crate::runstate::RunState,
    inv: &mut ShopInventory,
    index: usize,
) -> ShopPurchaseOutcome {
    match buy_relic_before_purchase_hook(run_state, inv, index) {
        Ok(price) => {
            let _ = crate::run_effects::dispatch_on_item_purchased(run_state, price);
            ShopPurchaseOutcome::Bought
        }
        Err(outcome) => outcome,
    }
}

/// [`buy_relic`] up to, not including, the `on_item_purchased` hook: `Ok(gold
/// spent)` once the relic is obtained, for a caller that must fire the hook
/// later (sts2-agent fork: after the reward sets the relic offered are done,
/// C# `MerchantEntry.OnTryPurchaseWrapper`).
pub fn buy_relic_before_purchase_hook(
    run_state: &mut crate::runstate::RunState,
    inv: &mut ShopInventory,
    index: usize,
) -> Result<i32, ShopPurchaseOutcome> {
    if index >= inv.relics.len() {
        return Err(ShopPurchaseOutcome::IndexOutOfBounds);
    }
    let entry = inv.relics[index];
    if !is_shop_entry_available(entry.price) {
        return Err(ShopPurchaseOutcome::SoldOut);
    }
    let price = merchant_price_with_modifiers(&run_state.active_relics(), entry.price);
    if run_state.gold < price {
        return Err(ShopPurchaseOutcome::CannotAfford);
    }
    let Some(relic_id) = entry.relic_id else {
        return Err(ShopPurchaseOutcome::SlotEmpty);
    };
    run_state.gain_gold(-price);
    run_state.relics.push(relic_id);
    run_state.last_obtained_relic = Some(relic_id);
    let _ = crate::run_effects::dispatch_after_obtained(run_state);
    run_state.last_obtained_relic = None;
    inv.relics[index].price = SHOP_ENTRY_SOLD_OUT_PRICE;
    Ok(price)
}

/// Buy from the potion row.
///
/// Mirrors Python `_buy_item("buy_potion", index)`
/// (`sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:1542-1555`).
///
/// On success: deducts `entry.price` gold, routes the potion through
/// [`crate::runstate::RunState::procure_potion`] (which fires the
/// `should_procure_potion` veto channel — Sozu et al. — and pushes a
/// [`crate::runstate::Reward::Potion`] onto `pending_rewards` when the
/// channel allows), and sets the entry's price to
/// [`SHOP_ENTRY_SOLD_OUT_PRICE`].
///
/// The slot is consumed and gold is spent regardless of the veto's
/// outcome — matches Python's branch ordering, where gold is debited
/// before `procure_potion` is invoked.
pub fn buy_potion(
    run_state: &mut crate::runstate::RunState,
    inv: &mut ShopInventory,
    index: usize,
) -> ShopPurchaseOutcome {
    if index >= inv.potions.len() {
        return ShopPurchaseOutcome::IndexOutOfBounds;
    }
    let entry = inv.potions[index];
    if !is_shop_entry_available(entry.price) {
        return ShopPurchaseOutcome::SoldOut;
    }
    let price = merchant_price_with_modifiers(&run_state.active_relics(), entry.price);
    if run_state.gold < price {
        return ShopPurchaseOutcome::CannotAfford;
    }
    let Some(potion_kind) = entry.potion_kind else {
        return ShopPurchaseOutcome::SlotEmpty;
    };
    // C# `MerchantPotionEntry.OnTryPurchase` calls `PotionCmd.TryToProcure`
    // BEFORE `PlayerCmd.LoseGold`: a full belt (FailureSpace) or a procure
    // veto (FailureForbidden, e.g. Sozu) fails the purchase with NO gold
    // spent and leaves the slot stocked. The old order deducted gold first
    // and then silently dropped the overflow potion at the `pending_rewards`
    // drain, burning gold for nothing (full-run parity miss: a full-belt
    // shop visit left Rust 75 gold poorer than C#). `procure_potion` only
    // queues onto `pending_rewards` (the belt-cap drop happens later at
    // drain), so the capacity gate must be checked explicitly here, up
    // front, mirroring `add_potion_to_belt`'s ceiling.
    let cap = (run_state.max_potion_slots.max(0) as usize).min(crate::potions::MAX_POTIONS);
    if run_state.potions.len() >= cap {
        return ShopPurchaseOutcome::PotionBeltFull;
    }
    if !run_state.procure_potion(crate::runstate::PotionReward::with_kind(potion_kind)) {
        // `should_procure_potion` veto (Sozu) — C# FailureForbidden, no gold.
        return ShopPurchaseOutcome::PotionBeltFull;
    }
    run_state.gain_gold(-price);
    inv.potions[index].price = SHOP_ENTRY_SOLD_OUT_PRICE;
    let _ = crate::run_effects::dispatch_on_item_purchased(run_state, price);
    ShopPurchaseOutcome::Bought
}

// ── Free auto-purchase (bead `sts2-rs-apy0a`) ────────────────────────

/// Walk every available entry in `inv` and apply its effect for free —
/// no gold deduction, no affordability check.
///
/// Mirrors Python `RunManager._auto_purchase_shop_inventory`
/// (`sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:697-737`),
/// which the `LordsParasol` Ancient relic invokes from its
/// `after_room_entered` listener whenever the player enters a merchant
/// room (`relics/shop_event.py:2343-2355`).
///
/// Side-effects per row (Python order; the card/relic/potion effect
/// hooks below advance no `Shops`/`Rewards` RNG — this stays a
/// deterministic post-roll walk):
///
/// 1. **Card row** (character + colorless): for each entry with
///    `price < SHOP_ENTRY_SOLD_OUT_PRICE`, add its
///    [`crate::cards::Card`] to [`crate::runstate::RunState::master_deck`]
///    via [`push_shop_card_to_deck`] (C# `OnTryPurchaseWrapper(ignoreCost:
///    true)` → `CardPileCmd.Add(Deck)`, firing the egg-upgrade + `LuckyFysh`
///    deck-add hooks) and mark the slot sold-out. Empty `card` slots are
///    skipped silently (matches Python's no-op on missing `entry.card`).
///    NOTE: the `TheCourier` `RestockAfterPurchase` re-roll is NOT applied
///    here — a Courier+LordsParasol owner won't re-stock auto-bought slots
///    (a doubly-niche relic combo, and restock would perturb the `Shops`/
///    `Rewards` streams); the interactive [`buy_card`] path does restock.
/// 2. **Relic row**: push `relic_id` onto
///    [`crate::runstate::RunState::relics`], fire the `after_obtained`
///    channel (same listener surface as [`buy_relic`] and
///    [`crate::runstate::RunState::accept_relic_reward`]), and mark
///    the slot sold-out. Python wraps each relic in a
///    `_deferred_followup_rewards()` context manager to batch the
///    follow-up reward fan-out; the Rust port doesn't have that
///    pending-rewards driver yet (run-loop layer), so per-relic
///    dispatch matches `buy_relic`'s shape exactly. Empty `relic_id`
///    slots are skipped.
/// 3. **Potion row**: route each available potion through
///    [`crate::runstate::RunState::procure_potion`] so the
///    `should_procure_potion` veto channel (Sozu et al.) still applies
///    and the potion lands as a [`crate::runstate::Reward::Potion`]
///    push on `pending_rewards`. Mark the slot sold-out regardless of
///    the veto's outcome (matches Python's `gold_spent=0` branch — the
///    inventory slot is consumed even when the veto refuses).
///
/// The free remove-card branch (`_finalize_shop_card_removal` —
/// Python lines 722-737) is **not** invoked here: it depends on the
/// interactive `RemoveCardReward` choice primitive + the ascension
/// gate (`_should_allow_merchant_card_removal`), both of which live in
/// the run-loop driver layer. A follow-up bead will add it once that
/// layer materializes.
///
/// No `on_item_purchased` / `refill_shop_entry` post-hooks fire (matches
/// [`buy_card`] / [`buy_relic`] / [`buy_potion`] — the
/// `_handle_post_shop_purchase` walk is deferred crate-wide).
pub fn auto_purchase_shop_inventory(
    run_state: &mut crate::runstate::RunState,
    inv: &mut ShopInventory,
) {
    // C# `LordsParasol.PurchaseEverything` buys each stocked card via
    // `OnTryPurchaseWrapper(ignoreCost: true)` → `CardPileCmd.Add(Deck)`, which
    // fires the SAME two deck-add hooks as an interactive buy: egg upgrade pre-add
    // and `LuckyFysh`'s +15g post-add. The gold gain fires even though `ignoreCost`
    // skips the price debit (the hooks live inside `Add`, not the `LoseGold` branch).
    for i in 0..inv.cards.len() {
        if !is_shop_entry_available(inv.cards[i].price) {
            continue;
        }
        if let Some(card) = inv.cards[i].card {
            push_shop_card_to_deck(run_state, card);
        }
        inv.cards[i].price = SHOP_ENTRY_SOLD_OUT_PRICE;
    }
    for i in 0..inv.colorless_cards.len() {
        if !is_shop_entry_available(inv.colorless_cards[i].price) {
            continue;
        }
        if let Some(card) = inv.colorless_cards[i].card {
            push_shop_card_to_deck(run_state, card);
        }
        inv.colorless_cards[i].price = SHOP_ENTRY_SOLD_OUT_PRICE;
    }
    for i in 0..inv.relics.len() {
        if !is_shop_entry_available(inv.relics[i].price) {
            continue;
        }
        if let Some(relic_id) = inv.relics[i].relic_id {
            run_state.relics.push(relic_id);
            run_state.last_obtained_relic = Some(relic_id);
            let _ = crate::run_effects::dispatch_after_obtained(run_state);
            run_state.last_obtained_relic = None;
        }
        inv.relics[i].price = SHOP_ENTRY_SOLD_OUT_PRICE;
    }
    for i in 0..inv.potions.len() {
        if !is_shop_entry_available(inv.potions[i].price) {
            continue;
        }
        if let Some(potion_kind) = inv.potions[i].potion_kind {
            let _ = run_state.procure_potion(crate::runstate::PotionReward::with_kind(potion_kind));
        }
        inv.potions[i].price = SHOP_ENTRY_SOLD_OUT_PRICE;
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn card_base_cost_matches_python_table() {
        assert_eq!(card_base_cost(RewardRarity::Rare), 150);
        assert_eq!(card_base_cost(RewardRarity::Uncommon), 75);
        assert_eq!(card_base_cost(RewardRarity::Common), 50);
    }

    #[test]
    fn potion_base_cost_matches_python_table() {
        assert_eq!(potion_base_cost(ShopPotionRarity::Rare), 100);
        assert_eq!(potion_base_cost(ShopPotionRarity::Uncommon), 75);
        assert_eq!(potion_base_cost(ShopPotionRarity::Common), 50);
    }

    #[test]
    fn relic_base_cost_matches_python_table() {
        assert_eq!(relic_base_cost(ShopRelicRarity::Common), 150);
        assert_eq!(relic_base_cost(ShopRelicRarity::Uncommon), 250);
        assert_eq!(relic_base_cost(ShopRelicRarity::Rare), 300);
        assert_eq!(relic_base_cost(ShopRelicRarity::Shop), 150);
    }

    #[test]
    fn card_removal_cost_grows_by_25_per_use() {
        assert_eq!(card_removal_cost(0, 0), 75);
        assert_eq!(card_removal_cost(1, 0), 100);
        assert_eq!(card_removal_cost(2, 0), 125);
        assert_eq!(card_removal_cost(7, 0), 250);
    }

    #[test]
    fn card_removal_cost_inflation_at_ascension_6() {
        // A6 Inflation: base 75→100, increment 25→50.
        // Below A6 is unchanged; the threshold is exactly 6.
        assert_eq!(card_removal_cost(0, 5), 75, "A5 still base pricing");
        assert_eq!(card_removal_cost(0, 6), 100, "A6 base 75→100");
        assert_eq!(card_removal_cost(1, 6), 150, "A6 first increment 25→50");
        assert_eq!(card_removal_cost(2, 6), 200);
        assert_eq!(card_removal_cost(0, 20), 100, "A20 still A6 pricing");
    }

    #[test]
    fn merchant_price_matches_csharp_truncation() {
        use crate::relics::RelicId;
        // No modifier relics → unchanged.
        assert_eq!(merchant_price_with_modifiers(&[], 143), 143);
        // Courier ×0.8, C# `(int)(143 * 0.8m)` = (int)114.4 = 114 (NOT the
        // old `143 - 143*20/100` = 115 floor-of-discount formula).
        assert_eq!(
            merchant_price_with_modifiers(&[RelicId::TheCourier], 143),
            114
        );
        assert_eq!(
            merchant_price_with_modifiers(&[RelicId::TheCourier], 175),
            140
        );
        // MembershipCard ×0.5, C# `(int)(75 * 0.5m)` = (int)37.5 = 37 (NOT
        // half-up 38).
        assert_eq!(
            merchant_price_with_modifiers(&[RelicId::MembershipCard], 75),
            37
        );
        // Both owned: `(int)(143 * 0.4m)` = (int)57.2 = 57. Fold is
        // order-independent and truncates once.
        let both = [RelicId::TheCourier, RelicId::MembershipCard];
        assert_eq!(merchant_price_with_modifiers(&both, 143), 57);
        assert_eq!(
            merchant_price_with_modifiers(&[RelicId::MembershipCard, RelicId::TheCourier], 143),
            57
        );
        // An unrelated relic doesn't discount.
        assert_eq!(merchant_price_with_modifiers(&[RelicId::Anchor], 143), 143);
    }

    #[test]
    fn merchant_price_leaves_sold_out_sentinel_untouched() {
        use crate::relics::RelicId;
        // A discounted sentinel must never drop below the sold-out
        // threshold (else a sold slot would read as available).
        assert_eq!(
            merchant_price_with_modifiers(&[RelicId::TheCourier], SHOP_ENTRY_SOLD_OUT_PRICE),
            SHOP_ENTRY_SOLD_OUT_PRICE
        );
        assert!(!is_shop_entry_available(merchant_price_with_modifiers(
            &[RelicId::TheCourier],
            SHOP_ENTRY_SOLD_OUT_PRICE
        )));
        // Zero / negative pass through.
        assert_eq!(merchant_price_with_modifiers(&[RelicId::TheCourier], 0), 0);
    }

    #[test]
    fn is_shop_entry_available_uses_sold_out_sentinel() {
        assert!(is_shop_entry_available(0));
        assert!(is_shop_entry_available(75));
        assert!(is_shop_entry_available(SHOP_ENTRY_SOLD_OUT_PRICE - 1));
        assert!(!is_shop_entry_available(SHOP_ENTRY_SOLD_OUT_PRICE));
        assert!(!is_shop_entry_available(SHOP_ENTRY_SOLD_OUT_PRICE + 100));
    }

    #[test]
    fn shop_inventory_empty_defaults() {
        let inv = ShopInventory::empty();
        assert!(inv.cards.is_empty());
        assert!(inv.colorless_cards.is_empty());
        assert!(inv.relics.is_empty());
        assert!(inv.potions.is_empty());
        assert_eq!(inv.removal_cost, CARD_REMOVAL_BASE_COST);
        assert!(!inv.removal_used);
    }

    #[test]
    fn merchant_character_card_types_matches_python_template() {
        use crate::cards::CardType;
        // Python `("Attack", "Attack", "Skill", "Skill", "Power")`.
        assert_eq!(
            MERCHANT_CHARACTER_CARD_TYPES,
            [
                CardType::Attack,
                CardType::Attack,
                CardType::Skill,
                CardType::Skill,
                CardType::Power,
            ]
        );
    }

    #[test]
    fn merchant_colorless_card_rarities_matches_python_template() {
        assert_eq!(
            MERCHANT_COLORLESS_CARD_RARITIES,
            [RewardRarity::Uncommon, RewardRarity::Rare]
        );
    }

    #[test]
    fn merchant_slot_counts_match_python_constants() {
        assert_eq!(MERCHANT_ROLLED_RELIC_COUNT, 2);
        assert_eq!(MERCHANT_POTION_COUNT, 3);
    }

    // ── Rolls + generator (bead `sts2-rs-64mq2`) ─────────────────────

    use crate::card_rewards::CardRarityOdds;
    use crate::dotnet_rng::Rng;

    #[test]
    fn roll_relic_rarity_covers_all_three_buckets() {
        let mut rng = Rng::new(0);
        let mut counts = [0_i32; 3];
        for _ in 0..2000 {
            match roll_relic_rarity(&mut rng) {
                ShopRelicRarity::Common => counts[0] += 1,
                ShopRelicRarity::Uncommon => counts[1] += 1,
                ShopRelicRarity::Rare => counts[2] += 1,
                ShopRelicRarity::Shop => unreachable!("Shop is not a rolled rarity"),
            }
        }
        // Loose distribution check: ~50/33/17. Allow ±10pp slack.
        assert!(
            counts[0] > 800 && counts[0] < 1200,
            "common count {}",
            counts[0]
        );
        assert!(
            counts[1] > 460 && counts[1] < 860,
            "uncommon count {}",
            counts[1]
        );
        assert!(
            counts[2] > 140 && counts[2] < 540,
            "rare count {}",
            counts[2]
        );
    }

    #[test]
    fn roll_potion_rarity_covers_all_three_buckets() {
        let mut rng = Rng::new(1);
        let mut counts = [0_i32; 3];
        for _ in 0..2000 {
            match roll_potion_rarity(&mut rng) {
                ShopPotionRarity::Common => counts[0] += 1,
                ShopPotionRarity::Uncommon => counts[1] += 1,
                ShopPotionRarity::Rare => counts[2] += 1,
            }
        }
        // Loose distribution check: ~65/25/10. Allow ±10pp slack.
        assert!(
            counts[0] > 1100 && counts[0] < 1500,
            "common count {}",
            counts[0]
        );
        assert!(
            counts[1] > 300 && counts[1] < 700,
            "uncommon count {}",
            counts[1]
        );
        assert!(
            counts[2] > 100 && counts[2] < 300,
            "rare count {}",
            counts[2]
        );
    }

    #[test]
    fn card_price_uses_base_table_within_variance_band() {
        let mut rng = Rng::new(0);
        for _ in 0..200 {
            let p = card_price(RewardRarity::Rare, &mut rng, false, false);
            // Base 150, variance 0.95-1.05 → 143..157 (rounded).
            assert!((142..=158).contains(&p), "rare price {p}");
        }
    }

    #[test]
    fn card_price_halves_on_sale() {
        let mut rng_a = Rng::new(7);
        let mut rng_b = Rng::new(7);
        let normal = card_price(RewardRarity::Uncommon, &mut rng_a, false, false);
        let on_sale = card_price(RewardRarity::Uncommon, &mut rng_b, false, true);
        // Same seed → same factor draw; on-sale path divides by 2.
        assert_eq!(on_sale, normal / SALE_PRICE_DIVISOR);
    }

    #[test]
    fn card_price_applies_colorless_multiplier() {
        let mut rng_a = Rng::new(11);
        let mut rng_b = Rng::new(11);
        let normal = card_price(RewardRarity::Rare, &mut rng_a, false, false);
        let colorless = card_price(RewardRarity::Rare, &mut rng_b, true, false);
        // Colorless base = round(150 * 1.15) = 173 → variance applied;
        // factor draw is the same on both calls.
        assert!(
            colorless > normal,
            "colorless {colorless} > normal {normal}"
        );
    }

    #[test]
    fn potion_price_within_variance_band() {
        let mut rng = Rng::new(0);
        for _ in 0..200 {
            let p = potion_price(ShopPotionRarity::Rare, &mut rng);
            // Base 100, variance 0.95-1.05 → 95..105.
            assert!((94..=106).contains(&p), "potion rare price {p}");
        }
    }

    #[test]
    fn relic_price_wider_variance_than_card() {
        let mut rng = Rng::new(0);
        for _ in 0..200 {
            let p = relic_price(ShopRelicRarity::Rare, &mut rng);
            // Base 300, variance 0.85-1.15 → 255..345.
            assert!((254..=346).contains(&p), "relic rare price {p}");
        }
    }

    #[test]
    fn generate_shop_inventory_fills_canonical_shape() {
        let mut rng = Rng::with_name(0, "shops");
        let odds = CardRarityOdds::default();

        let inv = generate_shop_inventory(&mut rng, &odds, 0);

        // 5 character cards, 2 colorless cards, 3 relics (2 rolled + 1 shop),
        // 3 potions.
        assert_eq!(inv.cards.len(), 5);
        assert_eq!(inv.colorless_cards.len(), 2);
        assert_eq!(inv.relics.len(), 3);
        assert_eq!(inv.potions.len(), 3);

        // Character row matches the Python template.
        for (i, &expected_type) in MERCHANT_CHARACTER_CARD_TYPES.iter().enumerate() {
            assert_eq!(inv.cards[i].card_type, expected_type);
            assert!(!inv.cards[i].is_colorless);
            assert!(inv.cards[i].price > 0);
        }
        // Exactly one slot is on sale.
        let on_sale_count = inv.cards.iter().filter(|c| c.on_sale).count();
        assert_eq!(on_sale_count, 1);

        // Colorless row: one Uncommon, one Rare; both flagged colorless.
        assert_eq!(inv.colorless_cards[0].rarity, RewardRarity::Uncommon);
        assert_eq!(inv.colorless_cards[1].rarity, RewardRarity::Rare);
        assert!(inv.colorless_cards.iter().all(|c| c.is_colorless));

        // The 3rd relic slot is always Shop-rarity (Python contract).
        assert_eq!(inv.relics[2].relic_rarity, ShopRelicRarity::Shop);
        assert!(inv.relics.iter().all(|r| r.relic_id.is_none()));
        assert!(inv.potions.iter().all(|p| p.potion_kind.is_none()));
        assert!(inv.cards.iter().all(|c| c.card.is_none()));

        // Removal cost initialised from base.
        assert_eq!(inv.removal_cost, CARD_REMOVAL_BASE_COST);
        assert!(!inv.removal_used);
    }

    #[test]
    fn generate_shop_inventory_removal_cost_scales_with_uses() {
        let mut rng = Rng::with_name(42, "shops");
        let odds = CardRarityOdds::default();
        let inv = generate_shop_inventory(&mut rng, &odds, 3);
        // 75 + 25 * 3 = 150.
        assert_eq!(inv.removal_cost, 150);
    }

    #[test]
    fn generate_shop_inventory_is_deterministic_for_seed() {
        let odds = CardRarityOdds::default();
        let mut rng_a = Rng::with_name(123, "shops");
        let mut rng_b = Rng::with_name(123, "shops");
        let inv_a = generate_shop_inventory(&mut rng_a, &odds, 0);
        let inv_b = generate_shop_inventory(&mut rng_b, &odds, 0);
        assert_eq!(inv_a, inv_b);
    }

    #[test]
    fn shop_blacklisted_relics_matches_csharp_is_allowed_in_shops() {
        // Authoritative source: the five C# relics overriding
        // `RelicModel.IsAllowedInShops => false`. (The Python pin only
        // listed the first two; the other three are C#-only overrides.)
        let set: std::collections::BTreeSet<&str> =
            SHOP_BLACKLISTED_RELICS.iter().copied().collect();
        for id in [
            "TheCourier",
            "OldCoin",
            "LuckyFysh",
            "BowlerHat",
            "AmethystAubergine",
        ] {
            assert!(set.contains(id), "missing blacklisted relic {id}");
        }
        assert_eq!(set.len(), 5);

        // `is_relic_allowed_in_shops` is the typed mirror of the same set.
        use crate::relics::RelicId;
        assert!(!is_relic_allowed_in_shops(RelicId::TheCourier));
        assert!(!is_relic_allowed_in_shops(RelicId::OldCoin));
        assert!(!is_relic_allowed_in_shops(RelicId::LuckyFysh));
        assert!(!is_relic_allowed_in_shops(RelicId::BowlerHat));
        assert!(!is_relic_allowed_in_shops(RelicId::AmethystAubergine));
        // A representative shop-allowed relic stays allowed.
        assert!(is_relic_allowed_in_shops(RelicId::BloodVial));
    }

    // ── purchase_card_removal (bead `sts2-rs-jbqw4`) ─────────────────

    fn card(id: crate::cards::CardId) -> crate::Card {
        crate::Card {
            card_id: id,
            ..crate::Card::pod_defaults()
        }
    }

    #[test]
    fn purchase_card_removal_succeeds_pops_card_and_debits_gold() {
        let mut rs = crate::RunState::default();
        rs.gold = 200;
        rs.master_deck
            .push(card(crate::cards::CardId::StrikeIronclad));
        rs.master_deck
            .push(card(crate::cards::CardId::Bloodletting));
        rs.master_deck
            .push(card(crate::cards::CardId::DefendIronclad));
        let mut inv = ShopInventory::empty();
        let cost_before = inv.removal_cost;

        let result = purchase_card_removal(&mut rs, &mut inv, 1);

        match result {
            PurchaseCardRemovalResult::Removed { card, gold_spent } => {
                assert_eq!(card.card_id, crate::cards::CardId::Bloodletting);
                assert_eq!(gold_spent, cost_before);
            }
            other => panic!("expected Removed, got {other:?}"),
        }
        assert_eq!(rs.gold, 200 - cost_before);
        assert_eq!(rs.master_deck.len(), 2);
        assert_eq!(
            rs.master_deck[0].card_id,
            crate::cards::CardId::StrikeIronclad
        );
        assert_eq!(
            rs.master_deck[1].card_id,
            crate::cards::CardId::DefendIronclad
        );
        assert_eq!(rs.card_shop_removals_used, 1);
        assert!(inv.removal_used);
        // Next use would cost 100 (75 + 25).
        assert_eq!(inv.removal_cost, 100);
    }

    #[test]
    fn purchase_card_removal_already_used_returns_marker_without_state_change() {
        let mut rs = crate::RunState::default();
        rs.gold = 500;
        rs.master_deck
            .push(card(crate::cards::CardId::Bloodletting));
        let mut inv = ShopInventory::empty();
        inv.removal_used = true;

        let result = purchase_card_removal(&mut rs, &mut inv, 0);

        assert_eq!(result, PurchaseCardRemovalResult::AlreadyUsed);
        assert_eq!(rs.gold, 500);
        assert_eq!(rs.master_deck.len(), 1);
        assert_eq!(rs.card_shop_removals_used, 0);
    }

    #[test]
    fn purchase_card_removal_insufficient_gold_returns_marker_without_state_change() {
        let mut rs = crate::RunState::default();
        rs.gold = 50;
        rs.master_deck
            .push(card(crate::cards::CardId::Bloodletting));
        let mut inv = ShopInventory::empty();
        // Default base cost is 75, player has 50.

        let result = purchase_card_removal(&mut rs, &mut inv, 0);

        assert_eq!(result, PurchaseCardRemovalResult::InsufficientGold);
        assert_eq!(rs.gold, 50);
        assert_eq!(rs.master_deck.len(), 1);
        assert!(!inv.removal_used);
        assert_eq!(rs.card_shop_removals_used, 0);
    }

    #[test]
    fn purchase_card_removal_invalid_index_returns_marker_without_state_change() {
        let mut rs = crate::RunState::default();
        rs.gold = 500;
        rs.master_deck
            .push(card(crate::cards::CardId::Bloodletting));
        let mut inv = ShopInventory::empty();

        let result = purchase_card_removal(&mut rs, &mut inv, 5);

        assert_eq!(result, PurchaseCardRemovalResult::InvalidDeckIndex);
        assert_eq!(rs.gold, 500);
        assert_eq!(rs.master_deck.len(), 1);
        assert!(!inv.removal_used);
        assert_eq!(rs.card_shop_removals_used, 0);
    }

    #[test]
    fn purchase_card_removal_repriced_after_use_for_next_visit() {
        // Simulates two shops in a row: first purchase bumps the counter
        // so the next shop's `generate_shop_inventory(..., removals_used)`
        // call (or a re-priced `inv.removal_cost`) sees 75 + 25 * 1 = 100.
        let mut rs = crate::RunState::default();
        rs.gold = 200;
        rs.master_deck
            .push(card(crate::cards::CardId::Bloodletting));
        rs.master_deck
            .push(card(crate::cards::CardId::StrikeIronclad));
        let mut inv = ShopInventory::empty();

        let _ = purchase_card_removal(&mut rs, &mut inv, 0);
        assert_eq!(rs.card_shop_removals_used, 1);

        // Refresh as if entering a new shop:
        let inv2_cost = card_removal_cost(rs.card_shop_removals_used, rs.ascension_level);
        assert_eq!(inv2_cost, 100);
    }

    // ── buy_card / buy_relic / buy_potion (bead `sts2-rs-rhcgq`) ─────

    fn inv_with_one_card(price: i32) -> ShopInventory {
        let mut inv = ShopInventory::empty();
        inv.cards.push(ShopCardEntry {
            rarity: RewardRarity::Rare,
            card_type: crate::cards::CardType::Attack,
            card: Some(card(crate::cards::CardId::Bloodletting)),
            is_colorless: false,
            on_sale: false,
            price,
        });
        inv
    }

    fn inv_with_one_colorless(price: i32) -> ShopInventory {
        let mut inv = ShopInventory::empty();
        inv.colorless_cards.push(ShopCardEntry {
            rarity: RewardRarity::Uncommon,
            card_type: crate::cards::CardType::Status,
            card: Some(card(crate::cards::CardId::Bloodletting)),
            is_colorless: true,
            on_sale: false,
            price,
        });
        inv
    }

    fn inv_with_one_relic(price: i32) -> ShopInventory {
        let mut inv = ShopInventory::empty();
        inv.relics.push(ShopRelicEntry {
            relic_rarity: ShopRelicRarity::Common,
            relic_id: Some(crate::relics::RelicId::Anchor),
            price,
        });
        inv
    }

    fn inv_with_one_potion(price: i32) -> ShopInventory {
        let mut inv = ShopInventory::empty();
        inv.potions.push(ShopPotionEntry {
            potion_rarity: ShopPotionRarity::Common,
            potion_kind: Some(crate::potions::PotionKind::BlockPotion),
            price,
        });
        inv
    }

    #[test]
    fn buy_card_succeeds_pushes_to_master_deck_and_marks_sold_out() {
        let mut rs = crate::RunState::default();
        rs.gold = 200;
        let starting_deck = rs.master_deck.len();
        let mut inv = inv_with_one_card(150);

        let outcome = buy_card(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::Bought);
        assert_eq!(rs.gold, 50);
        assert_eq!(rs.master_deck.len(), starting_deck + 1);
        assert_eq!(inv.cards[0].price, SHOP_ENTRY_SOLD_OUT_PRICE);
        assert!(!is_shop_entry_available(inv.cards[0].price));
    }

    #[test]
    fn buy_card_indexes_into_colorless_after_character_row() {
        let mut rs = crate::RunState::default();
        rs.gold = 200;
        let mut inv = inv_with_one_card(50);
        inv.colorless_cards.push(ShopCardEntry {
            rarity: RewardRarity::Rare,
            card_type: crate::cards::CardType::Status,
            card: Some(card(crate::cards::CardId::Bloodletting)),
            is_colorless: true,
            on_sale: false,
            price: 100,
        });

        let outcome = buy_card(&mut rs, &mut inv, 1);

        assert_eq!(outcome, ShopPurchaseOutcome::Bought);
        assert_eq!(rs.gold, 100);
        assert_eq!(inv.cards[0].price, 50);
        assert_eq!(inv.colorless_cards[0].price, SHOP_ENTRY_SOLD_OUT_PRICE);
    }

    // --- deck-add hooks on a merchant card buy (run-shop-gold: LuckyFysh) ----
    //
    // C# `MerchantCardEntry.OnTryPurchase` buys via `CardPileCmd.Add(Deck)`,
    // which fires `ModifyCardBeingAddedToDeck` (pre-add, egg relics) and
    // `AfterCardChangedPiles` (post-add, LuckyFysh's +15 gold). The real
    // listeners live in `sts2-content`; here stub listeners on the run-side
    // channels prove `buy_card` dispatches BOTH, with `source == ShopPurchase`.
    static ADD_FIRED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static ADD_SRC_SHOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static MODIFY_FIRED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn stub_on_card_added(
        rs: &mut crate::RunState,
        ctx: &crate::run_effects::CardAddedRunCtx,
    ) -> crate::EffectResult {
        use std::sync::atomic::Ordering;
        ADD_FIRED.fetch_add(1, Ordering::SeqCst);
        if ctx.source == crate::relic_effects::AddSource::ShopPurchase {
            ADD_SRC_SHOP.store(true, Ordering::SeqCst);
        }
        // Mirror LuckyFysh: grant gold on every card that lands in the deck.
        rs.gain_gold(15);
        crate::EffectResult::Resolved
    }

    fn stub_modify(
        _rs: &mut crate::RunState,
        _card: &mut crate::cards::Card,
    ) -> crate::EffectResult {
        MODIFY_FIRED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::EffectResult::Resolved
    }

    #[test]
    #[serial_test::serial(global_registry)]
    fn buy_card_fires_deck_add_hooks_with_shop_source() {
        use crate::run_effects::{
            register_modify_card_being_added_to_deck, register_on_card_added_to_deck,
            ModifyCardBeingAddedToDeckRunFn, OnCardAddedToDeckRunFn,
        };
        use std::sync::atomic::Ordering;
        ADD_FIRED.store(0, Ordering::SeqCst);
        ADD_SRC_SHOP.store(false, Ordering::SeqCst);
        MODIFY_FIRED.store(0, Ordering::SeqCst);
        register_on_card_added_to_deck(
            crate::relics::RelicId::LuckyFysh,
            stub_on_card_added as OnCardAddedToDeckRunFn,
        );
        register_modify_card_being_added_to_deck(
            crate::relics::RelicId::MoltenEgg,
            stub_modify as ModifyCardBeingAddedToDeckRunFn,
        );

        let mut rs = crate::RunState::default();
        rs.gold = 200;
        let mut inv = inv_with_one_card(50);

        let outcome = buy_card(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::Bought);
        assert!(
            MODIFY_FIRED.load(Ordering::SeqCst) >= 1,
            "ModifyCardBeingAddedToDeck fired pre-add (egg-relic surface)",
        );
        assert!(
            ADD_FIRED.load(Ordering::SeqCst) >= 1,
            "on_card_added_to_deck dispatched post-add",
        );
        assert!(
            ADD_SRC_SHOP.load(Ordering::SeqCst),
            "deck-add carried source == ShopPurchase",
        );
        // 200 base − 50 price + 15 LuckyFysh-style deck-add gold = 165.
        assert_eq!(
            rs.gold, 165,
            "deck-add gold applies on top of the merchant price debit",
        );
    }

    #[test]
    fn buy_card_refuses_when_gold_short_and_does_not_mutate() {
        let mut rs = crate::RunState::default();
        rs.gold = 50;
        let starting_deck = rs.master_deck.len();
        let mut inv = inv_with_one_card(150);

        let outcome = buy_card(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::CannotAfford);
        assert_eq!(rs.gold, 50);
        assert_eq!(rs.master_deck.len(), starting_deck);
        assert_eq!(inv.cards[0].price, 150);
    }

    #[test]
    fn buy_card_refuses_sold_out_entry_even_if_solvent() {
        let mut rs = crate::RunState::default();
        rs.gold = 9_999_999;
        let mut inv = inv_with_one_card(SHOP_ENTRY_SOLD_OUT_PRICE);

        let outcome = buy_card(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::SoldOut);
        assert_eq!(rs.gold, 9_999_999);
    }

    #[test]
    fn buy_card_rejects_index_out_of_bounds() {
        let mut rs = crate::RunState::default();
        rs.gold = 9_999;
        let mut inv = inv_with_one_card(50);

        assert_eq!(
            buy_card(&mut rs, &mut inv, 1),
            ShopPurchaseOutcome::IndexOutOfBounds
        );
        assert_eq!(rs.gold, 9_999);
    }

    #[test]
    fn buy_card_rejects_unrolled_slot() {
        let mut rs = crate::RunState::default();
        rs.gold = 9_999;
        let mut inv = ShopInventory::empty();
        inv.cards.push(ShopCardEntry {
            rarity: RewardRarity::Rare,
            card_type: crate::cards::CardType::Attack,
            card: None,
            is_colorless: false,
            on_sale: false,
            price: 50,
        });

        let outcome = buy_card(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::SlotEmpty);
        assert_eq!(rs.gold, 9_999);
        assert_eq!(inv.cards[0].price, 50);
    }

    #[test]
    fn buy_relic_pushes_relic_and_marks_sold_out() {
        let mut rs = crate::RunState::default();
        rs.gold = 500;
        let starting = rs.relics.len();
        let mut inv = inv_with_one_relic(150);

        let outcome = buy_relic(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::Bought);
        assert_eq!(rs.gold, 350);
        assert_eq!(rs.relics.len(), starting + 1);
        assert_eq!(
            rs.relics.last().copied(),
            Some(crate::relics::RelicId::Anchor)
        );
        assert_eq!(inv.relics[0].price, SHOP_ENTRY_SOLD_OUT_PRICE);
        // `last_obtained_relic` is cleared after dispatch.
        assert_eq!(rs.last_obtained_relic, None);
    }

    /// Bead `sts2-rs-8z3fl.9` — on-sale price flows through `buy_card`'s
    /// gold-debit path. The `card_price_halves_on_sale` unit test pins
    /// the price-generator's `/SALE_PRICE_DIVISOR` reduction; this test
    /// pins the *integration*: a purchase against an inventory entry
    /// whose `price` was generated with `on_sale=true` debits exactly
    /// that halved price (NOT the pre-sale base). Mirrors Python pin
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:1496-1512`
    /// `_buy_item("buy_card", index)` which uses `entry.price` directly
    /// regardless of `entry.on_sale`. A regression that doubled up the
    /// discount (e.g. halving again at purchase time) or ignored the
    /// `price` field in favour of a re-computed base would surface here.
    #[test]
    fn buy_card_on_sale_entry_debits_halved_price() {
        // Derive a base + on-sale pair from the same RNG seed: same
        // factor draw, so `on_sale = base / SALE_PRICE_DIVISOR`.
        let mut rng_a = Rng::new(7);
        let mut rng_b = Rng::new(7);
        let base = card_price(RewardRarity::Uncommon, &mut rng_a, false, false);
        let sale_price = card_price(RewardRarity::Uncommon, &mut rng_b, false, true);
        assert_eq!(
            sale_price,
            base / SALE_PRICE_DIVISOR,
            "precondition: on_sale path halves the base price"
        );

        let mut rs = crate::RunState::default();
        rs.gold = 9_999;
        let mut inv = ShopInventory::empty();
        inv.cards.push(ShopCardEntry {
            rarity: RewardRarity::Uncommon,
            card_type: crate::cards::CardType::Attack,
            card: Some(card(crate::cards::CardId::Bloodletting)),
            is_colorless: false,
            on_sale: true,
            price: sale_price,
        });

        let outcome = buy_card(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::Bought);
        assert_eq!(
            rs.gold,
            9_999 - sale_price,
            "buy_card debits the halved on_sale price exactly"
        );
        assert_eq!(
            inv.cards[0].price, SHOP_ENTRY_SOLD_OUT_PRICE,
            "slot transitions to sold-out sentinel after purchase"
        );
        assert!(
            inv.cards[0].on_sale,
            "on_sale flag is preserved through the purchase (decorative \
             post-buy; price already debited)"
        );
    }

    /// Bead `sts2-rs-8z3fl.9` — `buy_relic` mirror of
    /// `buy_card_refuses_sold_out_entry_even_if_solvent`. With a
    /// player who can comfortably afford the entry, a slot priced at
    /// [`SHOP_ENTRY_SOLD_OUT_PRICE`] still surfaces
    /// `ShopPurchaseOutcome::SoldOut` (NOT `Bought`, NOT
    /// `CannotAfford`). Locks the typed `ShopPurchaseOutcome` surface
    /// from bead `sts2-rs-8he0t` against any regression that collapses
    /// the SoldOut branch into another arm. Pin:
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:1523-1538`.
    #[test]
    fn buy_relic_refuses_sold_out_entry_even_if_solvent() {
        let mut rs = crate::RunState::default();
        rs.gold = 9_999_999;
        let starting_relics = rs.relics.len();
        let mut inv = inv_with_one_relic(SHOP_ENTRY_SOLD_OUT_PRICE);

        let outcome = buy_relic(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::SoldOut);
        assert_eq!(
            rs.gold, 9_999_999,
            "no gold debited on a SoldOut purchase attempt"
        );
        assert_eq!(
            rs.relics.len(),
            starting_relics,
            "no relic moved onto the player on a SoldOut purchase attempt"
        );
        assert_eq!(
            inv.relics[0].price, SHOP_ENTRY_SOLD_OUT_PRICE,
            "slot price unchanged"
        );
    }

    #[test]
    fn buy_relic_refuses_when_gold_short() {
        let mut rs = crate::RunState::default();
        rs.gold = 100;
        let starting = rs.relics.len();
        let mut inv = inv_with_one_relic(150);

        let outcome = buy_relic(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::CannotAfford);
        assert_eq!(rs.gold, 100);
        assert_eq!(rs.relics.len(), starting);
        assert_eq!(inv.relics[0].price, 150);
    }

    #[test]
    fn buy_relic_rejects_unrolled_slot() {
        let mut rs = crate::RunState::default();
        rs.gold = 500;
        let mut inv = ShopInventory::empty();
        inv.relics.push(ShopRelicEntry {
            relic_rarity: ShopRelicRarity::Common,
            relic_id: None,
            price: 150,
        });

        let outcome = buy_relic(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::SlotEmpty);
        assert_eq!(rs.gold, 500);
    }

    #[test]
    fn buy_potion_routes_through_procure_potion_and_marks_sold_out() {
        let mut rs = crate::RunState::default();
        rs.gold = 200;
        let pending_before = rs.pending_rewards.len();
        let mut inv = inv_with_one_potion(50);

        let outcome = buy_potion(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::Bought);
        assert_eq!(rs.gold, 150);
        assert_eq!(rs.pending_rewards.len(), pending_before + 1);
        assert_eq!(inv.potions[0].price, SHOP_ENTRY_SOLD_OUT_PRICE);
    }

    #[test]
    fn buy_potion_with_full_belt_refuses_without_spending_gold() {
        // C# `MerchantPotionEntry.OnTryPurchase`: TryToProcure fails
        // (FailureSpace) → no gold lost, slot stays stocked. Regression for
        // the full-run parity miss where Rust deducted gold then dropped the
        // overflow potion at the pending-rewards drain.
        let mut rs = crate::RunState::default();
        rs.gold = 200;
        // Fill the belt to its ceiling (default max_potion_slots = 3).
        let cap = (rs.max_potion_slots.max(0) as usize).min(crate::potions::MAX_POTIONS);
        for _ in 0..cap {
            assert!(rs.add_potion_to_belt(crate::potions::PotionKind::BlockPotion));
        }
        let pending_before = rs.pending_rewards.len();
        let mut inv = inv_with_one_potion(50);

        let outcome = buy_potion(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::PotionBeltFull);
        assert_eq!(rs.gold, 200, "no gold spent on a refused potion buy");
        assert_eq!(
            rs.pending_rewards.len(),
            pending_before,
            "nothing queued — the potion was never procured"
        );
        assert_eq!(inv.potions[0].price, 50, "slot stays stocked, not sold-out");
    }

    #[test]
    fn buy_potion_refuses_when_gold_short() {
        let mut rs = crate::RunState::default();
        rs.gold = 10;
        let mut inv = inv_with_one_potion(50);

        let outcome = buy_potion(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::CannotAfford);
        assert_eq!(rs.gold, 10);
        assert_eq!(inv.potions[0].price, 50);
    }

    #[test]
    fn buy_potion_rejects_sold_out_entry() {
        let mut rs = crate::RunState::default();
        rs.gold = 9_999_999;
        let mut inv = inv_with_one_potion(SHOP_ENTRY_SOLD_OUT_PRICE);

        let outcome = buy_potion(&mut rs, &mut inv, 0);

        assert_eq!(outcome, ShopPurchaseOutcome::SoldOut);
        assert_eq!(rs.gold, 9_999_999);
    }

    #[test]
    fn buy_repeat_on_sold_out_slot_is_idempotent() {
        let mut rs = crate::RunState::default();
        rs.gold = 500;
        let mut inv = inv_with_one_relic(150);

        assert_eq!(buy_relic(&mut rs, &mut inv, 0), ShopPurchaseOutcome::Bought);
        let gold_after_first = rs.gold;
        let relics_after_first = rs.relics.len();

        assert_eq!(
            buy_relic(&mut rs, &mut inv, 0),
            ShopPurchaseOutcome::SoldOut
        );
        assert_eq!(rs.gold, gold_after_first);
        assert_eq!(rs.relics.len(), relics_after_first);
    }

    #[test]
    fn buy_card_unrolled_colorless_slot_is_slot_empty_not_oob() {
        let mut rs = crate::RunState::default();
        rs.gold = 500;
        let mut inv = inv_with_one_card(50);
        inv.colorless_cards.push(ShopCardEntry {
            rarity: RewardRarity::Rare,
            card_type: crate::cards::CardType::Status,
            card: None,
            is_colorless: true,
            on_sale: false,
            price: 100,
        });

        assert_eq!(
            buy_card(&mut rs, &mut inv, 1),
            ShopPurchaseOutcome::SlotEmpty
        );
        assert_eq!(rs.gold, 500);
    }

    #[test]
    fn buy_colorless_via_combined_index_does_not_touch_character_slot() {
        let mut rs = crate::RunState::default();
        rs.gold = 500;
        let mut inv = inv_with_one_card(50);
        let mut colorless_inv = inv_with_one_colorless(100);
        inv.colorless_cards
            .append(&mut colorless_inv.colorless_cards);

        assert_eq!(buy_card(&mut rs, &mut inv, 1), ShopPurchaseOutcome::Bought);

        assert_eq!(inv.cards[0].price, 50);
        assert_eq!(inv.colorless_cards[0].price, SHOP_ENTRY_SOLD_OUT_PRICE);
    }

    // ── auto_purchase_shop_inventory (bead `sts2-rs-apy0a`) ──────────

    /// `auto_purchase_shop_inventory` does NOT deduct gold — even when
    /// every row carries a positive price the player's purse is
    /// untouched. Mirrors Python's `gold_spent=0` argument to
    /// `_handle_post_shop_purchase` in `_auto_purchase_shop_inventory`.
    #[test]
    fn auto_purchase_does_not_deduct_gold() {
        let mut rs = crate::RunState::default();
        rs.gold = 0;
        let mut inv = inv_with_one_card(150);
        inv.relics.push(ShopRelicEntry {
            relic_rarity: ShopRelicRarity::Common,
            relic_id: Some(crate::relics::RelicId::Anchor),
            price: 150,
        });
        inv.potions.push(ShopPotionEntry {
            potion_rarity: ShopPotionRarity::Common,
            potion_kind: Some(crate::potions::PotionKind::BlockPotion),
            price: 50,
        });

        auto_purchase_shop_inventory(&mut rs, &mut inv);

        assert_eq!(rs.gold, 0);
    }

    /// Every available card / colorless / relic / potion entry lands
    /// on the player and the slot flips to sold-out.
    #[test]
    fn auto_purchase_consumes_every_available_entry() {
        let mut rs = crate::RunState::default();
        let starting_deck = rs.master_deck.len();
        let starting_relics = rs.relics.len();
        let mut inv = inv_with_one_card(150);
        inv.colorless_cards.push(ShopCardEntry {
            rarity: RewardRarity::Uncommon,
            card_type: crate::cards::CardType::Status,
            card: Some(card(crate::cards::CardId::Bloodletting)),
            is_colorless: true,
            on_sale: false,
            price: 100,
        });
        inv.relics.push(ShopRelicEntry {
            relic_rarity: ShopRelicRarity::Common,
            relic_id: Some(crate::relics::RelicId::Anchor),
            price: 150,
        });
        inv.potions.push(ShopPotionEntry {
            potion_rarity: ShopPotionRarity::Common,
            potion_kind: Some(crate::potions::PotionKind::BlockPotion),
            price: 50,
        });

        auto_purchase_shop_inventory(&mut rs, &mut inv);

        assert_eq!(rs.master_deck.len(), starting_deck + 2);
        assert_eq!(rs.relics.len(), starting_relics + 1);
        assert!(rs.relics.contains(&crate::relics::RelicId::Anchor));
        assert_eq!(inv.cards[0].price, SHOP_ENTRY_SOLD_OUT_PRICE);
        assert_eq!(inv.colorless_cards[0].price, SHOP_ENTRY_SOLD_OUT_PRICE);
        assert_eq!(inv.relics[0].price, SHOP_ENTRY_SOLD_OUT_PRICE);
        assert_eq!(inv.potions[0].price, SHOP_ENTRY_SOLD_OUT_PRICE);
    }

    /// `LordsParasol` auto-buys also fire the deck-add hooks — C#
    /// `PurchaseEverything` buys each card via `OnTryPurchaseWrapper` →
    /// `CardPileCmd.Add(Deck)`, so egg upgrades + `LuckyFysh` gold fire per
    /// card even though the buy is free (`ignoreCost: true`). A stub listener
    /// standing in for LuckyFysh must fire once per card added (here: one
    /// character card + one colorless = 2), with `source == ShopPurchase`.
    #[test]
    #[serial_test::serial(global_registry)]
    fn auto_purchase_fires_deck_add_hooks_per_card() {
        use crate::run_effects::{register_on_card_added_to_deck, OnCardAddedToDeckRunFn};
        use std::sync::atomic::Ordering;
        ADD_FIRED.store(0, Ordering::SeqCst);
        ADD_SRC_SHOP.store(false, Ordering::SeqCst);
        register_on_card_added_to_deck(
            crate::relics::RelicId::LuckyFysh,
            stub_on_card_added as OnCardAddedToDeckRunFn,
        );

        let mut rs = crate::RunState::default();
        rs.gold = 0;
        let mut inv = inv_with_one_card(150);
        inv.colorless_cards.push(ShopCardEntry {
            rarity: RewardRarity::Uncommon,
            card_type: crate::cards::CardType::Status,
            card: Some(card(crate::cards::CardId::Bloodletting)),
            is_colorless: true,
            on_sale: false,
            price: 100,
        });

        auto_purchase_shop_inventory(&mut rs, &mut inv);

        assert_eq!(
            ADD_FIRED.load(Ordering::SeqCst),
            2,
            "deck-add hook fired once per auto-bought card (character + colorless)",
        );
        assert!(
            ADD_SRC_SHOP.load(Ordering::SeqCst),
            "auto-buy deck-add carried source == ShopPurchase",
        );
        // Free buy (ignoreCost): no price debit, but LuckyFysh-style +15/card fires.
        assert_eq!(
            rs.gold, 30,
            "LuckyFysh-style deck-add gold on both free cards"
        );
    }

    /// Sold-out slots are left untouched — the auto-purchase walk
    /// matches Python's `is_shop_entry_available` guard exactly: a
    /// slot whose price is already at the sentinel doesn't double-
    /// grant on a second visit.
    #[test]
    fn auto_purchase_skips_sold_out_entries() {
        let mut rs = crate::RunState::default();
        let starting_deck = rs.master_deck.len();
        let starting_relics = rs.relics.len();
        let mut inv = inv_with_one_card(SHOP_ENTRY_SOLD_OUT_PRICE);
        inv.relics.push(ShopRelicEntry {
            relic_rarity: ShopRelicRarity::Common,
            relic_id: Some(crate::relics::RelicId::Anchor),
            price: SHOP_ENTRY_SOLD_OUT_PRICE,
        });

        auto_purchase_shop_inventory(&mut rs, &mut inv);

        assert_eq!(rs.master_deck.len(), starting_deck);
        assert_eq!(rs.relics.len(), starting_relics);
    }

    /// Half-rolled inventory (a slot whose concrete payload is
    /// `None`) flips to sold-out without granting anything — matches
    /// `buy_card` / `buy_relic` / `buy_potion`'s `SlotEmpty` branch.
    /// The Python no-ops on the missing payload silently; the Rust
    /// port keeps the no-op but still consumes the slot so a stuck
    /// slot can't be auto-purchased a second time.
    #[test]
    fn auto_purchase_consumes_empty_slot_without_granting() {
        let mut rs = crate::RunState::default();
        let starting_deck = rs.master_deck.len();
        let mut inv = ShopInventory::empty();
        inv.cards.push(ShopCardEntry {
            rarity: RewardRarity::Rare,
            card_type: crate::cards::CardType::Attack,
            card: None,
            is_colorless: false,
            on_sale: false,
            price: 150,
        });

        auto_purchase_shop_inventory(&mut rs, &mut inv);

        assert_eq!(rs.master_deck.len(), starting_deck);
        assert_eq!(inv.cards[0].price, SHOP_ENTRY_SOLD_OUT_PRICE);
    }
}
