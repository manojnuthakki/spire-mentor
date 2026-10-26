//! Top-level run orchestrator (skeleton).
//!
//! Ports the *structure* of Python's `RunManager`
//! (`sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:182-450`) — the
//! [`Phase`] enum, the `RunManager` struct, and the phase / action API
//! surface — without porting any phase behavior. Bead `sts2-rs-zfmfz`
//! (P9.0.d) lights up the state-machine skeleton so downstream P9.0.e
//! (`sts2-rs-5fsbh` linear 3-room thin-slice map driver) and the rest
//! of the P9.0 phases can wire concrete transitions into the existing
//! `phase` / `take_action` slots without churning the public surface.
//!
//! ## What's intentionally NOT here
//!
//! - **No phase transitions.** `take_action` returns
//!   [`ActionResult::NotImplemented`] for every input. Per-phase
//!   `_enter_*` methods are added by individual phase beads.
//! - **No map.** `Phase::MapChoice` is the initial phase, but
//!   `get_available_actions` returns `[]` until P9.0.e ports the
//!   linear hand-coded map.
//! - **No combat bridge.** `combat` defaults to `None`; the
//!   `enter_combat` / `exit_combat` bridge (already landed in
//!   `sts2-rs-tjn0v`) is invoked by the future `_enter_combat` phase
//!   handler, not by `RunManager::new`.
//! - **No real RNG.** The legacy `rewards_rng_seed: u64` shim is set;
//!   the multi-stream [`crate::RunRngSet`] migration (bead
//!   `sts2-rs-ewm6b`) lands a follow-up that hangs the bundle on
//!   `RunState` directly.

use crate::act_selection::ActVariant;
use crate::characters::{character_starter, CharacterStarter};
use crate::potions::CharacterId;
use crate::runstate::RunState;
use crate::CombatState;

/// Run-loop phase. Mirrors Python's `RunManager.PHASE_*` string
/// constants (`run_manager.py:193-201`) as a typed enum.
///
/// All nine canonical phases are listed for forward-compat; the thin
/// slice (`sts2-rs-zfmfz` / `sts2-rs-5fsbh`) only drives `MapChoice`,
/// `Combat`, `CardReward`, and `RunOver`. The remaining variants
/// (`BossRelic`, `Shop`, `RestSite`, `Event`, `Treasure`) are activated
/// by their respective P9.1+ phase beads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Player picks the next map node (or, in the linear thin slice,
    /// auto-advances to the next room).
    MapChoice,
    /// Active combat. [`RunManager::combat`] is `Some`.
    Combat,
    /// Post-combat card reward screen. Player picks 1-of-3 or skips.
    CardReward,
    /// Boss-relic choice screen (3 picks from the boss-relic pool).
    BossRelic,
    /// Shop browsing / purchase phase.
    Shop,
    /// Rest-site option pick (rest vs upgrade vs channel-injected).
    RestSite,
    /// Event resolution (Neow + run events).
    Event,
    /// Treasure-room chest resolution.
    Treasure,
    /// Run is finished — `run_state.player_won` resolves outcome.
    RunOver,
}

/// Action the agent can submit to advance the run. Mirrors Python's
/// `action: dict` shape, but typed.
///
/// The skeleton only declares the variants; every variant is routed
/// to [`ActionResult::NotImplemented`] until the corresponding phase
/// handler lands. Adding a new action type is a non-breaking change
/// (the enum is `#[non_exhaustive]`), but adding behavior is the
/// downstream bead's job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunAction {
    /// MapChoice: move to a specified map column (or, in the linear
    /// slice, the only available next node). The `col` value is the
    /// `MapCoord.col` — interpreted by the future map driver.
    Move { col: i32 },
    /// Combat: play card at hand index `hand_idx`, optionally
    /// targeting creature `target`.
    PlayCard {
        hand_idx: usize,
        target: Option<u32>,
    },
    /// Combat: end turn.
    EndTurn,
    /// Combat: resolve an open `pending_choice` (Headbutt's PutBack,
    /// Armaments' UpgradeInHand, a potion choice, a deferred
    /// turn-setup choice, …). `Some(i)` selects option `i`; `None`
    /// skips (empty selection) when the choice allows it. Routed to
    /// [`crate::apply`]`(Action::ResolveChoice)`. Skip + single-pick
    /// only — a multi-pick combat choice (e.g. HIDDEN_DAGGERS' discard-2,
    /// now that a played card's OnPlay select SURFACES — bead
    /// `sts2-rs-mcts-surface-onplay-select`) is surfaced as
    /// [`Self::CombatResolveChoiceSelection`] instead. Bead `sts2-rs-6dtxp`.
    CombatResolveChoice { option: Option<usize> },
    /// Combat: resolve an open multi-pick `pending_choice` by taking the
    /// first `count` options. The general form of
    /// [`Self::CombatResolveChoice`] for `max_select > 1` choices (e.g.
    /// HIDDEN_DAGGERS "discard 2"), now reachable because a played card's
    /// OnPlay select SURFACES instead of resolving inline. `legal_actions`
    /// enumerates multi-pick selections as the first-`k` prefixes
    /// `[0, 1, …, k-1]` for `k in min_select..=max_select` (a bounded
    /// `O(max_select)` fan-out, not a `2^N` subset explosion), so a single
    /// `count` describes the selection exactly and keeps `RunAction` `Copy`.
    /// Routed to [`crate::apply`]`(Action::ResolveChoice)` as
    /// `(0..count).collect()`. Bead `sts2-rs-mcts-surface-onplay-select`.
    CombatResolveChoiceTakeFirst { count: u8 },
    /// Combat: drink the held potion in belt slot `potion_slot`,
    /// optionally targeting enemy index `target`. Routed to
    /// [`crate::apply`]`(Action::DrinkPotion)`, which consumes the slot
    /// and dispatches the potion effect. The combat-side belt is seeded
    /// from / written back to the run-level belt by
    /// `sts2-content::run_bridge` (`enter_combat` / `exit_combat`), so a
    /// potion drunk mid-fight is gone from the run belt afterward. Bead
    /// `sts2-rs-01o` follow-up (combat potions surfaced at the run-action
    /// level for the workbench UI).
    DrinkPotion {
        potion_slot: usize,
        target: Option<u32>,
    },
    /// CardReward: pick option index `idx` (0..3), or skip via `None`.
    PickCardReward { idx: Option<usize> },
    /// CardReward: accept the currently-offered relic reward.
    PickRelicReward,
    /// CardReward: skip the currently-offered relic reward.
    SkipRelic,
    /// BossRelic: pick relic index `idx` from the 3-option boss pool.
    PickBossRelic { idx: usize },
    /// Shop: skip-only initial impl — leave shop without buying.
    LeaveShop,
    /// Shop: purchase the card-row entry at `idx`. Indexes the
    /// concatenated `[character_cards … colorless_cards]` slice exactly
    /// like [`crate::shop::buy_card`]. Bead `sts2-rs-if6ry` (W3).
    ShopBuyCard { idx: usize },
    /// Shop: purchase the relic-row entry at `idx` (rolled-rarity
    /// slots 0..=1, shop-pool slot 2). Bead `sts2-rs-if6ry` (W3).
    ShopBuyRelic { idx: usize },
    /// Shop: purchase the potion-row entry at `idx`. Bead
    /// `sts2-rs-if6ry` (W3).
    ShopBuyPotion { idx: usize },
    /// Shop: consume the one-per-visit remove-card slot, popping
    /// `master_deck[deck_idx]`. The removable predicate
    /// (Strike/Defend basics) is NOT enforced at this surface —
    /// callers that present a UI should pre-filter via
    /// [`crate::RunState::removable_deck_card_count`]; scripted
    /// drivers get verbatim deck-index semantics. Bead
    /// `sts2-rs-if6ry` (W3).
    ShopRemoveCard { deck_idx: usize },
    /// RestSite: pick rest-site option at index `idx` from the list
    /// returned by [`RunManager::enter_rest_site`].
    RestOption { idx: usize },
    /// RestSite (smith sub-state): pick which upgradable card to
    /// upgrade after [`RestOption`] selected Smith and the resolver
    /// returned [`crate::runstate::RestSiteActionOutcome::SmithAwaitingChoice`]
    /// (>1 upgradable cards in the master deck — single-card case
    /// auto-resolves at the engine level). `idx` indexes into the
    /// `pending_choice.options` list opened during the Smith
    /// transition; the same list is enumerated by
    /// [`RunManager::get_available_actions`] in this sub-state.
    ///
    /// **Port pin:**
    /// - Python: `sts2-rl-agent@01c094d:sts2_env/run/rest_site.py:88-107`
    ///   (`SmithRestSiteOption.execute` — `request_deck_choice` with
    ///   `cards=candidates` over `not card.upgraded` filter, then
    ///   `resolver=lambda selected: [upgrade_card_instance(c) for c
    ///   in selected]`).
    /// - C#: `v0.103.2 MegaCrit.Sts2.Core.Entities.RestSite/SmithRestSiteOption.cs:58-75`
    ///   (`OnSelect` → `CardSelectCmd.FromDeckForUpgrade` with
    ///   `RequireManualConfirmation=true`, then `CardCmd.Upgrade` per
    ///   selected card + `Hook.AfterRestSiteSmith`).
    ///
    /// Bead `sts2-rs-5abnx` (W5b — drives the scraped
    /// `upgraded_cards[]` choice during Act-1 replay); pin headers
    /// added by `sts2-rs-z4qwd`.
    SmithCard { idx: usize },
    /// Resolve the front pending full-belt potion-reward swap (see
    /// [`crate::runstate::RunState::pending_potion_swaps`]).
    /// `Some(slot)` discards the held potion in belt slot `slot` and
    /// claims the pending reward potion in its place; `None` declines
    /// the reward (the potion is lost and counted in
    /// [`crate::runstate::RunState::potions_overflow_dropped`]).
    /// Phase-agnostic like [`Self::RemoveDeckCard`] — the swap preempts
    /// the current phase's actions while pending. Only ever surfaced
    /// when [`crate::runstate::RunState::interactive_potion_swap`] is
    /// set. Mirrors C# `PotionReward` TooFull + `DiscardPotionGameAction`
    /// semantics (v0.107.1 `PotionReward.cs:86`). GH #252.
    PotionSwap { discard_slot: Option<usize> },
    /// Event: pick event-choice option at index `idx`.
    EventChoice { idx: usize },
    /// Treasure: open the chest, consuming the pending relic reward.
    CollectTreasure,
    /// CardReward: sacrifice the current card reward (Pael's Wing).
    /// Available only when the player owns the PAELS_WING relic and a
    /// card-reward is currently being offered. Every 2nd sacrifice
    /// pulls a random relic from the player's relic grab-bag.
    /// Mirrors Python `_do_card_reward_sacrifice` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:1380-1399`.
    SacrificeCardReward,
    /// Remove the master-deck card at `idx`, draining one pending
    /// interactive removal (see [`crate::runstate::RunState::pending_deck_removals`]).
    /// Phase-agnostic: an on-obtain card-removal relic (Precise Scissors,
    /// Empty Cage, …) can be granted from several phases (Neow event,
    /// card reward, treasure, shop), so this resolves regardless of the
    /// current [`Phase`] while removals are pending. Only offered when
    /// [`crate::runstate::RunState::interactive_card_removal`] is set.
    RemoveDeckCard { idx: usize },
    /// Upgrade a deck card, draining one pending interactive upgrade (see
    /// [`crate::runstate::RunState::pending_deck_upgrades`]). `idx` is the
    /// position within the NON-UPGRADED subset of the master deck (the same
    /// index C#'s `FromDeckForUpgrade` `PendingOptions` / headless card_select
    /// uses), NOT a raw master-deck index. Phase-agnostic like
    /// [`Self::RemoveDeckCard`]: an on-obtain card-upgrade relic (Pomander)
    /// can be granted from several phases (Neow event, card reward, shop), so
    /// this resolves regardless of the current [`Phase`] while upgrades are
    /// pending. Only offered when
    /// [`crate::runstate::RunState::interactive_card_upgrade`] is set. Mirrors
    /// C# `Pomander.AfterObtained` → `CardSelectCmd.FromDeckForUpgrade`. Bead
    /// `sts2-rs-1hiht`.
    UpgradeDeckCard { idx: usize },
    /// Transform a deck card, draining one pending interactive transform
    /// (see [`crate::runstate::RunState::pending_deck_transforms`]). `idx`
    /// is the position within the ELIGIBLE subset for the front pending
    /// entry ([`RunManager::transform_eligible_deck_indices`] — the whole
    /// deck for the unfiltered NewLeaf shape, so backward-compatibly equal
    /// to the raw master-deck position there; the Basic-rarity subset for
    /// Wood Carvings Bird/Torus). The replacement is the entry's
    /// `fixed_target` when set (C# `CardCmd.TransformTo<T>`, no RNG) or is
    /// rolled at resolve time via the `roll_transform_replacement` content
    /// callback (C# rolls only after the pick: `NewLeaf.AfterObtained` →
    /// select → `CardCmd.TransformToRandom(item, Rng.Niche)`); applied with
    /// the C# Deck-pile transform shape — remove the original, append the
    /// replacement at the deck tail. Phase-agnostic like
    /// [`Self::RemoveDeckCard`]. Only offered when
    /// [`crate::runstate::RunState::interactive_card_transform`] is set.
    /// Bead 6ynt.4.deck-add / GH #34.
    TransformDeckCard { idx: usize },
    /// Enchant a deck card, draining one pending interactive enchant (see
    /// [`crate::runstate::RunState::pending_deck_enchants`] — Wood
    /// Carvings' Snake → Slither). `idx` is the position within the
    /// eligible subset ([`RunManager::enchant_eligible_deck_indices`],
    /// mirroring C# `FromDeckForEnchantment`'s `CanEnchant` pool).
    /// Phase-agnostic like [`Self::RemoveDeckCard`].
    EnchantDeckCard { idx: usize },
    /// Pick one of the FRONT pending card grant's generated options and add it
    /// to the deck, draining one pick (see
    /// [`crate::runstate::RunState::pending_card_grants`] — `RoomFullOfCheese`'s
    /// Gorge "pick 2 of 8 Common cards"). `idx` is the position within the
    /// front grant's current `options` list (C# `FromSimpleGridForRewards`
    /// index space). The run loop offers one per remaining option and the
    /// harness drives an N-index `select_cards` as N sequential picks (each
    /// pick removes its option, so later indices shift down — mirrors the
    /// `TransformDeckCard` fan-out). Phase-agnostic like [`Self::RemoveDeckCard`].
    /// Bead `sts2-rs-gorge-select`.
    GrantDeckCard { idx: usize },
    /// Rewards screen ([`crate::runstate::RunState::interactive_rewards`]):
    /// claim the reward at `idx` of [`RunManager::reward_screen`] (C#
    /// `Reward.OnSelect`). Gold and a relic apply at once, a potion lands in
    /// the belt (refused while the belt is full), a card reward opens its
    /// offering, after which the screen returns. sts2-agent fork.
    ClaimReward { idx: usize },
    /// Rewards screen: leave it, skipping every reward still listed (C#
    /// `Reward.OnSkipped`), and continue the run. sts2-agent fork.
    ProceedRewards,
    /// Discard the held potion at belt position `slot` (C#
    /// `PotionModel.Discard`); combat-side belt in a fight, the run belt
    /// otherwise. Only offered under
    /// [`crate::runstate::RunState::interactive_potions`]. sts2-agent fork.
    DiscardPotion { slot: usize },
    /// Catch-all for not-yet-modeled actions. Routes to
    /// [`ActionResult::NotImplemented`].
    Unhandled,
}

/// Outcome of [`RunManager::take_action`]. Mirrors the loose `dict`
/// return shape of Python's `take_action`, lifted to an enum so the
/// caller can pattern-match without string-checking.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ActionResult {
    /// The phase machine accepted the action and advanced. The new
    /// phase is reachable via [`RunManager::phase`].
    Advanced,
    /// The action is not valid in the current phase (e.g. `PlayCard`
    /// during `MapChoice`).
    InvalidForPhase,
    /// The action variant is recognized but the phase handler is not
    /// yet implemented. Returned by the skeleton for every action.
    NotImplemented,
    /// The run has finished. Subsequent `take_action` calls keep
    /// returning this until a new `RunManager` is constructed.
    RunOver,
    /// A `Phase::Shop` purchase attempt was rejected by the shop layer
    /// (mis-priced, mis-indexed, sold-out, etc.). Carries the typed
    /// failure reason from [`crate::shop::ShopPurchaseOutcome`] so
    /// replay/parity harnesses can distinguish failure modes instead
    /// of collapsing every non-success to `InvalidForPhase`. Bead
    /// `sts2-rs-8he0t` (follow-up to `sts2-rs-if6ry` W3).
    ShopPurchaseFailed(crate::shop::ShopPurchaseOutcome),
    /// A `Phase::Shop` card-removal attempt was rejected by the shop
    /// layer (already used this visit, insufficient gold, deck index
    /// out of range). Carries the typed
    /// [`crate::shop::PurchaseCardRemovalResult`] failure for the
    /// same diagnostic reasons as
    /// [`ActionResult::ShopPurchaseFailed`]. Bead `sts2-rs-8he0t`.
    ShopRemoveCardFailed(crate::shop::PurchaseCardRemovalResult),
}

/// Cross-crate content callbacks. The dispatch table in
/// [`RunManager::take_action`] lives in `sts2-core` (engine-agnostic),
/// but a full run requires `sts2-content` for combat bridging,
/// encounter selection, card-reward generation, etc. To keep the
/// `content → core` dep direction intact, those entry points are
/// injected as plain `fn` pointers — the caller (e.g. the
/// `full_run_act1to3_ironclad` test driver, or any future runtime
/// host) wires concrete `sts2-content` functions in at construction
/// via [`RunManager::with_content_callbacks`].
///
/// Every field is `Option`; a `None` slot makes the corresponding
/// phase dispatch return [`ActionResult::NotImplemented`] (so the
/// skeleton run-manager — `RunManager::new` with no callbacks wired
/// — still type-checks and the existing thin-slice tests stay green).
///
/// Bead `sts2-rs-7kl4f` (P9-DISPATCH).
#[derive(Clone, Copy, Default)]
pub struct ContentCallbacks {
    /// Build a fresh [`CombatState`] for `encounter`. Mirrors
    /// `sts2-content::run_bridge::enter_combat`.
    pub enter_combat: Option<fn(&RunState, &crate::encounters::EncounterSpec) -> CombatState>,
    /// Flush combat-side state back into [`RunState`] on combat end.
    /// Mirrors `sts2-content::run_bridge::exit_combat`.
    pub exit_combat: Option<fn(&mut RunState, &CombatState)>,
    /// Pick the encounter spec for the room the player just entered.
    /// `act_index` is 1-based.
    pub select_encounter: Option<
        fn(
            rs: &mut RunState,
            act_index: u8,
            room_type: crate::runstate::RoomType,
        ) -> &'static crate::encounters::EncounterSpec,
    >,
    /// Build a combat directly for the room the player just entered,
    /// returning `None` to defer to the [`Self::select_encounter`] +
    /// [`Self::enter_combat`] pair. Lets the content side select from its
    /// richer `EncounterDef` table (tier-by-floor, roster randomizers,
    /// per-monster starting-HP rolls, real `MonsterSpawnArgs`) instead of
    /// the pinned static [`crate::encounters::EncounterSpec`] pools.
    /// `enter_combat_phase` prefers this when wired and accepting.
    pub build_combat_for_room: Option<
        fn(
            rs: &mut RunState,
            act_index: u8,
            room_type: crate::runstate::RoomType,
        ) -> Option<CombatState>,
    >,
    /// Generate the 3-card option list for a normal card reward. The
    /// callback resolves character pool + rarity rolls.
    pub generate_card_reward_options: Option<fn(rs: &mut RunState) -> Vec<crate::cards::CardId>>,
    /// Generate the card option list for a NON-combat (`CardCreationSource.Other`)
    /// card reward — Lost Coffer / Orrery / event card grants drained from
    /// `pending_rewards`. Unlike [`Self::generate_card_reward_options`] (the
    /// combat-reward roller, `CardCreationSource.Encounter` → pity-updating
    /// `Roll`), this rolls **base odds** (`RollWithBaseOdds`): no pity offset
    /// read and — critically — no pity-counter mutation. Sharing the
    /// pity-updating roller here over-accumulated the run's pity offset, which
    /// then flipped a later combat reward's rarity (PARITY132). Consumes the
    /// identical float sequence (1 rarity + 1 pick + 1 upgrade per card) so the
    /// `Rewards` cursor stays aligned. `None` falls back to the pity roller.
    pub generate_noncombat_card_reward_options:
        Option<fn(rs: &mut RunState) -> Vec<crate::cards::CardId>>,
    /// Generate the 3-relic option list for a boss-relic reward.
    pub generate_boss_relic_pool: Option<fn(rs: &mut RunState) -> Vec<crate::RelicId>>,
    /// Apply any `Rewards`-stream side effects of *presenting* a boss-relic /
    /// next-act Ancient option list (mirrors the per-relic `SetupForPlayer`
    /// calls inside C# `AncientEventModel.GenerateInitialOptions`). Today the
    /// only such effect is `DustyTome.SetupForPlayer` rolling one
    /// `PlayerRng.Rewards` read to pick its Ancient card (`DustyTome.cs:55`);
    /// the callback consumes it (and stashes the card on
    /// [`RunState::dusty_tome_setup_card`]) iff `DustyTome` is among `offers`.
    /// Invoked at `Phase::BossRelic` entry — AFTER the boss combat reward's
    /// gold/potion/card `Rewards` reads, BEFORE the relic pick — so the shared
    /// `Rewards` cursor matches C# read-for-read. `None` is a no-op.
    pub setup_boss_relic_ancient: Option<fn(rs: &mut RunState, offers: &[crate::RelicId])>,
    /// Generate per-rarity relic pools for treasure rooms (common,
    /// uncommon, rare). Threaded into [`RunManager::enter_treasure`].
    pub generate_treasure_relic_pools: Option<
        fn(
            rs: &mut RunState,
        ) -> (
            Vec<crate::RelicId>,
            Vec<crate::RelicId>,
            Vec<crate::RelicId>,
        ),
    >,
    /// Resolve the current event by picking option `idx`. When
    /// [`RunState::current_event`] is `None` (engine-driven Event-room
    /// entry — the legacy pre-Neow path), the callback lazily rolls an
    /// event via `pick_event`, enters its room, and resolves option
    /// `idx`. When `current_event` is already set (e.g.
    /// [`RunManager::with_neow_start`] pre-binds Neow), the callback
    /// resolves option `idx` of that event directly without re-picking.
    /// Returns `true` on success, `false` if no event could be picked
    /// or the index was out of range.
    ///
    /// Bead `sts2-rs-nzdz1`: replaces the prior first-option-only
    /// `resolve_event` slot so multi-option events (Neow,
    /// `TheFutureOfPotions`, …) are reachable through the public
    /// action surface.
    pub resolve_event_at: Option<fn(rs: &mut RunState, idx: usize) -> bool>,
    /// Number of options the currently-active event offers. Returns 0
    /// when no event is active (`current_event == None`). The engine
    /// uses this in `get_available_actions` to enumerate one
    /// `RunAction::EventChoice { idx }` per option. Bead
    /// `sts2-rs-nzdz1`.
    pub event_option_count: Option<fn(rs: &RunState) -> usize>,
    /// Build the active [`crate::shop::ShopInventory`] for the shop
    /// room the player just entered. Invoked once on Shop-room entry;
    /// the returned inventory is owned by the [`RunManager`] until
    /// [`RunAction::LeaveShop`] drains it. When unwired, `Phase::Shop`
    /// surfaces only `LeaveShop` (the legacy skip-only behavior).
    /// Mirrors C# `Shop.Initialize` / `Shop.GenerateInventory` and
    /// `crate::shop::generate_shop_inventory`; the content-layer
    /// wrapper is responsible for filling concrete `card` / `relic_id`
    /// / `potion_kind` slots from the character + colorless pools.
    /// Bead `sts2-rs-if6ry` (W3).
    pub generate_shop_inventory: Option<fn(rs: &mut RunState) -> crate::shop::ShopInventory>,
    /// Re-populate a single merchant slot after a successful purchase
    /// when the player owns `TheCourier` (C#
    /// `MerchantEntry.OnTryPurchaseWrapper` →
    /// `Hook.ShouldRefillMerchantEntry` → `RestockAfterPurchase`, which
    /// only `TheCourier` overrides to `true`). The callback re-rolls the
    /// slot's concrete item + price on the run's `Shops`/`Rewards`
    /// streams read-for-read with the initial merchant roll, restoring
    /// stream alignment for any later shop visit. Mirrors
    /// `MerchantCardEntry.RestockAfterPurchase` (`Populate`),
    /// `MerchantRelicEntry.RestockAfterPurchase`, and
    /// `MerchantPotionEntry.RestockAfterPurchase`. When unwired, a
    /// bought slot simply stays sold-out (the pre-Courier behavior).
    /// Bead `sts2-rs` run-shop-gold (parity sweep 2026-07-07).
    pub restock_shop_entry: Option<
        fn(rs: &mut RunState, inv: &mut crate::shop::ShopInventory, slot: crate::shop::ShopSlot),
    >,
    /// Roll a single random out-of-combat potion kind for an
    /// unspecified [`crate::runstate::Reward::Potion`] sentinel (Lost
    /// Coffer / event "procure 1 random potion" grants). The callback
    /// owns the character + rarity pool walk and consumes the
    /// `CombatPotionGeneration` RNG stream, mirroring
    /// `content::potions::all::create_random_potions_out_of_combat`.
    /// Returns `None` only if the pool is empty (unreachable in
    /// practice). When unwired, potion sentinels are dropped during
    /// [`RunManager::drain_pending_rewards`]. Bead `sts2-rs-01o`.
    pub roll_potion_reward_kind:
        Option<fn(rs: &mut RunState) -> Option<crate::potions::PotionKind>>,
    /// Roll ONE `NeowsBones` curse on the `Niche` stream (C#
    /// `Rng.Niche.NextItem(availableCurses)`), returning its POD. Invoked by
    /// [`RunManager::drain_post_pick_deck_cards_if_settled`] once the grant
    /// batch settles, so the curse is drawn AFTER any granted relic's own
    /// interactive `Niche` read (NewLeaf's transform) — matching C#'s
    /// post-`Offer()` order. The pool is content-owned; when unwired the
    /// deferred curse count is simply dropped (raw-core tests). Wired by
    /// `sts2_content::relics::neow_relics::roll_neows_bones_curse`.
    pub roll_neows_bones_curse: Option<fn(rs: &mut RunState) -> Option<crate::cards::Card>>,
    /// Pre-roll the current act's boss encounter at act-map generation
    /// time and stash it on [`RunState::pinned_act_boss`]. Mirrors the
    /// C# game where the boss identity is locked when the map is laid
    /// out (visible on the map widget from floor 1), instead of rolled
    /// lazily at boss-room entry. The boss-room dispatch
    /// ([`Self::build_combat_for_room`]) consumes the pin instead of
    /// rolling fresh, so total `run_manager` RNG advancement is
    /// preserved — only the *timing* of the boss roll moves from boss
    /// entry to act start. Wired by
    /// `sts2_content::run_callbacks::pre_roll_act_boss`; when unwired
    /// (e.g. raw-core tests), the boss-room dispatch falls back to its
    /// legacy lazy roll.
    pub pre_roll_act_boss: Option<fn(rs: &mut RunState)>,
    /// Consume the run-level `Rewards` stream for a Monster/Elite combat
    /// win in C#'s `RewardsSet.GenerateRewardsFor` order — potion-odds
    /// `NextFloat` → gold `NextInt` → potion-contents — and apply the
    /// rolled gold ([`RunState::gain_gold`]) and any dropped potion (to
    /// the belt) immediately. Invoked by
    /// [`RunManager::exit_combat_and_transition`] BEFORE
    /// [`RunManager::enter_card_reward_phase`], so the card reward rolls
    /// off the now-correctly-advanced `Rewards` cursor (C# populates
    /// Gold/Potion before Card on the same stream). When unwired, the
    /// card reward rolls at the un-advanced cursor (legacy behavior).
    /// Mirrors `sts2_content::run_callbacks::apply_combat_gold_and_potion`.
    /// Bead `sts2-rs-jenwp` (M4 live reward stream).
    pub apply_combat_gold_and_potion:
        Option<fn(rs: &mut RunState, room_type: crate::runstate::RoomType)>,
    /// The rewards-screen twin of [`Self::apply_combat_gold_and_potion`]
    /// ([`RunState::interactive_rewards`]): the same `Rewards`-stream reads in
    /// the same order, returning the gold amounts and the dropped potion
    /// instead of applying them. sts2-agent fork.
    pub roll_combat_gold_and_potion:
        Option<fn(rs: &mut RunState, room_type: crate::runstate::RoomType) -> RoomRewardRoll>,
    /// One random potion off `rs.rng.combat_potion_generation` (C#
    /// `PotionFactory.CreateRandomPotionOutOfCombat(owner,
    /// RunState.Rng.CombatPotionGeneration)`), for Entropic Brew drunk out of
    /// combat. sts2-agent fork.
    pub roll_out_of_combat_potion:
        Option<fn(rs: &mut RunState) -> Option<crate::potions::PotionKind>>,
    /// Roll the replacement card for one interactive deck transform
    /// (NewLeaf's `CardCmd.TransformToRandom(item, RunState.Rng.Niche)`).
    /// The callback owns the character transform pool walk (C/U/R,
    /// non-MP-only, != original) and consumes the **niche** stream — or,
    /// when `event_stream` is `Some((name, skip))`, the named event's own
    /// non-shared stream (WhisperingHollow Hug; see
    /// [`crate::runstate::PendingDeckTransform::event_stream_roll`]). It
    /// returns a spec-populated POD (not `pod_defaults`). Consumed by
    /// [`RunManager::do_transform_deck_card_at`] while
    /// [`RunState::pending_deck_transforms`] drains. When unwired
    /// (raw-core tests), the transform pick is dropped without mutating
    /// the deck. Bead 6ynt.4.deck-add / GH #34.
    pub roll_transform_replacement: Option<
        fn(
            rs: &mut RunState,
            original: crate::cards::CardId,
            event_stream: Option<(&'static str, u32)>,
        ) -> Option<crate::cards::Card>,
    >,
    /// Build the [`CombatState`] for an event-nested combat
    /// ([`crate::events::EventCombatRequest`]). Mirrors C#
    /// `EventModel.EnterCombatWithoutExitingEvent` building a `CombatRoom`
    /// from the event encounter: maps the request's
    /// [`crate::events::EventEncounter`] to a concrete monster roster and
    /// runs the same combat-assembly path as a normal encounter
    /// (`run_bridge::enter_combat_from_roster`). Consumed by
    /// [`RunManager::do_event_choice`] when an event handler stages a
    /// fight on [`RunState::event_combat`]. When unwired, the request is
    /// dropped and the event routes straight to its post-event rewards
    /// (legacy instant-resolution). Bead `sts2-rs-ck9z6` (GH #166).
    pub build_event_combat:
        Option<fn(rs: &mut RunState, req: &crate::events::EventCombatRequest) -> CombatState>,
    /// Resume a parent event after its nested combat ends, granting the
    /// per-encounter victory reward. Mirrors C# `EventModel.Resume(room)`
    /// (e.g. `BattlewornDummy.Resume` rolling the per-setting potion /
    /// upgrade / relic). Called by [`RunManager::exit_combat_and_transition`]
    /// only for `resume`-flagged requests; `clean_victory` is `false` when
    /// the dummy ran out of time
    /// ([`CombatState::event_combat_timed_out`]) → no reward. When unwired,
    /// the resume is a no-op. Bead `sts2-rs-ck9z6` (GH #166).
    pub resume_event_after_combat: Option<
        fn(rs: &mut RunState, encounter: &crate::events::EventEncounter, clean_victory: bool),
    >,
}

impl core::fmt::Debug for ContentCallbacks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ContentCallbacks")
            .field("enter_combat", &self.enter_combat.is_some())
            .field("exit_combat", &self.exit_combat.is_some())
            .field("select_encounter", &self.select_encounter.is_some())
            .field(
                "build_combat_for_room",
                &self.build_combat_for_room.is_some(),
            )
            .field(
                "generate_card_reward_options",
                &self.generate_card_reward_options.is_some(),
            )
            .field(
                "generate_noncombat_card_reward_options",
                &self.generate_noncombat_card_reward_options.is_some(),
            )
            .field(
                "generate_boss_relic_pool",
                &self.generate_boss_relic_pool.is_some(),
            )
            .field(
                "setup_boss_relic_ancient",
                &self.setup_boss_relic_ancient.is_some(),
            )
            .field(
                "generate_treasure_relic_pools",
                &self.generate_treasure_relic_pools.is_some(),
            )
            .field("resolve_event_at", &self.resolve_event_at.is_some())
            .field("event_option_count", &self.event_option_count.is_some())
            .field(
                "generate_shop_inventory",
                &self.generate_shop_inventory.is_some(),
            )
            .field("restock_shop_entry", &self.restock_shop_entry.is_some())
            .field(
                "roll_potion_reward_kind",
                &self.roll_potion_reward_kind.is_some(),
            )
            .field(
                "roll_neows_bones_curse",
                &self.roll_neows_bones_curse.is_some(),
            )
            .field("pre_roll_act_boss", &self.pre_roll_act_boss.is_some())
            .field(
                "apply_combat_gold_and_potion",
                &self.apply_combat_gold_and_potion.is_some(),
            )
            .field(
                "roll_combat_gold_and_potion",
                &self.roll_combat_gold_and_potion.is_some(),
            )
            .field(
                "roll_out_of_combat_potion",
                &self.roll_out_of_combat_potion.is_some(),
            )
            .field(
                "roll_transform_replacement",
                &self.roll_transform_replacement.is_some(),
            )
            .field("build_event_combat", &self.build_event_combat.is_some())
            .field(
                "resume_event_after_combat",
                &self.resume_event_after_combat.is_some(),
            )
            .finish()
    }
}

/// What [`ContentCallbacks::roll_combat_gold_and_potion`] rolled for a won
/// combat room: each gold reward (the room's, then relic bonuses such as
/// Amethyst Aubergine's) and the dropped potion, if any. sts2-agent fork.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoomRewardRoll {
    pub gold: Vec<i32>,
    pub potion: Option<crate::potions::PotionKind>,
}

/// One reward on the opt-in rewards screen ([`RunManager::reward_screen`]).
/// sts2-agent fork.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenReward {
    Gold {
        amount: i32,
    },
    Potion {
        kind: crate::potions::PotionKind,
    },
    Relic {
        relic: crate::runstate::RelicReward,
    },
    /// A card reward with its options already rolled (C# populates every
    /// reward of the set when it is generated).
    Card {
        options: Vec<crate::cards::CardId>,
        upgraded: Vec<bool>,
    },
}

/// Top-level run orchestrator. Owns the run-side state ([`RunState`])
/// and the optional combat-side state ([`CombatState`]) plus the
/// per-run scratch (seed, character, ascension, post-combat heal).
///
/// The struct is intentionally `#[non_exhaustive]` so downstream phase
/// beads can grow private scratch fields (offered cards, pending
/// reward queue, current-room handle, …) without breaking call sites
/// that construct `RunManager` via [`Self::new`].
///
/// `Clone` is cheap-ish: it deep-copies the run-side state ([`RunState`])
/// and the live [`CombatState`] (when in combat) — no heap-shared
/// internals, every field is a plain value (the [`ContentCallbacks`]
/// slots are bare `fn` pointers, so the callback table is `Copy`). The
/// full-run-AI tier-2/3 rollout machinery and the offline labeler clone
/// the manager to simulate candidate futures without perturbing the
/// live run (plan 2026-06-12, P0 — bead `sts2-rs-40ikw.1`).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RunManager {
    /// Run-side state — deck, relics, gold, map, pending rewards.
    pub run_state: RunState,
    /// Live combat state during [`Phase::Combat`], `None` otherwise.
    pub combat: Option<CombatState>,
    /// Master seed for this run. Mirrors Python's `_seed`.
    pub seed: i32,
    /// Character class. Mirrors Python's `_character_id` (string in
    /// Python, typed here).
    pub character_id: CharacterId,
    /// Ascension level (0-20). Stubbed at 0 across the thin slice —
    /// see `feedback: ascension is identity at pin 01c094d`.
    pub ascension_level: u8,
    /// HP healed after every non-boss combat (Ironclad's Burning
    /// Blood is +6; every other starter relic is 0). Cached at
    /// construction from the [`CharacterStarter`] registry.
    pub heal_after_combat: i32,
    /// Current phase. Set by `RunManager::new` to [`Phase::MapChoice`].
    phase: Phase,
    /// Injected content-layer callbacks. Default is all-`None` (every
    /// content-dependent phase dispatch then returns
    /// [`ActionResult::NotImplemented`]). Wire concrete fns via
    /// [`Self::with_content_callbacks`]. Bead `sts2-rs-7kl4f`.
    callbacks: ContentCallbacks,
    /// Currently-offered card-reward options (CardReward phase). Set
    /// when entering CardReward, drained when consumed.
    card_reward_options: Vec<crate::cards::CardId>,
    /// Per-option upgrade flags parallel to `card_reward_options`,
    /// taken from `RunState::card_reward_upgraded_flags` when the
    /// offering is callback-generated (SilverCrucible's pre-upgraded
    /// rewards, act-2+ upgrade rolls). Empty for choose-screen / bundle
    /// offerings, which carry no upgrade state.
    card_reward_upgraded: Vec<bool>,
    /// When `Some`, the current `Phase::CardReward` offering is a Scroll
    /// Boxes *bundle* pick rather than a single-card reward: each inner
    /// `Vec<CardId>` is one pack, the player picks one pack index and **all**
    /// its cards are added to the deck, and no skip is offered. Kept parallel
    /// to `card_reward_options` (which holds one representative card per pack
    /// so the phase stays non-empty and the fan-out count is right); the pick
    /// reads this, not `card_reward_options`. Surfaced as a `bundle_select`
    /// decision by the Python backend. `None` for ordinary card rewards.
    card_reward_bundles: Option<Vec<Vec<crate::cards::CardId>>>,
    /// Currently-offered boss-relic pool (BossRelic phase).
    boss_relic_options: Vec<crate::RelicId>,
    /// Set when a boss-relic pick has routed into the boss's standard
    /// combat reward (gold + potion + card, mirroring C#
    /// `RewardsCmd.OfferForRoomEnd` firing for boss rooms too). The
    /// act-advance + next-map regen is deferred until that reward chain
    /// fully drains, at which point [`Self::advance_after_card_reward`]
    /// consumes this flag and calls [`Self::advance_boss_act`]. Without
    /// the defer, the act would advance (changing `current_room`) before
    /// the boss gold roll reads the boss room type. Bead `sts2-rs-jenwp`
    /// (M4 live reward stream — Boss follow-up).
    pending_boss_act_advance: bool,
    /// `true` while the current `card_reward_options` offering is a
    /// *choose screen* drained from
    /// [`crate::runstate::RunState::pending_card_selects`] (C#
    /// `CardSelectCmd.FromChooseACardScreen` — HeftyTablet's 3 Rares,
    /// LeadPaperweight's 2 Colorless) rather than a post-combat card
    /// reward. The Python backend reads this to emit a `card_select`
    /// decision (`min 0 / max 1`, skippable) matching the C# headless,
    /// instead of `card_reward`. Cleared when the offering resolves.
    card_offer_choose_screen: bool,
    /// Cards appended to the master deck once the active choose screen
    /// resolves (after the chosen card, if any) — see
    /// [`crate::runstate::PendingCardSelect::post_add`]. Drained on both
    /// the pick and the skip path of `do_card_reward_pick`.
    pending_choose_screen_post_add: Vec<crate::cards::Card>,
    /// Whether the open choose screen may be skipped
    /// ([`crate::runstate::PendingCardSelect::can_skip`]); `true` whenever no
    /// such screen is open. sts2-agent fork.
    card_offer_can_skip: bool,
    /// Active shop inventory during [`Phase::Shop`], built by
    /// [`ContentCallbacks::generate_shop_inventory`] on Shop-room
    /// entry and drained on [`RunAction::LeaveShop`]. `None` when no
    /// shop is open OR the callback isn't wired (legacy skip-only
    /// behavior). Bead `sts2-rs-if6ry` (W3).
    shop_inventory: Option<crate::shop::ShopInventory>,
    /// The open rewards screen ([`RunState::interactive_rewards`]), in the
    /// order the game lists it (gold, potion, relic, card). `None` when no
    /// screen is open. sts2-agent fork.
    reward_screen: Option<Vec<ScreenReward>>,
    /// The screen entry whose card reward is open right now (its options in
    /// `card_reward_options`). sts2-agent fork.
    screen_card_claim: Option<usize>,
}

impl RunManager {
    /// Construct a new run for `character_id` seeded by `seed`.
    /// Ascension defaults to 0 (the only ascension level at pin
    /// `01c094d`); use [`Self::with_ascension`] when the A1-A10
    /// follow-up (`sts2-rs-unf7l`) is ready.
    ///
    /// Initializes [`RunState`] with the per-character starter pack
    /// (HP/gold/starter relic via the bead-`jjcb8` registry),
    /// seeds the legacy `rewards_rng_seed`, and leaves
    /// [`Self::combat`] as `None`. Initial phase is
    /// [`Phase::MapChoice`].
    ///
    /// **Not** yet wired:
    /// - Starter-deck card-pod population (lives in `sts2-content`;
    ///   the `enter_combat` bridge in `sts2-rs-tjn0v` pulls it in).
    /// - Map generation (P9.2.b / linear thin-slice in P9.0.e).
    /// - Multi-stream [`crate::RunRngSet`] integration onto
    ///   [`RunState`] (deferred follow-up to bead `sts2-rs-ewm6b`).
    #[must_use]
    pub fn new(seed: i32, character_id: CharacterId) -> Self {
        Self::with_ascension(seed, character_id, 0)
    }

    /// Like [`Self::new`] but takes an explicit ascension level.
    /// Most A1-A10 effects live outside this constructor (map gen, Neow
    /// event, per-monster scaling); the run-state-side effects land via
    /// the per-effect sub-beads filed under `sts2-rs-unf7l`. A3 Poverty
    /// (reward gold × 0.75) lives in `RunState::roll_encounter_gold` +
    /// `resolve_treasure_room`, NOT here (C# never scales starting
    /// gold); wired here are A4 TightBelt
    /// (-1 max potion slot, `sts2-rs-dms9f`), A5 AscendersBane
    /// (append curse to deck, `sts2-rs-9vypz`), A7 Scarcity
    /// (halve card-upgrade scaling + flip *_SCARCITY rarity tables
    /// via the run's `CardRarityOdds`, `sts2-rs-brq2l`), and A10
    /// DoubleBoss (set `has_double_boss = true`, `sts2-rs-oyzw5`).
    #[must_use]
    pub fn with_ascension(seed: i32, character_id: CharacterId, ascension_level: u8) -> Self {
        let starter: CharacterStarter = character_starter(character_id);
        let mut run_state = RunState::new();

        // Bead `sts2-rs-xbkhd` (P9.3.b): thread the character class
        // onto `RunState.character_id` so the run-side sampler walks
        // (character-pool replacement sampling for card / relic
        // rewards, the post-combat heal gate, …) can read the class
        // directly instead of reverse-engineering it from starter-deck
        // contents. Mirrors Python `RunState.__init__` wiring at
        // `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:1383-1387`.
        run_state.character_id = character_id;
        // Bead `sts2-rs-4aunm`: thread ascension onto RunState so
        // `RunState::generate_map` and other map-side consumers can
        // read it without the caller plumbing an extra arg.
        run_state.ascension_level = ascension_level;
        run_state.current_hp = starter.hp;
        run_state.max_hp = starter.hp;
        run_state.gold = starter.gold;
        // A3 Poverty does NOT touch starting gold. C# starting gold is the
        // flat `CharacterModel.StartingGold = 99` (`Player.cs:226` — no
        // ascension hook anywhere on the construction path); the 0.75×
        // multiplier applies only to encounter gold REWARDS
        // (`EncounterModel.cs:52-57`, see `RunState::roll_encounter_gold`)
        // and treasure-chest gold (`OneOffSynchronizer.cs:110-114`, see
        // `resolve_treasure_room`). The Python pin's starting-gold multiply
        // (run_state.py:1513) was fictional — removed per the 2026-06-12
        // ascension audit (bead sts2-rs-40ikw.2).
        // A2 WearyTraveler: Neow is the act-1 Ancient event, whose
        // `BeforeEventStarted` zeroes HP then heals `0.8 * MaxHp` at A2 — so
        // the run begins at 80% HP. (At base ascension Neow heals to full,
        // which is the default full-HP start, so this is a no-op below A2.)
        // C# truncates the decimal heal (`(int)Math.Min(amount, MaxHp)`).
        // Inter-act Ancient heals land in `advance_boss_act` /
        // `advance_act_on_boss_kill` via `RunState::apply_ancient_event_heal`.
        if ascension_level >= crate::runstate::WEARY_TRAVELER_ASCENSION_LEVEL {
            run_state.current_hp = (f64::from(run_state.max_hp)
                * crate::runstate::WEARY_TRAVELER_HEAL_MULTIPLIER)
                .floor() as i32;
        }
        // A4 TightBelt: -1 max potion slot, saturating at 0.
        // Pin: sts2-rl-agent@01c094d run_state.py:1517.
        if ascension_level >= 4 {
            run_state.max_potion_slots = (run_state.max_potion_slots - 1).max(0);
        }
        // A5 AscendersBane: append the unplayable+ethereal curse to the
        // master deck. Pin: sts2-rl-agent@01c094d run_state.py:1520-1532
        // / cards/status.py:160-166. Spec mirrors
        // `sts2-content::cards::ironclad_basic::ascenders_bane`
        // (cost=-1, type=Curse, target=None); spec-only fields
        // (keywords/effect) hang off the `CardId` lookup, not `Card`.
        // Standalone vs the rest of starter-deck plumbing (still
        // pending per the module header) — the curse-add step is its
        // own line in the Python pin, distinct from base-deck assembly.
        if ascension_level >= 5 {
            let mut curse = crate::cards::Card::pod_defaults();
            curse.card_id = crate::cards::CardId::AscendersBane;
            curse.cost = -1;
            curse.card_type = crate::cards::CardType::Curse;
            curse.target_type = crate::cards::TargetType::None;
            run_state.master_deck.push(curse);
        }
        // A7 Scarcity: rebind the run's `CardRarityOdds` to the real
        // ascension level so both the rarity-roller's *_SCARCITY tables
        // (`CardRarityOdds::new`, gated on `SCARCITY_ASCENSION_LEVEL = 7`)
        // and the act-scaled upgrade roll (`roll_for_upgrade` →
        // `SCARCITY_CARD_UPGRADE_ODD_SCALING = 0.125` vs `0.25`) pick up
        // the halved upgrade rate at asc ≥ 7. Both branches were already
        // in place; this just threads the live level into `RunState`. Pin:
        // sts2-rl-agent@01c094d run_state.py:1534-1536 + rewards.py:30-31
        // + odds.py:160-189.
        run_state.card_rarity_odds = crate::card_rewards::CardRarityOdds::new(ascension_level);
        // A10 DoubleBoss: flip the run-level flag at asc ≥ 10. The
        // actual two-boss-per-act map-gen wiring is a P9.A.10b follow-up;
        // this is the field + threshold setter only. Pin:
        // sts2-rl-agent@01c094d run_state.py:1544-1545.
        run_state.has_double_boss = ascension_level >= 10;
        // Seed the legacy single-stream RNG so deterministic tests can
        // rely on a non-zero seed. The named-stream bundle on
        // RunState itself lands in the ewm6b follow-up.
        run_state.rewards_rng_seed = u64::from(seed as u32);
        // Bead `sts2-rs-4d93v`: re-seed the multi-stream RunRngSet from
        // the caller's seed. Without this, `RunState::new()` defaults
        // to `RunRngSet::new(0)` and every map/encounter/reward roll
        // uses the same fixed seed regardless of what the caller
        // passes. Discovered via R3.2 map-parity script: 3 different
        // string seeds produced byte-identical act maps.
        run_state.rng = crate::run_rng_set::RunRngSet::new(seed);
        // Grant the starter relic. Mirrors Python's
        // `player.obtain_relic(config["starter_relic"])`.
        run_state.relics.push(starter.starter_relic);
        run_state.last_obtained_relic = Some(starter.starter_relic);

        // Bead `sts2-rs-7kl4f` (P9-DISPATCH foundation): generate the
        // act-1 map so `Phase::MapChoice` has children to enumerate.
        // Mirrors Python `RunManager.__init__` ordering at
        // `sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:226-241` —
        // the run-start map is rolled before the agent ever sees
        // `get_available_actions`. The relic-grab-bag populate /
        // `after_relic_grab_bags_populated` / `_generate_rooms` /
        // `after_act_entered` legs of `initialize_run` stay caller-
        // driven (workspace-dep direction: their inputs live in
        // `sts2-content`); only the content-agnostic map roll is
        // wired here.
        run_state.generate_map();

        Self {
            run_state,
            combat: None,
            seed,
            character_id,
            ascension_level,
            heal_after_combat: starter.heal_after_combat,
            phase: Phase::MapChoice,
            callbacks: ContentCallbacks::default(),
            card_reward_options: Vec::new(),
            card_reward_upgraded: Vec::new(),
            card_reward_bundles: None,
            boss_relic_options: Vec::new(),
            pending_boss_act_advance: false,
            card_offer_choose_screen: false,
            pending_choose_screen_post_add: Vec::new(),
            card_offer_can_skip: true,
            shop_inventory: None,
            reward_screen: None,
            screen_card_claim: None,
        }
    }

    /// Choose which of the two Act-1 models this run walks (GH #544).
    ///
    /// Builder-style; chain after [`Self::new`] / [`Self::with_ascension`]
    /// and **before** `initialize_run_setup`, which is where the act's rooms
    /// are pre-generated off `up_front`. Setting it afterwards changes
    /// nothing about the rooms already rolled.
    ///
    /// Only [`ActVariant::Overgrowth`] and [`ActVariant::Underdocks`] are
    /// act-1 models; passing an act-2/3 variant leaves act 1 on the default.
    #[must_use]
    pub fn with_act1(mut self, act1: ActVariant) -> Self {
        self.run_state.act1 = act1;
        self
    }

    /// Roll the Act-1 variant from this run's seed, the way the real game
    /// does at `StartRunLobby.BeginRunLocally` (GH #544).
    ///
    /// Uses the dedicated `act_selection` stream and assumes Underdocks is
    /// **unlocked** with `TestMode.IsOn` semantics — i.e. the seed alone
    /// decides, with no unlock gate and no first-discovery force-pick. That
    /// is a deliberate simplification of the live game, where a player's
    /// first Underdocks-eligible run is forced to Underdocks regardless of
    /// seed; see [`crate::act_selection`] for why reproducibility from the
    /// seed is what the training harness needs.
    #[must_use]
    pub fn with_rolled_act1(self) -> Self {
        let acts = crate::act_selection::roll_acts(
            self.seed,
            &crate::act_selection::ActSelectionUnlocks::test_mode_on(true),
        );
        self.with_act1(acts[0])
    }

    /// Inject content-layer callbacks. Builder-style — chain after
    /// [`Self::new`] / [`Self::with_ascension`]. Bead `sts2-rs-7kl4f`.
    #[must_use]
    pub fn with_content_callbacks(mut self, callbacks: ContentCallbacks) -> Self {
        self.callbacks = callbacks;
        // The Act-1 map was generated in [`Self::new`] before callbacks
        // were available, so the act-start boss pre-roll happens here
        // instead. Subsequent acts get their pre-roll inline in
        // [`Self::advance_act_on_boss_kill`] right after the new map is
        // laid out.
        if let Some(pre_roll) = self.callbacks.pre_roll_act_boss {
            pre_roll(&mut self.run_state);
        }
        self
    }

    /// Start the run inside the Neow event room instead of the default
    /// [`Phase::MapChoice`]. Rolls the run-start relic offers via
    /// [`crate::runstate::RunState::roll_neow_offers_if_needed`], sets
    /// [`crate::runstate::RunState::current_event`] to
    /// [`crate::events::EventId::Neow`], and parks the phase at
    /// [`Phase::Event`] so the agent's first action is a Neow choice.
    ///
    /// Mirrors C# / real-game behavior: every captured scraped run
    /// (`../sts2_run_scraper/data/`) starts with `first_room_type='ancient'`
    /// and `EVENT.NEOW` active. The default [`Self::new`] skips Neow so
    /// the ~75 existing run_manager tests and downstream consumers that
    /// rely on "fresh run = ready to pick first map column" keep
    /// working unchanged; the corpus-replay path (sub-bead
    /// `sts2-rs-k8z6m`) opts into Neow-start via this builder.
    ///
    /// Resolving the Neow option transitions through the generic
    /// event-resolve path in [`Self::do_event_choice`] which sets phase
    /// to [`Phase::MapChoice`] — same code path used by every other
    /// `Phase::Event` exit.
    ///
    /// Builder-style; chain after [`Self::new`] /
    /// [`Self::with_content_callbacks`].
    ///
    /// Bead `sts2-rs-g24zv` (R3.1.3 under `sts2-rs-nr8l4`).
    #[must_use]
    pub fn with_neow_start(mut self) -> Self {
        self.run_state.roll_neow_offers_if_needed();
        self.run_state
            .enter_event_room(crate::events::EventId::Neow);
        // C# `RunManager` calls `State.Act.MarkRoomVisited(room.RoomType)` on
        // every top-level room entry (`RunManager.cs:903`), and the Neow room
        // is an EventRoom — so entering it advances act 1's `eventsVisited`
        // cursor to 1 BEFORE any pool event is pulled (verified live:
        // `dump_rooms` right after StartRun shows `events_visited == 1`).
        // Without this the first act-1 event room offers the slot-0 event
        // that C# never sees (RP3: Rust JungleMazeAdventure vs C#
        // RoomFullOfCheese).
        self.run_state.acts[0].events_visited += 1;
        self.phase = Phase::Event;
        self
    }

    /// Access the wired callbacks (mostly for tests).
    #[must_use]
    pub const fn content_callbacks(&self) -> &ContentCallbacks {
        &self.callbacks
    }

    /// Current run phase. Mirrors Python's `phase` property
    /// (`run_manager.py:274-278`). Returns [`Phase::RunOver`] iff
    /// the run is finished (currently never — `take_action` is a
    /// no-op skeleton).
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    /// Set the current phase. Restricted to within-crate callers so
    /// the future per-phase `_enter_*` handlers (sub-modules of
    /// `run_manager`) can transition without exposing a public
    /// "stomp the state machine" footgun. Currently only the unit
    /// tests use it; the `dead_code` allow drops once the first
    /// phase handler lands.
    #[allow(dead_code)]
    pub(crate) fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
    }

    /// Place this run on `act`'s map at `coord` without walking to it.
    ///
    /// The positioning half of a live-state import. The map graph is a pure
    /// function of `(seed, act)`, so a run rebuilt at the same seed already
    /// HAS the node the player is standing on — it just has not travelled
    /// there. This sets the traversal trackers directly (current node,
    /// [`RunState::visited_map_coords`], both floor counters) and parks the
    /// run in [`Phase::MapChoice`], so the next decision is "where do I go
    /// from here" with the correct children enumerated.
    ///
    /// No room is resolved and no `after_room_entered` hook fires: placement
    /// is not a walk, and a walk would consume RNG the importer has no
    /// business consuming — the cursors are restored from the world instead.
    /// Any live combat is discarded.
    ///
    /// `visited` is recorded verbatim (the live wire carries the exact list),
    /// with `coord` appended if it is missing.
    ///
    /// Reconstruction surface, NOT a parity surface: never call it on a
    /// recorded or parity-pinned run.
    ///
    /// # Errors
    /// When `act` is outside this run's range, when the run has already
    /// advanced past `act`, when the act map cannot be generated, or when
    /// `coord` is not a grid-live node on that map.
    pub fn debug_place_on_map(
        &mut self,
        act: u8,
        coord: crate::map::MapCoord,
        visited: Vec<crate::map::MapCoord>,
        act_floor: i32,
        total_floor: i32,
    ) -> Result<(), String> {
        let acts = self.run_state.acts.len().max(3);
        if act < 1 || usize::from(act) > acts {
            return Err(format!("act {act} is outside this run's 1..={acts}"));
        }
        if self.run_state.act_index > act {
            return Err(format!(
                "run is already at act {}, cannot go back to act {act}",
                self.run_state.act_index
            ));
        }
        // Advance the way the run itself would (per-act reset + regeneration
        // off the act's own map RNG), so the map placed onto is the one this
        // run would have generated on arrival.
        while self.run_state.act_index < act {
            self.run_state.advance_act();
            self.run_state.generate_map();
        }
        if self.run_state.current_act_map.is_none() {
            self.run_state.generate_map();
        }
        let point_type = {
            let map = self
                .run_state
                .current_act_map
                .as_ref()
                .ok_or_else(|| format!("act {act}'s map could not be generated"))?;
            let idx = *map.grid.get(&coord).ok_or_else(|| {
                format!(
                    "({}, {}) is not a live node on act {act}'s map",
                    coord.col, coord.row
                )
            })?;
            map.point(idx).point_type
        };
        self.run_state.visited_map_coords = visited;
        if !self.run_state.visited_map_coords.contains(&coord) {
            self.run_state.visited_map_coords.push(coord);
        }
        self.run_state.act_floor = act_floor;
        self.run_state.total_floor = total_floor;
        self.run_state.current_room = Some(crate::runstate::Room::at(
            coord,
            point_type,
            point_type.to_room_type(),
        ));
        self.combat = None;
        self.phase = Phase::MapChoice;
        Ok(())
    }

    /// Drop the run into `combat` as it stands — the workbench's state-import
    /// combat lane (GH #708): a fight rebuilt from a live-game paste
    /// (`sts2_wire::state_from_json`) grafted onto the run the run lane
    /// rebuilt, so the play loop, the advisors and the post-combat flow run
    /// over the exact board. Nothing is rolled: the combat carries its own
    /// RNG lineage. A run with no current room (a map-less paste parked at
    /// the act entrance) gets a map-less room of `room_type` so the reward
    /// chain after the fight knows what kind of fight it was.
    pub fn debug_enter_combat(
        &mut self,
        combat: CombatState,
        room_type: crate::runstate::RoomType,
    ) {
        if self.run_state.current_room.is_none() {
            self.run_state.current_room = Some(crate::runstate::Room::new(room_type));
        }
        self.combat = Some(combat);
        self.phase = Phase::Combat;
    }

    /// Actions legal in the current phase. Empty `Vec` when the run
    /// is over or the current phase has no callback wired. Bead
    /// `sts2-rs-7kl4f`.
    #[must_use]
    pub fn get_available_actions(&self) -> Vec<RunAction> {
        // Interactive deck removal preempts the current phase's actions:
        // an on-obtain relic (e.g. Precise Scissors) queues removals that
        // the player resolves before the run continues, regardless of which
        // phase granted the relic. Offer one `RemoveDeckCard` per deck card
        // (removal is mandatory — no skip, matching C#'s forced card_select).
        if self.run_state.pending_deck_removals > 0 {
            // `idx` indexes the C# `FromDeckForRemoval` offer order (removable
            // cards, curses sorted to the front), NOT raw master-deck position —
            // see `RunState::removal_offer_order`. `do_remove_deck_card_at` maps
            // it back. Bead: deck-removal curse-front parity (RP94).
            let n = self.run_state.removal_offer_order().len();
            return (0..n)
                .map(|idx| RunAction::RemoveDeckCard { idx })
                .collect();
        }
        // Interactive deck upgrade (Pomander) preempts the current phase's
        // actions the same way removals do. C# `FromDeckForUpgrade` only lets
        // the player pick NON-upgraded cards and indexes them by their
        // position in that upgradeable subset (the headless `PendingOptions`
        // index), so offer `UpgradeDeckCard{idx}` for `idx` in
        // `0..num_non_upgraded` — NOT raw master-deck indices. Upgrade is
        // mandatory (no skip, matching C#'s forced upgrade card_select).
        // Bead `sts2-rs-1hiht`.
        if self.run_state.pending_deck_upgrades > 0 {
            // C# `FromDeckForUpgrade` lists cards `Where(IsUpgradable)`, NOT
            // merely non-upgraded: non-upgradable Curse/Status (INJURY, GUILTY,
            // …) must be excluded or they occupy indices and shift the
            // lockstep-replayed pick to a different card. Must match the
            // `do_upgrade_deck_card_at` resolution and the pending-choice option
            // builder below so `idx` maps to the same deck card everywhere.
            let upgradeable = self
                .run_state
                .master_deck
                .iter()
                .filter(|c| c.is_upgradable())
                .count();
            return (0..upgradeable)
                .map(|idx| RunAction::UpgradeDeckCard { idx })
                .collect();
        }
        // Interactive deck transform (NewLeaf / Wood Carvings Bird+Torus)
        // preempts the same way. `idx` is the position within the ELIGIBLE
        // subset for the front pending entry — the whole deck for the
        // unfiltered NewLeaf shape (C# `FromDeckForTransformation`'s
        // `Type != Quest && IsTransformable` over-offer, documented at
        // `transform_eligible_deck_indices`) or the Basic-rarity subset for
        // Wood Carvings (C# `FromDeckGeneric(c => c.IsTransformable &&
        // c.Rarity == Basic)`) — matching the C# headless card_select index
        // space. Mandatory, no skip (`CardSelectorPrefs(
        // TransformSelectionPrompt, N)` → min==max==N).
        // Bead 6ynt.4.deck-add / GH #34.
        if !self.run_state.pending_deck_transforms.is_empty() {
            return (0..self.transform_eligible_deck_indices().len())
                .map(|idx| RunAction::TransformDeckCard { idx })
                .collect();
        }
        // Interactive deck enchant (Wood Carvings Snake → Slither). `idx`
        // indexes the eligible subset (C# `FromDeckForEnchantment` pool =
        // cards `CanEnchant` accepts). Mandatory, no skip.
        if !self.run_state.pending_deck_enchants.is_empty() {
            return (0..self.enchant_eligible_deck_indices().len())
                .map(|idx| RunAction::EnchantDeckCard { idx })
                .collect();
        }
        // Interactive multi-pick card grant (RoomFullOfCheese Gorge). `idx`
        // indexes the front grant's current generated options; one action per
        // option (C# `FromSimpleGridForRewards` grid). Mandatory, no skip.
        // Bead `sts2-rs-gorge-select`.
        if let Some(grant) = self.run_state.pending_card_grants.first() {
            return (0..grant.options.len())
                .map(|idx| RunAction::GrantDeckCard { idx })
                .collect();
        }
        // Interactive full-belt potion swap (GH #252): a potion reward
        // that found no free slot preempts the same way. Decline is
        // listed FIRST so a policy that blindly takes the first option
        // preserves the historical drop-the-reward behaviour instead of
        // always discarding belt slot 0; skippable, matching the C#
        // reward screen (the reward can simply be left unclaimed).
        if !self.run_state.pending_potion_swaps.is_empty() {
            return std::iter::once(RunAction::PotionSwap { discard_slot: None })
                .chain(
                    (0..self.run_state.potions.len()).map(|slot| RunAction::PotionSwap {
                        discard_slot: Some(slot),
                    }),
                )
                .collect();
        }
        let mut v = self.phase_actions();
        self.push_potion_actions(&mut v);
        v
    }

    /// Whether the phase's own decision shows the potion belt (the host's
    /// potion decisions: not a deck pick, a combat choice, a Scroll Boxes
    /// bundle, a choose-a-card screen or the Smith pick). sts2-agent fork.
    fn belt_shown(&self) -> bool {
        match self.phase {
            Phase::RunOver => false,
            Phase::Combat => self
                .combat
                .as_ref()
                .is_some_and(|c| !c.is_over && c.pending_choice.is_none()),
            Phase::CardReward => {
                self.card_reward_bundles.is_none() && !self.card_offer_choose_screen
            }
            Phase::RestSite => self
                .run_state
                .pending_choice
                .as_ref()
                .map_or(true, |pc| pc.kind != crate::ChoiceKind::CardSelect),
            _ => true,
        }
    }

    /// [`RunState::interactive_potions`]: a discard per held potion, and
    /// outside combat a drink per AnyTime potion the game lets the player
    /// drink here. Combat drinks come from the combat's own legal actions.
    /// sts2-agent fork.
    fn push_potion_actions(&self, v: &mut Vec<RunAction>) {
        if !self.run_state.interactive_potions || !self.belt_shown() || self.potions_locked() {
            return;
        }
        if self.phase == Phase::Combat {
            let n = self.combat.as_ref().map_or(0, |c| c.player.potions.len());
            v.extend((0..n).map(|slot| RunAction::DiscardPotion { slot }));
            return;
        }
        for (slot, &kind) in self.run_state.potions.iter().enumerate() {
            if self.drinkable_out_of_combat(kind) {
                v.push(RunAction::DrinkPotion {
                    potion_slot: slot,
                    target: None,
                });
            }
        }
        v.extend((0..self.run_state.potions.len()).map(|slot| RunAction::DiscardPotion { slot }));
    }

    /// The AnyTime potions' out-of-combat use (C# `PotionUsage.AnyTime` and
    /// `PassesCustomUsabilityCheck`): Foul Potion only at the merchant
    /// (`FoulPotion.PassesCustomUsabilityCheck`). sts2-agent fork.
    fn drinkable_out_of_combat(&self, kind: crate::potions::PotionKind) -> bool {
        use crate::potions::PotionKind;
        match kind {
            PotionKind::BloodPotion | PotionKind::FruitJuice | PotionKind::EntropicBrew => true,
            PotionKind::FoulPotion => self.phase == Phase::Shop,
            _ => false,
        }
    }

    fn has_open_potion_slot(&self) -> bool {
        let cap =
            (self.run_state.max_potion_slots.max(0) as usize).min(crate::potions::MAX_POTIONS);
        self.run_state.potions.len() < cap
    }

    fn phase_actions(&self) -> Vec<RunAction> {
        use crate::runstate::RestSiteOption;
        match self.phase {
            Phase::RunOver => Vec::new(),
            Phase::MapChoice => self
                .next_map_children_cols()
                .into_iter()
                .map(|col| RunAction::Move { col })
                .collect(),
            Phase::Combat => {
                let Some(combat) = self.combat.as_ref() else {
                    return Vec::new();
                };
                if combat.is_over {
                    return Vec::new();
                }
                // Bead `sts2-rs-rbb10`: delegate Combat enumeration to
                // the canonical combat-side [`crate::legal_actions`]
                // and translate each [`crate::Action`] to a
                // [`RunAction`]. This fixes the infinite-loop bug from
                // the rkz98 partial-fix: the prior hand-rolled
                // enumeration always emitted `target: None`, which
                // makes Enemy-targeted cards (Strike, Bash) no-op at
                // [`crate::combat_loop::play_card`]'s target-validation
                // gate — every PlayCard step left combat unchanged so
                // the driver looped forever without dealing damage.
                //
                // `crate::legal_actions` already fans out target=Some(i)
                // for each alive enemy on Enemy cards, respects energy
                // affordability via `effective_card_cost`, and emits
                // EndTurn as the trivial fallback.
                //
                // Bead `sts2-rs-6dtxp`: when combat has an open
                // `pending_choice`, `legal_actions` returns ONLY
                // `ResolveChoice` variants (no PlayCard / EndTurn). The
                // prior code dropped every `ResolveChoice` and then the
                // "empty → push EndTurn" fallback injected a bogus
                // EndTurn. Taking that EndTurn abandoned the unresolved
                // choice; the still-set `pending_choice` then poisoned
                // the next `start_player_turn` into resting at
                // `PlayerTurnStart` (deferred turn-setup), and the
                // following EndTurn tripped the `PlayerPlayPhase`
                // `debug_assert` in `end_player_turn_phase_one`. The run
                // loop must instead resolve the choice — surface each
                // legal selection as a `CombatResolveChoice` and never
                // fall through to the EndTurn fallback while a choice is
                // open.
                let has_pending_choice = combat.pending_choice.is_some();
                let mut v: Vec<RunAction> = Vec::new();
                for action in crate::legal_actions(combat) {
                    match action {
                        crate::Action::PlayCard { hand_index, target } => {
                            v.push(RunAction::PlayCard {
                                hand_idx: usize::from(hand_index),
                                target: target.map(u32::from),
                            });
                        }
                        crate::Action::EndTurn => v.push(RunAction::EndTurn),
                        crate::Action::ResolveChoice { selection } => {
                            // Bead `sts2-rs-sopgf`: `legal_actions` now fans a
                            // `max_select > 1` choice out SEQUENTIALLY — one
                            // incremental single-pick `[i]` per still-unpicked
                            // option plus a FINALIZE `[]` — so it emits only
                            // length-0 / length-1 selections (each maps to a
                            // `CombatResolveChoice`, which accumulates in
                            // `pc.picked` and re-opens until FINALIZE). The
                            // length >= 2 arm is kept as a defensive fallback
                            // for any external full-set selection and routes to
                            // the atomic take-first path.
                            match selection.as_slice() {
                                [] => v.push(RunAction::CombatResolveChoice { option: None }),
                                [i] => v.push(RunAction::CombatResolveChoice {
                                    option: Some(usize::from(*i)),
                                }),
                                #[allow(clippy::cast_possible_truncation)]
                                sel => v.push(RunAction::CombatResolveChoiceTakeFirst {
                                    count: sel.len() as u8,
                                }),
                            }
                        }
                        crate::Action::DrinkPotion {
                            potion_slot,
                            target,
                        } => {
                            v.push(RunAction::DrinkPotion {
                                potion_slot: usize::from(potion_slot),
                                target: target.map(u32::from),
                            });
                        }
                        // GH #303: the legacy alias maps onto the same
                        // RunAction as the canonical variant. Dropping it
                        // here was survivable only because
                        // `legal_actions` never emits it (asserted by
                        // `legal_actions_emits_drink_potion_per_slot`) —
                        // but this loop is fed a caller-supplied action
                        // list, so "unreachable today" is not a
                        // guarantee, and a dropped potion action silently
                        // shortens the run-level menu.
                        crate::Action::UsePotion { slot, target } => {
                            v.push(RunAction::DrinkPotion {
                                potion_slot: usize::from(slot),
                                target: target.map(u32::from),
                            });
                        }
                        // Channel / Evoke have no RunAction equivalent
                        // yet — file a follow-up bead when orb actions
                        // surface as a run-loop need.
                        _ => {}
                    }
                }
                if v.is_empty() && !has_pending_choice {
                    // legal_actions emits at least EndTurn unless the
                    // combat is over (guarded above) — defensive
                    // fallback so the driver never gets a stuck empty
                    // action list. Suppressed when a choice is open: a
                    // bogus EndTurn there abandons the choice and
                    // (eventually) trips the PlayerTurnStart assert.
                    v.push(RunAction::EndTurn);
                }
                v
            }
            Phase::CardReward => {
                // sts2-agent fork: the open rewards screen, unless something it
                // opened is up (its card reward, a relic a claim queued).
                if let Some(screen) = &self.reward_screen {
                    if self.screen_card_claim.is_none()
                        && self.card_reward_options.is_empty()
                        && self.card_reward_bundles.is_none()
                        && self.run_state.relic_rewards.is_empty()
                    {
                        let open_slot = self.has_open_potion_slot();
                        let mut v: Vec<RunAction> = screen
                            .iter()
                            .enumerate()
                            .filter(|(_, r)| open_slot || !matches!(r, ScreenReward::Potion { .. }))
                            .map(|(idx, _)| RunAction::ClaimReward { idx })
                            .collect();
                        v.push(RunAction::ProceedRewards);
                        return v;
                    }
                }
                // CardReward sub-states fan out from `advance_after_card_reward`:
                // (a) `card_reward_options` non-empty → PickCardReward{Some(i)}
                //     for each option + PickCardReward{None} for skip;
                // (b) `card_reward_options` empty + `relic_rewards` non-empty →
                //     PickRelicReward / SkipRelic for the head of the queue;
                // (c) both empty → unreachable (handler would have phase-
                //     advanced to MapChoice; emit empty so a stuck loop is
                //     visible to the caller rather than papered over).
                if let Some(bundles) = &self.card_reward_bundles {
                    // Scroll Boxes: pick exactly one pack, no skip, no
                    // Pael's Wing sacrifice (C# `FromChooseABundleScreen`
                    // offers no skip — the relic forces a pack choice).
                    (0..bundles.len())
                        .map(|i| RunAction::PickCardReward { idx: Some(i) })
                        .collect()
                } else if !self.card_reward_options.is_empty() {
                    let mut v: Vec<RunAction> = (0..self.card_reward_options.len())
                        .map(|i| RunAction::PickCardReward { idx: Some(i) })
                        .collect();
                    // A forced pick (Brain Leech's grid) offers neither a skip
                    // nor Pael's Wing's sacrifice (that is a card reward's).
                    if !self.card_offer_can_skip {
                        return v;
                    }
                    v.push(RunAction::PickCardReward { idx: None });
                    // PAELS_WING surface: only offered when the player
                    // owns the relic AND a card-reward is currently up.
                    // Mirrors Python `_actions` arm at
                    // `sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:945-946`.
                    if self.run_state.owns_active_relic(crate::RelicId::PaelsWing) {
                        v.push(RunAction::SacrificeCardReward);
                    }
                    v
                } else if !self.run_state.relic_rewards.is_empty() {
                    vec![RunAction::PickRelicReward, RunAction::SkipRelic]
                } else {
                    Vec::new()
                }
            }
            Phase::BossRelic => (0..self.boss_relic_options.len())
                .map(|idx| RunAction::PickBossRelic { idx })
                .collect(),
            Phase::Shop => {
                // Bead sts2-rs-if6ry (W3): when an inventory is wired,
                // emit one action per affordable, in-stock item plus
                // LeaveShop. Sold-out slots (price ==
                // SHOP_ENTRY_SOLD_OUT_PRICE) and slots with empty
                // payloads are filtered out. With no inventory wired
                // we keep the legacy skip-only behavior.
                let Some(inv) = self.shop_inventory.as_ref() else {
                    return vec![RunAction::LeaveShop];
                };
                let gold = self.run_state.gold;
                let mut actions: Vec<RunAction> = Vec::new();
                let card_total = inv.cards.len() + inv.colorless_cards.len();
                for idx in 0..card_total {
                    let entry = if idx < inv.cards.len() {
                        &inv.cards[idx]
                    } else {
                        &inv.colorless_cards[idx - inv.cards.len()]
                    };
                    if crate::shop::is_shop_entry_available(entry.price)
                        && entry.price <= gold
                        && entry.card.is_some()
                    {
                        actions.push(RunAction::ShopBuyCard { idx });
                    }
                }
                for (idx, entry) in inv.relics.iter().enumerate() {
                    if crate::shop::is_shop_entry_available(entry.price)
                        && entry.price <= gold
                        && entry.relic_id.is_some()
                    {
                        actions.push(RunAction::ShopBuyRelic { idx });
                    }
                }
                // GH #270: drop the whole potion row when the purchase
                // is guaranteed to refuse — `buy_potion` returns
                // `PotionBeltFull` on a full belt AND on the
                // `should_procure_potion` veto (Sozu), and
                // `do_shop_buy_potion` maps that to `Advanced` with
                // zero state change (mirroring the C# merchant
                // accepting the click and doing nothing). Surfacing it
                // livelocks deterministic greedy policies: identical
                // observation → identical argmax, forever, until the
                // stall watchdog kills the run as a loss. C# greys the
                // entry visually; this filter is the AI-side grey.
                let potion_cap = (self.run_state.max_potion_slots.max(0) as usize)
                    .min(crate::potions::MAX_POTIONS);
                let potion_row_locked = self.run_state.potions.len() >= potion_cap
                    || crate::run_effects::would_veto_procure_potion(&self.run_state);
                if !potion_row_locked {
                    for (idx, entry) in inv.potions.iter().enumerate() {
                        if crate::shop::is_shop_entry_available(entry.price)
                            && entry.price <= gold
                            && entry.potion_kind.is_some()
                        {
                            actions.push(RunAction::ShopBuyPotion { idx });
                        }
                    }
                }
                if !inv.removal_used && gold >= inv.removal_cost {
                    // v0.107.1 shop removal is `CardSelectCmd.FromDeckForRemoval`
                    // with NO extra filter (`OneOffSynchronizer.DoMerchantCard
                    // Removal` → `CardModel.IsRemovable`: `Type != Quest &&
                    // !Eternal`) — basic Strikes / Defends ARE removable at the
                    // shop; only Eternal cards are excluded. The old per-id
                    // basic-card filter (bead `sts2-rs-hrz2m`, a Python-pin
                    // rule) hid the standard Strike-thinning line from the
                    // search and desynced the shadow's removal select vs the
                    // DLL (census seeds 1042/1045/1047/1055). Enumerate in the
                    // C# offer order (curse-front) so pick indices line up.
                    for deck_idx in self.run_state.removal_offer_order() {
                        actions.push(RunAction::ShopRemoveCard { deck_idx });
                    }
                }
                actions.push(RunAction::LeaveShop);
                actions
            }
            Phase::RestSite => {
                // Smith sub-state: a Smith pick with >1 upgradable cards
                // opens a CardSelect `pending_choice` (see
                // `RunState::do_rest_site_action` → SmithAwaitingChoice).
                // The phase stays at RestSite until the agent picks
                // which card to upgrade; surface those picks as
                // `SmithCard{idx}` so the action dispatch can resolve
                // the pending choice and apply the upgrade.
                //
                // Port pin: mirrors Python
                // `sts2-rl-agent@01c094d:sts2_env/run/rest_site.py:96-105`
                // (`SmithRestSiteOption.execute` → `request_deck_choice`
                // over the `candidates = [c for c in deck if not
                // c.upgraded]` set) and C# `v0.103.2 MegaCrit.Sts2.Core
                // .Entities.RestSite/SmithRestSiteOption.cs:60-65`
                // (`CardSelectorPrefs(UpgradeSelectionPrompt, SmithCount=1)`
                // → `CardSelectCmd.FromDeckForUpgrade`).
                //
                // Bead `sts2-rs-5abnx` (W5b); pin headers added by
                // `sts2-rs-z4qwd`.
                if let Some(pc) = self.run_state.pending_choice.as_ref() {
                    if pc.kind == crate::ChoiceKind::CardSelect {
                        return (0..pc.options.len())
                            .map(|idx| RunAction::SmithCard { idx })
                            .collect();
                    }
                }
                // Enumerate one `RestOption{idx}` per option the rest site
                // actually offers. `rest_site_options` builds `[Rest, Smith]`
                // then folds the `modify_rest_site_options` channel (Shovel →
                // Dig, MeatCleaver → Cook) and appends `Hatch` when the deck
                // holds a `ByrdonisEgg` — so the offered set is dynamic. The
                // accessor is `&self` (its listeners only read RunState), so
                // this `&self` enumeration shares the exact index space
                // `do_rest_site_at` resolves against. Bead `sts2-rs-RP24`
                // (was hard-coded `[0, 1]`, which made Hatch/Dig/Cook
                // unselectable — an `act_reject` whenever C# offered one).
                let _ = RestSiteOption::Rest;
                (0..self.run_state.rest_site_options().len())
                    .map(|idx| RunAction::RestOption { idx })
                    .collect()
            }
            Phase::Event => {
                // Bead sts2-rs-nzdz1: enumerate one EventChoice per
                // option offered by the active event. When no
                // event_option_count callback is wired OR the active
                // event has not yet been picked (current_event ==
                // None — engine will lazily pick inside the
                // resolve_event_at callback), fall back to a single
                // idx=0 entry so the existing first-option-only
                // acceptance driver still has something to take.
                let n = self
                    .callbacks
                    .event_option_count
                    .map(|f| f(&self.run_state))
                    .filter(|&n| n > 0)
                    .unwrap_or(1);
                (0..n).map(|idx| RunAction::EventChoice { idx }).collect()
            }
            Phase::Treasure => vec![RunAction::CollectTreasure],
        }
    }

    /// Apply `action`. Dispatches to the per-phase handler. Returns
    /// [`ActionResult::RunOver`] when the run has finished,
    /// [`ActionResult::InvalidForPhase`] when the action variant is
    /// not legal in the current phase, [`ActionResult::NotImplemented`]
    /// when a required callback (see [`ContentCallbacks`]) is not
    /// wired, and [`ActionResult::Advanced`] on success. Bead
    /// `sts2-rs-7kl4f`.
    pub fn take_action(&mut self, action: RunAction) -> ActionResult {
        if self.phase == Phase::RunOver {
            return ActionResult::RunOver;
        }
        // Phase-agnostic: a pending interactive deck removal is resolved
        // before any phase action (it preempts the action surface in
        // `get_available_actions`).
        if let RunAction::RemoveDeckCard { idx } = action {
            return self.do_remove_deck_card_at(idx);
        }
        // Phase-agnostic, same as removal: drain a pending interactive deck
        // upgrade before any phase action. Bead `sts2-rs-1hiht`.
        if let RunAction::UpgradeDeckCard { idx } = action {
            return self.do_upgrade_deck_card_at(idx);
        }
        // Phase-agnostic: drain a pending interactive deck transform
        // (NewLeaf / Wood Carvings). Bead 6ynt.4.deck-add / GH #34.
        if let RunAction::TransformDeckCard { idx } = action {
            return self.do_transform_deck_card_at(idx);
        }
        // Phase-agnostic: drain a pending interactive deck enchant
        // (Wood Carvings Snake).
        if let RunAction::EnchantDeckCard { idx } = action {
            return self.do_enchant_deck_card_at(idx);
        }
        // Phase-agnostic: drain a pending interactive multi-pick card grant
        // (RoomFullOfCheese Gorge). Bead `sts2-rs-gorge-select`.
        if let RunAction::GrantDeckCard { idx } = action {
            return self.do_grant_deck_card_at(idx);
        }
        // Phase-agnostic: resolve a pending full-belt potion-reward swap
        // (GH #252).
        if let RunAction::PotionSwap { discard_slot } = action {
            return self.do_potion_swap(discard_slot);
        }
        // sts2-agent fork: the rewards screen and the opt-in potion actions,
        // each accepted only where `get_available_actions` offers it.
        match action {
            RunAction::ClaimReward { .. }
            | RunAction::ProceedRewards
            | RunAction::DiscardPotion { .. } => {
                if !self.get_available_actions().contains(&action) {
                    return ActionResult::InvalidForPhase;
                }
                return match action {
                    RunAction::ClaimReward { idx } => self.do_claim_reward(idx),
                    RunAction::ProceedRewards => self.do_proceed_rewards(),
                    RunAction::DiscardPotion { slot } => self.do_discard_potion(slot),
                    _ => unreachable!(),
                };
            }
            RunAction::DrinkPotion { potion_slot, .. } if self.phase != Phase::Combat => {
                if !self.get_available_actions().contains(&action) {
                    return ActionResult::InvalidForPhase;
                }
                return self.do_drink_potion_out_of_combat(potion_slot);
            }
            _ => {}
        }
        match (self.phase, action) {
            (Phase::MapChoice, RunAction::Move { col }) => self.do_map_move(col),
            (Phase::Combat, RunAction::PlayCard { hand_idx, target }) => {
                self.do_combat_play_card(hand_idx, target)
            }
            (Phase::Combat, RunAction::EndTurn) => self.do_combat_end_turn(),
            (Phase::Combat, RunAction::CombatResolveChoice { option }) => {
                self.do_combat_resolve_choice(option)
            }
            (Phase::Combat, RunAction::CombatResolveChoiceTakeFirst { count }) => {
                // ATOMIC: take-first forces the full `0..count` prefix in one
                // shot, so it bypasses sequential accumulation (bead
                // `sts2-rs-sopgf`).
                self.do_combat_resolve_choice_selection((0..count).collect(), /*atomic=*/ true)
            }
            (
                Phase::Combat,
                RunAction::DrinkPotion {
                    potion_slot,
                    target,
                },
            ) => self.do_combat_drink_potion(potion_slot, target),
            (Phase::CardReward, RunAction::PickCardReward { idx }) => self.do_card_reward_pick(idx),
            (Phase::CardReward, RunAction::PickRelicReward) => self.do_relic_reward_pick(),
            (Phase::CardReward, RunAction::SkipRelic) => self.do_relic_reward_skip(),
            (Phase::CardReward, RunAction::SacrificeCardReward) => self.do_card_reward_sacrifice(),
            (Phase::BossRelic, RunAction::PickBossRelic { idx }) => self.do_boss_relic_pick(idx),
            (Phase::Shop, RunAction::LeaveShop) => {
                self.shop_inventory = None;
                self.phase = Phase::MapChoice;
                ActionResult::Advanced
            }
            (Phase::Shop, RunAction::ShopBuyCard { idx }) => self.do_shop_buy_card(idx),
            (Phase::Shop, RunAction::ShopBuyRelic { idx }) => self.do_shop_buy_relic(idx),
            (Phase::Shop, RunAction::ShopBuyPotion { idx }) => self.do_shop_buy_potion(idx),
            (Phase::Shop, RunAction::ShopRemoveCard { deck_idx }) => {
                self.do_shop_remove_card(deck_idx)
            }
            (Phase::RestSite, RunAction::RestOption { idx }) => self.do_rest_site_at(idx),
            (Phase::RestSite, RunAction::SmithCard { idx }) => self.do_smith_card_at(idx),
            (Phase::Event, RunAction::EventChoice { idx }) => self.do_event_choice(idx),
            (Phase::Treasure, RunAction::CollectTreasure) => {
                // Open the chest. `enter_treasure` queued the relic via
                // `offer_relic_rewards`; surface it on the standard relic-reward
                // screen (Phase::CardReward fans out PickRelicReward/SkipRelic)
                // so the player sees which relic they got, then takes it. An
                // empty chest (pools exhausted) skips straight to the map.
                // Previously this silently auto-granted and jumped to the map,
                // so the player got no on-screen feedback.
                if self.run_state.relic_rewards.is_empty() {
                    self.phase = Phase::MapChoice;
                } else {
                    self.phase = Phase::CardReward;
                }
                ActionResult::Advanced
            }
            _ => ActionResult::InvalidForPhase,
        }
    }

    // ── Dispatch helpers ─────────────────────────────────────────────

    /// Available column values for the next map step. Mirrors Python's
    /// `_actions_map_choice` shape (current row's children).
    fn next_map_children_cols(&self) -> Vec<i32> {
        let Some(act_map) = self.run_state.current_act_map.as_ref() else {
            return Vec::new();
        };
        let cur = self.run_state.current_room.and_then(|r| r.map_coord);
        let children: Vec<&crate::map::MapPoint> = match cur {
            Some(coord) => act_map.children_of(coord),
            None => {
                // Row 0 — the start row. The legal first moves are the
                // start node's children, NOT every point at row 1.
                // `prune_and_repair` removes duplicate path segments from
                // `grid` but leaves the pruned ghost nodes in `points`
                // (see `full_map_view`'s grid-live filter / bead
                // `sts2-rs-l8hcp`). Reading `points.filter(row == 1)`
                // here surfaced those ghosts as moves; a ghost has no
                // children, so the move fell through to the boss below —
                // collapsing every run to "row-1 combat → act boss".
                // Use the start node's children instead, exactly as the
                // `Some` branch uses `children_of`.
                match act_map.start {
                    Some(s) => act_map.children_of(act_map.point(s).coord),
                    None => Vec::new(),
                }
            }
        };
        // Drop any pruned ghost children (present in `points` but removed
        // from `grid`), mirroring `full_map_view` so the move space and
        // the rendered map agree.
        // WingedBoots: while ignore-paths uses remain, the next move may
        // be ANY grid-live node in the next row, not just connected
        // children (the children are always a subset). Mirrors the C#
        // `RunSimulator.MapSelectState` `GetPointsInRow(nextRow)`
        // override. Only applies mid-map (current coord known) — the
        // start-row surface and the final-row→boss fall-through stay
        // unchanged, matching C# (the override lives inside the
        // `currentPoint != null` branch, gated on uses remaining).
        if let Some(coord) = cur {
            if self.run_state.winged_boots_uses_remaining() {
                let row_cols: Vec<i32> = act_map
                    .points_in_row(coord.row + 1)
                    .iter()
                    .map(|p| p.col())
                    .collect();
                // `points_in_row` is already grid-live + col-sorted.
                if !row_cols.is_empty() {
                    return row_cols;
                }
            }
        }
        let mut cols: Vec<i32> = children
            .iter()
            .filter(|p| act_map.grid.contains_key(&p.coord))
            .map(|p| p.col())
            .collect();
        if !cols.is_empty() {
            cols.sort_unstable();
            return cols;
        }
        // No children — final row. Fall through to boss node.
        if let Some(boss_idx) = act_map.boss {
            return vec![act_map.point(boss_idx).col()];
        }
        Vec::new()
    }

    fn do_map_move(&mut self, col: i32) -> ActionResult {
        // `off_path` rides out of the resolver block alongside the target
        // so the `WingedBoots` charge can be spent after the immutable
        // `act_map` borrow ends (the decrement needs `&mut self`).
        let (target_point, off_path): (Option<crate::map::MapPoint>, bool) = {
            let Some(act_map) = self.run_state.current_act_map.as_ref() else {
                return ActionResult::NotImplemented;
            };
            let cur = self.run_state.current_room.and_then(|r| r.map_coord);
            // Resolve the act boss node iff its col matches `col`. Shared
            // by both branches below.
            let boss_if_col = || {
                act_map
                    .boss
                    .map(|idx| act_map.point(idx).clone())
                    .filter(|p| p.col() == col)
            };
            // Grid-live children of `coord` (drops pruned ghosts that
            // linger in `points` but were removed from `grid`), matching
            // `next_map_children_cols` so every offered Move resolves.
            let live_children = |coord: crate::map::MapCoord| -> Vec<crate::map::MapPoint> {
                act_map
                    .children_of(coord)
                    .into_iter()
                    .filter(|p| act_map.grid.contains_key(&p.coord))
                    .cloned()
                    .collect()
            };
            match cur {
                Some(coord) => {
                    let children = live_children(coord);
                    if let Some(p) = children.iter().find(|p| p.col() == col) {
                        // On-path move to a connected child — no charge.
                        (Some(p.clone()), false)
                    } else {
                        // Not a connected child. `WingedBoots` (if uses
                        // remain) lets the player ignore paths and travel
                        // to any grid-live next-row node, spending one
                        // ignore-paths use. Mirrors C# `DoMapSelect`'s
                        // `IsOffPathChoice` + `_wingedBootsOffPathUsed++`.
                        let off_target = if self.run_state.winged_boots_uses_remaining() {
                            act_map
                                .points_in_row(coord.row + 1)
                                .into_iter()
                                .find(|p| p.col() == col)
                                .cloned()
                        } else {
                            None
                        };
                        if let Some(off) = off_target {
                            (Some(off), true)
                        } else if children.is_empty() {
                            // Final row before the act boss: the only
                            // forward move is the boss.
                            // `next_map_children_cols` surfaces the boss
                            // col here, so `do_map_move` must accept it
                            // too — otherwise `get_available_actions`
                            // offers a `Move` this resolver rejects,
                            // stalling the run loop. Bead `sts2-rs-wt4w1`
                            // (surfaced when on-obtain dispatch shifted a
                            // trajectory onto a final-row node).
                            (boss_if_col(), false)
                        } else {
                            (None, false)
                        }
                    }
                }
                // Start row: the start node's grid-live children, NOT
                // `points.filter(row == 1)` (which leaks pruned ghosts —
                // see `next_map_children_cols`). Falls through to the
                // boss col for the degenerate single-row map. WingedBoots
                // does not override the start row (matches C#, whose
                // override lives only in the `currentPoint != null` path).
                None => (
                    act_map
                        .start
                        .map(|s| act_map.point(s).coord)
                        .and_then(|sc| live_children(sc).into_iter().find(|p| p.col() == col))
                        .or_else(boss_if_col),
                    false,
                ),
            }
        };
        let Some(point) = target_point else {
            return ActionResult::InvalidForPhase;
        };
        if off_path {
            self.run_state.winged_boots_record_off_path();
        }
        let coord = point.coord;
        let point_type = point.point_type;
        // Resolve Unknown → concrete (consuming the same RNG as
        // `enter_room_for_point`), then enter the *coord-bearing* room
        // through `enter_room` so the map-traversal trackers
        // (`visited_map_coords`, `act_floor`, `total_floor`,
        // `map_point_history`) bump and `after_room_entered` fires once
        // with the player's actual position. Previously this entered a
        // coord-less `Room::new` (no floor bump) and then overwrote
        // `current_room` by direct assignment, which also skipped
        // `add_visited_coord` — so every run reported `total_floor == 0`
        // and an empty visit history. Setting a coord-bearing room is
        // still required so subsequent `next_map_children_cols` /
        // `do_map_move` calls see the real map position (bead
        // `sts2-rs-rbb10`).
        let resolved = if point_type == crate::map::MapPointType::Unknown {
            let blacklist = self.build_unknown_room_blacklist(coord);
            self.run_state.resolve_unknown_room(blacklist)
        } else {
            point_type.to_room_type()
        };
        self.run_state
            .enter_room(crate::runstate::Room::at(coord, point_type, resolved));
        // Dispatch by resolved room type.
        match resolved {
            crate::runstate::RoomType::Monster
            | crate::runstate::RoomType::Elite
            | crate::runstate::RoomType::Boss => self.enter_combat_phase(resolved),
            crate::runstate::RoomType::Shop => {
                // Bead sts2-rs-if6ry (W3): build the active inventory
                // via the content callback. When unwired (legacy thin-
                // slice tests, pre-W3 callers), leave `shop_inventory`
                // None — get_available_actions falls back to
                // `[LeaveShop]` so existing behavior is preserved.
                self.shop_inventory = self
                    .callbacks
                    .generate_shop_inventory
                    .map(|f| f(&mut self.run_state));
                self.phase = Phase::Shop;
                ActionResult::Advanced
            }
            crate::runstate::RoomType::RestSite => {
                self.enter_rest_site();
                ActionResult::Advanced
            }
            crate::runstate::RoomType::Treasure => self.resolve_treasure_room(),
            crate::runstate::RoomType::Event => {
                self.phase = Phase::Event;
                ActionResult::Advanced
            }
            crate::runstate::RoomType::Unknown => ActionResult::InvalidForPhase,
        }
    }

    /// C# treasure room, as the (BUG-016-patched) headless plays it —
    /// fully auto-resolved, no decision surface:
    ///   1. room enter → `BeginRelicPicking`: ONE float
    ///      (`RelicFactory.RollRarity(TreasureRoomRelics)`) +
    ///      `SharedRelicGrabBag.PullFromFront(rarity)`;
    ///   2. `DoNormalRewards`: gold = `Rewards.NextInt(42, 53)`;
    ///   3. the (single) player picks the relic up — direct obtain,
    ///      `AfterObtained` fires (AutoSlay's `TreasureRoomHandler`
    ///      always grabs it).
    /// When the shared bag was never installed (legacy hosts / raw-core
    /// tests), falls back to the old pool-callback `Phase::Treasure`
    /// decision surface.
    fn resolve_treasure_room(&mut self) -> ActionResult {
        // SilverCrucible's "first Treasure Chest you open is empty" veto. C#
        // `TreasureRoomRelicSynchronizer` / `OneOffSynchronizer` early-return
        // BEFORE any Rewards read when the veto fires, so the relic rarity roll,
        // the grab-bag pop, and the `rng.rewards.next_int(42,52)` gold read are
        // all SKIPPED ENTIRELY (not rolled-then-discarded) — rolling-then-
        // discarding would desync the rewards/treasure streams for the rest of
        // the run. `after_room_entered` already incremented the counter in
        // `enter_room`, so `TreasureRoomsEntered == 1` here on the first chest.
        if !crate::run_effects::dispatch_should_generate_treasure(&mut self.run_state) {
            self.phase = Phase::MapChoice;
            return ActionResult::Advanced;
        }
        if self.run_state.shared_relic_grab_bag_by_rarity.is_empty() {
            let pools = self
                .callbacks
                .generate_treasure_relic_pools
                .map(|f| f(&mut self.run_state));
            if let Some((mut common, mut uncommon, mut rare)) = pools {
                let _ = self.enter_treasure(&mut common, &mut uncommon, &mut rare);
            } else {
                self.phase = Phase::Treasure;
            }
            return ActionResult::Advanced;
        }
        let rolled = self.run_state.roll_treasure_relic_rarity();
        let rarity = match rolled {
            crate::card_rewards::RewardRarity::Common => crate::runstate::RelicRarity::Common,
            crate::card_rewards::RewardRarity::Uncommon => crate::runstate::RelicRarity::Uncommon,
            crate::card_rewards::RewardRarity::Rare => crate::runstate::RelicRarity::Rare,
        };
        let picked = self.run_state.pop_relic_from_shared_grab_bag(rarity);
        // C# `OneOffSynchronizer.DoTreasureRoomRewards`
        // (`OneOffSynchronizer.cs:104-117`): `double gold =
        // Rewards.NextInt(42, 53)`, then A3 Poverty multiplies the ROLLED
        // value by 0.75 and `GainGold((int)gold)` truncates — so at A3+
        // the chest pays [31, 39] off the same RNG read. Bead
        // sts2-rs-40ikw.2 (2026-06-12 ascension audit).
        let mut gold = f64::from(self.run_state.rng.rewards.next_int(42, 52));
        if self.run_state.ascension_level >= crate::runstate::POVERTY_ASCENSION_LEVEL {
            gold *= crate::runstate::POVERTY_ASCENSION_GOLD_MULTIPLIER;
        }
        self.run_state.gain_gold(gold.trunc() as i32);
        if let Some(id) = picked {
            if self.run_state.interactive_rewards {
                // sts2-agent fork: the chest's relic is offered, taken or left
                // (C# `TreasureRoom`: the chest opens with its gold, the relic waits
                // on the player; `PickRelicReward` takes it, `SkipRelic` leaves the
                // room without it). Taking it runs the same obtain and follow-up
                // routing through `do_relic_reward_pick`.
                self.run_state
                    .relic_rewards
                    .push(crate::runstate::RelicReward::new(id));
                self.phase = Phase::CardReward;
                return ActionResult::Advanced;
            }
            self.run_state.obtain_relic(id);
        }
        // AfterObtained listeners may have queued follow-up rewards
        // (same routing as the event path).
        let opened_card_reward = self.drain_pending_rewards();
        if opened_card_reward
            || !self.run_state.relic_rewards.is_empty()
            || !self.run_state.pending_card_selects.is_empty()
        {
            self.phase = Phase::CardReward;
            self.advance_after_card_reward();
        } else {
            self.phase = Phase::MapChoice;
        }
        ActionResult::Advanced
    }

    fn enter_combat_phase(&mut self, room_type: crate::runstate::RoomType) -> ActionResult {
        let act = self.run_state.act_index;
        // Prefer the EncounterDef-based builder (tier-by-floor selection,
        // roster randomizers, per-monster HP rolls). It returns `None` to
        // defer — e.g. for acts it doesn't yet cover — falling back to the
        // legacy `select_encounter` + `enter_combat` pinned-spec path.
        if let Some(build) = self.callbacks.build_combat_for_room {
            if let Some(combat) = build(&mut self.run_state, act, room_type) {
                self.combat = Some(combat);
                self.phase = Phase::Combat;
                return ActionResult::Advanced;
            }
        }
        let (Some(select), Some(enter)) =
            (self.callbacks.select_encounter, self.callbacks.enter_combat)
        else {
            return ActionResult::NotImplemented;
        };
        let spec = select(&mut self.run_state, act, room_type);
        let combat = enter(&self.run_state, spec);
        self.combat = Some(combat);
        self.phase = Phase::Combat;
        ActionResult::Advanced
    }

    fn do_combat_play_card(&mut self, hand_idx: usize, target: Option<u32>) -> ActionResult {
        let Some(combat) = self.combat.as_mut() else {
            return ActionResult::InvalidForPhase;
        };
        let Ok(hand_idx_u8) = u8::try_from(hand_idx) else {
            return ActionResult::InvalidForPhase;
        };
        // Bead `sts2-rs-rkz98`: auto-target the first live enemy when the
        // caller passed `target: None` but the card needs a single-enemy
        // target. The "first available action" driver pattern (used by
        // the full_run_act1to3_ironclad acceptance test) emits
        // `target: None` from `get_available_actions`; without this
        // fallback every untargeted Strike is silently rejected by
        // `combat_loop::play_card`, the player makes no progress, and
        // combat stalls forever. Mirrors Python's reward_picker default
        // of "target the first enemy" when the agent omits a target.
        let resolved_target: Option<u32> = if let Some(t) = target {
            Some(t)
        } else {
            let card_target_type = combat
                .player
                .hand
                .get(hand_idx)
                .and_then(|uid| {
                    let idx = uid.0.get() as usize - 1;
                    combat.cards.get(idx)
                })
                .map(|c| c.target_type);
            if matches!(card_target_type, Some(crate::cards::TargetType::Enemy)) {
                // `target: u8` is an INDEX into combat.enemy_uids (see
                // combat_loop::play_card lines 2530-2534), NOT a raw
                // CreatureUid. Find the first live enemy's position in
                // enemy_uids.
                combat
                    .enemy_uids
                    .iter()
                    .position(|uid| {
                        combat
                            .creatures
                            .iter()
                            .find(|c| c.id == uid.0)
                            .is_some_and(|c| c.current_hp > 0)
                    })
                    .and_then(|i| u32::try_from(i).ok())
            } else {
                None
            }
        };
        let target_u8 = match resolved_target {
            Some(t) => match u8::try_from(t) {
                Ok(t) => Some(t),
                Err(_) => return ActionResult::InvalidForPhase,
            },
            None => None,
        };
        let _ = crate::combat_loop::play_card(combat, hand_idx_u8, target_u8);
        if combat.is_over {
            return self.exit_combat_and_transition();
        }
        ActionResult::Advanced
    }

    fn do_combat_end_turn(&mut self) -> ActionResult {
        let Some(combat) = self.combat.as_mut() else {
            return ActionResult::InvalidForPhase;
        };
        let _ = crate::combat_loop::end_turn_dispatch(combat);
        if combat.is_over {
            return self.exit_combat_and_transition();
        }
        ActionResult::Advanced
    }

    /// Resolve an open combat `pending_choice` (bead `sts2-rs-6dtxp`).
    /// `option = Some(i)` → selection `[i]`; `None` → empty selection
    /// (skip). Routes through [`crate::apply`]`(Action::ResolveChoice)`,
    /// which clears the choice, runs the per-`ChoiceKind` resolver, and
    /// (if this choice paused turn-setup) resumes
    /// `continue_player_turn_setup` to advance back to `PlayerPlayPhase`.
    fn do_combat_resolve_choice(&mut self, option: Option<usize>) -> ActionResult {
        let mut selection: smallvec::SmallVec<[u8; 4]> = smallvec::SmallVec::new();
        if let Some(i) = option {
            let Ok(i_u8) = u8::try_from(i) else {
                return ActionResult::InvalidForPhase;
            };
            selection.push(i_u8);
        }
        // INCREMENTAL: `CombatResolveChoice { option }` is the single-pick /
        // skip surface the run loop drives, and for a `max_select > 1` choice a
        // bare `[i]` is one SEQUENTIAL pick that must ACCUMULATE in
        // `pc.picked` (bead `sts2-rs-sopgf`) rather than resolve at once.
        // Route through `apply` so the accumulation/finalize split runs.
        self.do_combat_resolve_choice_selection(selection, /*atomic=*/ false)
    }

    /// Resolve an open multi-pick combat `pending_choice` by the EXACT
    /// option indices in `selection` — NOT the first-`count` prefix that
    /// [`RunAction::CombatResolveChoiceTakeFirst`] forces. Needed when a
    /// parity/replay driver must reproduce a *specific* C# multi-select:
    /// e.g. Dredge's "choose 3 discard cards", where the chosen indices
    /// (say `[1, 2, 4]`) are not a `0..k` prefix, so taking the first 3
    /// would add different cards to hand and desync the run (sweep RP115).
    /// Thin public wrapper over [`Self::do_combat_resolve_choice_selection`].
    ///
    /// ATOMIC: the replay driver hands the COMPLETE C# multi-select in one
    /// shot, so this bypasses the sequential per-pick accumulation (bead
    /// `sts2-rs-sopgf`) — the per-`ChoiceKind` resolver sees the identical full
    /// index set, keeping the end state byte-identical to the pre-`sopgf` path.
    pub fn resolve_combat_choice_selection(
        &mut self,
        selection: smallvec::SmallVec<[u8; 4]>,
    ) -> ActionResult {
        self.do_combat_resolve_choice_selection(selection, /*atomic=*/ true)
    }

    /// General combat-choice resolver: apply a full `selection` (skip = empty,
    /// single-pick = `[i]`, multi-pick = `[i, j, …]`) to the open
    /// `pending_choice`. Backs both `CombatResolveChoice` (skip/single) and
    /// `CombatResolveChoiceSelection` (multi-pick). Bead
    /// `sts2-rs-mcts-surface-onplay-select`.
    ///
    /// When `atomic` is true the selection is resolved in ONE shot via
    /// [`crate::resolve_choice_atomic_apply`], bypassing the sequential
    /// multi-pick accumulation (the replay / take-first drivers). When false it
    /// routes through [`crate::apply`]`(Action::ResolveChoice)`, which
    /// accumulates a bare `[i]` on a `max_select > 1` choice into `pc.picked`
    /// and re-opens (the MCTS / run-loop incremental drive). Bead
    /// `sts2-rs-sopgf`.
    fn do_combat_resolve_choice_selection(
        &mut self,
        selection: smallvec::SmallVec<[u8; 4]>,
        atomic: bool,
    ) -> ActionResult {
        let Some(combat) = self.combat.as_mut() else {
            return ActionResult::InvalidForPhase;
        };
        // Reject when no choice is open — `legal_actions` wouldn't have
        // surfaced this, so it's a malformed input rather than a no-op.
        if combat.pending_choice.is_none() {
            return ActionResult::InvalidForPhase;
        }
        if atomic {
            let _ = crate::resolve_choice_atomic_apply(combat, &selection);
        } else {
            let _ = crate::apply(combat, crate::Action::ResolveChoice { selection });
        }
        if combat.is_over {
            return self.exit_combat_and_transition();
        }
        ActionResult::Advanced
    }

    /// Drink the held potion in belt slot `potion_slot` (combat-side
    /// belt, seeded from the run belt at `enter_combat`), optionally
    /// targeting enemy index `target`. Routes through
    /// [`crate::apply`]`(Action::DrinkPotion)`, which consumes the slot
    /// and dispatches the potion effect; `exit_combat` later writes the
    /// surviving belt back to the run state. An out-of-range slot is
    /// rejected (`legal_actions` only surfaces occupied, character-legal
    /// slots, so this guards against malformed scripted input).
    fn do_combat_drink_potion(&mut self, potion_slot: usize, target: Option<u32>) -> ActionResult {
        let Some(combat) = self.combat.as_mut() else {
            return ActionResult::InvalidForPhase;
        };
        if potion_slot >= combat.player.potions.len() {
            return ActionResult::InvalidForPhase;
        }
        let Ok(slot_u8) = u8::try_from(potion_slot) else {
            return ActionResult::InvalidForPhase;
        };
        let target_u8 = match target {
            Some(t) => match u8::try_from(t) {
                Ok(t) => Some(t),
                Err(_) => return ActionResult::InvalidForPhase,
            },
            None => None,
        };
        let _ = crate::apply(
            combat,
            crate::Action::DrinkPotion {
                potion_slot: slot_u8,
                target: target_u8,
            },
        );
        if combat.is_over {
            return self.exit_combat_and_transition();
        }
        ActionResult::Advanced
    }

    /// Fast-forward the current combat from a surrogate outcome instead of
    /// playing it out (AZ M2 planning-transition). Precondition: `phase ==
    /// Phase::Combat` with `combat` set (built by `enter_combat_phase`).
    ///
    /// Stamps the built `CombatState` terminal — `is_over = true`,
    /// `player_won = survived`, and player HP reduced by `hp_frac_lost` of max
    /// HP (clamped ≥ 1 on a win, set to 0 on a loss) — then delegates to the
    /// normal [`Self::exit_combat_and_transition`], so gold / potion / card-
    /// reward generation run through the exact deployed reward bridge. The
    /// surrogate replaces only the *playing* of the fight, not its rewards.
    ///
    /// `hp_frac_lost` is the surrogate's mean HP-loss fraction; there is no
    /// variance head, so a fast-forwarded fight is a two-outcome event
    /// (win-with-mean-HP / loss), not an HP distribution.
    pub fn resolve_combat_fast(&mut self, hp_frac_lost: f32, survived: bool) -> ActionResult {
        if self.phase != Phase::Combat {
            return ActionResult::InvalidForPhase;
        }
        let Some(combat) = self.combat.as_mut() else {
            return ActionResult::InvalidForPhase;
        };
        let max_hp = combat.player.max_hp;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let lost = (hp_frac_lost.clamp(0.0, 1.0) * max_hp as f32).round() as i32;
        if survived {
            combat.player.current_hp = (combat.player.current_hp - lost).max(1);
            combat.player_won = true;
        } else {
            combat.player.current_hp = 0;
            combat.player_won = false;
        }
        combat.is_over = true;
        self.exit_combat_and_transition()
    }

    fn exit_combat_and_transition(&mut self) -> ActionResult {
        let combat = self.combat.as_ref().expect("combat slot set");
        let player_won = combat.player_won;
        if let Some(exit) = self.callbacks.exit_combat {
            // Take ownership of combat ref-share via clone-free split: the
            // callback only reads combat + mutates rs, no aliasing on
            // RunManager itself.
            let mut combat_ref = self.combat.take().expect("combat slot set");
            exit(&mut self.run_state, &combat_ref);
            // The exit callback settles SwipePower steals into `master_deck`
            // (removing each un-reclaimed stolen card exactly once). Clear the
            // ledger so any deck view that subtracts in-progress steals (the
            // sts2-py `effective_master_deck_indices` serialization) stops
            // re-subtracting already-flushed entries during the reward screen —
            // with a duplicate-identity deck that double-subtract ate the
            // player's surviving copy (shadow census seed 1034, 2×Stampede).
            combat_ref.swipe_stolen.clear();
            self.combat = Some(combat_ref);
        }
        if !player_won {
            // Player died.
            self.run_state.is_over = true;
            self.run_state.player_won = false;
            self.phase = Phase::RunOver;
            return ActionResult::Advanced;
        }
        // C# `Hook.AfterCombatEnd` fires for the RUN-side relic models on
        // every combat end (`CombatManager.cs:784` — ChosenCheese's +1 max
        // HP, etc.), before the rewards screen. Fired here AFTER the
        // combat→run HP write-back so listener HP mutations stick — and only
        // on a WON combat: the C# hook runs on loss too, but the dead
        // player's post-hook HP is unobservable (the headless pins death at
        // 0), and a +1 max-HP bump on a dead run made Rust report hp=1
        // where C# shows 0. Ordering caveat: C# runs this BEFORE
        // `AfterCombatVictory` (BurningBlood's heal), Rust models the
        // victory heal combat-side — the swap only matters when the victory
        // heal saturates at max HP in the same combat a max-HP listener
        // fires (a 1-HP skew; revisit if a sweep pins it).
        let _ = crate::run_effects::dispatch_after_combat_end(&mut self.run_state);
        // C# `Hook.AfterCombatVictory` for the run-side relic models
        // (sts2-agent fork): Sword of Stone's replacement by Sword of Jade at
        // its fifth elite (the combat side counted the elite down and the exit
        // callback flushed the count) and War Hammer's deck upgrades. This was
        // the deferred firing site of `dispatch_after_combat_victory_run`.
        let is_elite = self.combat.as_ref().is_some_and(CombatState::is_elite);
        let _ = crate::run_effects::dispatch_after_combat_victory_run(
            &mut self.run_state,
            &crate::run_effects::CombatVictoryRunCtx { is_elite },
        );
        // Event→combat resume (bead `sts2-rs-ck9z6`, GH #166): if this fight
        // was launched from an event option (`EnterCombatWithoutExitingEvent`),
        // route back through the event instead of the normal combat-reward
        // chain. A player death already short-circuited to `Phase::RunOver`
        // above, so this is reached only on a WON (incl. timed-out) fight.
        if let Some(req) = self.run_state.event_combat.take() {
            return self.finish_event_combat(req);
        }
        // NOTE: the post-combat victory heal (Ironclad's BurningBlood +6
        // etc.) is now modeled COMBAT-SIDE — `enter_combat_from_roster`
        // mirrors every run-carried relic into combat, and the combat-end
        // sequence fires `dispatch_after_combat_victory`
        // (`combat_loop.rs:3911`), so BurningBlood heals `combat.player`
        // before `exit_combat` writes the HP back. This matches C#
        // `BurningBlood.AfterCombatVictory` firing inside
        // `CombatManager.EndCombatInternal` (before the rewards screen).
        // The legacy `heal_after_combat` run-manager shim (a Python-pin
        // port from when combat didn't model relics) DOUBLE-applied it —
        // removed. `heal_after_combat` is retained on the starter struct as
        // documentation of the relic amount but is no longer applied here.
        // Bead `sts2-rs-jenwp` (M3b/M4 whole-run RNG parity).
        // Boss-room win on act 3 → run won (unless A10's second boss is
        // still pending).
        let room_type = self.run_state.current_room.map(|r| r.room_type);
        if room_type == Some(crate::runstate::RoomType::Boss) {
            if self.run_state.act_index >= 3 {
                // A10 DoubleBoss: the final act carries a second boss node
                // directly above the first. After the FIRST boss the run is
                // NOT over — C# routes the terminal boss-rewards screen back
                // to map travel (`NRewardsScreen.cs:449-456`) so the player
                // walks boss → second boss; the win is stamped only at the
                // second-boss point, or at the boss point when no second
                // exists (`CombatManager.cs:801-806`). Bead sts2-rs-40ikw.2.
                if !self.run_state.awaiting_second_boss() {
                    self.run_state.is_over = true;
                    self.run_state.player_won = true;
                    self.phase = Phase::RunOver;
                    return ActionResult::Advanced;
                }
                // First boss of the A10 double-boss pair: full boss combat
                // rewards (gold + potion-odds + 3-rare card reward,
                // `RewardsSet.cs:176-185` — same set as any boss), but NO
                // boss-relic screen and NO act advance. After the reward
                // chain drains, `advance_after_card_reward` falls through to
                // `Phase::MapChoice` (both `boss_relic_options` and
                // `pending_boss_act_advance` stay unset) and the only legal
                // move is the second-boss node.
                let reward_room_type = self
                    .run_state
                    .current_room
                    .map_or(crate::runstate::RoomType::Boss, |r| r.room_type);
                let roll = self.begin_room_rewards(reward_room_type);
                self.enter_card_reward_phase();
                self.open_reward_screen(roll);
                return ActionResult::Advanced;
            }
            // Act-1/2 boss kill: grant the combat card reward FIRST (3
            // boss-context cards → all Rare), THEN surface the boss-relic
            // choice. Mirrors C# `RewardsSet.GenerateRewardsFor`
            // (`RewardsSet.cs:181-185`): a Boss room yields gold + potion
            // + `CardReward(ForRoom(Boss), 3)`, with the boss relic
            // offered as its own screen afterward. The prior code routed
            // straight to `Phase::BossRelic` and dropped the card reward
            // entirely (GH #25 — winning the Ceremonial Beast offered only
            // a relic, never the 3 rare cards).
            //
            // Pre-generate the boss-relic pool now (a pure, RNG-free peek
            // via `offer_boss_relic_choice`) and stash it on
            // `boss_relic_options`; `advance_after_card_reward` routes into
            // `Phase::BossRelic` once the card reward resolves.
            //
            // Bead `sts2-rs-7ufeb`: an empty pool (callback yielded
            // nothing — pool exhaustion / content gap) leaves
            // `boss_relic_options` empty, so `advance_after_card_reward`
            // falls through to `Phase::MapChoice` on its own — no softlock
            // on a zero-action `BossRelic` screen.
            if let Some(gen) = self.callbacks.generate_boss_relic_pool {
                self.boss_relic_options = gen(&mut self.run_state);
            }
            // Apply the boss combat reward (gold + potion-odds) on the
            // `Rewards` stream BEFORE the card reward, matching C#
            // `RewardsSet.GenerateRewardsFor(Boss)` populate order
            // (gold → potion → card; `RewardsSet.cs:181-185`). The
            // boss-relic / next-act Ancient choice reads no `Rewards`
            // (pre-shuffled off `up_front`), so showing it AFTER the card
            // reward does not disturb this cursor. Bead `sts2-rs-jenwp`
            // (M4 live reward stream — Boss).
            let reward_room_type = self
                .run_state
                .current_room
                .map_or(crate::runstate::RoomType::Boss, |r| r.room_type);
            let roll = self.begin_room_rewards(reward_room_type);
            // Advance into the next act only AFTER the boss reward chain
            // (card reward, then the boss-relic / Ancient choice) fully
            // drains — `advance_after_card_reward` (empty pool) or
            // `do_boss_relic_pick` (relic picked) consumes this flag and
            // calls `advance_boss_act`.
            self.pending_boss_act_advance = true;
            self.enter_card_reward_phase();
            self.open_reward_screen(roll);
            return ActionResult::Advanced;
        }
        // Normal / Elite combat win → combat rewards + CardReward.
        //
        // C# `RewardsSet.GenerateRewardsFor(Monster/Elite)` consumes the
        // run-level `PlayerRng.Rewards` stream in this exact order, all
        // BEFORE the card reward (which `enter_card_reward_phase` rolls off
        // the same cursor):
        //   1. potion-odds `NextFloat`  (`RollForPotionAndAddTo`, ctor)
        //   2. gold `NextInt`           (`GoldReward.Populate`)
        //   3. potion-contents          (`PotionReward.Populate`, if (1) dropped)
        //   4. card 3×[rarity,pick,upgrade] (`CardReward.Populate`)
        // The content callback applies gold + any dropped potion eagerly
        // (C# auto-claims both on the rewards screen; only the card needs a
        // pick). Without it wired (raw-core tests) the card rolls at the
        // un-advanced cursor. Bead `sts2-rs-jenwp`.
        let reward_room_type = self
            .run_state
            .current_room
            .map_or(crate::runstate::RoomType::Monster, |r| r.room_type);
        let roll = self.begin_room_rewards(reward_room_type);
        self.enter_card_reward_phase();
        // Elite rooms append a 5th reward: C# `RewardsSet.GenerateRewardsFor`
        // (Elite case) adds `new RelicReward(player)` LAST, whose `Populate`
        // runs AFTER the card reward — `RelicFactory.RollRarity` reads one
        // `NextFloat` off `PlayerRng.Rewards`, then pulls front-of-bucket from
        // the (up_front-shuffled) grab bag with NO further `Rewards` read.
        // `offer_random_relic_rewards(1, &[None])` replicates both exactly
        // (`roll_reward_relic_rarity` on `rewards` + `pop_relic_from_grab_bag`)
        // and queues the relic so the player picks it via the existing
        // PickRelicReward/SkipRelic fan-out the CardReward phase already
        // surfaces when `relic_rewards` is non-empty. Must come after
        // `enter_card_reward_phase` so the `Rewards` cursor matches C#'s
        // populate order (gold → potion → card 3× → relic rarity). Bead
        // `sts2-rs-jenwp` (M4 live reward stream — Elite follow-up).
        if reward_room_type == crate::runstate::RoomType::Elite {
            self.run_state.offer_random_relic_rewards(1, &[None]);
            // The C# headless oracle auto-collects the Elite `RelicReward`
            // (along with gold + potion) the moment rewards are generated —
            // only `CardReward` is surfaced to the agent. See
            // `Sts2Headless/RunSimulator.cs` (`GenerateWithoutOffering` →
            // foreach reward: GoldReward/RelicReward/PotionReward get
            // `OnSelectWrapper()` auto-invoked), matching C#
            // `RewardsSet.Offer()` under `TestMode.IsOn`
            // (`RewardsSet.cs:125-136`). So the relic is already in the
            // player's relic list at the card-reward screen, NOT a separate
            // PickRelicReward/SkipRelic decision presented after the card.
            // `drain_relic_rewards_directly` obtains the just-queued relic
            // immediately (no further `Rewards` read), keeping the relic
            // list — and the rewards-stream cursor — in lockstep with the
            // oracle. Without this the relic surfaced one step late, so the
            // card-reward-screen relic-list diff diverged (RP109/RP72).
            if !self.run_state.interactive_rewards {
                self.run_state.drain_relic_rewards_directly();
            }
        }
        self.open_reward_screen(roll);
        ActionResult::Advanced
    }

    /// A won combat room's gold and potion: applied at once (the
    /// auto-collect path), or under [`RunState::interactive_rewards`] rolled
    /// in the same order and returned for the rewards screen. sts2-agent fork.
    fn begin_room_rewards(
        &mut self,
        room_type: crate::runstate::RoomType,
    ) -> Option<RoomRewardRoll> {
        if self.run_state.interactive_rewards {
            if let Some(roll) = self.callbacks.roll_combat_gold_and_potion {
                return Some(roll(&mut self.run_state, room_type));
            }
        }
        if let Some(apply) = self.callbacks.apply_combat_gold_and_potion {
            apply(&mut self.run_state, room_type);
        }
        None
    }

    /// Open the rewards screen for a won combat room (sts2-agent fork): the
    /// rolled gold and potion, the relics the room queued (an elite's), and
    /// the card reward `enter_card_reward_phase` rolled, in the order the
    /// game lists them (gold, potion, relic, card). No-op without a roll
    /// (the auto-collect path).
    fn open_reward_screen(&mut self, roll: Option<RoomRewardRoll>) {
        let Some(roll) = roll else {
            return;
        };
        let mut screen: Vec<ScreenReward> = roll
            .gold
            .into_iter()
            .map(|amount| ScreenReward::Gold { amount })
            .collect();
        if let Some(kind) = roll.potion {
            screen.push(ScreenReward::Potion { kind });
        }
        screen.extend(
            std::mem::take(&mut self.run_state.relic_rewards)
                .into_iter()
                .map(|relic| ScreenReward::Relic { relic }),
        );
        if !self.card_reward_options.is_empty() {
            screen.push(ScreenReward::Card {
                options: std::mem::take(&mut self.card_reward_options),
                upgraded: std::mem::take(&mut self.card_reward_upgraded),
            });
        }
        self.reward_screen = Some(screen);
        self.screen_card_claim = None;
        self.phase = Phase::CardReward;
    }

    /// Claim one reward of the open screen (C# `Reward.OnSelect`). sts2-agent fork.
    fn do_claim_reward(&mut self, idx: usize) -> ActionResult {
        let Some(reward) = self
            .reward_screen
            .as_ref()
            .and_then(|s| s.get(idx))
            .cloned()
        else {
            return ActionResult::InvalidForPhase;
        };
        match reward {
            ScreenReward::Gold { amount } => {
                self.remove_screen_reward(idx);
                self.run_state.gain_gold(amount);
            }
            ScreenReward::Potion { kind } => {
                // C# `PotionCmd.TryToProcure`: refused on a full belt, and the
                // reward stays on the screen (not offered then).
                if !self.run_state.add_potion_to_belt(kind) {
                    return ActionResult::InvalidForPhase;
                }
                self.remove_screen_reward(idx);
            }
            ScreenReward::Relic { relic } => {
                self.remove_screen_reward(idx);
                // Through the relic-reward queue so `accept_relic_reward`
                // obtains it (wax flag, `after_obtained`); a follow-up reward it
                // queues (Small Capsule) is offered before the screen returns.
                self.run_state.relic_rewards.push(relic);
                let last = self.run_state.relic_rewards.len() - 1;
                let _ = self.run_state.accept_relic_reward(last);
            }
            ScreenReward::Card { options, upgraded } => {
                self.card_reward_options = options;
                self.card_reward_upgraded = upgraded;
                self.screen_card_claim = Some(idx);
            }
        }
        self.phase = Phase::CardReward;
        ActionResult::Advanced
    }

    fn remove_screen_reward(&mut self, idx: usize) {
        if let Some(screen) = self.reward_screen.as_mut() {
            if idx < screen.len() {
                screen.remove(idx);
            }
        }
    }

    /// Leave the rewards screen, skipping what is left (C# `Reward.OnSkipped`),
    /// and continue as the auto-collect path does after its card reward.
    /// sts2-agent fork.
    fn do_proceed_rewards(&mut self) -> ActionResult {
        self.reward_screen = None;
        self.screen_card_claim = None;
        self.advance_after_card_reward();
        ActionResult::Advanced
    }

    /// Discard the potion at `slot` (C# `PotionModel.Discard`: the slot is
    /// emptied, nothing else happens; Belt Buckle, the one discard listener,
    /// isn't modelled by the engine). sts2-agent fork.
    fn do_discard_potion(&mut self, slot: usize) -> ActionResult {
        if self.phase == Phase::Combat {
            let Some(combat) = self.combat.as_mut() else {
                return ActionResult::InvalidForPhase;
            };
            if slot >= combat.player.potions.len() {
                return ActionResult::InvalidForPhase;
            }
            let _ = combat.player.remove_potion_at(slot);
        } else {
            if slot >= self.run_state.potions.len() {
                return ActionResult::InvalidForPhase;
            }
            let _ = self.run_state.remove_potion_from_belt(slot);
        }
        ActionResult::Advanced
    }

    /// Drink an AnyTime potion outside combat (C# `PotionModel.OnUseWrapper`:
    /// the potion leaves the belt first, then its `OnUse` runs). The four
    /// potions' out-of-combat effects: `BloodPotion` heals 20% of max HP
    /// (floored, as the combat dispatch does), `FruitJuice` gains 5 max HP,
    /// `EntropicBrew` fills every open slot with random potions off
    /// `CombatPotionGeneration` until a procure fails, `FoulPotion` thrown at
    /// the merchant gains 100 gold. Their `AfterPotionUsed` listeners
    /// (`ReptileTrinket`, `BeltBuckle`) act only in combat. sts2-agent fork.
    fn do_drink_potion_out_of_combat(&mut self, slot: usize) -> ActionResult {
        use crate::potions::PotionKind;
        let Some(&kind) = self.run_state.potions.get(slot) else {
            return ActionResult::InvalidForPhase;
        };
        let _ = self.run_state.remove_potion_from_belt(slot);
        match kind {
            PotionKind::BloodPotion => {
                let amount = self.run_state.max_hp.saturating_mul(20) / 100;
                if amount > 0 {
                    let _ = self.run_state.heal(amount);
                }
            }
            PotionKind::FruitJuice => self.run_state.gain_max_hp(5),
            PotionKind::EntropicBrew => {
                while self.has_open_potion_slot() {
                    let Some(roll) = self.callbacks.roll_out_of_combat_potion else {
                        break;
                    };
                    let Some(new) = roll(&mut self.run_state) else {
                        break;
                    };
                    if !crate::run_effects::dispatch_should_procure_potion(&mut self.run_state)
                        || !self.run_state.add_potion_to_belt(new)
                    {
                        break;
                    }
                }
            }
            PotionKind::FoulPotion => self.run_state.gain_gold(100),
            _ => return ActionResult::InvalidForPhase,
        }
        ActionResult::Advanced
    }

    fn enter_card_reward_phase(&mut self) {
        self.card_reward_options = self
            .callbacks
            .generate_card_reward_options
            .map(|f| f(&mut self.run_state))
            .unwrap_or_default();
        self.card_reward_upgraded = std::mem::take(&mut self.run_state.card_reward_upgraded_flags);
        // A fresh post-combat offering is never a choose screen.
        self.card_offer_choose_screen = false;
        self.card_offer_can_skip = true;
        self.phase = Phase::CardReward;
        // If the offering came back empty (no card callback wired, or an
        // exhausted pool), route onward immediately rather than stranding
        // the caller on a zero-action CardReward screen. With a non-empty
        // offering `advance_after_card_reward` returns at its first guard,
        // so this is a no-op for the normal pick path; when empty it
        // surfaces any queued relic-reward, the pre-generated boss-relic
        // choice (boss combats), or finally the map.
        // Under the rewards screen (sts2-agent fork) the screen opens next,
        // empty offering or not.
        if self.card_reward_options.is_empty() && !self.run_state.interactive_rewards {
            self.advance_after_card_reward();
        }
    }

    fn do_card_reward_pick(&mut self, idx: Option<usize>) -> ActionResult {
        // Scroll Boxes bundle pick: add EVERY card in the chosen pack to the
        // deck (C# `foreach (card in ...) CardPileCmd.Add(card, Deck)`), with
        // no skip. Reads `card_reward_bundles`, not `card_reward_options`.
        if let Some(bundles) = self.card_reward_bundles.take() {
            let Some(i) = idx else {
                // No skip for bundles; restore and reject.
                self.card_reward_bundles = Some(bundles);
                return ActionResult::InvalidForPhase;
            };
            let Some(pack) = bundles.get(i) else {
                self.card_reward_bundles = Some(bundles);
                return ActionResult::InvalidForPhase;
            };
            for &card_id in pack {
                let mut card = crate::lookup_card_factory()
                    .and_then(|f| f(card_id, false))
                    .unwrap_or_else(|| {
                        let mut c = crate::cards::Card::pod_defaults();
                        c.card_id = card_id;
                        c
                    });
                // C# `CardPileCmd.cs:502` fires `Hook.ModifyCardBeingAddedToDeck`
                // on EVERY new Deck add, BEFORE the add — so egg relics
                // (MoltenEgg/FrozenEgg/ToxicEgg) upgrade a matching reward card at
                // pick time. Must run before the push (run-starter-upgrade-flip A).
                self.run_state.modify_card_being_added_to_deck(&mut card);
                // C# `CardReward.cs:173 CardPileCmd.Add(result, Deck)` →
                // `CardPileCmd.cs:583 Hook.AfterCardChangedPiles` fires for
                // every successful deck add, which is where `LuckyFysh.cs`
                // grants `GoldVar(15)` per card landing in the deck. Capture
                // identity/type BEFORE the moving push, then dispatch the
                // run-side `on_card_added_to_deck` channel so LuckyFysh fires
                // per bundled card (parity map_gold_necro).
                let ctx = crate::run_effects::CardAddedRunCtx {
                    card_id: card.card_id,
                    card_type: card.card_type,
                    source: crate::relic_effects::AddSource::Reward,
                };
                self.run_state.master_deck.push(card);
                let _ =
                    crate::run_effects::dispatch_on_card_added_to_deck(&mut self.run_state, &ctx);
            }
            self.card_reward_options.clear();
            self.advance_after_card_reward();
            return ActionResult::Advanced;
        }
        // A forced pick (Brain Leech's grid, `Cancelable = false`) can't be
        // skipped.
        if idx.is_none() && !self.card_offer_can_skip {
            return ActionResult::InvalidForPhase;
        }
        match idx {
            Some(i) if i < self.card_reward_options.len() => {
                let card_id = self.card_reward_options[i];
                // Offer-time upgrade state (SilverCrucible pre-upgrade,
                // act-2+ upgrade rolls) — empty for choose screens/bundles.
                let upgraded = self.card_reward_upgraded.get(i).copied().unwrap_or(false);
                // Build the card through the registered factory so its
                // spec stats (cost / base_damage / base_block / keywords)
                // are populated. `pod_defaults()` alone leaves cost at the
                // POD default (1), which mis-displayed e.g. Anger (cost 0)
                // as cost 1. Fall back to a bare POD only if no factory /
                // spec is available.
                let mut card = crate::lookup_card_factory()
                    .and_then(|f| f(card_id, upgraded))
                    .unwrap_or_else(|| {
                        let mut c = crate::cards::Card::pod_defaults();
                        c.card_id = card_id;
                        c.upgraded = upgraded;
                        c
                    });
                // C# fires `Hook.ModifyCardBeingAddedToDeck` before the deck add
                // (CardPileCmd.cs:502) — egg relics upgrade a matching pick at add
                // time (run-starter-upgrade-flip A).
                self.run_state.modify_card_being_added_to_deck(&mut card);
                // Fire the run-side `on_card_added_to_deck` channel for the
                // single-pick deck add — see the bundle-loop comment above
                // (C# CardReward.cs:173 → CardPileCmd.cs:583 AfterCardChangedPiles;
                // LuckyFysh grants 15 gold per deck add). parity map_gold_necro.
                let ctx = crate::run_effects::CardAddedRunCtx {
                    card_id: card.card_id,
                    card_type: card.card_type,
                    source: crate::relic_effects::AddSource::Reward,
                };
                self.run_state.master_deck.push(card);
                let _ =
                    crate::run_effects::dispatch_on_card_added_to_deck(&mut self.run_state, &ctx);
                self.card_reward_options.clear();
            }
            Some(_) => return ActionResult::InvalidForPhase,
            None => self.card_reward_options.clear(),
        }
        // sts2-agent fork: a card reward claimed from the rewards screen
        // returns to it. A pick uses the reward up; the Skip alternative
        // keeps it claimable (C# `EndSelectionAndDoNotCompleteReward`).
        if let Some(claim) = self.screen_card_claim.take() {
            self.card_reward_upgraded.clear();
            if idx.is_some() {
                self.remove_screen_reward(claim);
            }
            self.phase = Phase::CardReward;
            return ActionResult::Advanced;
        }
        // A resolved choose screen (HeftyTablet / LeadPaperweight) appends
        // its deferred cards now — after the chosen card, on pick AND skip —
        // matching C#'s post-select `CardPileCmd.Add([chosen?, Injury], Deck)`.
        self.card_offer_can_skip = true;
        if std::mem::take(&mut self.card_offer_choose_screen) {
            let post = std::mem::take(&mut self.pending_choose_screen_post_add);
            // Each appended card is a separate `CardPileCmd.Add(.., Deck)` in
            // C# (post-select `Add([chosen?, Injury], Deck)`), so fire the
            // run-side `on_card_added_to_deck` channel once per card (LuckyFysh
            // grants 15 gold per deck add). parity map_gold_necro.
            for mut card in post {
                // Egg-relic deck-add upgrade also applies to the choose-screen
                // post-add cards (C# ModifyCardBeingAddedToDeck per Deck add).
                self.run_state.modify_card_being_added_to_deck(&mut card);
                let ctx = crate::run_effects::CardAddedRunCtx {
                    card_id: card.card_id,
                    card_type: card.card_type,
                    source: crate::relic_effects::AddSource::Reward,
                };
                self.run_state.master_deck.push(card);
                let _ =
                    crate::run_effects::dispatch_on_card_added_to_deck(&mut self.run_state, &ctx);
            }
        }
        self.advance_after_card_reward();
        ActionResult::Advanced
    }

    /// `PAELS_WING.sacrifice_card_reward` (bead sts2-rs-1jg54). Consumes
    /// the current card-reward offering and, on every Nth sacrifice
    /// (N=2 per `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py:
    /// 2827 class PaelsWing.SACRIFICES`), pulls one relic from the
    /// player's grab-bag and obtains it.
    ///
    /// The sacrifice-counter lives on [`RunState`] (see
    /// [`crate::runstate::RunState::pael_record_sacrifice`]) so it
    /// survives `RunManager` reconstruction and matches Python's
    /// per-instance `_rewards_sacrificed` semantics (collapsed into the
    /// Pael cluster ledger at bead `sts2-rs-70anj.3.20`). Pull rarity
    /// defaults to Common — Python rolls a random rarity via the
    /// rewards RNG; the cross-rarity fallback in
    /// [`crate::runstate::RunState::pop_relic_from_grab_bag`] covers
    /// the degenerate exhaustion path. A follow-up bead can wire the
    /// proper rarity-roll once a per-RunState rewards-RNG handle is
    /// threaded.
    fn do_card_reward_sacrifice(&mut self) -> ActionResult {
        if self.card_reward_options.is_empty() {
            return ActionResult::InvalidForPhase;
        }
        if !self.card_offer_can_skip {
            return ActionResult::InvalidForPhase;
        }
        if !self.run_state.owns_active_relic(crate::RelicId::PaelsWing) {
            return ActionResult::InvalidForPhase;
        }
        let count = self.run_state.pael_record_sacrifice();
        let screen_claim = self.screen_card_claim.take();
        if count % 2 == 0 {
            if let Some(relic) = self
                .run_state
                .pop_relic_from_grab_bag(crate::RelicRarity::Common)
            {
                self.run_state.relics.push(relic);
                self.run_state.last_obtained_relic = Some(relic);
                // Bead `sts2-rs-wt4w1`: fire the on-obtain channel
                // (eager, matching the shop path) so run-side
                // `after_obtained` listeners apply for the grab-bag
                // grant. `last_obtained_relic` is intentionally left
                // set — a downstream test + display read it post-pick.
                let _ = crate::run_effects::dispatch_after_obtained(&mut self.run_state);
            }
        }
        self.card_reward_options.clear();
        // sts2-agent fork: SACRIFICE completes a screen's card reward.
        if let Some(claim) = screen_claim {
            self.card_reward_upgraded.clear();
            self.remove_screen_reward(claim);
            self.phase = Phase::CardReward;
            return ActionResult::Advanced;
        }
        // Defensive: a sacrifice consuming a choose screen still owes its
        // post-add cards (HeftyTablet's Injury is unconditional in C#).
        self.card_offer_can_skip = true;
        if std::mem::take(&mut self.card_offer_choose_screen) {
            let post = std::mem::take(&mut self.pending_choose_screen_post_add);
            self.run_state.master_deck.extend(post);
        }
        self.advance_after_card_reward();
        ActionResult::Advanced
    }

    fn do_relic_reward_pick(&mut self) -> ActionResult {
        // Redeem the last queued relic reward through the canonical
        // helper so the `after_obtained` channel fires (Strawberry /
        // PotionBelt / NeowsTalisman / MawBank / Orrery / … run-side
        // listeners). The raw `relics.push` here previously skipped
        // dispatch, silently no-op'ing every on-obtain relic picked
        // through the native run loop. `accept_relic_reward` runs the
        // dispatch under `with_deferred_followup_rewards`, so any
        // follow-up-reward listener (WarPaint et al.) queues a
        // player-pickable reward that `advance_after_card_reward` then
        // surfaces. Bead `sts2-rs-wt4w1`.
        if let Some(idx) = self.run_state.relic_rewards.len().checked_sub(1) {
            let _ = self.run_state.accept_relic_reward(idx);
        }
        // Bead `sts2-rs-l0wbw` (GH #70): same C# `TestMode.IsOn`
        // auto-resolution as `do_event_choice` — if accepting this relic
        // fired an after-obtained listener that queued more rewards
        // (SmallCapsule chains a follow-up Common; ToyBox chains 1),
        // drain them inline so the workbench's `direct_event_relic_grant`
        // path doesn't bounce back to `Phase::CardReward` for each.
        if self.run_state.direct_event_relic_grant {
            let _ = self.run_state.drain_relic_rewards_directly();
        }
        self.advance_after_card_reward();
        ActionResult::Advanced
    }

    fn do_relic_reward_skip(&mut self) -> ActionResult {
        let _ = self.run_state.relic_rewards.pop();
        self.advance_after_card_reward();
        ActionResult::Advanced
    }

    /// Materialize the otherwise write-only `pending_rewards` queue
    /// (bead `sts2-rs-01o`). Drains every [`Reward::Potion`] sentinel
    /// into the run-level potion belt (rolling a random kind via
    /// `roll_potion_reward_kind` when the sentinel carries none), and —
    /// if no card-reward offering is already open — pops one queued
    /// [`Reward::Card`] sentinel and fills `card_reward_options` via
    /// `generate_card_reward_options`.
    ///
    /// Returns `true` iff a card-reward offering was opened, so the
    /// caller can route to [`Phase::CardReward`]. Potion drains resolve
    /// immediately and never block this drain — though on a full belt
    /// under `interactive_potion_swap` the potion parks on
    /// `pending_potion_swaps`, whose `PotionSwap` decision preempts the
    /// action surface afterwards (GH #252). Other [`Reward`] variants
    /// (Gold / Relic / UpgradeCards / EnchantCards) are left in place —
    /// they have their own surfaces or are not yet routed through this
    /// queue.
    ///
    /// Before this, relics and events that pushed `Reward::Card` /
    /// `Reward::Potion` onto `pending_rewards` (Lost Coffer, Orrery,
    /// PhialHolster, BrainLeech, Overgrowth, …) silently granted
    /// nothing: nothing in the run loop ever read the queue.
    ///
    /// [`Reward::Potion`]: crate::runstate::Reward::Potion
    /// [`Reward::Card`]: crate::runstate::Reward::Card
    /// [`Reward`]: crate::runstate::Reward
    fn drain_pending_rewards(&mut self) -> bool {
        use crate::runstate::Reward;
        // Walk the queue IN ORDER: C# `RewardsCmd.OfferCustom` populates the
        // offered rewards in list order, so a `[Card, Potion]` queue (Lost
        // Coffer) must consume the Rewards stream as [card rolls, potion
        // rolls] — resolving all potions first shifted every card pick (RP0:
        // Rust offered Bully where C# offered PommelStrike). The first Card
        // sentinel surfaces inline (rolling its options right here, in queue
        // position); processing stops at a SECOND Card sentinel, whose rolls
        // belong to a later drain.
        let mut opened = false;
        let mut i = 0;
        while i < self.run_state.pending_rewards.len() {
            // Copy the resolvable payload out before mutating — the branches
            // carry `Copy` data, so this ends the immutable borrow cleanly.
            enum Resolve {
                Potion(Option<crate::potions::PotionKind>),
                RemoveCard(u32),
                SurfaceCard(Vec<crate::cards::CardId>),
            }
            let queued = match &self.run_state.pending_rewards[i] {
                Reward::Potion(reward) => Some(Resolve::Potion(reward.potion_kind)),
                Reward::RemoveCard { count } => Some(Resolve::RemoveCard(*count)),
                Reward::Card(reward) if self.card_reward_options.is_empty() && !opened => {
                    // Pre-resolved options (C# `OfferCustom` with an
                    // already-populated CardReward — BrainLeech's
                    // colorless 3) surface verbatim; an empty sentinel
                    // rolls at queue position below.
                    Some(Resolve::SurfaceCard(reward.options.clone()))
                }
                Reward::Card(_) => break, // later drain rolls this one
                _ => None,
            };
            let Some(resolve) = queued else {
                i += 1;
                continue;
            };
            self.run_state.pending_rewards.remove(i);
            match resolve {
                Resolve::Potion(kind_opt) => {
                    let kind = kind_opt.or_else(|| {
                        self.callbacks
                            .roll_potion_reward_kind
                            .and_then(|f| f(&mut self.run_state))
                    });
                    if let Some(kind) = kind {
                        // Full belt: queue an interactive swap or count the
                        // drop — never silently evaporate (GH #252).
                        let _ = self.run_state.claim_potion_reward(kind);
                    }
                }
                Resolve::RemoveCard(count) => {
                    // Interactive hosts queue `count` deck picks
                    // (`pending_deck_removals`); parity mode auto-removes
                    // `count` from the deck front. Either way this is the
                    // `CardRemovalReward.OnSelect` → `DoLocalCardRemoval` analog.
                    let _ = self.run_state.remove_cards_from_deck(count as usize);
                }
                Resolve::SurfaceCard(pre_resolved) => {
                    if pre_resolved.is_empty() {
                        // Non-combat (`CardCreationSource.Other`) reward: roll
                        // BASE odds, NOT the pity-updating combat roller. These
                        // sentinels (Lost Coffer, Orrery, event card grants)
                        // must not mutate the run pity counter — doing so
                        // over-accumulated the offset and flipped a later
                        // combat reward's rarity (PARITY132). Falls back to the
                        // pity roller only if the base-odds callback is unwired.
                        let roller = self
                            .callbacks
                            .generate_noncombat_card_reward_options
                            .or(self.callbacks.generate_card_reward_options);
                        self.card_reward_options =
                            roller.map(|f| f(&mut self.run_state)).unwrap_or_default();
                        self.card_reward_upgraded =
                            std::mem::take(&mut self.run_state.card_reward_upgraded_flags);
                    } else {
                        self.card_reward_options = pre_resolved;
                        self.card_reward_upgraded = Vec::new();
                    }
                    opened = true;
                }
            }
            // Do not advance `i`: the vector shifted left.
        }
        opened
    }

    /// Move a pending Scroll Boxes bundle choice (`run_state.pending_bundles`)
    /// into the `card_reward_bundles` marker, surfacing it through the normal
    /// `Phase::CardReward` fan-out as a pick-1-of-N-packs (no skip). Returns
    /// `true` iff a bundle offering was opened. No-op when a card/bundle
    /// offering is already up or no bundle is pending.
    fn surface_pending_bundle(&mut self) -> bool {
        if !self.card_reward_options.is_empty() || self.card_reward_bundles.is_some() {
            return false;
        }
        if self.run_state.pending_bundles.is_empty() {
            return false;
        }
        let bundles = std::mem::take(&mut self.run_state.pending_bundles);
        // One representative card per pack keeps `card_reward_options`
        // non-empty (so the phase stays CardReward and the fan-out yields one
        // pick per pack); `do_card_reward_pick` reads `card_reward_bundles`,
        // never these representatives.
        self.card_reward_options = bundles.iter().filter_map(|b| b.first().copied()).collect();
        self.card_reward_upgraded.clear();
        self.card_reward_bundles = Some(bundles);
        true
    }

    fn advance_after_card_reward(&mut self) {
        if !self.card_reward_options.is_empty() {
            return;
        }
        // sts2-agent fork: the rewards screen stays until `ProceedRewards`.
        if self.reward_screen.is_some() {
            self.phase = Phase::CardReward;
            return;
        }
        if !self.run_state.relic_rewards.is_empty() {
            // Stay in CardReward; the caller picks PickRelicReward/SkipRelic next.
            return;
        }
        // Drain any pending custom card-select (HeftyTablet's 3-Rare /
        // LeadPaperweight's 2-Colorless offer) once the relic-reward queue
        // is empty. Moving its options into `card_reward_options` surfaces
        // the choice through the normal PickCardReward fan-out (skip is
        // always offered, matching C# `canSkip: true`); the
        // `card_offer_choose_screen` marker lets the Python backend emit it
        // as a `card_select` decision (C# `FromChooseACardScreen` headless
        // shape, min 0 / max 1) instead of `card_reward`. Any `post_add`
        // cards (HeftyTablet's Injury) land when the screen resolves —
        // AFTER the chosen card, matching C#'s single
        // `CardPileCmd.Add([chosen?, Injury], Deck)`. The standard
        // post-combat reward is callback-generated and never touches this
        // queue, so the two never collide.
        if !self.run_state.pending_card_selects.is_empty() {
            let select = self.run_state.pending_card_selects.remove(0);
            self.card_reward_options = select.options;
            // Carry the per-option offered-upgrade flags so `do_card_reward_pick`
            // grants the picked card with its offered upgrade state. For
            // SilverCrucible-modified slates (BrainLeech ShareKnowledge routed
            // through `offer_card_reward`) every upgradable option is offered
            // upgraded (C# `SilverCrucible.TryModifyCardRewardOptionsLate`);
            // for the bare-id callers (HeftyTablet, shop-event) these are
            // all-false. Replaces the prior unconditional `.clear()`, which
            // always granted unupgraded.
            self.card_reward_upgraded = select.upgraded;
            self.card_offer_choose_screen = true;
            self.pending_choose_screen_post_add = select.post_add;
            self.card_offer_can_skip = select.can_skip;
            // Stay in CardReward — get_available_actions now fans out the
            // option picks.
            return;
        }
        // Bead `sts2-rs-01o`: materialize any reward sentinels left on
        // `pending_rewards` (Lost Coffer / Orrery / event card+potion
        // grants). Potions drain into the belt; a queued card opens a
        // fresh offering and keeps us in CardReward.
        if self.drain_pending_rewards() {
            return;
        }
        // Scroll Boxes: surface the pending bundle pick (pick 1 of N packs).
        if self.surface_pending_bundle() {
            return;
        }
        // Card/relic-reward queues are now drained — a relic grant that opened
        // an offering (a NeowsBones-granted ArcaneScroll / LostCoffer) has
        // settled, so append any curse it deferred. No-op unless the deck-pick
        // queues are also clear; safe regardless. See
        // `drain_post_pick_deck_cards_if_settled`.
        self.drain_post_pick_relic_grants_if_settled();
        self.drain_post_pick_deck_cards_if_settled();
        // A deferred relic grant (NeowsBones → LostCoffer-then-HeftyTablet)
        // just fired its `after_obtained`, which may have queued an
        // interactive card select (HeftyTablet's 3-Rare choice + post_add
        // Injury) or a follow-up reward. Surface it now instead of falling
        // through to the map — mirroring `settle_after_deck_pick`'s tail.
        // Without this, the card-reward-first defer path (LostCoffer's card
        // pick resolves and re-enters here with `pending_card_selects` still
        // empty at line ~2284, THEN the drain above grants HeftyTablet and
        // queues its select) strands the just-queued select and drops the
        // Injury (C# `HeftyTablet.cs:32-38` adds `[chosen?, Injury]` to the
        // Deck only after the surfaced `FromChooseACardScreen` resolves).
        // Recursion terminates: the inner call hits the
        // `pending_card_selects` branch, moves the options into
        // `card_reward_options`, and returns; any further inner drain is a
        // no-op because `grant_batch_settled()` is false while
        // `card_reward_options` is non-empty. The `return` keeps the
        // boss/map fall-through from running once a select is surfaced.
        if !self.run_state.pending_card_selects.is_empty()
            || !self.run_state.relic_rewards.is_empty()
            || self.drain_pending_rewards()
        {
            self.phase = Phase::CardReward;
            self.advance_after_card_reward();
            return;
        }
        // Fall-through reached ONLY when nothing above re-entered: the reward
        // queues drained without surfacing a fresh card/relic select. A
        // chained POTION reward (PhialHolster's 2 procured potions, queued on
        // `pending_rewards` by its `AfterObtained`) drains into the belt as a
        // side effect of the `drain_pending_rewards()` call above WITHOUT
        // returning a Card sentinel — so none of the three `||` conditions
        // fire and we never recurse. That side-effect drain is exactly what
        // settles the grant batch: with PhialHolster's potions cleared,
        // `pending_rewards` is empty, the last owed Neow relic is in, and
        // `card_reward_options`/`relic_rewards` are empty, so
        // `grant_batch_settled()` finally becomes true. Roll the deferred
        // NeowsBones curse NOW — matching C# `NeowsBones.cs:45-54`, which adds
        // the curse via `Rng.Niche.NextItem` strictly AFTER
        // `await RewardsSet().Offer()` returns (i.e. after BOTH granted Neow
        // relics' `AfterObtained` — including PhialHolster's potion procure —
        // settle). Without this the curse stays stranded and the deck is
        // off-by-one (parity neow_deck_size_minus1). Idempotent/safe: the
        // drain returns immediately when no curse/post-pick-card is pending
        // (the normal no-defer path), and the curse count is consumed via
        // `std::mem::take`, so it can never double-roll. This runs only on the
        // non-recursing fall-through, so it cannot perturb the recursion path
        // (which `return`s above) nor the already-passing chained-RELIC defer
        // case (A5P41 SmallCapsule).
        self.drain_post_pick_deck_cards_if_settled();
        // Boss combat: after the card reward resolves, surface the boss-relic
        // / next-act Ancient choice pre-generated at boss-win time (C# shows
        // the card reward first, then the relic — the boss `RewardsSet`
        // carries no relic; `RewardsSet.cs:181-185`). `boss_relic_options` is
        // only ever non-empty in this window (set in the Boss branch of the
        // win handler, cleared on `do_boss_relic_pick`), so this never fires
        // for a normal/elite card reward. The act advances only AFTER the
        // relic is picked (`do_boss_relic_pick` → `advance_boss_act`), so the
        // `pending_boss_act_advance` flag stays set here. GH #25 + bead
        // `sts2-rs-jenwp`.
        if !self.boss_relic_options.is_empty() {
            // Apply the `Rewards`-stream side effect of presenting the
            // next-act Ancient's options (C# `GenerateInitialOptions` runs the
            // per-relic `SetupForPlayer` here). Only `DustyTome.SetupForPlayer`
            // touches RNG — one `Rewards` read whenever DustyTome is offered.
            // This must fire AFTER the boss reward's gold/potion/card reads
            // (already drained above) and BEFORE the relic pick, so the shared
            // `Rewards` cursor stays read-for-read aligned with C#.
            if let Some(setup) = self.callbacks.setup_boss_relic_ancient {
                setup(&mut self.run_state, &self.boss_relic_options);
            }
            self.phase = Phase::BossRelic;
            return;
        }
        // Boss with an EMPTY relic pool (content gap / exhaustion): the relic
        // screen is skipped, so advance into the next act now that the boss's
        // card reward has drained. Consumes the flag so a later (non-boss)
        // card reward doesn't re-trigger it. Bead `sts2-rs-jenwp`.
        if std::mem::take(&mut self.pending_boss_act_advance) {
            self.advance_boss_act();
        }
        // Nothing left — back to map.
        self.phase = Phase::MapChoice;
    }

    fn do_boss_relic_pick(&mut self, idx: usize) -> ActionResult {
        if idx >= self.boss_relic_options.len() {
            return ActionResult::InvalidForPhase;
        }
        let relic = self.boss_relic_options[idx];
        self.boss_relic_options.clear();
        // ADVANCE THE ACT *BEFORE* GRANTING THE BOSS RELIC. C# obtains the
        // boss relic via the NEW act's opening Ancient event: `EnterNextAct`
        // → `EnterAct(CurrentActIndex + 1)` → `SetActInternal`
        // (`RunManager.cs:987/1003/1041`) sets `CurrentActIndex` to the new
        // act, runs `AfterActEntered` + the Ancient-entry heal, THEN the
        // Ancient event grants the relic and fires its `AfterObtained`. So a
        // boss relic is obtained while `CurrentActIndex` already names the new
        // act. This matters for act-snapshot relics — FurCoat, GoldenCompass,
        // SpoilsMap — whose `AfterObtained` records `CurrentActIndex` and which
        // are active only while `CurrentActIndex == snapshot` (e.g.
        // `FurCoat.after_obtained` latches the obtain act, granting its bonus
        // only in that act). Dispatching the obtain BEFORE the advance
        // snapshotted the OLD act, so the relic was dead the instant we landed
        // in the act where C# considers it obtained (a Regent A5 run-parity
        // divergence found by the first C#-oracle ascension sweep).
        // (PumpkinCandle was in this family under v0.103.2 but its v0.107.1
        // redesign replaced the act gate with a decaying KindleCount.)
        // The map regen + Ancient heal in `advance_boss_act` likewise precede
        // the relic obtain in C# (heal-to-full, then boss relic). Consumes the
        // `pending_boss_act_advance` flag set in the Boss win branch
        // (bead `sts2-rs-jenwp`).
        let _ = std::mem::take(&mut self.pending_boss_act_advance);
        self.advance_boss_act();
        self.run_state.relics.push(relic);
        self.run_state.last_obtained_relic = Some(relic);
        // Bead `sts2-rs-wt4w1`: fire the on-obtain channel so boss-relic
        // `after_obtained` listeners apply (matches the shop path, which
        // dispatches eagerly) — now with the act already advanced, matching C#.
        let _ = crate::run_effects::dispatch_after_obtained(&mut self.run_state);
        // An Ancient-rarity boss relic with an upon-pickup effect can QUEUE
        // follow-up relic rewards rather than just mutating run state — Toy Box
        // (`ToyBox.AfterObtained`, ToyBox.cs:87-97) pulls 4 wax relics via
        // `RewardsCmd.OfferCustom` (relic-queue pull, act-independent). The
        // boss pick has no later card-reward drain step before the map, so
        // those queued `relic_rewards` would be stranded (the player silently
        // loses them). C# auto-collects them on obtain (the headless grants all
        // four "from relic reward"), so drain the queue directly here to match.
        // No-op for the common case where the picked relic queues nothing.
        let _ = self.run_state.drain_relic_rewards_directly();
        // An Ancient-rarity boss relic can ALSO queue follow-up CARD rewards
        // via `RewardsCmd.OfferCustom` — Glass Eye's `AfterObtained`
        // (`GlassEye.cs:15-33`) builds 5 `CardReward`s (Common, Common,
        // Uncommon, Uncommon, Rare; each pick-1-of-3) and offers them. The C#
        // headless surfaces these as real interactive card-reward decisions
        // (the auto-collect short-circuit covers Gold/Relic/Potion only, NOT
        // Card; `RunSimulator.cs:3102-3115`), with the act already advanced
        // (probe shows act=2 alongside the card reward). So drain the
        // card/potion queue exactly like the treasure-relic path
        // (`do_relic_reward_pick`, run_manager.rs ~1573) and, if a card
        // offering opened, route to `Phase::CardReward` instead of straight to
        // the map. No-op for the common case (e.g. Toy Box, drained above as
        // `relic_rewards`) where the picked relic queued no card rewards.
        let opened_card_reward = self.drain_pending_rewards();
        if opened_card_reward
            || !self.run_state.relic_rewards.is_empty()
            || !self.run_state.pending_card_selects.is_empty()
        {
            self.phase = Phase::CardReward;
            self.advance_after_card_reward();
        } else {
            self.phase = Phase::MapChoice;
        }
        ActionResult::Advanced
    }

    /// Advance from a just-cleared boss room into the next act: bump the
    /// act index, carry run state forward, regenerate the Act-N+1 map, and
    /// pre-roll that act's boss. Split out of [`Self::do_boss_relic_pick`]
    /// (bead `sts2-rs-el3lx`) so the boss's standard combat reward (gold +
    /// card, bead `sts2-rs-jenwp`) can drain BEFORE the act advances —
    /// `advance_after_card_reward` calls this once `pending_boss_act_advance`
    /// is set and every boss reward is claimed.
    ///
    /// Without the act-advance, `current_act_map` stays pinned to Act-1 and
    /// `next_map_children_cols` / `do_map_move` read a stale map whose nodes
    /// don't match the player's freshly-cleared boss coord. Mirrors Python
    /// `_on_boss_relic_picked` → `_advance_to_next_act` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:1331-1345`.
    fn advance_boss_act(&mut self) {
        if self.run_state.act_index < 3
            && self.run_state.current_room.map(|r| r.room_type)
                == Some(crate::runstate::RoomType::Boss)
        {
            self.run_state.advance_act();
            let new_act = self.run_state.act_index;
            self.carry_to_next_act();
            // Ancient-event entry heal (C# AncientEventModel.BeforeEventStarted):
            // the new act's Ancient event heals the player — to full at base
            // ascension, 80% of missing HP at A2 WearyTraveler. Mirror of the
            // same step in `advance_act_on_boss_kill`; this is the production
            // phase-machine act-advance path driven by `do_boss_relic_pick`.
            let _ = self.run_state.apply_ancient_event_heal();
            // Each act opens with its Ancient event room (the "boss relic"
            // Ancient for acts 2/3). C# `MarkRoomVisited`s that RoomType.Event
            // room on entry (`RunManager.cs:901-903` — CurrentRoomCount==1 →
            // `RoomSet.MarkVisited(Event)` → `eventsVisited++`,
            // `RoomSet.cs:92-93`), so the NEW act's event-pool cursor is 1
            // before any pool event is pulled. WITHOUT this bump the act-2/3
            // first event is pulled from pool slot 0 instead of 1, serving the
            // wrong event (cascading gold/relic/phase divergences). The sibling
            // `advance_act_on_boss_kill` (3219-3221) and the act-1 Neow path
            // (`with_neow_start`, ~836) already do this; this LIVE
            // phase-machine path had drifted and omitted it (sts2-rs-RP19).
            if let Some(act) = self.run_state.acts.get_mut(usize::from(new_act) - 1) {
                act.events_visited += 1;
            }
            // Per-act map length: acts shrink (C# `ActModel.BaseNumberOfRooms`
            // 15/14/13 for acts 1/2/3 → `StandardActMap._mapLength = rooms + 1`,
            // boss at rows 16/15/14). Source the NEW act's `num_rooms` instead
            // of a hardcoded 15 (which built a too-tall act-2/3 map with the
            // boss one row low); mirrors `RunState::generate_map` / `act_map_for`.
            let num_rooms = self
                .run_state
                .acts
                .get(usize::from(new_act).saturating_sub(1))
                .map_or(crate::acts::DEFAULT_ACT_NUM_ROOMS, |a| {
                    i32::from(a.num_rooms)
                });
            let act_idx_rng_arg = i32::from(new_act).saturating_sub(1);
            let act_idx_gen = i32::from(new_act);
            // A1 SwarmingElites must reach EVERY act's map, not just act
            // 1 — C# `MapPointTypeCounts.NumOfElites` reads the live
            // ascension on each `StandardActMap` build
            // (`MapPointTypeCounts.cs:15`). This seam previously passed a
            // hardcoded 0, so acts 2/3 dropped back to 5 elites at A1+.
            // Fixed per the 2026-06-12 ascension audit (sts2-rs-40ikw.2).
            let ascension_level = i32::from(self.run_state.ascension_level);
            let mut act_map = {
                let rng = self.run_state.rng.map_rng(act_idx_rng_arg);
                crate::map_generator::generate_act_map(
                    num_rooms,
                    rng,
                    ascension_level,
                    false,
                    act_idx_gen,
                    None,
                )
            };
            self.run_state.modify_generated_map(&mut act_map, new_act);
            // A10 DoubleBoss: the final act's map carries the second boss
            // node (C# threads `Act.HasSecondBoss` into the map ctor,
            // `StandardActMap.cs:96`). No RNG. Bead sts2-rs-40ikw.2.
            if self.run_state.has_double_boss && self.run_state.is_last_act() {
                act_map.attach_second_boss();
            }
            let _ = crate::run_effects::dispatch_after_act_entered(&mut self.run_state, new_act);
            self.run_state.current_act_map = Some(act_map);
            // Pre-roll the new act's boss now that the map is laid out
            // (same timing as the C# game). Mirrors the Act-1 invocation
            // wired in [`Self::with_content_callbacks`].
            if let Some(pre_roll) = self.callbacks.pre_roll_act_boss {
                pre_roll(&mut self.run_state);
            }
        }
    }

    /// Active shop inventory while [`Self::phase`] is [`Phase::Shop`].
    /// `None` outside Shop or when
    /// [`ContentCallbacks::generate_shop_inventory`] is not wired.
    /// Bead `sts2-rs-if6ry` (W3).
    #[must_use]
    pub const fn shop_inventory(&self) -> Option<&crate::shop::ShopInventory> {
        self.shop_inventory.as_ref()
    }

    /// Boss-relic offers currently visible to the player on the
    /// [`Phase::BossRelic`] choice screen, in the order
    /// `RunAction::PickBossRelic { idx }` indexes them. Empty outside
    /// `BossRelic` (cleared on the pick / on every act-advance).
    #[must_use]
    pub fn boss_relic_options(&self) -> &[crate::RelicId] {
        &self.boss_relic_options
    }

    /// After a successful merchant purchase, re-populate the just-bought
    /// slot when the player owns `TheCourier` (the only relic whose C#
    /// `ShouldRefillMerchantEntry` returns `true`). Re-rolls the slot's
    /// item + price on the run's `Shops`/`Rewards` streams via the
    /// content-layer [`ContentCallbacks::restock_shop_entry`], keeping
    /// the streams aligned with C# `RestockAfterPurchase` for any later
    /// shop visit. No-op without Courier or when the callback is unwired.
    fn restock_slot_if_courier(&mut self, slot: crate::shop::ShopSlot) {
        if !self
            .run_state
            .owns_active_relic(crate::relics::RelicId::TheCourier)
        {
            return;
        }
        let Some(restock) = self.callbacks.restock_shop_entry else {
            return;
        };
        let Some(inv) = self.shop_inventory.as_mut() else {
            return;
        };
        restock(&mut self.run_state, inv, slot);
    }

    fn do_shop_buy_card(&mut self, idx: usize) -> ActionResult {
        let Some(inv) = self.shop_inventory.as_mut() else {
            return ActionResult::InvalidForPhase;
        };
        match crate::shop::buy_card(&mut self.run_state, inv, idx) {
            crate::shop::ShopPurchaseOutcome::Bought => {
                self.restock_slot_if_courier(crate::shop::ShopSlot::Card(idx));
                ActionResult::Advanced
            }
            other => ActionResult::ShopPurchaseFailed(other),
        }
    }

    fn do_shop_buy_relic(&mut self, idx: usize) -> ActionResult {
        let Some(inv) = self.shop_inventory.as_mut() else {
            return ActionResult::InvalidForPhase;
        };
        match crate::shop::buy_relic(&mut self.run_state, inv, idx) {
            crate::shop::ShopPurchaseOutcome::Bought => {
                self.restock_slot_if_courier(crate::shop::ShopSlot::Relic(idx));
                ActionResult::Advanced
            }
            other => ActionResult::ShopPurchaseFailed(other),
        }
    }

    fn do_shop_buy_potion(&mut self, idx: usize) -> ActionResult {
        let Some(inv) = self.shop_inventory.as_mut() else {
            return ActionResult::InvalidForPhase;
        };
        match crate::shop::buy_potion(&mut self.run_state, inv, idx) {
            crate::shop::ShopPurchaseOutcome::Bought => {
                // `buy_potion` routes through `procure_potion`, which
                // queues the potion on `pending_rewards` rather than
                // adding it to the belt directly. Combat rewards drain
                // that queue at the reward screen, but a shop stays in
                // `Phase::Shop`, so without an explicit drain the bought
                // potion never reaches `run_state.potions` and the UI
                // shows nothing for it. Drain now so the held belt
                // reflects the purchase immediately (matching the Python
                // pin, where the potion is held the moment it's bought).
                //
                // Restock BEFORE the drain: C# `RestockAfterPurchase` runs
                // inside `OnTryPurchaseWrapper` right after the buy, and the
                // re-roll consumes `Shops` RNG while the pending-reward drain
                // is RNG-free — so the stream position stays C#-aligned.
                self.restock_slot_if_courier(crate::shop::ShopSlot::Potion(idx));
                self.drain_pending_rewards();
                ActionResult::Advanced
            }
            // A full belt (or procure veto) is a graceful no-op in C#:
            // `MerchantPotionEntry.OnTryPurchase` returns `(false, 0)` —
            // no gold spent, slot stays stocked, the merchant room stays
            // open. Surface it as an accepted no-op so the run stays in
            // `Phase::Shop` rather than a hard failure (a failure would
            // read as an illegal action and break whole-run lockstep,
            // where C# accepts the click and simply does nothing).
            crate::shop::ShopPurchaseOutcome::PotionBeltFull => ActionResult::Advanced,
            other => ActionResult::ShopPurchaseFailed(other),
        }
    }

    fn do_shop_remove_card(&mut self, deck_idx: usize) -> ActionResult {
        let Some(inv) = self.shop_inventory.as_mut() else {
            return ActionResult::InvalidForPhase;
        };
        match crate::shop::purchase_card_removal(&mut self.run_state, inv, deck_idx) {
            crate::shop::PurchaseCardRemovalResult::Removed { .. } => ActionResult::Advanced,
            other => ActionResult::ShopRemoveCardFailed(other),
        }
    }

    fn do_rest_site_at(&mut self, idx: usize) -> ActionResult {
        let opts = self.run_state.rest_site_options();
        if idx >= opts.len() {
            return ActionResult::InvalidForPhase;
        }
        let _ = self.do_rest_site_action(opts[idx]);
        // do_rest_site_action transitions to MapChoice on terminal outcomes.
        ActionResult::Advanced
    }

    /// Resolve the smith sub-state opened by a multi-card Smith pick.
    /// Pops the [`crate::ChoiceKind::CardSelect`] pending choice via
    /// [`crate::runstate::RunState::resolve_pending_card_choice`],
    /// upgrades the first matching un-upgraded master-deck entry, and
    /// transitions back to [`Phase::MapChoice`]. Mirrors the side
    /// effect of the single-card auto-resolve path inside
    /// `RunState::do_rest_site_action`.
    ///
    /// `idx` indexes the same options list that
    /// [`RunManager::get_available_actions`] enumerates while the
    /// smith pending choice is open. Returns
    /// [`ActionResult::InvalidForPhase`] when no pending choice is
    /// open, the choice is not a `CardSelect`, or `idx` is out of
    /// range.
    ///
    /// **Port pin:** the upgrade side effect mirrors Python
    /// `sts2-rl-agent@01c094d:sts2_env/run/rest_site.py:99-104`
    /// (`resolver=lambda selected: [player.upgrade_card_instance(c)
    /// for c in selected]` consumed by `request_deck_choice`) and C#
    /// `v0.103.2 MegaCrit.Sts2.Core.Entities.RestSite/SmithRestSiteOption.cs:69-72`
    /// (`foreach (CardModel item in _selection) CardCmd.Upgrade(item,
    /// CardPreviewStyle.None);`). The `Hook.AfterRestSiteSmith`
    /// listener-channel fan-out from the C# pin (line 73) is NOT
    /// invoked here yet — file a follow-up bead once a listener
    /// requires it (no production listener at the current pin).
    ///
    /// Bead `sts2-rs-5abnx` (W5b); pin headers added by
    /// `sts2-rs-z4qwd`.
    /// Resolve one pending interactive deck removal: drop the master-deck
    /// card at `idx` and decrement the pending count. Bounds-checked; only
    /// valid while [`crate::runstate::RunState::pending_deck_removals`] is
    /// `> 0` (the action surface guards this). When the count hits zero the
    /// run resumes the phase that granted the removing relic.
    fn do_remove_deck_card_at(&mut self, idx: usize) -> ActionResult {
        if self.run_state.pending_deck_removals == 0 {
            return ActionResult::InvalidForPhase;
        }
        // `idx` is a position in the C# `FromDeckForRemoval` offer order
        // (removable, curse-front), recomputed each call so a prior removal in
        // the same batch keeps earlier indices valid — map it to the raw
        // master-deck position before removing. Bead: deck-removal curse-front
        // parity (RP94 Necrobinder).
        let Some(&deck_idx) = self.run_state.removal_offer_order().get(idx) else {
            return ActionResult::InvalidForPhase;
        };
        self.run_state.master_deck.remove(deck_idx);
        self.run_state.pending_deck_removals -= 1;
        // Once the removal batch fully drains, append any cards a
        // "remove-then-add" event deferred (Wellspring Bathe → Guilty).
        // Deferring kept them out of the removal `card_select` pool so the
        // offering matched C#. Bead `sts2-rs-1hiht`.
        if self.run_state.pending_deck_removals == 0 {
            if !self.run_state.pending_post_removal_cards.is_empty() {
                let cards = std::mem::take(&mut self.run_state.pending_post_removal_cards);
                self.run_state.master_deck.extend(cards);
            }
            // Apply any HP damage a "remove-then-damage" relic deferred
            // (PrecariousShears → Damage(16)). C# suspends on the removal pick
            // and damages only after it returns, so the HP drop must land here,
            // not at obtain. Bead `sts2-rs-1hiht`.
            let hp_loss = std::mem::take(&mut self.run_state.pending_post_removal_hp_loss);
            if hp_loss > 0 {
                let _ = self.run_state.lose_hp(hp_loss);
            }
        }
        self.settle_after_deck_pick();
        ActionResult::Advanced
    }

    /// Resolve the FRONT pending full-belt potion-reward swap (GH #252).
    /// `Some(slot)` discards the held belt potion at `slot` and claims the
    /// pending reward potion in its place (C# `DiscardPotionGameAction`
    /// followed by re-selecting the TooFull `PotionReward`); `None`
    /// declines — the reward potion is lost and counted in
    /// [`crate::runstate::RunState::potions_overflow_dropped`] (C#: the
    /// reward screen is left with the potion unclaimed). Out-of-range
    /// slots are rejected. Only valid while
    /// [`crate::runstate::RunState::pending_potion_swaps`] is non-empty.
    fn do_potion_swap(&mut self, discard_slot: Option<usize>) -> ActionResult {
        if self.run_state.pending_potion_swaps.is_empty() {
            return ActionResult::InvalidForPhase;
        }
        if let Some(slot) = discard_slot {
            if slot >= self.run_state.potions.len() {
                return ActionResult::InvalidForPhase;
            }
            let kind = self.run_state.pending_potion_swaps.remove(0);
            self.run_state.potions[slot] = kind;
        } else {
            self.run_state.pending_potion_swaps.remove(0);
            self.run_state.potions_overflow_dropped += 1;
        }
        ActionResult::Advanced
    }

    /// Resolve one pending interactive deck upgrade. `idx` is the position
    /// within the NON-UPGRADED subset of the master deck (matching C#'s
    /// `FromDeckForUpgrade` `PendingOptions` index — the same value the
    /// headless card_select emits), NOT a raw master-deck index. Maps it to
    /// the deck card, upgrades it, and decrements the pending count.
    /// Out-of-range picks are rejected. Only valid while
    /// [`crate::runstate::RunState::pending_deck_upgrades`] is `> 0`. When the
    /// count hits zero the run resumes the phase that granted the upgrading
    /// relic. Bead `sts2-rs-1hiht`.
    fn do_upgrade_deck_card_at(&mut self, idx: usize) -> ActionResult {
        if self.run_state.pending_deck_upgrades == 0 {
            return ActionResult::InvalidForPhase;
        }
        let Some(deck_idx) = self
            .run_state
            .master_deck
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_upgradable())
            .nth(idx)
            .map(|(i, _)| i)
        else {
            return ActionResult::InvalidForPhase;
        };
        self.run_state.master_deck[deck_idx].upgrade();
        self.run_state.pending_deck_upgrades -= 1;
        self.settle_after_deck_pick();
        ActionResult::Advanced
    }

    /// Append any cards a relic-grant batch deferred (currently only
    /// `NeowsBones`' curse, via [`crate::runstate::RunState::pending_post_pick_cards`])
    /// once the run has fully settled after the grant — i.e. every interactive
    /// deck pick (upgrade / removal / transform / enchant) the granted relics
    /// queued AND any card/relic-reward offering they opened has resolved.
    /// Mirrors C# `NeowsBones.AfterObtained`, which adds its curse only after
    /// `await RewardsSet.Offer()` (the granted relics, each running its own
    /// `AfterObtained`) returns. A no-op while any of those queues is still
    /// pending, and a no-op when the deferral queue is empty (the common case),
    /// so it is safe — and cheap — to call from every settle point.
    /// True when no interactive deck pick or reward offering opened by a
    /// relic/event grant is still outstanding — the RunState-side check
    /// ([`RunState::has_pending_grant_choice`]) plus this manager's own
    /// `card_reward_options`. The shared settle gate for the two post-pick
    /// drains below.
    fn grant_batch_settled(&self) -> bool {
        !self.run_state.has_pending_grant_choice() && self.card_reward_options.is_empty()
    }

    /// Grant the next relic a multi-grant listener deferred
    /// ([`RunState::pending_post_pick_relic_grants`]) once the previously
    /// granted relic's interactive `AfterObtained` has fully settled. Grants
    /// exactly ONE per call: obtaining it may itself surface a fresh pick, in
    /// which case the batch is no longer settled and the remaining relics stay
    /// queued until the next drain. Mirrors C# `RewardsSet.Offer()` awaiting
    /// each relic in turn. Must run BEFORE
    /// [`Self::drain_post_pick_deck_cards_if_settled`] so the deferred curse
    /// lands only after every owed relic (and its picks) is in.
    fn drain_post_pick_relic_grants_if_settled(&mut self) {
        if self.run_state.pending_post_pick_relic_grants.is_empty() || !self.grant_batch_settled() {
            return;
        }
        let rid = self.run_state.pending_post_pick_relic_grants.remove(0);
        self.run_state
            .relic_rewards
            .push(crate::runstate::RelicReward::new(rid));
        let idx = self.run_state.relic_rewards.len() - 1;
        let _ = self.run_state.accept_relic_reward(idx);
        // The deferred relic's own `AfterObtained` may have chained a fresh
        // relic reward (e.g. NeowsBones → SmallCapsule deferred here, whose
        // `OfferCustom(new RelicReward)` rolls PotionBelt). Under
        // `direct_event_relic_grant` (run-parity / workbench) auto-collect it
        // inline, exactly as `do_relic_reward_pick` (L2120) and the
        // `do_event_choice` block (L3115) do — mirroring C#
        // `RewardsSet.Offer()` under `TestMode.IsOn`
        // (`MegaCrit.Sts2.Core.Rewards/RewardsSet.cs:125-136`), which
        // auto-invokes `OnSelectWrapper()` per reward with no UI step. Without
        // this the chained PotionBelt lingers on `relic_rewards`, so
        // `grant_batch_settled()` stays false forever and the immediately
        // following `drain_post_pick_deck_cards_if_settled()` can never roll
        // the deferred NeowsBones curse — stranding BOTH the relic and the
        // curse (deck off-by-one). (parity deck_size_minus1_step2)
        if self.run_state.direct_event_relic_grant {
            let _ = self.run_state.drain_relic_rewards_directly();
        }
    }

    /// Shared tail for the interactive deck-pick resolvers (transform /
    /// remove / upgrade / enchant). Once this pick settles it grants the next
    /// relic a multi-grant listener deferred (NeowsBones' 2nd relic) and
    /// SURFACES any follow-up reward that relic's `AfterObtained` opened —
    /// e.g. Lost Coffer's card + potion — mirroring the direct grant paths
    /// (treasure room / event ~L1523), so the promised reward screen appears
    /// instead of silently dropping to the map. The deferred curse lands
    /// last, and only after any reward offering surfaced here also resolves
    /// (`grant_batch_settled` gates the deck-card drain on
    /// `card_reward_options`).
    fn settle_after_deck_pick(&mut self) {
        self.drain_post_pick_relic_grants_if_settled();
        // Surface a follow-up reward only once every interactive deck pick the
        // grant batch queued has resolved — mid-batch picks keep their own
        // queues hot. The reward queues (`pending_rewards` / `relic_rewards`)
        // are exactly what we're about to surface, so they're excluded from
        // this "still picking?" test (which is why `has_pending_grant_choice`
        // can't be reused here verbatim).
        let still_picking = self.run_state.pending_deck_removals != 0
            || self.run_state.pending_deck_upgrades != 0
            || !self.run_state.pending_deck_transforms.is_empty()
            || !self.run_state.pending_deck_enchants.is_empty();
        if !still_picking {
            let opened_card_reward = self.drain_pending_rewards();
            if opened_card_reward
                || !self.run_state.relic_rewards.is_empty()
                || !self.run_state.pending_card_selects.is_empty()
            {
                self.phase = Phase::CardReward;
                self.advance_after_card_reward();
            }
        }
        self.drain_post_pick_deck_cards_if_settled();
    }

    fn drain_post_pick_deck_cards_if_settled(&mut self) {
        if self.run_state.pending_post_pick_cards.is_empty()
            && self.run_state.pending_neows_bones_curses == 0
        {
            return;
        }
        // The curse waits for every owed relic grant too, not just picks —
        // C# adds it only after the whole `RewardsSet.Offer()` await returns.
        let settled =
            self.grant_batch_settled() && self.run_state.pending_post_pick_relic_grants.is_empty();
        if !settled {
            return;
        }
        // Roll any deferred NeowsBones curse NOW — AFTER every granted relic's
        // own interactive `Niche` read (NewLeaf's transform) — matching C#'s
        // post-`Offer()` draw order, then append it (RP135). When the callback
        // is unwired (raw-core tests) the count is consumed without a roll.
        let curses = std::mem::take(&mut self.run_state.pending_neows_bones_curses);
        if let Some(roll) = self.callbacks.roll_neows_bones_curse {
            for _ in 0..curses {
                if let Some(card) = roll(&mut self.run_state) {
                    self.run_state.master_deck.push(card);
                }
            }
        }
        let cards = std::mem::take(&mut self.run_state.pending_post_pick_cards);
        self.run_state.master_deck.extend(cards);
    }

    /// Master-deck positions eligible for the FRONT pending interactive
    /// transform. Mirrors C# `CardSelectCmd.FromDeckForTransformation`
    /// (`CardSelectCmd.cs:306`), which offers
    /// `Deck.Cards.Where(c => c.Type != Quest && c.IsTransformable)`. In the
    /// Deck pile `IsTransformable` (`CardModel.cs:576-587`) reduces to
    /// `IsRemovable` (`CardModel.cs:574` = `!Keywords.Contains(Eternal)`):
    /// a non-removable card (the Eternal curses — Ascender's Bane, Greed) is
    /// transformable only when it is OUTSIDE the Deck pile, so in the deck it
    /// is excluded. We drop those via the registered
    /// [`crate::lookup_card_removable`] fn (Ascender's Bane carries
    /// `CardKeyword.Eternal`, `AscendersBane.cs:12-17`; appended to the
    /// starter deck at Ascension 5+). This is load-bearing: an over-offer
    /// here shifts every later deck index, so a fixed-index pick lands on the
    /// wrong card (PARITY127, Silent A5). Quest exclusion needs no analogue —
    /// Rust has no Quest card type.
    ///
    /// `basic_only` entries (Wood Carvings) additionally filter to Basic
    /// rarity via the registered [`crate::lookup_card_rarity_rank`] fn (C#
    /// discriminant `Basic == 1`). When either registry is unregistered
    /// (raw-core tests that never install the content registry) that filter
    /// falls back to "everything", keeping the legacy unfiltered order.
    #[must_use]
    pub fn transform_eligible_deck_indices(&self) -> Vec<usize> {
        let basic_only = self
            .run_state
            .pending_deck_transforms
            .first()
            .is_some_and(|t| t.basic_only);
        let rank = crate::lookup_card_rarity_rank();
        let removable = crate::lookup_card_removable();
        self.run_state
            .master_deck
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                // C# `IsTransformable` in the Deck pile == `IsRemovable`:
                // drop the Eternal curses (Ascender's Bane / Greed).
                if let Some(f) = removable {
                    if !f(c) {
                        return false;
                    }
                }
                if !basic_only {
                    return true;
                }
                match rank {
                    Some(f) => f(c.card_id) == 1,
                    None => true,
                }
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Master-deck positions eligible for the FRONT pending interactive
    /// enchant. Mirrors C# `EnchantmentModel.CanEnchant` (type not
    /// Status/Curse/Quest, no existing enchantment, not Unplayable) plus
    /// the entry's optional `type_restriction` (SelfHelpBook's
    /// `c => c.Type == typeRestriction` selector predicate), OR-ed with
    /// `secondary_type` for two-type gates (RoyalStamp's RoyallyApproved =
    /// Attack OR Skill). X-cost cards
    /// pass the BASE `CanEnchant`; only `Slither.CanEnchant` overrides to
    /// reject them, so the `cost == -1` exclusion is keyed to Slither.
    /// `Nimble.CanEnchant` additionally requires `card.GainsBlock`
    /// (`Nimble.cs:7-14`) — proxied by `base_block > 0`; surfaced by the
    /// 2026-07-18 shadow-replay census (the real DLL offered only the
    /// deck's Defends for SelfHelpBook's Nimble pick, the engine also
    /// offered blockless Skills).
    #[must_use]
    pub fn enchant_eligible_deck_indices(&self) -> Vec<usize> {
        use crate::cards::CardType;
        let front = self.run_state.pending_deck_enchants.first();
        let restriction = front.and_then(|e| e.type_restriction);
        let secondary = front.and_then(|e| e.secondary_type);
        let allow_x_cost =
            front.is_some_and(|e| e.enchantment != crate::cards::EnchantmentId::Slither);
        let requires_block =
            front.is_some_and(|e| e.enchantment == crate::cards::EnchantmentId::Nimble);
        self.run_state
            .master_deck
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                // Rust `CardType` has no Quest variant (Quest cards are
                // identified by rarity); Status/Curse covers C#'s
                // `(uint)(type - 4) <= 2u` exclusion for the live decks.
                !matches!(c.card_type, CardType::Status | CardType::Curse)
                    && c.enchantments.is_empty()
                    && (c.cost >= 0 || (allow_x_cost && c.is_x_cost()))
                    && (!requires_block || c.base_block > 0)
                    && restriction
                        .map_or(true, |t| c.card_type == t || secondary == Some(c.card_type))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Resolve one pending interactive deck transform: map `idx` (position
    /// in [`Self::transform_eligible_deck_indices`]) to the deck card, take
    /// the front entry's `fixed_target` (C# `TransformTo<T>`, no RNG) or
    /// roll the replacement (content callback — C# rolls only AFTER the
    /// pick, `CardCmd.TransformToRandom(item, Rng.Niche)`), then apply the
    /// C# Deck-pile transform shape: remove the original, append the
    /// replacement at the deck tail (`CardCmd.Transform` `Deck` branch →
    /// `AddInternal(index: -1)`; the master-deck list order is load-bearing
    /// as the combat-start shuffle input). Bounds-checked; only valid while
    /// [`crate::runstate::RunState::pending_deck_transforms`] is non-empty.
    /// When a rolled entry's callback is unwired (raw-core tests) the pick
    /// drains without mutating the deck. Bead 6ynt.4.deck-add / GH #34.
    fn do_transform_deck_card_at(&mut self, idx: usize) -> ActionResult {
        if self.run_state.pending_deck_transforms.is_empty() {
            return ActionResult::InvalidForPhase;
        }
        let Some(&deck_idx) = self.transform_eligible_deck_indices().get(idx) else {
            return ActionResult::InvalidForPhase;
        };
        let entry = self.run_state.pending_deck_transforms.remove(0);
        if let Some(replacement) = entry.fixed_target {
            self.run_state.master_deck.remove(deck_idx);
            self.run_state.master_deck.push(replacement);
        } else if let Some(roll) = self.callbacks.roll_transform_replacement {
            let original = self.run_state.master_deck[deck_idx].card_id;
            if let Some(replacement) = roll(&mut self.run_state, original, entry.event_stream_roll)
            {
                self.run_state.master_deck.remove(deck_idx);
                self.run_state.master_deck.push(replacement);
            }
        }
        // C# applies the event's follow-up damage only after the picked
        // card transforms (WhisperingHollow.Hug: `CreatureCmd.Damage`
        // awaits the `TransformToRandom`), so the HP change lands on the
        // same step as the deck mutation. Out-of-combat death ends the
        // run on the spot (same check as `do_choose_event_option`).
        if entry.post_hp_loss > 0 {
            self.run_state.lose_hp(entry.post_hp_loss);
            if self.run_state.current_hp <= 0 {
                self.run_state.is_over = true;
                self.run_state.player_won = false;
                self.phase = Phase::RunOver;
            }
        }
        self.settle_after_deck_pick();
        ActionResult::Advanced
    }

    /// Resolve one pending interactive deck enchant: map `idx` (position in
    /// [`Self::enchant_eligible_deck_indices`]) to the deck card and stamp
    /// the front entry's enchantment stacks onto it (C#
    /// `CardCmd.Enchant<T>(card, amount)` — the enchantment is
    /// run-persistent on the master-deck card).
    fn do_enchant_deck_card_at(&mut self, idx: usize) -> ActionResult {
        if self.run_state.pending_deck_enchants.is_empty() {
            return ActionResult::InvalidForPhase;
        }
        let Some(&deck_idx) = self.enchant_eligible_deck_indices().get(idx) else {
            return ActionResult::InvalidForPhase;
        };
        let entry = self.run_state.pending_deck_enchants.remove(0);
        let card = &mut self.run_state.master_deck[deck_idx];
        // Add to the first existing stack, else append `(id, amount)` (an
        // absent id reads 0). Plain `+`, not the saturating `add_enchantment`.
        let stacks = card.enchantment(entry.enchantment);
        card.enchantments
            .set(entry.enchantment, stacks + entry.amount);
        self.settle_after_deck_pick();
        ActionResult::Advanced
    }

    fn do_smith_card_at(&mut self, idx: usize) -> ActionResult {
        {
            let Some(pc) = self.run_state.pending_choice.as_ref() else {
                return ActionResult::InvalidForPhase;
            };
            if pc.kind != crate::ChoiceKind::CardSelect {
                return ActionResult::InvalidForPhase;
            }
            if pc.options.get(idx).is_none() {
                return ActionResult::InvalidForPhase;
            }
        }
        let Ok(idx_u8) = u8::try_from(idx) else {
            return ActionResult::InvalidForPhase;
        };
        // resolve_pending_card_choice validates bounds + duplicates,
        // clears the slot on success, and returns the chosen options.
        if self
            .run_state
            .resolve_pending_card_choice(&[idx_u8])
            .is_none()
        {
            return ActionResult::InvalidForPhase;
        }
        // Upgrade the SPECIFIC deck instance the pick refers to: the Smith
        // pending_choice options are built 1:1 from `upgradable_deck_cards()`
        // (master-deck order, non-upgraded), so option `idx` is the idx-th
        // non-upgraded deck card. C# `FromDeckForUpgrade` presents the
        // upgradable cards in deck order and `CardCmd.Upgrade(list[idx])`
        // upgrades that instance in place. The previous body upgraded the
        // FIRST deck card matching the chosen id, which for a deck with
        // duplicate non-upgraded cards (e.g. four base STRIKE_REGENT) upgraded
        // the wrong copy — diverging the deck ORDER from C# (whole-run seed
        // RP0: picking the 4th Strike upgraded the 1st, so STRIKE+ landed at
        // deck index 0 instead of 3, cascading into a later combat-draw diff).
        // Mirrors `do_upgrade_deck_card_at` (Pomander). Bead `sts2-rs-smith-idx`.
        if let Some(deck_idx) = self
            .run_state
            .master_deck
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_upgradable())
            .nth(idx)
            .map(|(i, _)| i)
        {
            self.run_state.master_deck[deck_idx].upgrade();
        }
        self.phase = Phase::MapChoice;
        ActionResult::Advanced
    }

    /// Resolve one pick of the FRONT pending card grant (RoomFullOfCheese
    /// Gorge): append the chosen generated option to the master deck, remove it
    /// from the grant's option list, and decrement the remaining-pick count.
    /// When the count reaches 0 the grant is removed and its un-picked options
    /// are discarded (C# `FromSimpleGridForRewards` keeps only the selected
    /// cards). `idx` indexes the front grant's CURRENT options; out-of-range or
    /// no-grant is rejected. Bead `sts2-rs-gorge-select`.
    fn do_grant_deck_card_at(&mut self, idx: usize) -> ActionResult {
        let Some(grant) = self.run_state.pending_card_grants.first_mut() else {
            return ActionResult::InvalidForPhase;
        };
        if idx >= grant.options.len() {
            return ActionResult::InvalidForPhase;
        }
        let card = grant.options.remove(idx);
        grant.pick = grant.pick.saturating_sub(1);
        let done = grant.pick == 0 || grant.options.is_empty();
        // C# `CardPileCmd.Add(card, PileType.Deck)` — default index -1 = tail.
        self.run_state.master_deck.push(card);
        if done {
            self.run_state.pending_card_grants.remove(0);
        }
        ActionResult::Advanced
    }

    fn do_event_choice(&mut self, idx: usize) -> ActionResult {
        // Bead `sts2-rs-26qg3`: snapshot the current map coord BEFORE the
        // event resolver runs so we can re-write `current_room` with a
        // coord-bearing variant after. The event framework's
        // `RunState::enter_event_room` calls `enter_room(Room::new(...))`
        // (runstate.rs:1512) which writes a Room WITHOUT map_coord —
        // identical to the `enter_room_for_point` clobber fixed in
        // rkz98/rbb10. Without the restore, `next_map_children_cols`
        // reads coord=None on the next MapChoice tick and falls back to
        // row-1 children, teleporting the player back to the start row
        // every event visit. (Same restore should land on every other
        // phase whose resolve callback may call `enter_room`; doing
        // event here pre-emptively until the broader sweep lands.)
        let coord_keep = self.run_state.current_room.and_then(|r| r.map_coord);
        let point_keep = self.run_state.current_room.and_then(|r| r.map_point_type);
        if let Some(resolve) = self.callbacks.resolve_event_at {
            let _ = resolve(&mut self.run_state, idx);
        }
        // Event HP-loss death: an event option that loses HP (BrainLeech "Rip",
        // RoomFullOfCheese "Search", TheLegendsWereTrue "Slowly Find an Exit",
        // …) can drop the player to 0 HP. C# `CreatureCmd` ends the run when
        // every player is dead (`CreatureCmd.cs:306`), so a lethal event kill
        // routes straight to the death screen — it does NOT first surface the
        // option's card/relic reward. Mirror that here BEFORE draining any
        // queued rewards, so a fatal Rip ends the run instead of opening a
        // phantom card-reward (parity RP45: hp 4 − 5 = dead, C# game_over vs
        // Rust card_reward).
        if self.run_state.current_hp <= 0 {
            self.run_state.is_over = true;
            self.run_state.player_won = false;
            self.phase = Phase::RunOver;
            return ActionResult::Advanced;
        }
        if let (Some(coord), Some(point_type)) = (coord_keep, point_keep) {
            // Preserve coord even if resolve_event left the room as
            // RoomType::Event — what matters for `next_map_children_cols`
            // is `map_coord`. Use the existing room's room_type so we
            // don't override any resolve-time room re-classification.
            let room_type = self
                .run_state
                .current_room
                .map_or(crate::runstate::RoomType::Event, |r| r.room_type);
            self.run_state.current_room =
                Some(crate::runstate::Room::at(coord, point_type, room_type));
        }
        // Out-of-combat death: C# `CreatureCmd.Damage` on the event screen
        // kills at 0 HP and ends the run right there (RP21: DenseVegetation
        // TrudgeOn at <=8 HP → game_over before any reward). The run-side
        // HP-loss surfaces clamp at 0 without a phase transition, so check
        // here before draining rewards a dead player never sees.
        if self.run_state.current_hp <= 0 {
            self.run_state.is_over = true;
            self.run_state.player_won = false;
            self.phase = Phase::RunOver;
            return ActionResult::Advanced;
        }
        // Event→combat bridge (bead `sts2-rs-ck9z6`, GH #166): the option
        // handler staged a real fight on `event_combat` (C#
        // `EnterCombatWithoutExitingEvent`). Build the combat and enter
        // `Phase::Combat`; the request stays on `RunState::event_combat` so
        // `exit_combat_and_transition` knows to resume the event / grant the
        // victory reward when the fight ends. Must precede the multi-page
        // check below — the handler returns a NOT-finished outcome (the event
        // resumes after combat), which would otherwise route to `Phase::Event`.
        if self.run_state.event_combat.is_some() {
            if let Some(build) = self.callbacks.build_event_combat {
                // Clone the request out so the builder can take `&mut run_state`
                // (it rolls per-monster HP off the shared `niche` stream, like
                // C# `CreateCreature` → `SetUniqueMonsterHpValue`). The request
                // stays set on `run_state.event_combat` for the resume path.
                let req = self.run_state.event_combat.clone().expect("set");
                let combat = build(&mut self.run_state, &req);
                self.combat = Some(combat);
                self.phase = Phase::Combat;
                return ActionResult::Advanced;
            }
            // No builder wired (raw-core tests): fall through to the legacy
            // instant-resolution path — drop the request and grant any
            // staged victory rewards / resume immediately.
            let req = self.run_state.event_combat.take().expect("set");
            self.grant_event_combat_outcome(&req, true);
        }
        // Multi-page event chain (bead `sts2-rs-m3n0g.3`): an intermediate
        // page handler returns a NOT-finished outcome, so the dispatcher
        // leaves `current_event` set instead of calling `exit_event_room`.
        // Stay in `Phase::Event` so the next page's options are presented;
        // reward draining + the map transition belong to the FINAL
        // (finished) page, which clears `current_event`. Intermediate-page
        // HP loss (DollRoom Examine −15) is already applied and checked for
        // lethality above, so this is reached only with the player alive.
        //
        // Guarded on a wired `resolve_event_at`: the legacy no-callback
        // fallback never dispatches a handler (so it can't clear
        // `current_event`) and must keep its single-shot advance-to-map
        // behavior rather than wedge the run in `Phase::Event`.
        if self.callbacks.resolve_event_at.is_some() && self.run_state.current_event.is_some() {
            self.phase = Phase::Event;
            return ActionResult::Advanced;
        }
        self.route_after_event_rewards()
    }

    /// Drain an event's staged rewards and route to the next phase
    /// (`Phase::CardReward` if anything needs a pick / relic / bundle
    /// resolution, else `Phase::MapChoice`). Shared by [`Self::do_event_choice`]
    /// (single-page finish) and [`Self::finish_event_combat`] (post-combat
    /// resume), so both reach the same post-event reward routing.
    fn route_after_event_rewards(&mut self) -> ActionResult {
        // Bead sts2-rs-5pjsc: if the event resolver queued any
        // relic / card rewards (Neow → state.relic_rewards,
        // TheFutureOfPotions → potion grants on a future bead, …),
        // route into Phase::CardReward so the agent can drain them via
        // PickRelicReward / SkipRelic / PickCardReward before
        // returning to the map. `advance_after_card_reward` clears
        // back to MapChoice once both queues are empty.
        // Bead `sts2-rs-01o`: materialize reward sentinels the event
        // resolver pushed onto `pending_rewards` (Neow → Lost Coffer's
        // card+potion, BrainLeech / Overgrowth card grants, …). Potions
        // drain straight into the belt; a queued card opens an offering.
        let opened_card_reward = self.drain_pending_rewards();
        // Scroll Boxes (`ScrollBoxes::after_obtained`) queued a bundle pick on
        // `pending_bundles`; surface it as a CardReward-phase bundle offering.
        let opened_bundle = self.surface_pending_bundle();
        // Bead `sts2-rs-l0wbw` (GH #70): under `direct_event_relic_grant`
        // (workbench), auto-resolve any relic-reward sentinels queued by
        // an after-obtained listener (SmallCapsule's free-Common roll,
        // LargeCapsule x2, CallingBell C/U/R, ToyBox, …). Matches C#
        // `RewardsSet.Offer()` under `TestMode.IsOn`, which auto-invokes
        // `OnSelectWrapper()` on each reward and never surfaces a
        // "Take / Skip" decision. Without this, Rust would route to
        // `Phase::CardReward` and expose a phantom `PickRelicReward`
        // action the C# headless never has.
        if self.run_state.direct_event_relic_grant {
            let _ = self.run_state.drain_relic_rewards_directly();
        }
        if opened_card_reward
            || opened_bundle
            || !self.run_state.relic_rewards.is_empty()
            || !self.card_reward_options.is_empty()
            || !self.run_state.pending_card_selects.is_empty()
        {
            self.phase = Phase::CardReward;
            // A pending custom select with no relic/card reward ahead of
            // it would otherwise sit undrained — surface it now.
            self.advance_after_card_reward();
        } else {
            self.phase = Phase::MapChoice;
        }
        // A relic-grant batch (NeowsBones) may have deferred a curse to
        // `pending_post_pick_cards`. If the grant queued no interactive pick or
        // reward (all granted relics passive), the run is already settled and
        // the curse drops in now — same step C# adds it once `Offer()` returns.
        // Otherwise this is a no-op and the curse drains at the last pick /
        // reward settle point instead.
        self.drain_post_pick_relic_grants_if_settled();
        self.drain_post_pick_deck_cards_if_settled();
        ActionResult::Advanced
    }

    /// Grant an event→combat request's victory outcome. For a
    /// `resume`-flagged request (BattlewornDummy) this fires the
    /// `resume_event_after_combat` content callback (which rolls the
    /// per-setting reward, gated on `clean_victory`); for a no-resume
    /// request (TheLanternKey / FakeMerchant) it pushes the request's
    /// `victory_rewards` onto `pending_rewards` only on a clean victory.
    /// Bead `sts2-rs-ck9z6` (GH #166).
    fn grant_event_combat_outcome(
        &mut self,
        req: &crate::events::EventCombatRequest,
        clean_victory: bool,
    ) {
        if req.resume {
            if let Some(resume) = self.callbacks.resume_event_after_combat {
                resume(&mut self.run_state, &req.encounter, clean_victory);
            }
        } else if clean_victory {
            // No-resume combat-room rewards (C# `CombatRoom.AddExtraReward`):
            // a relic offer routes to the `relic_rewards` queue (drained via
            // the PickRelicReward/SkipRelic surface), everything else
            // (TheLanternKey's card) onto `pending_rewards`.
            for reward in &req.victory_rewards {
                match reward {
                    crate::runstate::Reward::Relic(rr) => {
                        self.run_state.offer_relic_rewards(rr.relic_id, 1);
                    }
                    other => self.run_state.pending_rewards.push(other.clone()),
                }
            }
        }
    }

    /// Resolve a finished event→combat: grant the victory outcome, exit the
    /// parent event room, and route the staged rewards. Called from
    /// [`Self::exit_combat_and_transition`] on a WON fight whose
    /// [`RunState::event_combat`] is set (a player DEATH never reaches here —
    /// the lost-combat branch routes to `Phase::RunOver` first). A
    /// `BattlewornDummyTimeLimit` expiry makes `player_won` true but flags
    /// [`CombatState::event_combat_timed_out`], which suppresses the reward.
    /// Bead `sts2-rs-ck9z6` (GH #166).
    fn finish_event_combat(&mut self, req: crate::events::EventCombatRequest) -> ActionResult {
        // A timed-out dummy "wins" (it fled) but yields nothing. The flag is
        // read off the combat slot, set back by `exit_combat` having already
        // run above; read it directly from the live combat here.
        let timed_out = self
            .combat
            .as_ref()
            .is_some_and(|c| c.event_combat_timed_out);
        let clean_victory = !timed_out;
        self.grant_event_combat_outcome(&req, clean_victory);
        // Tear down the parent event (C# `SetEventFinished` inside Resume /
        // the no-resume combat room exit). `exit_event_room` clears
        // `current_event` + `event_state`; `event_combat` is already taken.
        self.run_state.exit_event_room();
        self.route_after_event_rewards()
    }

    /// `true` iff the run has finished — `Phase::RunOver`.
    #[must_use]
    pub const fn is_over(&self) -> bool {
        matches!(self.phase, Phase::RunOver)
    }

    /// Enter a treasure room. Transitions to [`Phase::Treasure`],
    /// rolls a rarity bucket via
    /// [`crate::runstate::RunState::roll_treasure_relic_rarity`]
    /// (uses the `treasure_room` RNG stream), and walks the
    /// Python fallback chain to pick the first non-owned relic from
    /// the caller-supplied per-rarity pools:
    ///
    /// - `Common` rolled → try `common`, then `uncommon`, then `rare`.
    /// - `Uncommon` rolled → try `uncommon`, then `rare`.
    /// - `Rare` rolled → try `rare` only.
    ///
    /// Mirrors `_relic_grab_bag_order` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py:284-292` and
    /// the wider `pull_next_relic_reward_id` walk (`run_state.py:304-339`).
    ///
    /// On the first match: drains the picked entry from the relevant
    /// pool, calls
    /// [`crate::runstate::RunState::offer_relic_rewards`] with
    /// `count = 1`, and returns `Some(id)`. When every pool in the
    /// fallback chain is exhausted of non-owned entries: leaves
    /// `relic_rewards` untouched and returns `None` (the Python
    /// `"CIRCLET"` sentinel is intentionally NOT modeled — the Rust
    /// thin slice exposes the None case so the caller can fall back
    /// onto the sentinel path themselves when it lands).
    ///
    /// Pools are passed by `&mut Vec<RelicId>` so the driver can
    /// drain the chosen entry in-place; this matches the established
    /// content-agnostic idiom of
    /// [`crate::runstate::RunState::populate_relic_grab_bag`] (the
    /// per-rarity buckets live in `sts2-content` since rarity is a
    /// `RelicSpec` field, but the run-side handler stays
    /// content-free by accepting pre-resolved
    /// [`crate::RelicId`] lists).
    ///
    /// Bead `sts2-rs-ycria` (P9.2.a).
    pub fn enter_treasure(
        &mut self,
        common: &mut Vec<crate::RelicId>,
        uncommon: &mut Vec<crate::RelicId>,
        rare: &mut Vec<crate::RelicId>,
    ) -> Option<crate::RelicId> {
        self.phase = Phase::Treasure;
        let rolled = self.run_state.roll_treasure_relic_rarity();
        let owned: std::collections::BTreeSet<crate::RelicId> =
            self.run_state.relics.iter().copied().collect();
        let pick_from = |pool: &mut Vec<crate::RelicId>| -> Option<crate::RelicId> {
            let idx = pool.iter().position(|id| !owned.contains(id))?;
            Some(pool.remove(idx))
        };
        let picked = match rolled {
            crate::card_rewards::RewardRarity::Common => pick_from(common)
                .or_else(|| pick_from(uncommon))
                .or_else(|| pick_from(rare)),
            crate::card_rewards::RewardRarity::Uncommon => {
                pick_from(uncommon).or_else(|| pick_from(rare))
            }
            crate::card_rewards::RewardRarity::Rare => pick_from(rare),
        };
        if let Some(id) = picked {
            self.run_state.offer_relic_rewards(id, 1);
        }
        picked
    }

    /// Handle the act-boundary crossing on a boss-kill. Mirrors the
    /// exit path of Python `run_manager._enter_boss_relic` — once the
    /// player wins the boss combat and walks out of the boss room,
    /// the run advances to the next act and a fresh `ActMap` is
    /// generated for it.
    ///
    /// **Guards**: returns `None` (no state mutation) when any of:
    /// - `combat.is_over` is `false`,
    /// - `combat.player_won` is `false`,
    /// - `self.run_state.current_room` is not a [`crate::runstate::RoomType::Boss`],
    /// - `self.run_state.act_index >= 3` (final-boss handling lives
    ///   in bead `sts2-rs-8i1kn`; act 3 is the last act at this pin).
    ///
    /// **On success**:
    /// 1. [`crate::runstate::RunState::advance_act`] bumps
    ///    `act_index` (1→2 or 2→3).
    /// 2. A new [`crate::map::ActMap`] is generated by
    ///    [`crate::map_generator::generate_act_map`] using the new
    ///    act's `num_rooms` (15/14/13 for acts 1/2/3), the per-act `map_rng`
    ///    stream (1-based salt `act_<i>_map`), the supplied
    ///    `ascension_level`, `replace_treasure_with_elite = false`
    ///    (matches the Python default), the new `act_index`, and
    ///    `num_elites_override = None`.
    /// 3. [`crate::runstate::RunState::modify_generated_map`] fires
    ///    the `modify_generated_map` channel so listeners like
    ///    `GoldenCompass` can rewrite the layout (Python's
    ///    `for relic in self.relics: act_map = relic.modify_generated_map(...)`).
    /// 4. [`crate::run_effects::dispatch_after_act_entered`] fires
    ///    with the new `act_index` so per-act listeners (LavaRock,
    ///    FurCoat) react.
    /// 5. Returns `Some(new_act_map)` — the caller stores the map
    ///    wherever it tracks per-act layouts (RunManager does not
    ///    own an `ActMap` field at this pin; see
    ///    [`crate::runstate::RunState::current_room`] for the
    ///    current-position-only state).
    ///
    /// Bead `sts2-rs-7uzk3` (P9.2.g.1). Unblocks `sts2-rs-oo78v`
    /// (cross-act state carry) and `sts2-rs-8i1kn` (final-boss
    /// kill → `is_over`).
    pub fn advance_act_on_boss_kill(
        &mut self,
        combat: &CombatState,
        ascension_level: i32,
    ) -> Option<crate::map::ActMap> {
        if !combat.is_over || !combat.player_won {
            return None;
        }
        let room_type = self.run_state.current_room.map(|r| r.room_type);
        if room_type != Some(crate::runstate::RoomType::Boss) {
            return None;
        }
        if self.run_state.act_index >= 3 {
            return None;
        }
        self.run_state.advance_act();
        let new_act = self.run_state.act_index;

        // Carry run-persistent state into the new act, clear the
        // combat-only slot. Bead `sts2-rs-oo78v` (P9.2.g.2).
        self.carry_to_next_act();

        // Ancient-event entry heal (C# AncientEventModel.BeforeEventStarted):
        // entering the new act's Ancient event heals the player — to full at
        // base ascension, or 80% of the missing HP at A2 WearyTraveler. This
        // is the between-acts recovery from boss damage.
        let _ = self.run_state.apply_ancient_event_heal();

        // Each act opens with its Ancient event room (Neow for act 1, the
        // "boss relic" Ancient for acts 2/3 — task #19). C# MarkRoomVisited's
        // that room on entry (`RunManager.cs:903`, RoomType.Event), advancing
        // the NEW act's `eventsVisited` cursor to 1 before any pool event is
        // pulled — same mechanism verified live for Neow/act 1 (`dump_rooms`
        // right after StartRun shows act-1 `events_visited == 1`).
        if let Some(act) = self.run_state.acts.get_mut(usize::from(new_act) - 1) {
            act.events_visited += 1;
        }

        // Map regen — use the dedicated per-act map RNG stream from
        // ewm6b's RunRngSet so re-running the same seed reproduces
        // the same layout independent of any rewards/treasure rolls.
        // The RunRngSet API takes a 0-based `act_index` argument but
        // names the stream salt `act_<1-based>_map`; pass
        // `new_act - 1` to mirror Python exactly.
        // Per-act map length: each act has fewer rooms (15/14/13, boss rows
        // 16/15/14) — source the new act's `num_rooms` rather than a hardcoded
        // 15 so acts 2/3 aren't one row too tall. See `advance_boss_act`.
        let num_rooms = self
            .run_state
            .acts
            .get(usize::from(new_act).saturating_sub(1))
            .map_or(crate::acts::DEFAULT_ACT_NUM_ROOMS, |a| {
                i32::from(a.num_rooms)
            });
        let act_idx_rng_arg = i32::from(new_act).saturating_sub(1);
        let act_idx_gen = i32::from(new_act);
        let mut act_map = {
            let rng = self.run_state.rng.map_rng(act_idx_rng_arg);
            crate::map_generator::generate_act_map(
                num_rooms,
                rng,
                ascension_level,
                false,
                act_idx_gen,
                None,
            )
        };
        self.run_state.modify_generated_map(&mut act_map, new_act);
        // A10 DoubleBoss: the final act's map carries the second boss node
        // (C# `StandardActMap.cs:96` via `Act.HasSecondBoss`). No RNG.
        // Bead sts2-rs-40ikw.2.
        if self.run_state.has_double_boss && self.run_state.is_last_act() {
            act_map.attach_second_boss();
        }
        let _ = crate::run_effects::dispatch_after_act_entered(&mut self.run_state, new_act);
        Some(act_map)
    }

    /// Handle the final-boss kill — Act-3 boss-clear is the run-end
    /// terminal state. Mirrors Python's `_on_boss_combat_won` path
    /// that sets `self._run_state.is_over = True` /
    /// `self._run_state.player_won = True` when
    /// `post_combat_phase == PHASE_RUN_OVER`
    /// (`sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:1318-1322`).
    ///
    /// **Guards** — returns `false` (no mutation) when any of:
    /// - `combat.is_over` is `false`,
    /// - `combat.player_won` is `false`,
    /// - `self.run_state.current_room` is not [`crate::runstate::RoomType::Boss`],
    /// - `self.run_state.act_index != 3` (Act-1 / Act-2 boss-clear is
    ///   handled by [`Self::advance_act_on_boss_kill`]; this method is
    ///   the Act-3-only terminal sibling).
    ///
    /// **On success**:
    /// - `run_state.is_over = true`
    /// - `run_state.player_won = true`
    /// - `self.phase` → [`Phase::RunOver`]
    /// - returns `true`.
    ///
    /// Bead `sts2-rs-8i1kn` (P9.2.g.3).
    pub fn finish_run_on_final_boss_kill(&mut self, combat: &CombatState) -> bool {
        if !combat.is_over || !combat.player_won {
            return false;
        }
        let room_type = self.run_state.current_room.map(|r| r.room_type);
        if room_type != Some(crate::runstate::RoomType::Boss) {
            return false;
        }
        if self.run_state.act_index != 3 {
            return false;
        }
        self.run_state.is_over = true;
        self.run_state.player_won = true;
        self.phase = Phase::RunOver;
        true
    }

    /// Debug/coverage override mirroring the scalar subset of the C#
    /// headless `set_player` (`RunSimulator.SetPlayerImpl`): overwrite the
    /// run-state's current/max HP and gold, and — when a combat is live —
    /// propagate the HP override into the mirror creature + legacy
    /// `PlayerCombatState` fields so the d03 unified-Creature invariant
    /// holds either way (same dual-write as
    /// [`crate::full_run::apply_invincible_buffs`]).
    ///
    /// C# writes `player.Creature._currentHp/_maxHp` directly; because STS2
    /// runs combat on that same persistent Creature, a mid-run `set_player`
    /// reaches the active fight. This mirrors that for Rust.
    ///
    /// Each argument is applied only when `Some`. Writes consume no RNG and
    /// fire no hooks, so they do not perturb the run stream — but they DO
    /// perturb combat state, so this must never run on a trace-recorded run
    /// (same contract as the invincible coverage mode).
    pub fn debug_set_player(&mut self, hp: Option<i32>, max_hp: Option<i32>, gold: Option<i32>) {
        if let Some(v) = max_hp {
            self.run_state.max_hp = v;
        }
        if let Some(v) = hp {
            self.run_state.current_hp = v;
        }
        if let Some(v) = gold {
            self.run_state.gold = v;
        }
        if let Some(combat) = self.combat.as_mut() {
            if let Some(uid) = combat.player.creature_uid {
                if let Some(c) = combat.creatures.iter_mut().find(|c| c.id == uid.0) {
                    if let Some(v) = max_hp {
                        c.max_hp = v;
                    }
                    if let Some(v) = hp {
                        c.current_hp = v;
                    }
                }
            }
            if let Some(v) = max_hp {
                combat.player.max_hp = v;
            }
            if let Some(v) = hp {
                combat.player.current_hp = v;
            }
        }
    }

    /// Cross-act state-carry: clear per-act and combat-only fields
    /// while leaving every run-persistent surface untouched. Called
    /// by [`Self::advance_act_on_boss_kill`] before the map regen +
    /// `after_act_entered` dispatch, but also exposed standalone so
    /// future per-character / Neow-driven act transitions can reuse
    /// the same boundary contract.
    ///
    /// **Preserved (run-persistent — explicitly untouched here):**
    /// - `run_state.master_deck`
    /// - `run_state.relics` + `run_state.relic_counters` +
    ///   `run_state.last_obtained_relic`
    /// - `run_state.current_hp` / `max_hp` / `gold`
    /// - `run_state.pael_state` and other cluster-wide trackers
    /// - `run_state.rng` (per-act streams are derivable from the
    ///   master seed; the bundle itself stays put so non-map
    ///   streams keep advancing across acts)
    /// - `run_state.acts` (the per-act registry snapshot)
    /// - `run_state.card_rarity_odds` (Python carries this across
    ///   acts; the rare-pity offset is run-scoped, not act-scoped)
    ///
    /// **Cleared / reset (per-act or combat-only):**
    /// - `self.combat` → `None` (combat-only fields — energy, hand,
    ///   draw, discard, exhaust, status/curse cards — live on
    ///   [`CombatState`] which is reconstructed per fight via the
    ///   `enter_combat` bridge in `sts2-rs-tjn0v`)
    /// - `run_state.current_room` → `None` (the boss room is done;
    ///   the next act has no current room until the map driver
    ///   places the player on the new start node)
    /// - `run_state.unknown_map_point_odds.reset_to_base()` —
    ///   Python's `unknown_odds.reset_to_base()` from `enter_act`
    ///
    /// **Not yet modeled** (Python pin lists these but they're not
    /// ported at this Rust pin — file follow-up sub-beads when the
    /// host fields land):
    /// - `visited_map_coords.clear()` (no field on RunState yet)
    /// - `act_floor = 0` (no field on RunState yet)
    /// - `map_point_history` carry semantics
    ///
    /// Mirrors the cross-act behavior in
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py::enter_act`
    /// (lines 1577-1583), composed with the run-persistent default
    /// of every other field.
    ///
    /// Bead `sts2-rs-oo78v` (P9.2.g.2).
    pub fn carry_to_next_act(&mut self) {
        self.combat = None;
        self.run_state.current_room = None;
        self.run_state.unknown_map_point_odds.reset_to_base();
    }

    /// Top-level run-start sequencer. Mirrors Python
    /// `RunState.initialize_run` (`run_state.py:1466-1479`):
    ///
    /// 1. **Ascension effects** — already applied by
    ///    [`Self::with_ascension`] at construction, NOT re-applied
    ///    here. Python combines construction and `initialize_run`
    ///    over two calls; the Rust port consolidates A3/A4/A5/A7/A10
    ///    into the constructor because every field is set at the same
    ///    boundary and there is no Rust counterpart to Python's
    ///    "construct → mutate fields → initialize" two-step.
    /// 2. **`on_run_created` modifier chain** — identity at the pin
    ///    (`RunState.modifiers: list[Any]` is empty on a fresh run;
    ///    the modifier system isn't ported in Rust yet). Will fold in
    ///    once the modifier registry lands.
    /// 3. **Populate relic grab bag** —
    ///    [`crate::runstate::RunState::populate_relic_grab_bag`] with
    ///    the caller-supplied pre-resolved pools. The workspace dep
    ///    direction (`content → core`) prevents `sts2-core` from
    ///    inverting onto `sts2-content`'s `RelicSpec` rarity registry,
    ///    so the caller (which has access to `sts2-content`) merges
    ///    rarity buckets and hands them in here.
    /// 4. **`after_relic_grab_bags_populated`** — identity (no modifier
    ///    registry; see step 2).
    /// 5. **`_generate_rooms`** — for each act in
    ///    [`crate::runstate::RunState::acts`], populate
    ///    `event_ids_shuffled` from the static `event_ids` slice and
    ///    shuffle in place via the `rng.up_front` stream, then reset
    ///    `events_visited = 0`. Guarded by
    ///    [`crate::runstate::RunState::rooms_generated`] so repeat
    ///    `initialize_run` calls are no-ops at this step. Mirrors
    ///    Python `_generate_rooms` (`run_state.py:1481-1487`). Bead
    ///    `sts2-rs-wk1t6` (P9.2.n follow-up).
    /// 6. **Generate first `ActMap`** — uses the same path as
    ///    [`Self::advance_act_on_boss_kill`]'s map regen
    ///    (`map_rng(act_idx - 1)` + the act's `num_rooms` + the
    ///    `replace_treasure_with_elite = false` Python default), then
    ///    fires the `modify_generated_map` channel so listeners like
    ///    `GoldenCompass` can rewrite layout.
    /// 7. **`after_act_entered`** —
    ///    [`crate::run_effects::dispatch_after_act_entered`] with the
    ///    current `act_index` so LavaRock / FurCoat listeners get their
    ///    entry signal.
    ///
    /// Returns the generated `ActMap` so the caller can store it
    /// wherever it tracks per-act layouts. Bead `sts2-rs-otj0g`
    /// (P9.2.n).
    pub fn initialize_run(
        &mut self,
        grab_bag: Vec<crate::RelicId>,
        grab_bag_fallback: Vec<crate::RelicId>,
        boss_pool: Vec<crate::RelicId>,
    ) -> crate::map::ActMap {
        // Step 3: populate relic grab bag.
        self.run_state
            .populate_relic_grab_bag(grab_bag, grab_bag_fallback, boss_pool);

        // Step 5: _generate_rooms — shuffle each act's event pool via
        // the `up_front` named RNG stream. Idempotent: gated by
        // `rooms_generated` so subsequent `initialize_run` calls
        // (re-init paths) don't re-shuffle.
        if !self.run_state.rooms_generated {
            for act in &mut self.run_state.acts {
                let mut shuffled: Vec<&'static str> = act.event_ids.to_vec();
                self.run_state.rng.up_front.shuffle(&mut shuffled);
                act.event_ids_shuffled = shuffled;
                act.events_visited = 0;
            }
            self.run_state.rooms_generated = true;
        }

        // Step 6: generate first ActMap. Use the per-act map RNG
        // stream (1-based salt `act_<i>_map`) — pass `act_index - 1`
        // since `RunRngSet::map_rng` takes a 0-based index argument.
        // Per-act map length from the act's `num_rooms` (act 1 = 15 at init,
        // but source it so this stays correct if ever called for a later act);
        // mirrors `RunState::generate_map`.
        let act_idx_gen = i32::from(self.run_state.act_index);
        let act_idx_rng_arg = act_idx_gen.saturating_sub(1);
        let num_rooms = self
            .run_state
            .acts
            .get(act_idx_rng_arg as usize)
            .map_or(crate::acts::DEFAULT_ACT_NUM_ROOMS, |a| {
                i32::from(a.num_rooms)
            });
        let ascension_level = i32::from(self.ascension_level);
        let mut act_map = {
            let rng = self.run_state.rng.map_rng(act_idx_rng_arg);
            crate::map_generator::generate_act_map(
                num_rooms,
                rng,
                ascension_level,
                false,
                act_idx_gen,
                None,
            )
        };
        let act_index = self.run_state.act_index;
        self.run_state.modify_generated_map(&mut act_map, act_index);

        // Step 7: fire after_act_entered for the run's starting act.
        let _ = crate::run_effects::dispatch_after_act_entered(&mut self.run_state, act_index);

        act_map
    }

    /// Resolve a [`crate::map::MapPointType`] to a concrete
    /// [`crate::runstate::RoomType`] and enter that room. Mirrors the
    /// dispatch Python performs at `_enter_room` (`run_manager.py`):
    /// when the player steps onto an `?` node, roll
    /// [`crate::runstate::RunState::resolve_unknown_room`] (which
    /// consumes the `unknown_map_point` named RNG stream, walks the
    /// `modify_unknown_map_point_room_types` channel for JuzuBracelet
    /// et al., and rolls `UnknownMapPointOdds`); for every other point
    /// type, map directly via [`crate::map::MapPointType::to_room_type`].
    ///
    /// The resolved [`crate::runstate::RoomType`] is wrapped in a
    /// [`crate::runstate::Room`] and routed through
    /// [`crate::runstate::RunState::enter_room`], so the
    /// `after_room_entered` channel (MawBank gold, DreamCatcher card,
    /// etc.) still fires for the resolved type. Returns the resolved
    /// room type so the caller can dispatch to the matching `_enter_*`
    /// phase handler.
    ///
    /// `point_type == MapPointType::Unassigned` is treated as a bug:
    /// `to_room_type` would return `RoomType::Unknown`, which the
    /// resolver does not handle; callers must only pass map-generated
    /// nodes (`is_assigned() == true`).
    ///
    /// Bead `sts2-rs-2h627` (P9.2.h).
    /// Port of C# `RunManager.BuildRoomTypeBlacklist` (RunManager.cs:410):
    /// a `?` room may not roll Shop when the room just left was a Shop, or
    /// when every child node ahead is a Shop. Rust previously passed an empty
    /// blacklist, over-rolling Shop where C# excluded it (RP111/RP139:
    /// cs=event rs=shop). `current_room` is still the PREVIOUS room at this
    /// point — the call runs before `enter_room` records the new one.
    fn build_unknown_room_blacklist(
        &self,
        coord: crate::map::MapCoord,
    ) -> std::collections::BTreeSet<crate::runstate::RoomType> {
        use crate::runstate::RoomType;
        let mut blacklist = std::collections::BTreeSet::new();
        let prev_was_shop = self
            .run_state
            .current_room
            .as_ref()
            .is_some_and(|r| r.room_type == RoomType::Shop);
        let children_all_shop = self.run_state.current_act_map.as_ref().is_some_and(|m| {
            let children = m.children_of(coord);
            !children.is_empty()
                && children
                    .iter()
                    .all(|p| p.point_type == crate::map::MapPointType::Shop)
        });
        if prev_was_shop || children_all_shop {
            blacklist.insert(RoomType::Shop);
        }
        blacklist
    }

    pub fn enter_room_for_point(
        &mut self,
        point_type: crate::map::MapPointType,
    ) -> crate::runstate::RoomType {
        let resolved = if point_type == crate::map::MapPointType::Unknown {
            self.run_state
                .resolve_unknown_room(std::collections::BTreeSet::new())
        } else {
            point_type.to_room_type()
        };
        self.run_state
            .enter_room(crate::runstate::Room::new(resolved));
        resolved
    }

    /// Enter a rest-site room: transition to [`Phase::RestSite`] and
    /// return the offered option list.
    ///
    /// Delegates option generation to
    /// [`crate::runstate::RunState::rest_site_options`], which builds
    /// the base [`Rest`, `Smith`] pair (with `Smith.enabled` keyed off
    /// the deck's upgradable cards) and folds the
    /// `modify_rest_site_options` channel — so relic / modifier
    /// listeners (e.g. `Shovel` enabling `Dig`, `Girya` injecting
    /// `Lift`) get a chance to add or disable choices before the
    /// agent picks.
    ///
    /// Mirrors Python `_enter_rest_site` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:739-741`.
    ///
    /// [`Rest`]: crate::runstate::RestSiteOption::Rest
    /// [`Smith`]: crate::runstate::RestSiteOption::Smith
    ///
    /// Bead `sts2-rs-76mdy` (P9.2.d).
    pub fn enter_rest_site(&mut self) -> Vec<crate::runstate::RestSiteOption> {
        self.phase = Phase::RestSite;
        self.run_state.rest_site_options()
    }

    /// Apply a rest-site choice. Delegates the mutation to
    /// [`crate::runstate::RunState::do_rest_site_action`] and, on a
    /// terminal outcome, transitions the run back to
    /// [`Phase::MapChoice`].
    ///
    /// **Terminal outcomes** (phase → `MapChoice`):
    /// - [`Rested`](crate::runstate::RestSiteActionOutcome::Rested)
    /// - [`SmithUpgraded`](crate::runstate::RestSiteActionOutcome::SmithUpgraded)
    /// - [`Hatched`](crate::runstate::RestSiteActionOutcome::Hatched)
    /// - [`Disabled`](crate::runstate::RestSiteActionOutcome::Disabled)
    ///   (no-op — the agent picked a disabled option; the room is
    ///   still consumed, matching Python's "fallback to first enabled"
    ///   safety net which also exits the phase on completion)
    /// - [`Unsupported`](crate::runstate::RestSiteActionOutcome::Unsupported)
    ///   (deferred variant — handler not yet ported; the run still
    ///   advances so simulation can continue past partial content)
    ///
    /// **Non-terminal outcome** (phase stays at `RestSite`):
    /// - [`SmithAwaitingChoice`](crate::runstate::RestSiteActionOutcome::SmithAwaitingChoice)
    ///   — a `pending_choice` is open over the upgradable cards. The
    ///   caller resolves it via
    ///   [`RunState::resolve_pending_card_choice`] and applies the
    ///   upgrade out-of-band; the rest-site phase exits on the next
    ///   `do_rest_site_action` call (or the caller transitions
    ///   directly once the upgrade lands).
    ///
    /// Mirrors `_do_rest_site` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:1589-1638`.
    /// The Python `should_disable_remaining_rest_site_options` /
    /// re-prompt path (multi-choice rest sites) is **not** modeled
    /// here — the engine's outcome enum collapses to a single-pick
    /// transition. File a follow-up sub-bead when a relic that
    /// requires the multi-pick surface lands.
    ///
    /// [`RunState::resolve_pending_card_choice`]: crate::runstate::RunState::resolve_pending_card_choice
    ///
    /// Bead `sts2-rs-76mdy` (P9.2.d).
    pub fn do_rest_site_action(
        &mut self,
        chosen: crate::runstate::RestSiteOption,
    ) -> crate::runstate::RestSiteActionOutcome {
        let outcome = self.run_state.do_rest_site_action(chosen);
        use crate::runstate::RestSiteActionOutcome as O;
        match outcome {
            O::Rested { .. }
            | O::SmithUpgraded { .. }
            | O::Hatched
            | O::Kindled
            | O::Lifted
            | O::Disabled
            | O::Unsupported => {
                self.phase = Phase::MapChoice;
            }
            O::SmithAwaitingChoice => {}
        }
        outcome
    }
}

// Public accessors added by bead `sts2-rs-ze744` (R-Act1-Replay W5a):
// expose the currently-offered card-reward CardId list so the Python
// replay driver can match scraped `card_choices[*].card_id` against
// the engine's offer and dispatch the matching `PickCardReward{Some(idx)}`.
// Lives in a separate `impl` block to avoid touching the dispatch core
// (which is concurrently being extended by sibling W5b smith work).
impl RunManager {
    /// The open rewards screen ([`RunState::interactive_rewards`]), or `None`.
    /// sts2-agent fork.
    #[must_use]
    pub fn reward_screen(&self) -> Option<&[ScreenReward]> {
        self.reward_screen.as_deref()
    }

    /// The screen entry whose card reward is open now, if any. sts2-agent fork.
    #[must_use]
    pub fn screen_card_claim(&self) -> Option<usize> {
        self.screen_card_claim
    }

    /// Currently-offered card-reward [`crate::cards::CardId`] list for
    /// the `Phase::CardReward` sub-state. Empty when no card-reward
    /// fan-out is pending (e.g. relic-only reward, or outside the
    /// CardReward phase entirely).
    ///
    /// Bead `sts2-rs-ze744` (W5a). Mirrors the index space used by
    /// [`RunAction::PickCardReward`] — `card_reward_options()[i]` is
    /// the card offered at `idx = i`.
    #[must_use]
    pub fn card_reward_options(&self) -> &[crate::cards::CardId] {
        &self.card_reward_options
    }

    /// Per-option upgrade flags parallel to [`Self::card_reward_options`]
    /// (SilverCrucible pre-upgrades, act-2+ upgrade rolls). May be shorter
    /// than the options list (choose-screen / bundle offerings carry no
    /// upgrade state) — missing entries mean "not upgraded". On-screen
    /// player-visible info, exposed for the run-AI `VisibleRunView`
    /// feature chokepoint (full-run-AI plan §2.5, bead `sts2-rs-40ikw.5`).
    #[must_use]
    pub fn card_reward_upgraded_flags(&self) -> &[bool] {
        &self.card_reward_upgraded
    }

    /// When the current `CardReward` offering is a Scroll Boxes bundle pick,
    /// returns the packs (`[[CardId; 3]; 2]`-shaped — one inner `Vec` per
    /// selectable pack, aligned with `PickCardReward{Some(i)}`). `None` for an
    /// ordinary single-card reward. Lets a host render the "choose 1 of N
    /// packs" screen instead of a single-card fan-out.
    #[must_use]
    pub fn card_reward_bundles(&self) -> Option<&[Vec<crate::cards::CardId>]> {
        self.card_reward_bundles.as_deref()
    }

    /// `true` while the current `CardReward` offering is a *choose screen*
    /// (C# `CardSelectCmd.FromChooseACardScreen` — HeftyTablet /
    /// LeadPaperweight) rather than a post-combat card reward. Hosts render
    /// it as a skippable 1-of-N `card_select` (min 0 / max 1), matching the
    /// C# headless decision vocabulary. Bead 6ynt.4.deck-add / GH #34.
    #[must_use]
    pub const fn card_offer_choose_screen(&self) -> bool {
        self.card_offer_choose_screen
    }

    /// Whether the belt is locked (C# `Player.CanRemovePotions == false`): from
    /// The Future of Potions' or Stone of All Time's start until the event is
    /// done, its reward or deck pick included; back on the map it is free.
    /// No drink or discard is offered while it is. sts2-agent fork.
    #[must_use]
    pub const fn potions_locked(&self) -> bool {
        self.run_state.potions_locked && !matches!(self.phase, Phase::MapChoice | Phase::RunOver)
    }

    /// Whether the open choose screen may be skipped: `false` for a pick the
    /// game forces (Brain Leech's Share Knowledge grid, `Cancelable = false`),
    /// `true` otherwise. sts2-agent fork.
    #[must_use]
    pub const fn card_offer_can_skip(&self) -> bool {
        self.card_offer_can_skip
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RelicId;

    // -----------------------------------------------------------------
    // parity map_gold_necro: `do_card_reward_pick` must fire the run-side
    // `on_card_added_to_deck` channel for EVERY card it adds to the master
    // deck — single-pick, Scroll-Boxes bundle, and choose-screen post-add.
    //
    // C# `CardReward.cs:173 CardPileCmd.Add(.., Deck)` → `CardPileCmd.cs:583
    // Hook.AfterCardChangedPiles` runs per successful deck add; `LuckyFysh.cs`
    // overrides that hook to grant `GoldVar(15)` per card. The Rust card-reward
    // path raw-pushed cards without dispatching the channel, so a LuckyFysh
    // holder missed its 15 gold on every card-reward pick. These tests register
    // a counting/gold-granting stub on the channel and assert the firing sites
    // dispatch once per added card. (The full LuckyFysh integration is covered
    // by `uncommon.rs::lucky_fysh_*` in sts2-content, where the real listener
    // is installed.)
    // -----------------------------------------------------------------

    /// Stub `on_card_added_to_deck` listener: grants a fixed +15 gold per
    /// dispatch, mirroring LuckyFysh's `GoldVar(15)`.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn map_gold_necro_stub(
        rs: &mut crate::runstate::RunState,
        _ctx: &crate::run_effects::CardAddedRunCtx,
    ) -> crate::EffectResult {
        rs.gain_gold(15);
        crate::EffectResult::Resolved
    }

    fn register_map_gold_necro_stub() {
        crate::run_effects::register_on_card_added_to_deck(
            RelicId::LuckyFysh,
            map_gold_necro_stub as crate::run_effects::OnCardAddedToDeckRunFn,
        );
    }

    #[test]
    #[serial_test::serial(global_registry)]
    fn map_gold_necro_single_pick_fires_card_added_channel() {
        register_map_gold_necro_stub();
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::CardReward);
        rm.card_reward_options = vec![crate::cards::CardId::Bash];
        let gold_before = rm.run_state.gold;
        let result = rm.do_card_reward_pick(Some(0));
        assert_eq!(result, ActionResult::Advanced);
        // Exactly one card added → exactly one +15 gold grant.
        assert_eq!(rm.run_state.gold, gold_before + 15);
    }

    #[test]
    #[serial_test::serial(global_registry)]
    fn map_gold_necro_bundle_fires_per_card() {
        register_map_gold_necro_stub();
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::CardReward);
        // A Scroll-Boxes bundle pack with three cards: all three land in the
        // deck (no skip), so all three fire the channel → +45 gold.
        rm.card_reward_bundles = Some(vec![vec![
            crate::cards::CardId::Bash,
            crate::cards::CardId::StrikeIronclad,
            crate::cards::CardId::DefendIronclad,
        ]]);
        rm.card_reward_options = vec![crate::cards::CardId::Bash];
        let gold_before = rm.run_state.gold;
        let result = rm.do_card_reward_pick(Some(0));
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(rm.run_state.gold, gold_before + 3 * 15);
    }

    #[test]
    #[serial_test::serial(global_registry)]
    fn map_gold_necro_choose_screen_post_add_fires_per_card() {
        register_map_gold_necro_stub();
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::CardReward);
        // Choose-screen pick (HeftyTablet-style): the chosen option PLUS the
        // deferred post-add cards each fire the channel. One option + two
        // post-add cards = three deck adds → +45 gold.
        rm.card_offer_choose_screen = true;
        rm.card_reward_options = vec![crate::cards::CardId::Thrash];
        let mk = |id| {
            let mut c = crate::cards::Card::pod_defaults();
            c.card_id = id;
            c
        };
        rm.pending_choose_screen_post_add = vec![
            mk(crate::cards::CardId::Injury),
            mk(crate::cards::CardId::Injury),
        ];
        let gold_before = rm.run_state.gold;
        let result = rm.do_card_reward_pick(Some(0));
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(rm.run_state.gold, gold_before + 3 * 15);
    }

    #[test]
    fn new_ironclad_pulls_starter_pack_from_registry() {
        let rm = RunManager::new(0, CharacterId::Ironclad);
        assert_eq!(rm.run_state.max_hp, 80);
        assert_eq!(rm.run_state.current_hp, 80);
        assert_eq!(rm.run_state.gold, 99);
        assert_eq!(rm.run_state.relics, vec![RelicId::BurningBlood]);
        assert_eq!(rm.heal_after_combat, 6);
    }

    #[test]
    fn debug_set_player_overrides_run_state_scalars_selectively() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        // Only the `Some` fields are applied; `None` leaves the value alone.
        rm.debug_set_player(Some(100_000), Some(100_000), None);
        assert_eq!(rm.run_state.current_hp, 100_000);
        assert_eq!(rm.run_state.max_hp, 100_000);
        assert_eq!(rm.run_state.gold, 99, "gold untouched when None");
        rm.debug_set_player(None, None, Some(1234));
        assert_eq!(rm.run_state.gold, 1234);
        assert_eq!(rm.run_state.current_hp, 100_000, "hp untouched when None");
    }

    #[test]
    fn new_silent_starter_pack() {
        let rm = RunManager::new(42, CharacterId::Silent);
        assert_eq!(rm.run_state.max_hp, 70);
        assert_eq!(rm.run_state.relics, vec![RelicId::RingOfTheSnake]);
        assert_eq!(rm.heal_after_combat, 0);
    }

    #[test]
    fn initial_phase_is_map_choice() {
        let rm = RunManager::new(0, CharacterId::Ironclad);
        assert_eq!(rm.phase(), Phase::MapChoice);
        assert!(!rm.is_over());
    }

    // ── g24zv: `with_neow_start()` builder ──────────────────────────────

    #[test]
    fn with_neow_start_lands_in_event_phase_with_neow_active() {
        let rm = RunManager::new(0, CharacterId::Ironclad).with_neow_start();
        assert_eq!(rm.phase(), Phase::Event);
        assert_eq!(
            rm.run_state.current_event,
            Some(crate::events::EventId::Neow)
        );
        assert!(
            rm.run_state.neow_offers.is_some(),
            "with_neow_start populates the offer cache"
        );
    }

    #[test]
    fn with_neow_start_offers_are_deterministic_per_seed() {
        let a = RunManager::new(123, CharacterId::Ironclad).with_neow_start();
        let b = RunManager::new(123, CharacterId::Ironclad).with_neow_start();
        assert_eq!(a.run_state.neow_offers, b.run_state.neow_offers);
    }

    #[test]
    fn with_neow_start_event_choice_advances_to_map_choice() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad).with_neow_start();
        // Without any content-callbacks wired (`RunManager::new` ships
        // zero callbacks), the engine falls back to a single
        // `EventChoice { idx: 0 }` entry — the broader 3-option
        // enumeration is verified separately via
        // `with_event_option_count_callback_enumerates_all_options`
        // below and end-to-end through
        // `run_callbacks::tests::neow_choice_at_idx_2_grants_cursed_relic`.
        assert_eq!(
            rm.get_available_actions(),
            vec![RunAction::EventChoice { idx: 0 }]
        );
        let result = rm.take_action(RunAction::EventChoice { idx: 0 });
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::MapChoice);
    }

    #[test]
    fn multi_page_event_choice_stays_in_event_phase_until_finished() {
        // Bead `sts2-rs-m3n0g.3`: a multi-page event's intermediate page
        // leaves `current_event` SET (the dispatcher skips
        // `exit_event_room` on a not-finished outcome). `do_event_choice`
        // must then KEEP `Phase::Event` so the next page is presented,
        // only transitioning to the map once a finished page clears the
        // event. Simulated here with a 2-page resolve callback keyed off
        // `event_state.page`.
        fn two_page_resolve(rs: &mut crate::runstate::RunState, _idx: usize) -> bool {
            if rs.event_state.page == 0 {
                rs.event_state.page = 1; // linger: current_event stays Some
            } else {
                rs.exit_event_room(); // final page: clear current_event
            }
            true
        }
        let callbacks = ContentCallbacks {
            resolve_event_at: Some(two_page_resolve),
            ..Default::default()
        };
        let mut rm = RunManager::new(0, CharacterId::Ironclad).with_content_callbacks(callbacks);
        rm.run_state
            .enter_event_room(crate::events::EventId::DollRoom);
        rm.phase = Phase::Event;

        // Page-0 choice: lingers, so we stay in the event.
        let r1 = rm.take_action(RunAction::EventChoice { idx: 0 });
        assert_eq!(r1, ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::Event, "intermediate page keeps Event");
        assert!(rm.run_state.current_event.is_some());

        // Page-1 choice: finishes, so the run advances to the map.
        let r2 = rm.take_action(RunAction::EventChoice { idx: 0 });
        assert_eq!(r2, ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::MapChoice, "finished page exits to map");
        assert!(rm.run_state.current_event.is_none());
    }

    #[test]
    fn with_event_option_count_callback_enumerates_all_options() {
        // Bead sts2-rs-nzdz1: when an `event_option_count` callback is
        // wired and reports N > 0, `get_available_actions` enumerates
        // one `EventChoice { idx }` per option. Uses a test-only
        // count-3 stub so the engine layer can assert the
        // enumeration shape without depending on sts2-content.
        fn count_three(_: &crate::runstate::RunState) -> usize {
            3
        }
        let callbacks = ContentCallbacks {
            event_option_count: Some(count_three),
            ..Default::default()
        };
        let rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(callbacks)
            .with_neow_start();
        assert_eq!(
            rm.get_available_actions(),
            vec![
                RunAction::EventChoice { idx: 0 },
                RunAction::EventChoice { idx: 1 },
                RunAction::EventChoice { idx: 2 },
            ]
        );
    }

    #[test]
    fn event_option_count_zero_falls_back_to_idx_zero() {
        // Defensive: a callback that returns 0 (e.g. current_event is
        // None at the time `get_available_actions` is called — engine
        // will lazily pick inside `resolve_event_at`) must NOT collapse
        // the action list to empty. Fall back to the single idx=0
        // entry so the legacy first-option driver path still works.
        fn count_zero(_: &crate::runstate::RunState) -> usize {
            0
        }
        let callbacks = ContentCallbacks {
            event_option_count: Some(count_zero),
            ..Default::default()
        };
        let mut rm = RunManager::new(0, CharacterId::Ironclad).with_content_callbacks(callbacks);
        // Force Phase::Event without pre-binding current_event so the
        // count callback sees the None case.
        rm.phase = Phase::Event;
        assert_eq!(
            rm.get_available_actions(),
            vec![RunAction::EventChoice { idx: 0 }]
        );
    }

    /// Regression for `sts2-rs-6dtxp`: a combat resting at
    /// `PlayerTurnStart` with a deferred-turn-setup `pending_choice`
    /// must surface a `CombatResolveChoice` (NOT a bogus `EndTurn`), and
    /// taking it must resume turn-setup to `PlayerPlayPhase` without
    /// tripping the `end_player_turn_phase_one` `PlayerPlayPhase`
    /// `debug_assert`. Before the fix, `get_available_actions` dropped
    /// the `ResolveChoice` and injected an `EndTurn` fallback; driving
    /// that `EndTurn` re-entered `end_player_turn_phase_one` at
    /// `PlayerTurnStart` and panicked.
    #[test]
    fn combat_pending_choice_surfaces_resolve_not_endturn() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.phase = Phase::Combat;

        let combat = crate::CombatState {
            phase: crate::CombatPhase::PlayerTurnStart,
            pending_turn_setup_resume: true,
            pending_curse_choice: None,
            pending_potion_hook_resume: None,
            pending_enemy_exec_resume: None,
            pending_choice: Some(crate::PendingChoice {
                kind: crate::ChoiceKind::CardSelect,
                options: Vec::new(),
                min_select: 0,
                max_select: 0,
                can_skip: false,
                post_effect: None,
                picked: smallvec::SmallVec::new(),
            }),
            ..crate::CombatState::default()
        };
        rm.combat = Some(combat);

        let actions = rm.get_available_actions();
        assert!(
            !actions.contains(&RunAction::EndTurn),
            "must not surface a bogus EndTurn while a choice is open: {actions:?}"
        );
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, RunAction::CombatResolveChoice { .. })),
            "must surface CombatResolveChoice: {actions:?}"
        );

        // Resolving the (empty) choice resumes the deferred turn-setup,
        // advancing the combat to PlayerPlayPhase without panic.
        let res = rm.take_action(RunAction::CombatResolveChoice { option: None });
        assert_eq!(res, ActionResult::Advanced);
        let combat = rm.combat.as_ref().expect("combat slot set");
        assert!(combat.pending_choice.is_none(), "choice cleared");
        assert_eq!(combat.phase, crate::CombatPhase::PlayerPlayPhase);
    }

    /// A mid-play-phase combat choice is also resolvable through the run
    /// loop (no bogus EndTurn, no abandonment). Resolving leaves the
    /// combat in `PlayerPlayPhase` (no turn-setup resume here).
    #[test]
    fn combat_pending_choice_in_play_phase_is_resolvable() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.phase = Phase::Combat;

        let combat = crate::CombatState {
            phase: crate::CombatPhase::PlayerPlayPhase,
            pending_choice: Some(crate::PendingChoice {
                kind: crate::ChoiceKind::CardSelect,
                options: Vec::new(),
                min_select: 0,
                max_select: 0,
                can_skip: false,
                post_effect: None,
                picked: smallvec::SmallVec::new(),
            }),
            ..crate::CombatState::default()
        };
        rm.combat = Some(combat);

        let actions = rm.get_available_actions();
        assert!(!actions.contains(&RunAction::EndTurn));
        assert!(actions
            .iter()
            .any(|a| matches!(a, RunAction::CombatResolveChoice { .. })));

        let res = rm.take_action(RunAction::CombatResolveChoice { option: None });
        assert_eq!(res, ActionResult::Advanced);
        let combat = rm.combat.as_ref().expect("combat slot set");
        assert!(combat.pending_choice.is_none());
        assert_eq!(combat.phase, crate::CombatPhase::PlayerPlayPhase);
    }

    /// A held potion is surfaced as a `RunAction::DrinkPotion` during
    /// combat, and taking it consumes the combat-belt slot. (Effect
    /// dispatch is a no-op here — no content registry is installed in a
    /// bare sts2-core test — but the slot is consumed regardless, which
    /// is what the run-action surface must guarantee.)
    #[test]
    fn drink_potion_run_action_consumes_combat_belt_slot() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.phase = Phase::Combat;

        let mut combat = crate::CombatState {
            phase: crate::CombatPhase::PlayerPlayPhase,
            ..crate::CombatState::default()
        };
        let _ = combat.init_player_creature();
        combat
            .player
            .potions
            .push(crate::potions::PotionKind::BlockPotion);
        combat
            .player
            .potions
            .push(crate::potions::PotionKind::FirePotion);
        rm.combat = Some(combat);

        // Both held potions are offered as DrinkPotion actions.
        let actions = rm.get_available_actions();
        let drink_slots: Vec<usize> = actions
            .iter()
            .filter_map(|a| match a {
                RunAction::DrinkPotion { potion_slot, .. } => Some(*potion_slot),
                _ => None,
            })
            .collect();
        assert_eq!(
            drink_slots,
            vec![0, 1],
            "one DrinkPotion per held slot: {actions:?}"
        );

        let res = rm.take_action(RunAction::DrinkPotion {
            potion_slot: 0,
            target: None,
        });
        assert_eq!(res, ActionResult::Advanced);
        let combat = rm.combat.as_ref().expect("combat slot set");
        assert_eq!(
            combat.player.potions.as_slice(),
            [crate::potions::PotionKind::FirePotion],
            "slot 0 (BlockPotion) consumed, FirePotion shifts down",
        );
    }

    // ── sts2-agent fork: the opt-in rewards screen and potion actions ──

    fn screen_rm(screen: Vec<ScreenReward>) -> RunManager {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_rewards = true;
        rm.run_state.max_potion_slots = 2;
        rm.phase = Phase::CardReward;
        rm.reward_screen = Some(screen);
        rm
    }

    /// The screen offers a claim per reward and a proceed; gold applies on
    /// claim, and the claimed reward leaves the screen.
    #[test]
    fn reward_screen_claims_gold_and_proceeds_to_the_map() {
        let mut rm = screen_rm(vec![ScreenReward::Gold { amount: 17 }]);
        let gold = rm.run_state.gold;
        assert_eq!(
            rm.get_available_actions(),
            vec![RunAction::ClaimReward { idx: 0 }, RunAction::ProceedRewards]
        );
        assert_eq!(
            rm.take_action(RunAction::ClaimReward { idx: 0 }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.gold, gold + 17);
        assert_eq!(rm.reward_screen(), Some(&[][..]));
        assert_eq!(rm.phase(), Phase::CardReward);
        assert_eq!(
            rm.take_action(RunAction::ProceedRewards),
            ActionResult::Advanced
        );
        assert!(rm.reward_screen().is_none());
        assert_eq!(rm.phase(), Phase::MapChoice);
    }

    /// Leaving the screen skips what is left: the gold is never gained.
    #[test]
    fn reward_screen_proceed_skips_unclaimed_rewards() {
        let mut rm = screen_rm(vec![ScreenReward::Gold { amount: 17 }]);
        let gold = rm.run_state.gold;
        assert_eq!(
            rm.take_action(RunAction::ProceedRewards),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.gold, gold);
        assert_eq!(rm.phase(), Phase::MapChoice);
    }

    /// A card reward opens its offering; its Skip returns to the screen with
    /// the reward still claimable, and a pick uses it up.
    #[test]
    fn reward_screen_card_reward_skip_keeps_it_and_pick_uses_it() {
        use crate::cards::CardId;
        let options = vec![CardId::Anger, CardId::Bash];
        let mut rm = screen_rm(vec![ScreenReward::Card {
            options: options.clone(),
            upgraded: vec![false, true],
        }]);
        assert_eq!(
            rm.take_action(RunAction::ClaimReward { idx: 0 }),
            ActionResult::Advanced
        );
        assert_eq!(rm.card_reward_options(), options.as_slice());
        assert_eq!(rm.screen_card_claim(), Some(0));
        let actions = rm.get_available_actions();
        assert!(
            actions.contains(&RunAction::PickCardReward { idx: None }),
            "{actions:?}"
        );
        assert!(!actions.contains(&RunAction::ProceedRewards), "{actions:?}");
        // Skip: back on the screen, the reward still there.
        assert_eq!(
            rm.take_action(RunAction::PickCardReward { idx: None }),
            ActionResult::Advanced
        );
        assert_eq!(rm.reward_screen().map(<[_]>::len), Some(1));
        assert!(rm.card_reward_options().is_empty());
        assert_eq!(rm.phase(), Phase::CardReward);
        // Claim again and pick the second card: added upgraded, reward gone.
        let deck = rm.run_state.master_deck.len();
        assert_eq!(
            rm.take_action(RunAction::ClaimReward { idx: 0 }),
            ActionResult::Advanced
        );
        assert_eq!(
            rm.take_action(RunAction::PickCardReward { idx: Some(1) }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.master_deck.len(), deck + 1);
        let added = rm.run_state.master_deck.last().expect("a card was added");
        assert_eq!((added.card_id, added.upgraded), (CardId::Bash, true));
        assert_eq!(rm.reward_screen(), Some(&[][..]));
        assert_eq!(rm.get_available_actions(), vec![RunAction::ProceedRewards]);
    }

    /// A potion reward is unclaimable on a full belt and stays listed; a
    /// discard frees the slot (C# `PotionProcureFailureReason.TooFull`).
    #[test]
    fn reward_screen_potion_waits_for_a_free_slot() {
        use crate::potions::PotionKind;
        let mut rm = screen_rm(vec![ScreenReward::Potion {
            kind: PotionKind::FirePotion,
        }]);
        rm.run_state.interactive_potions = true;
        rm.run_state.potions = vec![PotionKind::BlockPotion, PotionKind::SwiftPotion];
        let actions = rm.get_available_actions();
        assert!(
            !actions.contains(&RunAction::ClaimReward { idx: 0 }),
            "{actions:?}"
        );
        assert_eq!(
            rm.take_action(RunAction::ClaimReward { idx: 0 }),
            ActionResult::InvalidForPhase
        );
        assert!(
            actions.contains(&RunAction::DiscardPotion { slot: 1 }),
            "{actions:?}"
        );
        assert_eq!(
            rm.take_action(RunAction::DiscardPotion { slot: 1 }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.potions, vec![PotionKind::BlockPotion]);
        assert_eq!(
            rm.take_action(RunAction::ClaimReward { idx: 0 }),
            ActionResult::Advanced
        );
        assert_eq!(
            rm.run_state.potions,
            vec![PotionKind::BlockPotion, PotionKind::FirePotion]
        );
        assert_eq!(rm.reward_screen(), Some(&[][..]));
    }

    /// The Future of Potions and Stone of All Time lock the belt from the
    /// event's start until it finishes (`Player.CanRemovePotions = false` in
    /// `BeforeEventStarted`, true in `OnEventFinished`): no drink, no
    /// discard, through The Future of Potions' card reward too. Back on the
    /// map, or in another event, the belt is free (S13).
    #[test]
    fn two_events_lock_the_belt_until_they_finish() {
        use crate::events::EventId;
        use crate::potions::PotionKind;
        let potion_actions = |rm: &RunManager| {
            rm.get_available_actions()
                .into_iter()
                .filter(|a| {
                    matches!(
                        a,
                        RunAction::DiscardPotion { .. } | RunAction::DrinkPotion { .. }
                    )
                })
                .count()
        };
        for event in [EventId::TheFutureOfPotions, EventId::StoneOfAllTime] {
            let mut rm = RunManager::new(0, CharacterId::Ironclad);
            rm.run_state.interactive_potions = true;
            rm.run_state.max_potion_slots = 3;
            rm.run_state.potions = vec![PotionKind::BloodPotion, PotionKind::FirePotion];
            rm.run_state.enter_event_room(event);
            rm.set_phase(Phase::Event);
            assert!(rm.potions_locked(), "{event:?}");
            assert_eq!(potion_actions(&rm), 0, "{event:?}");
            assert_eq!(
                rm.take_action(RunAction::DiscardPotion { slot: 0 }),
                ActionResult::InvalidForPhase
            );
            // The finished page's reward (The Future of Potions' card reward):
            // still locked until the event is done.
            let _ = rm.run_state.exit_event_room();
            rm.set_phase(Phase::CardReward);
            assert!(rm.potions_locked(), "{event:?}");
            rm.set_phase(Phase::MapChoice);
            assert!(!rm.potions_locked(), "{event:?}");
            assert_eq!(potion_actions(&rm), 3, "{event:?}: one drink, two discards");
        }
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_potions = true;
        rm.run_state.potions = vec![PotionKind::FirePotion];
        rm.run_state.enter_event_room(EventId::BrainLeech);
        rm.set_phase(Phase::Event);
        assert!(!rm.potions_locked());
        assert_eq!(potion_actions(&rm), 1);
    }

    /// Without the flags nothing changes: no screen actions, no discard.
    #[test]
    fn potion_actions_are_off_by_default() {
        use crate::potions::PotionKind;
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.max_potion_slots = 3;
        rm.run_state.potions = vec![PotionKind::BloodPotion];
        let actions = rm.get_available_actions();
        assert!(
            actions.iter().all(|a| !matches!(
                a,
                RunAction::DiscardPotion { .. } | RunAction::DrinkPotion { .. }
            )),
            "{actions:?}"
        );
        assert_eq!(
            rm.take_action(RunAction::DiscardPotion { slot: 0 }),
            ActionResult::InvalidForPhase
        );
        assert_eq!(
            rm.take_action(RunAction::DrinkPotion {
                potion_slot: 0,
                target: None
            }),
            ActionResult::InvalidForPhase
        );
        assert_eq!(rm.run_state.potions, vec![PotionKind::BloodPotion]);
    }

    /// Out of combat the AnyTime potions are drinkable: Blood Potion heals 20%
    /// of max HP (floored), Fruit Juice gains 5 max HP; a combat-only potion
    /// can only be discarded; Foul Potion only at the merchant.
    #[test]
    fn anytime_potions_drink_out_of_combat() {
        use crate::potions::PotionKind;
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_potions = true;
        rm.run_state.max_potion_slots = 4;
        rm.run_state.max_hp = 83;
        rm.run_state.current_hp = 40;
        rm.run_state.potions = vec![
            PotionKind::BloodPotion,
            PotionKind::FirePotion,
            PotionKind::FruitJuice,
            PotionKind::FoulPotion,
        ];
        let actions = rm.get_available_actions();
        let drinks: Vec<usize> = actions
            .iter()
            .filter_map(|a| match a {
                RunAction::DrinkPotion { potion_slot, .. } => Some(*potion_slot),
                _ => None,
            })
            .collect();
        assert_eq!(
            drinks,
            vec![0, 2],
            "Blood Potion and Fruit Juice; Foul Potion only at a shop: {actions:?}"
        );
        assert_eq!(
            rm.take_action(RunAction::DrinkPotion {
                potion_slot: 1,
                target: None
            }),
            ActionResult::InvalidForPhase
        );
        assert_eq!(
            rm.take_action(RunAction::DrinkPotion {
                potion_slot: 0,
                target: None
            }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.current_hp, 40 + 16);
        assert_eq!(
            rm.run_state.potions,
            vec![
                PotionKind::FirePotion,
                PotionKind::FruitJuice,
                PotionKind::FoulPotion
            ]
        );
        assert_eq!(
            rm.take_action(RunAction::DrinkPotion {
                potion_slot: 1,
                target: None
            }),
            ActionResult::Advanced
        );
        assert_eq!((rm.run_state.max_hp, rm.run_state.current_hp), (88, 61));
        // At the merchant the Foul Potion is thrown for 100 gold.
        rm.phase = Phase::Shop;
        let gold = rm.run_state.gold;
        assert_eq!(
            rm.take_action(RunAction::DrinkPotion {
                potion_slot: 1,
                target: None
            }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.gold, gold + 100);
        assert_eq!(rm.run_state.potions, vec![PotionKind::FirePotion]);
    }

    /// In a fight a discard empties the combat belt's slot; the combat's own
    /// legal actions (what the search sees) never offer it.
    #[test]
    fn discard_potion_in_combat() {
        use crate::potions::PotionKind;
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_potions = true;
        rm.phase = Phase::Combat;
        let mut combat = crate::CombatState {
            phase: crate::CombatPhase::PlayerPlayPhase,
            ..crate::CombatState::default()
        };
        let _ = combat.init_player_creature();
        combat.player.potions.push(PotionKind::BlockPotion);
        combat.player.potions.push(PotionKind::FirePotion);
        rm.combat = Some(combat);
        let actions = rm.get_available_actions();
        assert!(
            actions.contains(&RunAction::DiscardPotion { slot: 0 }),
            "{actions:?}"
        );
        assert_eq!(
            rm.take_action(RunAction::DiscardPotion { slot: 0 }),
            ActionResult::Advanced
        );
        assert_eq!(
            rm.combat
                .as_ref()
                .expect("combat")
                .player
                .potions
                .as_slice(),
            [PotionKind::FirePotion]
        );
    }

    /// Under the rewards screen the chest's relic is offered (take or leave);
    /// its gold is gained on opening either way. Without the flag the relic
    /// is obtained at once.
    #[test]
    fn treasure_relic_is_offered_under_interactive_rewards() {
        use crate::runstate::RelicRarity;
        let chest = |interactive: bool| {
            let mut rm = RunManager::new(0, CharacterId::Ironclad);
            rm.run_state.interactive_rewards = interactive;
            for rarity in [
                RelicRarity::Common,
                RelicRarity::Uncommon,
                RelicRarity::Rare,
            ] {
                rm.run_state
                    .shared_relic_grab_bag_by_rarity
                    .insert(rarity, vec![crate::RelicId::Anchor]);
            }
            let gold = rm.run_state.gold;
            assert_eq!(rm.resolve_treasure_room(), ActionResult::Advanced);
            assert!(
                rm.run_state.gold > gold,
                "the chest's gold is gained on opening"
            );
            rm
        };
        let auto = chest(false);
        assert!(auto.run_state.relics.contains(&crate::RelicId::Anchor));
        assert_eq!(auto.phase(), Phase::MapChoice);

        let mut left = chest(true);
        assert!(!left.run_state.relics.contains(&crate::RelicId::Anchor));
        assert_eq!(left.phase(), Phase::CardReward);
        assert_eq!(
            left.get_available_actions(),
            vec![RunAction::PickRelicReward, RunAction::SkipRelic]
        );
        let mut taken = left.clone();
        assert_eq!(
            left.take_action(RunAction::SkipRelic),
            ActionResult::Advanced
        );
        assert!(!left.run_state.relics.contains(&crate::RelicId::Anchor));
        assert_eq!(left.phase(), Phase::MapChoice);
        assert_eq!(
            taken.take_action(RunAction::PickRelicReward),
            ActionResult::Advanced
        );
        assert!(taken.run_state.relics.contains(&crate::RelicId::Anchor));
        assert_eq!(taken.phase(), Phase::MapChoice);
    }

    /// An out-of-range `DrinkPotion` slot is rejected rather than
    /// panicking on the belt index.
    #[test]
    fn drink_potion_out_of_range_slot_is_rejected() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.phase = Phase::Combat;
        let combat = crate::CombatState {
            phase: crate::CombatPhase::PlayerPlayPhase,
            ..crate::CombatState::default()
        };
        rm.combat = Some(combat);
        let res = rm.take_action(RunAction::DrinkPotion {
            potion_slot: 9,
            target: None,
        });
        assert_eq!(res, ActionResult::InvalidForPhase);
    }

    #[test]
    fn default_new_does_not_roll_neow_offers() {
        // Per bead g24zv, RunManager::new (without the builder) is
        // unchanged — no Neow roll, no Event phase. Keeps the ~75
        // existing run_manager tests passing without modification.
        let rm = RunManager::new(0, CharacterId::Ironclad);
        assert_eq!(rm.phase(), Phase::MapChoice);
        assert!(rm.run_state.current_event.is_none());
        assert!(rm.run_state.neow_offers.is_none());
    }

    // ── 7kl4f foundation: act-1 map exists at Phase::MapChoice ──────

    #[test]
    fn new_generates_act_1_map_so_map_choice_has_children_to_enumerate() {
        let rm = RunManager::new(7, CharacterId::Ironclad);
        let act_map = rm
            .run_state
            .current_act_map
            .as_ref()
            .expect("RunManager::new populates the act-1 map");
        assert!(act_map.start.is_some(), "start node placed");
        assert!(act_map.boss.is_some(), "boss node placed");
        assert!(!act_map.points.is_empty(), "map has nodes");
    }

    #[test]
    fn new_map_generation_is_deterministic_under_same_seed() {
        let a = RunManager::new(42, CharacterId::Ironclad);
        let b = RunManager::new(42, CharacterId::Ironclad);
        let ma = a.run_state.current_act_map.as_ref().unwrap();
        let mb = b.run_state.current_act_map.as_ref().unwrap();
        assert_eq!(ma.start, mb.start);
        assert_eq!(ma.boss, mb.boss);
        assert_eq!(
            ma.points.iter().map(|p| p.point_type).collect::<Vec<_>>(),
            mb.points.iter().map(|p| p.point_type).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn take_action_endturn_during_map_choice_is_invalid_for_phase() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        assert_eq!(
            rm.take_action(RunAction::EndTurn),
            ActionResult::InvalidForPhase
        );
        // Wrong-phase actions don't advance the state machine.
        assert_eq!(rm.phase(), Phase::MapChoice);
    }

    #[test]
    fn run_over_take_action_returns_run_over() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::RunOver);
        assert!(rm.is_over());
        assert_eq!(rm.take_action(RunAction::EndTurn), ActionResult::RunOver);
    }

    #[test]
    fn card_reward_actions_emit_relic_variants_when_only_relic_rewards_pending() {
        // Bead `sts2-rs-7kl4f`: when CardReward is reached with no card
        // options remaining (e.g. a relic reward from an event flowed
        // into the CardReward phase), enumeration must surface the
        // PickRelicReward / SkipRelic pair so the caller can drain the
        // queue without getting stuck on PickCardReward variants that
        // route to an empty option list.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::CardReward);
        assert!(rm.card_reward_options.is_empty());
        assert!(rm.run_state.relic_rewards.is_empty());
        // (c): both empty → no actions (stuck-loop visible).
        assert!(rm.get_available_actions().is_empty());

        // (b): relic-reward-only fan-out.
        rm.run_state
            .relic_rewards
            .push(crate::runstate::RelicReward::new(RelicId::BurningBlood));
        let actions = rm.get_available_actions();
        assert_eq!(actions.len(), 2, "{actions:?}");
        assert!(actions.contains(&RunAction::PickRelicReward));
        assert!(actions.contains(&RunAction::SkipRelic));

        // (a): card-options take priority over relic-reward fan-out.
        rm.card_reward_options.push(crate::cards::CardId::Bash);
        let actions = rm.get_available_actions();
        assert!(actions
            .iter()
            .all(|a| matches!(a, RunAction::PickCardReward { .. })));
        assert!(actions.contains(&RunAction::PickCardReward { idx: Some(0) }));
        assert!(actions.contains(&RunAction::PickCardReward { idx: None }));
    }

    /// A select the game opens with `Cancelable = false` (Brain Leech's
    /// Share Knowledge grid, `BrainLeech.cs:67-70`) has no skip: no
    /// `PickCardReward { idx: None }`, no Pael's Wing sacrifice, and a skip
    /// taken anyway is refused (S5).
    #[test]
    fn a_no_skip_card_select_offers_and_takes_no_skip() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::CardReward);
        rm.run_state.relics.push(crate::RelicId::PaelsWing);
        let mut select = crate::runstate::PendingCardSelect::from_options(vec![
            crate::cards::CardId::Bash,
            crate::cards::CardId::Anger,
        ]);
        select.can_skip = false;
        rm.run_state.pending_card_selects.push(select);
        rm.advance_after_card_reward();
        assert!(rm.card_offer_choose_screen());
        assert!(!rm.card_offer_can_skip());
        let actions = rm.get_available_actions();
        assert_eq!(
            actions,
            [
                RunAction::PickCardReward { idx: Some(0) },
                RunAction::PickCardReward { idx: Some(1) }
            ]
        );
        assert_eq!(
            rm.take_action(RunAction::PickCardReward { idx: None }),
            ActionResult::InvalidForPhase
        );
        assert_eq!(
            rm.take_action(RunAction::PickCardReward { idx: Some(1) }),
            ActionResult::Advanced
        );
        assert!(
            rm.card_offer_can_skip(),
            "the next offer is skippable again"
        );
    }

    #[test]
    fn pending_card_select_drains_into_card_reward_then_back_to_map() {
        // Bead `sts2-rs-w6hdj` (W9d): a custom card-select queued on
        // `RunState.pending_card_selects` (HeftyTablet's 3-Rare offer)
        // must drain into `card_reward_options` once the relic-reward
        // queue is empty, surface as a normal PickCardReward fan-out
        // (with skip), and clear back to MapChoice once picked.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::CardReward);
        rm.run_state
            .pending_card_selects
            .push(crate::runstate::PendingCardSelect::from_options(vec![
                crate::cards::CardId::Bash,
                crate::cards::CardId::Anger,
            ]));
        // No card/relic rewards ahead → advance drains the select.
        rm.advance_after_card_reward();
        assert_eq!(
            rm.card_reward_options(),
            &[crate::cards::CardId::Bash, crate::cards::CardId::Anger]
        );
        assert!(rm.run_state.pending_card_selects.is_empty());
        assert_eq!(rm.phase(), Phase::CardReward);
        // The drained offering is marked as a choose screen (C#
        // FromChooseACardScreen → headless `card_select`, not
        // `card_reward`).
        assert!(rm.card_offer_choose_screen());
        // Skip is offered (canSkip: true).
        let actions = rm.get_available_actions();
        assert!(actions.contains(&RunAction::PickCardReward { idx: None }));
        // Picking the first option adds it and returns to the map.
        assert_eq!(
            rm.take_action(RunAction::PickCardReward { idx: Some(0) }),
            ActionResult::Advanced
        );
        assert!(rm
            .run_state
            .master_deck
            .iter()
            .any(|c| c.card_id == crate::cards::CardId::Bash));
        assert_eq!(rm.phase(), Phase::MapChoice);
        assert!(!rm.card_offer_choose_screen());
    }

    #[test]
    fn choose_screen_post_add_lands_after_chosen_card_on_pick_and_on_skip() {
        // C# HeftyTablet adds `[chosen?, Injury]` in ONE CardPileCmd.Add
        // AFTER the select resolves — the deck-tail order is
        // chosen-then-Injury on a pick, and the Injury alone on a skip.
        let mk_post = || {
            let mut c = crate::cards::Card::pod_defaults();
            c.card_id = crate::cards::CardId::Injury;
            vec![c]
        };
        // Pick path.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::CardReward);
        rm.run_state
            .pending_card_selects
            .push(crate::runstate::PendingCardSelect::with_post_add(
                vec![crate::cards::CardId::Bash],
                mk_post(),
            ));
        rm.advance_after_card_reward();
        let deck_before = rm.run_state.master_deck.len();
        assert_eq!(
            rm.take_action(RunAction::PickCardReward { idx: Some(0) }),
            ActionResult::Advanced
        );
        let deck = &rm.run_state.master_deck;
        assert_eq!(deck.len(), deck_before + 2);
        assert_eq!(deck[deck.len() - 2].card_id, crate::cards::CardId::Bash);
        assert_eq!(deck[deck.len() - 1].card_id, crate::cards::CardId::Injury);
        // Skip path: the post-add still lands.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::CardReward);
        rm.run_state
            .pending_card_selects
            .push(crate::runstate::PendingCardSelect::with_post_add(
                vec![crate::cards::CardId::Bash],
                mk_post(),
            ));
        rm.advance_after_card_reward();
        let deck_before = rm.run_state.master_deck.len();
        assert_eq!(
            rm.take_action(RunAction::PickCardReward { idx: None }),
            ActionResult::Advanced
        );
        let deck = &rm.run_state.master_deck;
        assert_eq!(deck.len(), deck_before + 1);
        assert_eq!(deck[deck.len() - 1].card_id, crate::cards::CardId::Injury);
        assert!(!rm.card_offer_choose_screen());
    }

    #[test]
    fn transform_deck_card_preempts_and_applies_remove_then_append() {
        // Interactive transform (NewLeaf): pending picks preempt the phase
        // with one TransformDeckCard per deck card; resolving rolls the
        // replacement via the content callback and applies the C# Deck-pile
        // shape (remove original, append replacement at the tail).
        // The Option wrap is dictated by the callback signature.
        #[allow(clippy::unnecessary_wraps)]
        fn fake_roll(
            _rs: &mut RunState,
            _original: crate::cards::CardId,
            _event_stream: Option<(&'static str, u32)>,
        ) -> Option<crate::cards::Card> {
            let mut c = crate::cards::Card::pod_defaults();
            c.card_id = crate::cards::CardId::Anger;
            Some(c)
        }
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.callbacks.roll_transform_replacement = Some(fake_roll);
        rm.run_state.interactive_card_transform = true;
        for id in [
            crate::cards::CardId::Bash,
            crate::cards::CardId::StrikeIronclad,
            crate::cards::CardId::DefendIronclad,
        ] {
            let mut c = crate::cards::Card::pod_defaults();
            c.card_id = id;
            rm.run_state.master_deck.push(c);
        }
        assert!(rm.run_state.offer_transform_cards_reward(1));
        assert_eq!(rm.run_state.pending_deck_transforms.len(), 1);
        let actions = rm.get_available_actions();
        assert_eq!(actions.len(), rm.run_state.master_deck.len());
        assert!(actions
            .iter()
            .all(|a| matches!(a, RunAction::TransformDeckCard { .. })));
        let deck_len = rm.run_state.master_deck.len();
        let original = rm.run_state.master_deck[0].card_id;
        assert_eq!(
            rm.take_action(RunAction::TransformDeckCard { idx: 0 }),
            ActionResult::Advanced
        );
        assert!(rm.run_state.pending_deck_transforms.is_empty());
        assert_eq!(rm.run_state.master_deck.len(), deck_len);
        // Original removed from slot 0, replacement appended at the tail.
        assert_ne!(rm.run_state.master_deck[0].card_id, original);
        assert_eq!(
            rm.run_state.master_deck[deck_len - 1].card_id,
            crate::cards::CardId::Anger
        );
    }

    #[test]
    fn fixed_target_transform_uses_eligible_subset_index_and_no_callback() {
        // Wood Carvings Bird/Torus shape: a queued fixed-target transform
        // resolves WITHOUT the roll callback (C# `TransformTo<T>`), and
        // `idx` indexes the eligible subset, not the raw deck. With no
        // rarity-rank fn registered the basic_only filter falls back to the
        // whole deck, so subset position == deck position here; the
        // fixed-target path itself is what's under test.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_card_transform = true;
        for id in [
            crate::cards::CardId::Bash,
            crate::cards::CardId::StrikeIronclad,
        ] {
            let mut c = crate::cards::Card::pod_defaults();
            c.card_id = id;
            rm.run_state.master_deck.push(c);
        }
        let mut peck = crate::cards::Card::pod_defaults();
        peck.card_id = crate::cards::CardId::Peck;
        rm.run_state
            .pending_deck_transforms
            .push(crate::runstate::PendingDeckTransform {
                basic_only: true,
                fixed_target: Some(peck),
                ..Default::default()
            });
        assert_eq!(
            rm.take_action(RunAction::TransformDeckCard { idx: 1 }),
            ActionResult::Advanced
        );
        assert!(rm.run_state.pending_deck_transforms.is_empty());
        let deck = &rm.run_state.master_deck;
        assert_eq!(deck.len(), 2);
        // StrikeIronclad (subset idx 1) removed; Peck appended at the tail.
        assert_eq!(deck[0].card_id, crate::cards::CardId::Bash);
        assert_eq!(deck[1].card_id, crate::cards::CardId::Peck);
    }

    #[test]
    fn enchant_deck_card_stamps_enchantment_on_eligible_subset_pick() {
        // Wood Carvings Snake shape: the eligible pool skips
        // Status/Curse-typed, already-enchanted, and cost<0 cards; the
        // pick stamps the queued enchantment stacks on the subset card.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_card_transform = true;
        let mut wound = crate::cards::Card::pod_defaults();
        wound.card_id = crate::cards::CardId::Wound;
        wound.card_type = crate::cards::CardType::Status;
        wound.cost = -1;
        let mut bash = crate::cards::Card::pod_defaults();
        bash.card_id = crate::cards::CardId::Bash;
        let mut strike = crate::cards::Card::pod_defaults();
        strike.card_id = crate::cards::CardId::StrikeIronclad;
        rm.run_state.master_deck.extend([wound, bash, strike]);
        rm.run_state
            .pending_deck_enchants
            .push(crate::runstate::PendingDeckEnchant {
                enchantment: crate::cards::EnchantmentId::Slither,
                amount: 1,
                type_restriction: None,
                secondary_type: None,
            });
        // Eligible subset = [Bash, Strike] (Wound excluded) → idx 1 = Strike.
        assert_eq!(rm.enchant_eligible_deck_indices(), vec![1, 2]);
        let actions = rm.get_available_actions();
        assert_eq!(actions.len(), 2);
        assert!(actions
            .iter()
            .all(|a| matches!(a, RunAction::EnchantDeckCard { .. })));
        assert_eq!(
            rm.take_action(RunAction::EnchantDeckCard { idx: 1 }),
            ActionResult::Advanced
        );
        assert!(rm.run_state.pending_deck_enchants.is_empty());
        assert_eq!(
            rm.run_state.master_deck[2].enchantments.as_slice(),
            &[(crate::cards::EnchantmentId::Slither, 1)]
        );
        // The enchanted card drops out of the eligible pool.
        assert_eq!(rm.enchant_eligible_deck_indices(), vec![1]);
    }

    #[test]
    fn enchant_eligible_indices_honor_type_restriction() {
        // SelfHelpBook shape: a type-restricted pending enchant narrows
        // the eligible subset to deck cards of that CardType (C#
        // `SelectAndEnchant`'s `c => c.Type == typeRestriction`).
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_card_transform = true;
        let mut bash = crate::cards::Card::pod_defaults();
        bash.card_id = crate::cards::CardId::Bash;
        bash.card_type = crate::cards::CardType::Attack;
        let mut defend = crate::cards::Card::pod_defaults();
        defend.card_id = crate::cards::CardId::DefendIronclad;
        defend.card_type = crate::cards::CardType::Skill;
        let mut strike = crate::cards::Card::pod_defaults();
        strike.card_id = crate::cards::CardId::StrikeIronclad;
        strike.card_type = crate::cards::CardType::Attack;
        rm.run_state.master_deck.extend([bash, defend, strike]);
        rm.run_state
            .pending_deck_enchants
            .push(crate::runstate::PendingDeckEnchant {
                enchantment: crate::cards::EnchantmentId::Sharp,
                amount: 2,
                type_restriction: Some(crate::cards::CardType::Attack),
                secondary_type: None,
            });
        // Attacks only: Bash (0) + Strike (2); the Skill is filtered out.
        assert_eq!(rm.enchant_eligible_deck_indices(), vec![0, 2]);
        assert_eq!(
            rm.take_action(RunAction::EnchantDeckCard { idx: 1 }),
            ActionResult::Advanced
        );
        // Subset idx 1 = deck idx 2 (Strike).
        assert_eq!(
            rm.run_state.master_deck[2].enchantments.as_slice(),
            &[(crate::cards::EnchantmentId::Sharp, 2)]
        );
    }

    #[test]
    fn enchant_eligible_indices_honor_secondary_type() {
        // RoyalStamp shape: RoyallyApproved's `can_enchant_card` admits
        // Attack OR Skill, expressed as type_restriction + secondary_type.
        // A Power card is filtered out.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_card_transform = true;
        let mut attack = crate::cards::Card::pod_defaults();
        attack.card_id = crate::cards::CardId::StrikeIronclad;
        attack.card_type = crate::cards::CardType::Attack;
        let mut skill = crate::cards::Card::pod_defaults();
        skill.card_id = crate::cards::CardId::DefendIronclad;
        skill.card_type = crate::cards::CardType::Skill;
        let mut power = crate::cards::Card::pod_defaults();
        power.card_id = crate::cards::CardId::Inflame;
        power.card_type = crate::cards::CardType::Power;
        rm.run_state.master_deck.extend([attack, skill, power]);
        rm.run_state
            .pending_deck_enchants
            .push(crate::runstate::PendingDeckEnchant {
                enchantment: crate::cards::EnchantmentId::RoyallyApproved,
                amount: 1,
                type_restriction: Some(crate::cards::CardType::Attack),
                secondary_type: Some(crate::cards::CardType::Skill),
            });
        // Attack (0) + Skill (1); the Power (2) is filtered out.
        assert_eq!(rm.enchant_eligible_deck_indices(), vec![0, 1]);
    }

    #[test]
    fn relic_rewards_drain_before_pending_card_selects() {
        // Ordering invariant: a queued relic reward is surfaced before a
        // pending card-select, so HeftyTablet's own pick (relic) resolves
        // before the 3-Rare select it queues.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::CardReward);
        rm.run_state
            .relic_rewards
            .push(crate::runstate::RelicReward::new(RelicId::BurningBlood));
        rm.run_state
            .pending_card_selects
            .push(crate::runstate::PendingCardSelect::from_options(vec![
                crate::cards::CardId::Bash,
            ]));
        rm.advance_after_card_reward();
        // Still on the relic reward — card options not yet populated.
        assert!(rm.card_reward_options().is_empty());
        assert_eq!(rm.run_state.pending_card_selects.len(), 1);
        let actions = rm.get_available_actions();
        assert!(actions.contains(&RunAction::PickRelicReward));
    }

    /// Parity `heftytablet_pickup_select` (A5P187/Ironclad/A5): NeowsBones
    /// shuffles the Neow-relic pool, takes 2, and offers them sequentially —
    /// in this repro a CARD-REWARD relic (LostCoffer) is granted/awaited
    /// FIRST and a CARD-SELECT relic (HeftyTablet) is deferred onto
    /// `pending_post_pick_relic_grants`. When LostCoffer's card pick resolves,
    /// `advance_after_card_reward` re-runs, finds `pending_card_selects` empty,
    /// then drains the deferred HeftyTablet grant — whose `after_obtained`
    /// queues HeftyTablet's skippable 3-Rare select (+ a deck-tail Injury
    /// `post_add`). Before the fix the function fell through to the boss/map
    /// branches, STRANDING that just-queued select and dropping the Injury
    /// (C# `HeftyTablet.cs:32-38` adds `[chosen?, Injury]` to the Deck only
    /// after the surfaced `FromChooseACardScreen` resolves). The re-check
    /// inserted after the two grant drains must now surface it.
    #[test]
    fn deferred_relic_grant_after_card_pick_surfaces_its_card_select() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{register_after_obtained, AfterObtainedRunFn};

        // Stand-in for HeftyTablet's `after_obtained`: queue a skippable
        // 3-card select with a deferred Injury `post_add`, mirroring
        // `listener_hefty_tablet` (sts2-content). Kunai is otherwise
        // un-granted / un-listened-on in sts2-core tests, so overriding its
        // process-global listener is parallel-safe.
        fn hefty_like_stub(rs: &mut RunState) -> EffectResult {
            if rs.last_obtained_relic == Some(RelicId::Kunai) {
                rs.pending_card_selects
                    .push(crate::runstate::PendingCardSelect::with_post_add(
                        vec![
                            crate::cards::CardId::Thrash,
                            crate::cards::CardId::DemonFormCard,
                            crate::cards::CardId::Feed,
                        ],
                        vec![{
                            let mut c = crate::cards::Card::pod_defaults();
                            c.card_id = crate::cards::CardId::Injury;
                            c
                        }],
                    ));
            }
            EffectResult::Resolved
        }
        register_after_obtained(RelicId::Kunai, hefty_like_stub as AfterObtainedRunFn);

        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(drain_test_callbacks());
        rm.set_phase(Phase::CardReward);
        let deck_before = rm.run_state.master_deck.len();

        // LostCoffer's card offering is already up; HeftyTablet is deferred —
        // the exact state `advance_after_card_reward` re-enters once the
        // LostCoffer card pick resolves. `pending_post_pick_relic_grants` only
        // drains while `card_reward_options` is empty (`grant_batch_settled`),
        // so it stays parked until the pick clears the offering.
        rm.card_reward_options = vec![crate::cards::CardId::Bash];
        rm.run_state
            .pending_post_pick_relic_grants
            .push(RelicId::Kunai);

        // Resolve LostCoffer's card pick (idx 0 → the offered Bash).
        let result = rm.do_card_reward_pick(Some(0));
        assert_eq!(result, ActionResult::Advanced);

        // LostCoffer's chosen card landed in the deck.
        assert!(rm
            .run_state
            .master_deck
            .iter()
            .any(|c| c.card_id == crate::cards::CardId::Bash));
        // The deferred HeftyTablet was granted.
        assert!(rm.run_state.relics.contains(&RelicId::Kunai));

        // CRITICAL: HeftyTablet's 3-Rare select is now SURFACED (was stranded
        // before the fix), not the map.
        assert_eq!(
            rm.phase(),
            Phase::CardReward,
            "deferred relic's card-select must surface, not fall through to the map"
        );
        assert!(
            rm.card_offer_choose_screen(),
            "the surfaced offer is a FromChooseACardScreen (skippable 1-of-N)"
        );
        assert_eq!(
            rm.card_reward_options(),
            &[
                crate::cards::CardId::Thrash,
                crate::cards::CardId::DemonFormCard,
                crate::cards::CardId::Feed,
            ],
        );
        // Skip is always offered (C# `canSkip: true`).
        let actions = rm.get_available_actions();
        assert!(actions.contains(&RunAction::PickCardReward { idx: None }));
        assert!(actions.contains(&RunAction::PickCardReward { idx: Some(0) }));

        // Resolve the select by picking the first option (THRASH). The Injury
        // post_add lands AFTER the chosen card, deck-tail — matching C#'s
        // single `CardPileCmd.Add([chosen?, Injury], Deck)`.
        let _ = rm.do_card_reward_pick(Some(0));
        let tail: Vec<_> = rm
            .run_state
            .master_deck
            .iter()
            .rev()
            .take(2)
            .map(|c| c.card_id)
            .collect();
        assert_eq!(
            tail,
            vec![crate::cards::CardId::Injury, crate::cards::CardId::Thrash],
            "deck tail is chosen-then-Injury (reversed: Injury, Thrash)"
        );
        // Bash (LostCoffer) + Thrash (chosen) + Injury = 3 cards added.
        assert_eq!(rm.run_state.master_deck.len(), deck_before + 3);
        // Choice fully resolved — back to the map.
        assert_eq!(rm.phase(), Phase::MapChoice);
    }

    /// Parity `neow_deck_size_minus1` (A9P400/Defect/A9): NeowsBones shuffles
    /// the Neow-relic pool, takes 2, and grants both — here a CARD-SELECT relic
    /// (LeadPaperweight's 2-colorless offer, surfaced via `pending_card_selects`)
    /// resolves FIRST, and a POTION-PROCURING relic (PhialHolster) is deferred
    /// onto `pending_post_pick_relic_grants`. PhialHolster's `after_obtained`
    /// procures 2 random potions onto `pending_rewards` (a chained POTION
    /// reward), then a NeowsBones curse is deferred onto
    /// `pending_neows_bones_curses` to be rolled LAST.
    ///
    /// When the LeadPaperweight select resolves, `advance_after_card_reward`
    /// re-runs: it grants the deferred PhialHolster (queuing 2 potions), so
    /// `drain_post_pick_deck_cards_if_settled` SKIPS (the grant batch isn't
    /// settled while `pending_rewards` is non-empty). The fall-through
    /// `drain_pending_rewards()` then drains those potions into the belt but
    /// returns `false` (no Card sentinel) — so the function does NOT recurse,
    /// and BEFORE the fix it fell straight through to the map with the curse
    /// stranded (deck off-by-one). The unconditional
    /// `drain_post_pick_deck_cards_if_settled()` added on the non-recursing
    /// fall-through must now roll/append the curse, matching C#
    /// `NeowsBones.cs:45-54` (curse added AFTER `RewardsSet.Offer()` — including
    /// PhialHolster's potion procure — settles). Distinct from the chained-RELIC
    /// SmallCapsule case (`drain_post_pick_relic_grants_if_settled`'s tail).
    #[test]
    fn deferred_curse_lands_after_chained_potion_reward_drains() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{register_after_obtained, AfterObtainedRunFn};
        use crate::runstate::{PotionReward, Reward};

        // Stand-in for PhialHolster's `after_obtained`: procure 2 potions onto
        // `pending_rewards` (a chained POTION reward, NOT a card/relic). Kunai
        // is otherwise un-granted / un-listened-on in sts2-core tests, so
        // overriding its process-global listener is parallel-safe.
        fn phial_like_stub(rs: &mut RunState) -> EffectResult {
            if rs.last_obtained_relic == Some(RelicId::Kunai) {
                let _ = rs.procure_potion(PotionReward::with_kind(
                    crate::potions::PotionKind::FirePotion,
                ));
                let _ = rs.procure_potion(PotionReward::with_kind(
                    crate::potions::PotionKind::FirePotion,
                ));
            }
            EffectResult::Resolved
        }
        register_after_obtained(RelicId::Kunai, phial_like_stub as AfterObtainedRunFn);

        // Roll-curse callback: append a single GUILTY (the A9P400 curse).
        // The `Option` wrap is required by the `roll_neows_bones_curse`
        // callback signature even though this stub always returns `Some`.
        #[allow(clippy::unnecessary_wraps)]
        fn roll_guilty(_rs: &mut RunState) -> Option<crate::cards::Card> {
            let mut c = crate::cards::Card::pod_defaults();
            c.card_id = crate::cards::CardId::Guilty;
            Some(c)
        }
        let mut callbacks = drain_test_callbacks();
        callbacks.roll_neows_bones_curse = Some(roll_guilty);

        let mut rm = RunManager::new(0, CharacterId::Defect).with_content_callbacks(callbacks);
        rm.set_phase(Phase::CardReward);
        // Direct-grant mode (run-parity / workbench backend), so the deferred
        // relic auto-collects inline.
        rm.run_state.direct_event_relic_grant = true;
        let deck_before = rm.run_state.master_deck.len();

        // LeadPaperweight's 2-colorless offer is already surfaced as a choose
        // screen; PhialHolster is deferred and the NeowsBones curse is parked.
        // This is exactly the state `advance_after_card_reward` re-enters once
        // the LeadPaperweight pick resolves.
        rm.card_reward_options = vec![crate::cards::CardId::Prolong];
        rm.card_offer_choose_screen = true;
        rm.run_state
            .pending_post_pick_relic_grants
            .push(RelicId::Kunai);
        rm.run_state.pending_neows_bones_curses = 1;

        // Resolve LeadPaperweight's pick (idx 0 → PROLONG).
        let result = rm.do_card_reward_pick(Some(0));
        assert_eq!(result, ActionResult::Advanced);

        // PhialHolster was granted, its 2 potions drained into the belt, and —
        // CRITICALLY — the deferred GUILTY curse landed in the deck (was
        // stranded before the fix). PROLONG (chosen) + GUILTY = 2 cards added.
        assert!(rm.run_state.relics.contains(&RelicId::Kunai));
        assert!(
            rm.run_state
                .master_deck
                .iter()
                .any(|c| c.card_id == crate::cards::CardId::Guilty),
            "deferred NeowsBones curse must land after the chained potion reward drains"
        );
        assert!(rm.run_state.pending_rewards.is_empty());
        assert_eq!(rm.run_state.pending_neows_bones_curses, 0);
        assert!(!matches!(
            rm.run_state.pending_rewards.first(),
            Some(Reward::Potion(_))
        ));
        assert_eq!(rm.run_state.master_deck.len(), deck_before + 2);
        // Run is fully settled — back to the map.
        assert_eq!(rm.phase(), Phase::MapChoice);
    }

    #[test]
    fn card_reward_options_accessor_mirrors_internal_offer_list() {
        // Bead `sts2-rs-ze744` (W5a): public accessor returns the same
        // CardId list that get_available_actions() fans out as
        // PickCardReward{Some(i)} variants. Index alignment matters for
        // Python drivers that look up `card_choices[*].card_id` and
        // dispatch the matching idx.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::CardReward);
        assert!(rm.card_reward_options().is_empty());
        rm.card_reward_options.push(crate::cards::CardId::Bash);
        rm.card_reward_options.push(crate::cards::CardId::Anger);
        let offers = rm.card_reward_options();
        assert_eq!(
            offers,
            &[crate::cards::CardId::Bash, crate::cards::CardId::Anger]
        );
        // Index alignment with the public action fan-out.
        let actions = rm.get_available_actions();
        assert!(actions.contains(&RunAction::PickCardReward { idx: Some(0) }));
        assert!(actions.contains(&RunAction::PickCardReward { idx: Some(1) }));
    }

    #[test]
    fn exit_combat_does_not_double_apply_heal_after_combat_on_win() {
        // Bead `sts2-rs-jenwp`: the BurningBlood post-combat heal is now
        // modeled COMBAT-SIDE (`dispatch_after_combat_victory` heals
        // `combat.player` before `exit_combat` writes HP back), matching
        // C# `BurningBlood.AfterCombatVictory` in `EndCombatInternal`. The
        // legacy `heal_after_combat` run-manager shim that re-applied +6
        // here is removed, so `exit_combat_and_transition` must NOT mutate
        // HP on a win.
        //
        // This synthetic slot wires no `exit_combat` callback, so
        // `run_state.current_hp` is whatever the test set (the combat-side
        // heal already ran in a real combat). Assert it is left untouched.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.current_hp = 70; // 10 below Ironclad's 80 max.
        rm.run_state.current_room = Some(crate::runstate::Room::new(
            crate::runstate::RoomType::Monster,
        ));
        rm.combat = Some(crate::CombatState {
            is_over: true,
            player_won: true,
            ..crate::CombatState::default()
        });
        rm.set_phase(Phase::Combat);

        let _ = rm.exit_combat_and_transition();
        assert_eq!(
            rm.run_state.current_hp, 70,
            "no run-manager shim heal: HP must be left at the combat-side value",
        );
        // Same on an Elite win — no shim heal applied at the exit seam.
        rm.run_state.current_hp = 77;
        rm.run_state.current_room =
            Some(crate::runstate::Room::new(crate::runstate::RoomType::Elite));
        rm.combat = Some(crate::CombatState {
            is_over: true,
            player_won: true,
            ..crate::CombatState::default()
        });
        rm.set_phase(Phase::Combat);
        let _ = rm.exit_combat_and_transition();
        assert_eq!(
            rm.run_state.current_hp, 77,
            "no shim heal on Elite win either"
        );
    }

    #[test]
    fn exit_combat_clears_settled_swipe_ledger() {
        // Bead `sts2-rs-RP13` follow-up (shadow census seed 1034): the exit
        // callback removes each un-reclaimed stolen card from `master_deck`
        // exactly once. The combat object is then restored into `self.combat`
        // for the reward screen, so a stale `swipe_stolen` ledger makes the
        // sts2-py deck view subtract the same identity AGAIN — with a
        // duplicate-id deck (2× Stampede) that ate the surviving copy. After
        // `exit_combat_and_transition` the ledger must be empty.
        fn flushing_exit(rs: &mut crate::runstate::RunState, combat: &crate::CombatState) {
            // Minimal stand-in for run_bridge's exit_combat swipe flush.
            for stolen in &combat.swipe_stolen {
                if stolen.reclaimed {
                    continue;
                }
                if let Some(pos) = rs
                    .master_deck
                    .iter()
                    .position(|c| c.card_id == stolen.card_id && c.upgraded == stolen.upgraded)
                {
                    rs.master_deck.remove(pos);
                }
            }
        }
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.callbacks.exit_combat = Some(flushing_exit);
        rm.run_state.current_room = Some(crate::runstate::Room::new(
            crate::runstate::RoomType::Monster,
        ));
        // Duplicate-identity deck: the double-subtract only bites when a
        // second (card_id, upgraded) match survives the flush.
        for _ in 0..2 {
            let mut strike = crate::cards::Card::pod_defaults();
            strike.card_id = crate::cards::CardId::StrikeIronclad;
            rm.run_state.master_deck.push(strike);
        }
        let strikes_before = rm
            .run_state
            .master_deck
            .iter()
            .filter(|c| c.card_id == crate::cards::CardId::StrikeIronclad && !c.upgraded)
            .count();
        let mut combat = crate::CombatState {
            is_over: true,
            player_won: true,
            ..crate::CombatState::default()
        };
        let thief = crate::CreatureUid::new(1).unwrap();
        combat.record_swipe_steal(thief, crate::cards::CardId::StrikeIronclad, false);
        rm.combat = Some(combat);
        rm.set_phase(Phase::Combat);

        let _ = rm.exit_combat_and_transition();
        let strikes_after = rm
            .run_state
            .master_deck
            .iter()
            .filter(|c| c.card_id == crate::cards::CardId::StrikeIronclad && !c.upgraded)
            .count();
        assert_eq!(
            strikes_after,
            strikes_before - 1,
            "the flush removes the stolen copy exactly once"
        );
        assert!(
            rm.combat
                .as_ref()
                .expect("combat retained for reward screen")
                .swipe_stolen
                .is_empty(),
            "settled steals must not linger for the deck view to re-subtract"
        );
    }

    #[test]
    fn exit_combat_skips_heal_after_combat_on_loss() {
        // The !player_won branch returns before the heal application,
        // so a defeated Ironclad's post-RunOver current_hp stays
        // wherever the combat-side exit_combat callback left it (here
        // the synthetic combat had current_hp=0 via default).
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.current_hp = 0;
        rm.run_state.current_room = Some(crate::runstate::Room::new(
            crate::runstate::RoomType::Monster,
        ));
        rm.combat = Some(crate::CombatState {
            is_over: true,
            player_won: false,
            ..crate::CombatState::default()
        });
        rm.set_phase(Phase::Combat);
        let _ = rm.exit_combat_and_transition();
        assert_eq!(rm.phase(), Phase::RunOver);
        assert_eq!(
            rm.run_state.current_hp, 0,
            "heal must not fire on loss path"
        );
    }

    #[test]
    fn resolve_combat_fast_loss_ends_run() {
        // AZ M2 planning-transition seam: resolve_combat_fast stamps the
        // built CombatState terminal from a surrogate outcome (instead of
        // playing it) and delegates to exit_combat_and_transition. This
        // exercises the loss -> RunOver branch, which is fully
        // core-testable without content callbacks.
        let mut rm = RunManager::new(1, CharacterId::Ironclad);
        rm.combat = Some(crate::CombatState::default());
        rm.set_phase(Phase::Combat);
        let res = rm.resolve_combat_fast(0.5, /* survived = */ false);
        assert_eq!(res, ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::RunOver);
        assert!(rm.run_state.is_over && !rm.run_state.player_won);
    }

    #[test]
    fn bossrelic_dispatch_via_exit_combat_on_act1_or_act2_boss_win() {
        // Bead `sts2-rs-4srxg`: verify the BossRelic transition is
        // wired correctly. exit_combat_and_transition routes a player-
        // won Boss-room combat (act_index < 3) through the
        // `generate_boss_relic_pool` callback into Phase::BossRelic
        // with the offered options stashed on RunManager. This is the
        // engine-side proof-of-correctness that complements the
        // full_run_act1to3_ironclad acceptance test, whose pick-first
        // driver cannot win Vantom and therefore can't exercise this
        // path end-to-end.
        fn pool_stub(_rs: &mut RunState) -> Vec<crate::RelicId> {
            vec![
                crate::RelicId::CallingBell,
                crate::RelicId::Anchor,
                crate::RelicId::OldCoin,
            ]
        }
        let cb = ContentCallbacks {
            generate_boss_relic_pool: Some(pool_stub),
            ..ContentCallbacks::default()
        };
        let mut rm = RunManager::new(0, CharacterId::Ironclad).with_content_callbacks(cb);
        rm.run_state.act_index = 1;
        rm.run_state.current_room =
            Some(crate::runstate::Room::new(crate::runstate::RoomType::Boss));
        rm.combat = Some(crate::CombatState {
            is_over: true,
            player_won: true,
            ..crate::CombatState::default()
        });
        rm.set_phase(Phase::Combat);

        let result = rm.exit_combat_and_transition();
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::BossRelic);
        // Available actions on the BossRelic phase fan out one
        // PickBossRelic per pool entry.
        let actions = rm.get_available_actions();
        assert_eq!(
            actions,
            vec![
                RunAction::PickBossRelic { idx: 0 },
                RunAction::PickBossRelic { idx: 1 },
                RunAction::PickBossRelic { idx: 2 },
            ],
            "got {actions:?}"
        );
        // PickBossRelic transitions back to MapChoice and grants the
        // chosen relic.
        let result = rm.take_action(RunAction::PickBossRelic { idx: 1 });
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::MapChoice);
        assert!(rm.run_state.relics.contains(&crate::RelicId::Anchor));
    }

    #[test]
    fn act1_boss_win_offers_card_reward_before_boss_relic() {
        // GH #25: winning an Act-1 boss (e.g. the Ceremonial Beast) must
        // offer the combat card reward FIRST and only then the boss-relic
        // choice — mirroring C# `RewardsSet.GenerateRewardsFor`
        // (`RewardsSet.cs:181-185`), where a Boss room yields a
        // `CardReward(ForRoom(Boss), 3)` alongside the boss relic. The
        // pre-fix dispatch routed straight to `Phase::BossRelic` and
        // dropped the card reward entirely.
        fn cards(_rs: &mut RunState) -> Vec<crate::cards::CardId> {
            vec![
                crate::cards::CardId::StrikeIronclad,
                crate::cards::CardId::DefendIronclad,
                crate::cards::CardId::Bash,
            ]
        }
        fn pool_stub(_rs: &mut RunState) -> Vec<crate::RelicId> {
            vec![crate::RelicId::CallingBell, crate::RelicId::Anchor]
        }
        let cb = ContentCallbacks {
            generate_card_reward_options: Some(cards),
            generate_boss_relic_pool: Some(pool_stub),
            ..ContentCallbacks::default()
        };
        let mut rm = RunManager::new(0, CharacterId::Ironclad).with_content_callbacks(cb);
        rm.run_state.act_index = 1;
        rm.run_state.current_room =
            Some(crate::runstate::Room::new(crate::runstate::RoomType::Boss));
        rm.combat = Some(crate::CombatState {
            is_over: true,
            player_won: true,
            ..crate::CombatState::default()
        });
        rm.set_phase(Phase::Combat);

        // Boss win lands on the card-reward screen, not BossRelic.
        let result = rm.exit_combat_and_transition();
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(
            rm.phase(),
            Phase::CardReward,
            "Act-1 boss win must surface the card reward first"
        );
        assert!(
            rm.get_available_actions()
                .iter()
                .any(|a| matches!(a, RunAction::PickCardReward { idx: Some(_) })),
            "a card pick is offered"
        );
        let deck_before = rm.run_state.master_deck.len();

        // Picking a card adds it to the deck and then advances to the
        // boss-relic choice (pre-generated at boss-win time).
        let result = rm.take_action(RunAction::PickCardReward { idx: Some(0) });
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(rm.run_state.master_deck.len(), deck_before + 1);
        assert_eq!(
            rm.phase(),
            Phase::BossRelic,
            "after the card reward resolves, the boss relic is offered"
        );
        assert_eq!(rm.boss_relic_options().len(), 2);

        // Picking the boss relic finally returns to the map.
        let result = rm.take_action(RunAction::PickBossRelic { idx: 0 });
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::MapChoice);
        assert!(rm.run_state.relics.contains(&crate::RelicId::CallingBell));
    }

    #[test]
    fn act1_boss_card_reward_can_be_skipped_to_boss_relic() {
        // The boss card reward honors skip (`PickCardReward { idx: None }`)
        // and still surfaces the boss-relic choice afterward — skipping the
        // card must not strand the run nor drop the boss relic.
        fn cards(_rs: &mut RunState) -> Vec<crate::cards::CardId> {
            vec![crate::cards::CardId::Bash]
        }
        fn pool_stub(_rs: &mut RunState) -> Vec<crate::RelicId> {
            vec![crate::RelicId::Anchor]
        }
        let cb = ContentCallbacks {
            generate_card_reward_options: Some(cards),
            generate_boss_relic_pool: Some(pool_stub),
            ..ContentCallbacks::default()
        };
        let mut rm = RunManager::new(0, CharacterId::Ironclad).with_content_callbacks(cb);
        rm.run_state.act_index = 2;
        rm.run_state.current_room =
            Some(crate::runstate::Room::new(crate::runstate::RoomType::Boss));
        rm.combat = Some(crate::CombatState {
            is_over: true,
            player_won: true,
            ..crate::CombatState::default()
        });
        rm.set_phase(Phase::Combat);

        let _ = rm.exit_combat_and_transition();
        assert_eq!(rm.phase(), Phase::CardReward);
        let deck_before = rm.run_state.master_deck.len();

        // Skip the card.
        let result = rm.take_action(RunAction::PickCardReward { idx: None });
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(
            rm.run_state.master_deck.len(),
            deck_before,
            "skipping adds no card"
        );
        assert_eq!(rm.phase(), Phase::BossRelic);
    }

    // ── sts2-rs-01o: pending_rewards drain ──────────────────────────────

    fn drain_test_callbacks() -> ContentCallbacks {
        fn cards(_rs: &mut RunState) -> Vec<crate::cards::CardId> {
            vec![
                crate::cards::CardId::StrikeIronclad,
                crate::cards::CardId::DefendIronclad,
                crate::cards::CardId::Bash,
            ]
        }
        // `Option` is required by the `roll_potion_reward_kind` callback
        // signature (`fn(&mut RunState) -> Option<PotionKind>`); the wrap is
        // not removable here even though this stub always returns `Some`.
        #[allow(clippy::unnecessary_wraps)]
        fn potion(_rs: &mut RunState) -> Option<crate::potions::PotionKind> {
            Some(crate::potions::PotionKind::FirePotion)
        }
        ContentCallbacks {
            generate_card_reward_options: Some(cards),
            roll_potion_reward_kind: Some(potion),
            ..ContentCallbacks::default()
        }
    }

    #[test]
    fn drain_pending_rewards_materializes_card_and_potion_sentinels() {
        use crate::runstate::{CardReward, PotionReward, Reward};
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(drain_test_callbacks());
        // Lost Coffer's after_obtained shape: 1 empty Card + 1 empty Potion.
        rm.run_state
            .pending_rewards
            .push(Reward::Card(CardReward::empty()));
        rm.run_state
            .pending_rewards
            .push(Reward::Potion(PotionReward::empty()));

        let opened = rm.drain_pending_rewards();

        assert!(
            opened,
            "a queued Card sentinel opens a card-reward offering"
        );
        assert_eq!(
            rm.card_reward_options.len(),
            3,
            "card offering filled via generate_card_reward_options"
        );
        assert_eq!(
            rm.run_state.potions,
            vec![crate::potions::PotionKind::FirePotion],
            "potion sentinel rolled + procured into the run belt"
        );
        assert!(
            rm.run_state.pending_rewards.is_empty(),
            "both sentinels consumed"
        );
    }

    #[test]
    fn drain_pending_card_reward_prefers_noncombat_roller_and_preserves_pity() {
        // PARITY132: a `pending_rewards` Card sentinel (Lost Coffer / Orrery /
        // event card grant) is a `CardCreationSource.Other` reward — it must
        // roll through the NON-combat (base-odds) roller, NOT the pity-updating
        // combat roller, and must leave the run pity counter untouched.
        use crate::runstate::{CardReward, Reward};

        // Distinct stubs so we can tell which roller fired. The non-combat
        // stub also MUTATES pity the way the buggy pity path would, to prove
        // the production base-odds roller is the one that must not.
        fn combat_cards(_rs: &mut RunState) -> Vec<crate::cards::CardId> {
            vec![crate::cards::CardId::Bash]
        }
        fn noncombat_cards(_rs: &mut RunState) -> Vec<crate::cards::CardId> {
            vec![
                crate::cards::CardId::StrikeIronclad,
                crate::cards::CardId::DefendIronclad,
            ]
        }
        let callbacks = ContentCallbacks {
            generate_card_reward_options: Some(combat_cards),
            generate_noncombat_card_reward_options: Some(noncombat_cards),
            ..ContentCallbacks::default()
        };
        let mut rm = RunManager::new(0, CharacterId::Ironclad).with_content_callbacks(callbacks);
        let pity_before = rm.run_state.card_rarity_odds.current_value;
        rm.run_state
            .pending_rewards
            .push(Reward::Card(CardReward::empty()));

        let opened = rm.drain_pending_rewards();

        assert!(opened, "queued Card sentinel opens an offering");
        assert_eq!(
            rm.card_reward_options,
            vec![
                crate::cards::CardId::StrikeIronclad,
                crate::cards::CardId::DefendIronclad,
            ],
            "non-combat sentinel rolls via generate_noncombat_card_reward_options, \
             NOT the combat pity roller"
        );
        assert!(
            (rm.run_state.card_rarity_odds.current_value - pity_before).abs() < f64::EPSILON,
            "non-combat card reward must not mutate the run pity counter"
        );
    }

    #[test]
    fn drain_pending_card_reward_falls_back_to_combat_roller_when_noncombat_unwired() {
        // Back-compat: if only the combat roller is wired, the drain path
        // still fills the offering (the `.or(...)` fallback).
        use crate::runstate::{CardReward, Reward};
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(drain_test_callbacks());
        rm.run_state
            .pending_rewards
            .push(Reward::Card(CardReward::empty()));
        assert!(rm.drain_pending_rewards());
        assert_eq!(
            rm.card_reward_options.len(),
            3,
            "fallback roller filled offering"
        );
    }

    #[test]
    fn drain_pending_rewards_resolves_card_removal() {
        use crate::runstate::Reward;
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(drain_test_callbacks());
        // Seed a 3-card master deck (parity mode auto-removes from the front).
        rm.run_state.master_deck = vec![
            crate::cards::Card::pod_defaults(),
            crate::cards::Card::pod_defaults(),
            crate::cards::Card::pod_defaults(),
        ];
        rm.run_state
            .pending_rewards
            .push(Reward::RemoveCard { count: 2 });

        let opened = rm.drain_pending_rewards();

        assert!(
            !opened,
            "card-removal resolves immediately, opens no offering"
        );
        assert_eq!(
            rm.run_state.master_deck.len(),
            1,
            "RemoveCard {{ count: 2 }} thinned the deck by 2 (parity mode)"
        );
        assert!(
            rm.run_state.pending_rewards.is_empty(),
            "the RemoveCard sentinel is consumed"
        );
    }

    #[test]
    fn drain_pending_rewards_honors_belt_capacity() {
        use crate::runstate::{PotionReward, Reward};
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(drain_test_callbacks());
        rm.run_state.max_potion_slots = 1;
        for _ in 0..3 {
            rm.run_state
                .pending_rewards
                .push(Reward::Potion(PotionReward::empty()));
        }

        let opened = rm.drain_pending_rewards();

        assert!(!opened, "no card sentinel queued");
        assert_eq!(
            rm.run_state.potions.len(),
            1,
            "belt capped at max_potion_slots; overflow potions dropped"
        );
        assert!(
            rm.run_state.pending_rewards.is_empty(),
            "queue fully drained"
        );
        assert_eq!(
            rm.run_state.potions_overflow_dropped, 2,
            "the 2 overflow potions are counted, not silently evaporated (GH #252)"
        );
        assert!(
            rm.run_state.pending_potion_swaps.is_empty(),
            "non-interactive default never queues a swap decision"
        );
    }

    #[test]
    fn full_belt_potion_reward_surfaces_interactive_swap() {
        use crate::potions::PotionKind;
        use crate::runstate::{PotionReward, Reward};
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(drain_test_callbacks());
        rm.run_state.interactive_potion_swap = true;
        rm.run_state.max_potion_slots = 2;
        assert!(rm.run_state.add_potion_to_belt(PotionKind::BlockPotion));
        assert!(rm.run_state.add_potion_to_belt(PotionKind::FirePotion));
        rm.run_state
            .pending_rewards
            .push(Reward::Potion(PotionReward::with_kind(
                PotionKind::EnergyPotion,
            )));

        let _ = rm.drain_pending_rewards();

        assert_eq!(
            rm.run_state.pending_potion_swaps,
            vec![PotionKind::EnergyPotion],
            "full belt queues the reward as a pending swap decision"
        );
        // Decline-first action surface: [None, Some(0), Some(1)].
        let actions = rm.get_available_actions();
        assert_eq!(
            actions,
            vec![
                RunAction::PotionSwap { discard_slot: None },
                RunAction::PotionSwap {
                    discard_slot: Some(0)
                },
                RunAction::PotionSwap {
                    discard_slot: Some(1)
                },
            ],
            "swap preempt lists decline first, then one discard per held slot"
        );
        // Swap out slot 1: FirePotion discarded, EnergyPotion in its place.
        let result = rm.take_action(RunAction::PotionSwap {
            discard_slot: Some(1),
        });
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(
            rm.run_state.potions,
            vec![PotionKind::BlockPotion, PotionKind::EnergyPotion],
            "discarded slot is refilled in place (C# fill-first-empty)"
        );
        assert!(rm.run_state.pending_potion_swaps.is_empty());
        assert_eq!(
            rm.run_state.potions_overflow_dropped, 0,
            "a completed swap loses nothing"
        );
    }

    #[test]
    fn declined_potion_swap_counts_overflow_and_keeps_belt() {
        use crate::potions::PotionKind;
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(drain_test_callbacks());
        rm.run_state.interactive_potion_swap = true;
        rm.run_state.max_potion_slots = 1;
        assert!(rm.run_state.add_potion_to_belt(PotionKind::BlockPotion));
        assert!(!rm.run_state.claim_potion_reward(PotionKind::EnergyPotion));

        let result = rm.take_action(RunAction::PotionSwap { discard_slot: None });

        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(
            rm.run_state.potions,
            vec![PotionKind::BlockPotion],
            "declining keeps the held belt untouched"
        );
        assert!(rm.run_state.pending_potion_swaps.is_empty());
        assert_eq!(
            rm.run_state.potions_overflow_dropped, 1,
            "a declined reward potion is counted as overflow waste"
        );
        // With no swap pending, the action is rejected.
        assert_eq!(
            rm.take_action(RunAction::PotionSwap { discard_slot: None }),
            ActionResult::InvalidForPhase
        );
    }

    #[test]
    fn drain_pending_rewards_uses_prerolled_potion_kind() {
        use crate::runstate::{PotionReward, Reward};
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(drain_test_callbacks());
        // A pre-rolled kind (PhialHolster procure) must be kept verbatim,
        // not re-rolled through the callback (which would yield FirePotion).
        rm.run_state
            .pending_rewards
            .push(Reward::Potion(PotionReward::with_kind(
                crate::potions::PotionKind::EnergyPotion,
            )));

        let _ = rm.drain_pending_rewards();

        assert_eq!(
            rm.run_state.potions,
            vec![crate::potions::PotionKind::EnergyPotion],
            "pre-rolled potion kind preserved"
        );
    }

    #[test]
    fn bossrelic_empty_pool_falls_through_to_map_not_softlock() {
        // Bead `sts2-rs-7ufeb`: a won Act-1 boss whose
        // `generate_boss_relic_pool` callback yields an EMPTY pool must
        // NOT strand at Phase::BossRelic (which would surface zero legal
        // actions and softlock the external driver) — it falls through to
        // MapChoice, same as the no-callback path. Surfaced by the W8.2
        // act1_replay corpus driver: seed H4AJPJL7Y2 (asc 5) won the boss
        // but the driver recorded engine_won=None on no_actions_phase=
        // boss_relic.
        fn empty_pool(_rs: &mut RunState) -> Vec<crate::RelicId> {
            Vec::new()
        }
        let cb = ContentCallbacks {
            generate_boss_relic_pool: Some(empty_pool),
            ..ContentCallbacks::default()
        };
        let mut rm = RunManager::new(0, CharacterId::Ironclad).with_content_callbacks(cb);
        rm.run_state.act_index = 1;
        rm.run_state.current_room =
            Some(crate::runstate::Room::new(crate::runstate::RoomType::Boss));
        rm.combat = Some(crate::CombatState {
            is_over: true,
            player_won: true,
            ..crate::CombatState::default()
        });
        rm.set_phase(Phase::Combat);

        let result = rm.exit_combat_and_transition();
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(
            rm.phase(),
            Phase::MapChoice,
            "empty boss-relic pool must fall through to MapChoice, not strand at BossRelic"
        );
        assert!(
            rm.boss_relic_options.is_empty(),
            "no boss-relic options should be offered"
        );
        // And the run is not over (a won Act-1 boss continues to Act-2).
        assert!(!rm.run_state.is_over);
    }

    #[test]
    fn bossrelic_skipped_on_act3_boss_win_for_run_over() {
        // Act-3 boss-win is terminal (player_won + final act); the
        // dispatch routes directly to RunOver, NOT BossRelic.
        let cb = ContentCallbacks::default();
        let mut rm = RunManager::new(0, CharacterId::Ironclad).with_content_callbacks(cb);
        rm.run_state.act_index = 3;
        rm.run_state.current_room =
            Some(crate::runstate::Room::new(crate::runstate::RoomType::Boss));
        rm.combat = Some(crate::CombatState {
            is_over: true,
            player_won: true,
            ..crate::CombatState::default()
        });
        rm.set_phase(Phase::Combat);
        let _ = rm.exit_combat_and_transition();
        assert_eq!(rm.phase(), Phase::RunOver);
        assert!(rm.run_state.player_won);
    }

    // SLOW (~30s in debug): sweeps 24 seeds × full act-map generation.
    // The `slow_` name prefix is the project's slow-test marker —
    // `.config/nextest.toml`'s `default-filter` excludes it from the
    // default run. Execute it with `cargo nextest run --profile slow`.
    #[test]
    fn slow_start_map_moves_only_offer_live_row1_children_not_pruned_ghosts() {
        // Regression (workbench-combat-parity): `prune_and_repair`
        // removes duplicate path segments from the map's `grid` but
        // leaves the pruned ghost nodes in the raw `points` vec (the
        // convention `full_map_view` documents). `next_map_children_cols`
        // / `do_map_move` used to read `points.filter(row == 1)` for the
        // start row, so those ghosts surfaced as legal first moves. A
        // ghost has no children, so taking that move fell through to the
        // act-boss col — collapsing every run to "row-1 combat → boss".
        //
        // Invariant: the start row's offered Move cols must exactly equal
        // the grid-live row-1 nodes (= the start node's children, what
        // `full_map_view` renders), and taking any offered start move must
        // land on row 1 (never the boss).
        // 24 seeds spans both pruned-ghost maps (the pre-fix bug, common
        // — ~half of seeds) and clean maps; map generation is the costly
        // part in a debug build, so cap the sweep rather than probe
        // hundreds of layouts.
        for seed in 0..24_i32 {
            let rm = RunManager::new(seed, CharacterId::Ironclad);
            assert_eq!(rm.phase(), Phase::MapChoice, "seed {seed}");

            let offered: Vec<i32> = rm
                .get_available_actions()
                .into_iter()
                .filter_map(|a| match a {
                    RunAction::Move { col } => Some(col),
                    _ => None,
                })
                .collect();

            let act_map = rm.run_state.current_act_map.as_ref().expect("map");
            let view = act_map.full_map_view(None, &[]);
            let mut row1_cols: Vec<i32> = view
                .rows
                .get(1)
                .map(|row| row.iter().map(|n| n.coord.col).collect())
                .unwrap_or_default();
            row1_cols.sort_unstable();
            let boss_row = view.boss.map_or(i32::MAX, |b| b.coord.row);

            assert_eq!(
                offered, row1_cols,
                "seed {seed}: offered start moves {offered:?} must match \
                 grid-live row-1 cols {row1_cols:?} (no pruned ghosts)"
            );

            // Every offered start move must resolve to a row-1 node, not
            // teleport to the boss. `do_map_move` writes `current_room`
            // with the resolved coord before dispatching by room type, so
            // we read the coord directly — the dispatch itself returns
            // `NotImplemented` here because pure-core tests wire no content
            // combat callback, which is irrelevant to the map resolution
            // under test.
            for &col in &offered {
                let mut probe = RunManager::new(seed, CharacterId::Ironclad);
                let _ = probe.do_map_move(col);
                let landed = probe
                    .run_state
                    .current_room
                    .and_then(|r| r.map_coord)
                    .expect("entered room carries a coord");
                assert_eq!(
                    landed.row, 1,
                    "seed {seed}: start move col {col} landed at row {} (boss row {boss_row}); \
                     a ghost first move fell through to the boss",
                    landed.row
                );
            }
        }
    }

    #[test]
    fn interactive_card_removal_defers_to_a_player_pick_else_auto_removes() {
        fn deck_of(n: u32) -> Vec<crate::Card> {
            (1..=n)
                .map(|i| crate::Card {
                    uid: crate::CardUid::new(i).expect("nz"),
                    card_id: crate::cards::CardId::StrikeIronclad,
                    ..crate::Card::pod_defaults()
                })
                .collect()
        }

        // Default (parity) mode: removal happens immediately, no pending
        // picks surface.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.master_deck = deck_of(3);
        assert_eq!(rm.run_state.remove_cards_from_deck(1), 1);
        assert_eq!(rm.run_state.master_deck.len(), 2);
        assert_eq!(rm.run_state.pending_deck_removals, 0);

        // Interactive mode: removal is deferred to a player pick.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_card_removal = true;
        rm.run_state.master_deck = deck_of(3);
        assert_eq!(
            rm.run_state.remove_cards_from_deck(1),
            0,
            "nothing removed yet"
        );
        assert_eq!(
            rm.run_state.master_deck.len(),
            3,
            "deck untouched until the pick"
        );
        assert_eq!(rm.run_state.pending_deck_removals, 1);

        // The pending removal preempts the current phase's actions with one
        // RemoveDeckCard per deck card, and resolving one drains the queue.
        let actions = rm.get_available_actions();
        assert_eq!(
            actions,
            vec![
                RunAction::RemoveDeckCard { idx: 0 },
                RunAction::RemoveDeckCard { idx: 1 },
                RunAction::RemoveDeckCard { idx: 2 },
            ],
        );
        assert_eq!(
            rm.take_action(RunAction::RemoveDeckCard { idx: 1 }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.master_deck.len(), 2);
        assert_eq!(rm.run_state.pending_deck_removals, 0);
        // With the queue drained, the normal phase action surface returns.
        assert!(!rm
            .get_available_actions()
            .iter()
            .any(|a| matches!(a, RunAction::RemoveDeckCard { .. })));
    }

    /// Bead `sts2-rs-1hiht`: a "remove-then-damage" relic (PrecariousShears)
    /// parks its HP cost in `pending_post_removal_hp_loss`; the damage must
    /// land only when the interactive removal batch FULLY drains — not on the
    /// first of multiple picks — matching C#'s suspend-then-`Damage` order.
    #[test]
    fn pending_post_removal_hp_loss_applies_when_removal_batch_drains() {
        fn deck_of(n: u32) -> Vec<crate::Card> {
            (1..=n)
                .map(|i| crate::Card {
                    uid: crate::CardUid::new(i).expect("nz"),
                    card_id: crate::cards::CardId::StrikeIronclad,
                    ..crate::Card::pod_defaults()
                })
                .collect()
        }
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_card_removal = true;
        rm.run_state.master_deck = deck_of(5);
        let hp0 = rm.run_state.current_hp;
        // Simulate PrecariousShears: queue 2 removals + defer 16 damage.
        rm.run_state.pending_deck_removals = 2;
        rm.run_state.pending_post_removal_hp_loss = 16;

        // First pick: one removal resolved, batch not yet drained → no damage.
        assert_eq!(
            rm.take_action(RunAction::RemoveDeckCard { idx: 0 }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.pending_deck_removals, 1);
        assert_eq!(rm.run_state.current_hp, hp0, "no damage mid-batch");
        assert_eq!(rm.run_state.pending_post_removal_hp_loss, 16);

        // Second pick: batch drains → the deferred 16 damage applies once.
        assert_eq!(
            rm.take_action(RunAction::RemoveDeckCard { idx: 0 }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.pending_deck_removals, 0);
        assert_eq!(rm.run_state.current_hp, hp0 - 16);
        assert_eq!(rm.run_state.pending_post_removal_hp_loss, 0, "consumed");
    }

    /// NeowsBones defers its curse to `pending_post_pick_cards`; it must land in
    /// the deck only AFTER the granted relics' interactive picks (here a
    /// Pomander upgrade batch) fully drain — matching C#'s
    /// `await RewardsSet.Offer()` THEN add-curse order. Deck-size-offset parity
    /// cluster (Neow's Bones curse-timing).
    #[test]
    fn pending_post_pick_curse_lands_only_after_upgrade_batch_drains() {
        fn deck_of(n: u32) -> Vec<crate::Card> {
            (1..=n)
                .map(|i| crate::Card {
                    uid: crate::CardUid::new(i).expect("nz"),
                    card_id: crate::cards::CardId::StrikeIronclad,
                    ..crate::Card::pod_defaults()
                })
                .collect()
        }
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_card_upgrade = true;
        rm.run_state.master_deck = deck_of(5);
        let curse = crate::Card {
            uid: crate::CardUid::new(99).expect("nz"),
            card_id: crate::cards::CardId::Decay,
            ..crate::Card::pod_defaults()
        };
        // Simulate NeowsBones granting a Pomander (2 interactive upgrades
        // queued) and deferring its curse.
        rm.run_state.pending_deck_upgrades = 2;
        rm.run_state.pending_post_pick_cards = vec![curse];

        // First upgrade pick: batch not yet drained → curse still deferred.
        assert_eq!(
            rm.take_action(RunAction::UpgradeDeckCard { idx: 0 }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.pending_deck_upgrades, 1);
        assert_eq!(
            rm.run_state.master_deck.len(),
            5,
            "curse not added mid-batch"
        );
        assert_eq!(rm.run_state.pending_post_pick_cards.len(), 1);

        // Second upgrade pick: batch drains → the deferred curse drops in once.
        assert_eq!(
            rm.take_action(RunAction::UpgradeDeckCard { idx: 0 }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.pending_deck_upgrades, 0);
        assert_eq!(rm.run_state.master_deck.len(), 6);
        assert_eq!(
            rm.run_state.master_deck.last().map(|c| c.card_id),
            Some(crate::cards::CardId::Decay),
            "curse appended at the deck tail, after the upgrade batch"
        );
        assert!(
            rm.run_state.pending_post_pick_cards.is_empty(),
            "deferral queue consumed"
        );
    }

    /// NeowsBones grants its 2 relics ONE AT A TIME: a relic deferred onto
    /// `pending_post_pick_relic_grants` lands only after the previously
    /// granted relic's interactive pick (here an upgrade batch) settles, and
    /// strictly BEFORE the deferred curse — matching C#'s sequential
    /// `await RewardsSet.Offer()` THEN add-curse order. Multi-relic
    /// interleaving cluster (PARITY22 deck_size, PARITY69 bundle relics).
    #[test]
    fn deferred_relic_grant_lands_after_pick_settles_and_before_curse() {
        fn deck_of(n: u32) -> Vec<crate::Card> {
            (1..=n)
                .map(|i| crate::Card {
                    uid: crate::CardUid::new(i).expect("nz"),
                    card_id: crate::cards::CardId::StrikeIronclad,
                    ..crate::Card::pod_defaults()
                })
                .collect()
        }
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_card_upgrade = true;
        rm.run_state.master_deck = deck_of(5);
        let curse = crate::Card {
            uid: crate::CardUid::new(99).expect("nz"),
            card_id: crate::cards::CardId::Decay,
            ..crate::Card::pod_defaults()
        };
        // Simulate NeowsBones having granted relic[0] (which surfaced a
        // 1-upgrade pick), deferred relic[1] (LavaRock, passive), and deferred
        // its curse.
        rm.run_state.pending_deck_upgrades = 1;
        rm.run_state.pending_post_pick_relic_grants = vec![crate::RelicId::LavaRock];
        rm.run_state.pending_post_pick_cards = vec![curse];

        // Before the pick resolves: neither the relic nor the curse has landed.
        assert!(!rm.run_state.relics.contains(&crate::RelicId::LavaRock));
        assert_eq!(rm.run_state.master_deck.len(), 5);

        // Resolve the upgrade pick → batch settles. The relic-grant drain runs
        // first (LavaRock obtained), then the curse drops in.
        assert_eq!(
            rm.take_action(RunAction::UpgradeDeckCard { idx: 0 }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.pending_deck_upgrades, 0);
        assert!(
            rm.run_state.relics.contains(&crate::RelicId::LavaRock),
            "deferred relic granted once the pick settled"
        );
        assert!(
            rm.run_state.pending_post_pick_relic_grants.is_empty(),
            "relic-grant queue consumed"
        );
        assert_eq!(rm.run_state.master_deck.len(), 6);
        assert_eq!(
            rm.run_state.master_deck.last().map(|c| c.card_id),
            Some(crate::cards::CardId::Decay),
            "curse appended only after the deferred relic was granted"
        );
    }

    /// Bead `sts2-rs-1hiht`: Pomander's `upgrade_random_cards(1)` either
    /// auto-upgrades a random card (default/parity) or — with
    /// `interactive_card_upgrade` — defers to a player pick that surfaces only
    /// the NON-upgraded deck cards, mirroring C#
    /// `Pomander.AfterObtained` → `CardSelectCmd.FromDeckForUpgrade`.
    #[test]
    fn interactive_card_upgrade_defers_to_a_player_pick_else_auto_upgrades() {
        fn deck_mixed() -> Vec<crate::Card> {
            // 3 cards: idx 1 starts already-upgraded (so the interactive
            // surface must skip it).
            (1..=3)
                .map(|i| {
                    let mut c = crate::Card {
                        uid: crate::CardUid::new(i).expect("nz"),
                        card_id: crate::cards::CardId::StrikeIronclad,
                        ..crate::Card::pod_defaults()
                    };
                    if i == 2 {
                        c.upgrade();
                    }
                    c
                })
                .collect()
        }

        // Default (parity) mode: a card is upgraded immediately off the
        // rewards stream; no pending picks surface.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.master_deck = deck_mixed();
        assert_eq!(rm.run_state.upgrade_random_cards(1), 1);
        assert_eq!(
            rm.run_state
                .master_deck
                .iter()
                .filter(|c| c.upgraded)
                .count(),
            2,
            "the pre-upgraded card plus one freshly upgraded"
        );
        assert_eq!(rm.run_state.pending_deck_upgrades, 0);

        // Interactive mode: the upgrade is deferred, consuming no RNG.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.interactive_card_upgrade = true;
        rm.run_state.master_deck = deck_mixed();
        let rewards_before = rm.run_state.rng.rewards.counter();
        assert_eq!(
            rm.run_state.upgrade_random_cards(1),
            0,
            "nothing upgraded yet"
        );
        assert_eq!(
            rm.run_state
                .master_deck
                .iter()
                .filter(|c| c.upgraded)
                .count(),
            1,
            "deck untouched until the pick (only the pre-upgraded card)"
        );
        assert_eq!(rm.run_state.pending_deck_upgrades, 1);
        assert_eq!(
            rm.run_state.rng.rewards.counter(),
            rewards_before,
            "interactive defer consumes no rewards RNG"
        );

        // The pending upgrade preempts the phase actions, offering one
        // UpgradeDeckCard per NON-upgraded card, indexed by position in the
        // upgradeable subset (the pre-upgraded master idx 1 is not a slot):
        // subset = [master0, master2] → idx 0, 1.
        let actions = rm.get_available_actions();
        assert_eq!(
            actions,
            vec![
                RunAction::UpgradeDeckCard { idx: 0 },
                RunAction::UpgradeDeckCard { idx: 1 },
            ],
        );
        // An out-of-range subset pick is rejected.
        assert_eq!(
            rm.take_action(RunAction::UpgradeDeckCard { idx: 2 }),
            ActionResult::InvalidForPhase
        );
        // Subset idx 1 maps to master-deck idx 2 (the 2nd non-upgraded card).
        assert_eq!(
            rm.take_action(RunAction::UpgradeDeckCard { idx: 1 }),
            ActionResult::Advanced
        );
        assert!(rm.run_state.master_deck[2].upgraded);
        assert_eq!(rm.run_state.pending_deck_upgrades, 0);
        assert!(!rm
            .get_available_actions()
            .iter()
            .any(|a| matches!(a, RunAction::UpgradeDeckCard { .. })));
    }

    #[test]
    fn rest_site_actions_enumerate_both_rest_and_smith() {
        // Bead `sts2-rs-7kl4f`: the base rest-site list is always
        // `[Rest, Smith{enabled:?}]`; enumeration emits both idx slots
        // so a "pick first option" driver hitting a rest site can fall
        // through to Smith when the relic/modifier registry later
        // disables Rest. The `enabled:false` Smith case is handled in
        // `do_rest_site_action` by the `Disabled` outcome → MapChoice.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::RestSite);
        let actions = rm.get_available_actions();
        assert_eq!(
            actions,
            vec![
                RunAction::RestOption { idx: 0 },
                RunAction::RestOption { idx: 1 },
            ],
            "got {actions:?}",
        );
    }

    #[test]
    fn map_choice_actions_enumerate_row1_children() {
        // Bead `sts2-rs-7kl4f`: at run start (Phase::MapChoice, no
        // current room) the action list mirrors the row-1 node columns
        // of the generated act-1 map. RunOver still yields `[]`.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        let actions = rm.get_available_actions();
        assert!(
            !actions.is_empty(),
            "MapChoice at run start must offer at least one Move action"
        );
        for a in &actions {
            assert!(matches!(a, RunAction::Move { .. }), "got {a:?}");
        }
        rm.set_phase(Phase::RunOver);
        assert!(rm.get_available_actions().is_empty());
    }

    #[test]
    fn combat_starts_none() {
        let rm = RunManager::new(0, CharacterId::Ironclad);
        assert!(rm.combat.is_none());
    }

    /// WingedBoots off-path map navigation (W9 Neow relic). While the
    /// relic has ignore-paths uses remaining, the map-choice surface
    /// offers EVERY grid-live next-row node (not just connected
    /// children), and moving to an off-path node spends one use.
    /// Mirrors C# `RunSimulator.MapSelectState` / `DoMapSelect`. This
    /// is the seed-1 lockstep divergence (run-RNG parity harness).
    #[test]
    fn winged_boots_offers_all_next_row_and_spends_a_use_off_path() {
        use crate::map::{ActMap, MapPointType};
        use crate::runstate::{Room, RoomType};

        // Build a controlled map: start (3,0) → only child (3,1); the
        // player sits at (3,1). Row 2 has three Event nodes at cols
        // 1/3/5, but only (3,2) is a connected child of (3,1).
        let build_map = || {
            let mut m = ActMap::new(3);
            let s = m.get_or_create(3, 0);
            let r1 = m.get_or_create(3, 1);
            let off_l = m.get_or_create(1, 2);
            let on = m.get_or_create(3, 2);
            let off_r = m.get_or_create(5, 2);
            let b = m.get_or_create(3, 3);
            for idx in [off_l, on, off_r] {
                m.points[idx as usize].point_type = MapPointType::Ancient; // Event
            }
            m.start = Some(s);
            m.boss = Some(b);
            m.add_child(s, r1);
            m.add_child(r1, on); // (3,1)'s only edge is to (3,2)
            m.add_child(on, b);
            m
        };

        // --- With WingedBoots: all three row-2 cols offered; off-path OK.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.current_act_map = Some(build_map());
        rm.run_state.relics.push(crate::RelicId::WingedBoots);
        rm.run_state.current_room = Some(Room::at(
            crate::map::MapCoord::new(3, 1),
            MapPointType::Monster,
            RoomType::Monster,
        ));
        rm.phase = Phase::MapChoice;

        let cols: Vec<i32> = rm
            .get_available_actions()
            .into_iter()
            .filter_map(|a| match a {
                RunAction::Move { col } => Some(col),
                _ => None,
            })
            .collect();
        assert_eq!(
            cols,
            vec![1, 3, 5],
            "boots offers every grid-live row-2 node"
        );

        // Move off-path to col 1 → accepted, one use spent.
        let res = rm.do_map_move(1);
        assert_ne!(res, ActionResult::InvalidForPhase, "off-path move accepted");
        assert_eq!(rm.run_state.winged_boots_off_path_used, 1, "spent one use");
        assert_eq!(
            rm.run_state.current_room.and_then(|r| r.map_coord),
            Some(crate::map::MapCoord::new(1, 2)),
            "landed on the off-path node"
        );

        // --- Without WingedBoots: only the connected child is offered,
        // and an off-path move is rejected.
        let mut rm2 = RunManager::new(0, CharacterId::Ironclad);
        rm2.run_state.current_act_map = Some(build_map());
        rm2.run_state.current_room = Some(Room::at(
            crate::map::MapCoord::new(3, 1),
            MapPointType::Monster,
            RoomType::Monster,
        ));
        rm2.phase = Phase::MapChoice;
        let cols2: Vec<i32> = rm2
            .get_available_actions()
            .into_iter()
            .filter_map(|a| match a {
                RunAction::Move { col } => Some(col),
                _ => None,
            })
            .collect();
        assert_eq!(cols2, vec![3], "no boots → only the connected child");
        assert_eq!(
            rm2.do_map_move(1),
            ActionResult::InvalidForPhase,
            "no boots → off-path move rejected"
        );
        assert_eq!(rm2.run_state.winged_boots_off_path_used, 0);
    }

    #[test]
    fn seed_round_trips() {
        let rm = RunManager::with_ascension(12345, CharacterId::Defect, 0);
        assert_eq!(rm.seed, 12345);
        assert_eq!(rm.character_id, CharacterId::Defect);
        assert_eq!(rm.ascension_level, 0);
    }

    // ── RunState.character_id wiring — bead sts2-rs-xbkhd ───────────

    /// `RunManager::with_ascension` threads `character_id` onto
    /// `run_state.character_id` so the run-side sampler walks
    /// can read the class directly. Mirrors the Python pin's
    /// `RunState.__init__(character_id=...)` wiring at
    /// `run_state.py:1383-1387`.
    #[test]
    fn run_state_character_id_set_from_constructor() {
        for c in [
            CharacterId::Ironclad,
            CharacterId::Silent,
            CharacterId::Defect,
            CharacterId::Necrobinder,
            CharacterId::Regent,
        ] {
            let rm = RunManager::new(0, c);
            assert_eq!(
                rm.run_state.character_id, c,
                "{c:?} not threaded to RunState",
            );
            // Also pinned on the RunManager itself for backwards
            // compat with the existing `seed_round_trips` shape.
            assert_eq!(rm.character_id, c, "{c:?} not on RunManager");
        }
    }

    /// `RunState::default()` defaults `character_id` to Ironclad,
    /// matching Python's `RunState.__init__(character_id="Ironclad")`
    /// default. Hand-built `RunState`s used in tests / scripted
    /// fixtures never see an "uninitialized" class.
    #[test]
    fn run_state_character_id_default_is_ironclad() {
        let rs = RunState::new();
        assert_eq!(rs.character_id, CharacterId::Ironclad);
    }

    // ── W5b: SmithCard sub-state — bead sts2-rs-5abnx ───────────────

    #[test]
    fn smith_with_multiple_upgradable_cards_enumerates_smith_card_actions() {
        // Two distinct upgradable cards in the deck → Smith opens a
        // CardSelect pending_choice; get_available_actions(Phase::RestSite)
        // must enumerate one SmithCard{idx} per option, not the base
        // Rest/Smith pair.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.master_deck.clear();
        rm.run_state.master_deck.push(crate::Card {
            uid: crate::CardUid::new(1).expect("nz"),
            card_id: crate::cards::CardId::StrikeIronclad,
            ..crate::Card::pod_defaults()
        });
        rm.run_state.master_deck.push(crate::Card {
            uid: crate::CardUid::new(2).expect("nz"),
            card_id: crate::cards::CardId::DefendIronclad,
            ..crate::Card::pod_defaults()
        });
        rm.set_phase(Phase::RestSite);
        let outcome =
            rm.do_rest_site_action(crate::runstate::RestSiteOption::Smith { enabled: true });
        assert_eq!(
            outcome,
            crate::runstate::RestSiteActionOutcome::SmithAwaitingChoice
        );
        assert_eq!(rm.phase(), Phase::RestSite, "phase stays at RestSite");
        let actions = rm.get_available_actions();
        assert_eq!(
            actions,
            vec![
                RunAction::SmithCard { idx: 0 },
                RunAction::SmithCard { idx: 1 },
            ],
            "smith sub-state enumerates one SmithCard per upgradable card",
        );
    }

    #[test]
    fn smith_card_pick_applies_upgrade_and_returns_to_map_choice() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.master_deck.clear();
        rm.run_state.master_deck.push(crate::Card {
            uid: crate::CardUid::new(1).expect("nz"),
            card_id: crate::cards::CardId::StrikeIronclad,
            ..crate::Card::pod_defaults()
        });
        rm.run_state.master_deck.push(crate::Card {
            uid: crate::CardUid::new(2).expect("nz"),
            card_id: crate::cards::CardId::DefendIronclad,
            ..crate::Card::pod_defaults()
        });
        rm.set_phase(Phase::RestSite);
        let _ = rm.do_rest_site_action(crate::runstate::RestSiteOption::Smith { enabled: true });
        let res = rm.take_action(RunAction::SmithCard { idx: 1 });
        assert_eq!(res, ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::MapChoice, "smith resolves → MapChoice");
        assert!(
            rm.run_state.pending_choice.is_none(),
            "pending choice cleared",
        );
        let upgraded: Vec<_> = rm
            .run_state
            .master_deck
            .iter()
            .filter(|c| c.upgraded)
            .map(|c| c.card_id)
            .collect();
        assert_eq!(upgraded, vec![crate::cards::CardId::DefendIronclad]);
    }

    /// Regression for `sts2-rs-smith-idx` (whole-run seed RP0): with several
    /// IDENTICAL non-upgraded cards in the deck, a Smith pick at index N must
    /// upgrade the N-th such card IN PLACE, not the first one by id. C#
    /// `FromDeckForUpgrade` lists upgradable cards in deck order and upgrades
    /// `list[N]` in place; the prior body upgraded the first id-match, landing
    /// the `+` at deck index 0 and diverging the deck ORDER from C#.
    #[test]
    fn smith_card_pick_upgrades_the_specific_indexed_instance_not_first_id_match() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.master_deck.clear();
        // Four identical Strikes (uids 1..=4) + one Defend.
        for uid in 1u32..=4 {
            rm.run_state.master_deck.push(crate::Card {
                uid: crate::CardUid::new(uid).expect("nz"),
                card_id: crate::cards::CardId::StrikeIronclad,
                ..crate::Card::pod_defaults()
            });
        }
        rm.run_state.master_deck.push(crate::Card {
            uid: crate::CardUid::new(5).expect("nz"),
            card_id: crate::cards::CardId::DefendIronclad,
            ..crate::Card::pod_defaults()
        });
        rm.set_phase(Phase::RestSite);
        let _ = rm.do_rest_site_action(crate::runstate::RestSiteOption::Smith { enabled: true });
        // Pick index 3 — the FOURTH Strike (deck index 3).
        let res = rm.take_action(RunAction::SmithCard { idx: 3 });
        assert_eq!(res, ActionResult::Advanced);
        // Exactly the card at deck index 3 (uid 4) is upgraded; indices 0–2 stay base.
        let upgraded_positions: Vec<usize> = rm
            .run_state
            .master_deck
            .iter()
            .enumerate()
            .filter(|(_, c)| c.upgraded)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            upgraded_positions,
            vec![3],
            "the 4th Strike (deck index 3) is upgraded in place, not the first",
        );
    }

    #[test]
    fn smith_card_out_of_range_is_invalid_and_does_not_advance() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.master_deck.clear();
        rm.run_state.master_deck.push(crate::Card {
            uid: crate::CardUid::new(1).expect("nz"),
            card_id: crate::cards::CardId::StrikeIronclad,
            ..crate::Card::pod_defaults()
        });
        rm.run_state.master_deck.push(crate::Card {
            uid: crate::CardUid::new(2).expect("nz"),
            card_id: crate::cards::CardId::DefendIronclad,
            ..crate::Card::pod_defaults()
        });
        rm.set_phase(Phase::RestSite);
        let _ = rm.do_rest_site_action(crate::runstate::RestSiteOption::Smith { enabled: true });
        let res = rm.take_action(RunAction::SmithCard { idx: 99 });
        assert_eq!(res, ActionResult::InvalidForPhase);
        assert_eq!(
            rm.phase(),
            Phase::RestSite,
            "phase unchanged on invalid idx"
        );
        assert!(
            rm.run_state.pending_choice.is_some(),
            "pending choice preserved on invalid idx",
        );
        assert!(rm.run_state.master_deck.iter().all(|c| !c.upgraded));
    }

    /// `sts2-rs-gorge-select`: a pending multi-pick card grant
    /// (RoomFullOfCheese Gorge) preempts the phase with one `GrantDeckCard`
    /// per option; resolving `pick` of them appends exactly those cards to the
    /// deck (in pick order) and discards the unpicked remainder. The backend
    /// drives an N-index `select_cards` as N sequential picks with the same
    /// `i - n` shift the transform fan-out uses (so picking original indices
    /// 0 then 2 = `GrantDeckCard{0}` then `GrantDeckCard{1}`).
    #[test]
    fn card_grant_pick_two_of_three_adds_picks_to_deck_in_order_and_consumes_grant() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.master_deck.clear();
        let mk = |uid: u32, id: crate::cards::CardId| crate::Card {
            uid: crate::CardUid::new(uid).expect("nz"),
            card_id: id,
            ..crate::Card::pod_defaults()
        };
        rm.run_state
            .pending_card_grants
            .push(crate::runstate::PendingCardGrant {
                options: vec![
                    mk(101, crate::cards::CardId::StrikeIronclad),
                    mk(102, crate::cards::CardId::DefendIronclad),
                    mk(103, crate::cards::CardId::Bash),
                ],
                pick: 2,
            });
        // Preempts the phase: one GrantDeckCard per current option.
        assert_eq!(
            rm.get_available_actions(),
            vec![
                RunAction::GrantDeckCard { idx: 0 },
                RunAction::GrantDeckCard { idx: 1 },
                RunAction::GrantDeckCard { idx: 2 },
            ],
        );
        // Pick original indices 0 and 2 (ascending → 0, then 2-1=1).
        assert_eq!(
            rm.take_action(RunAction::GrantDeckCard { idx: 0 }),
            ActionResult::Advanced,
        );
        assert_eq!(
            rm.run_state.pending_card_grants.len(),
            1,
            "grant stays open after the 1st of 2 picks",
        );
        assert_eq!(
            rm.take_action(RunAction::GrantDeckCard { idx: 1 }),
            ActionResult::Advanced,
        );
        assert!(
            rm.run_state.pending_card_grants.is_empty(),
            "grant consumed after `pick` picks",
        );
        // Deck got exactly the two picked cards, in pick order; Defend (the
        // un-picked original index 1) was discarded.
        let deck: Vec<_> = rm.run_state.master_deck.iter().map(|c| c.card_id).collect();
        assert_eq!(
            deck,
            vec![
                crate::cards::CardId::StrikeIronclad,
                crate::cards::CardId::Bash
            ],
        );
    }

    #[test]
    fn smith_card_without_pending_choice_is_invalid() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::RestSite);
        let res = rm.take_action(RunAction::SmithCard { idx: 0 });
        assert_eq!(res, ActionResult::InvalidForPhase);
    }

    #[test]
    fn smith_single_upgradable_card_still_auto_resolves_no_sub_state() {
        // Only one upgradable card → auto-upgrades inside
        // do_rest_site_action, returns SmithUpgraded (terminal). The
        // SmithCard surface should never appear in this path.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state.master_deck.clear();
        rm.run_state.master_deck.push(crate::Card {
            uid: crate::CardUid::new(1).expect("nz"),
            card_id: crate::cards::CardId::StrikeIronclad,
            ..crate::Card::pod_defaults()
        });
        rm.set_phase(Phase::RestSite);
        let outcome =
            rm.do_rest_site_action(crate::runstate::RestSiteOption::Smith { enabled: true });
        assert!(matches!(
            outcome,
            crate::runstate::RestSiteActionOutcome::SmithUpgraded { .. }
        ));
        assert_eq!(rm.phase(), Phase::MapChoice);
        assert!(rm.run_state.pending_choice.is_none());
        assert!(rm.run_state.master_deck[0].upgraded);
    }

    // ── W3: Shop dispatch — bead sts2-rs-if6ry ──────────────────────

    /// Hand-build a minimal `ShopInventory` for engine-side tests
    /// (independent of the content-side payload roller, which is
    /// deferred). Has one affordable character card, one unaffordable
    /// rare card, one sold-out relic slot, one affordable potion, and
    /// a fresh removal slot. Card / relic_id / potion_kind payloads
    /// are concrete so `buy_*` doesn't return `SlotEmpty`.
    fn make_test_shop_inventory() -> crate::shop::ShopInventory {
        use crate::card_rewards::RewardRarity;
        use crate::shop::{ShopCardEntry, ShopPotionEntry, ShopRelicEntry, ShopRelicRarity};
        crate::shop::ShopInventory {
            cards: vec![
                ShopCardEntry {
                    rarity: RewardRarity::Common,
                    card_type: crate::cards::CardType::Attack,
                    card: Some(crate::Card {
                        uid: crate::CardUid::new(101).expect("nz"),
                        card_id: crate::cards::CardId::StrikeIronclad,
                        ..crate::Card::pod_defaults()
                    }),
                    is_colorless: false,
                    on_sale: false,
                    price: 50, // affordable on starter 99 gold
                },
                ShopCardEntry {
                    rarity: RewardRarity::Rare,
                    card_type: crate::cards::CardType::Attack,
                    card: Some(crate::Card {
                        uid: crate::CardUid::new(102).expect("nz"),
                        card_id: crate::cards::CardId::DefendIronclad,
                        ..crate::Card::pod_defaults()
                    }),
                    is_colorless: false,
                    on_sale: false,
                    price: 999, // unaffordable
                },
            ],
            colorless_cards: vec![],
            relics: vec![ShopRelicEntry {
                relic_rarity: ShopRelicRarity::Shop,
                relic_id: Some(crate::RelicId::Anchor),
                // Sold-out sentinel — must be filtered from enumeration.
                price: crate::shop::SHOP_ENTRY_SOLD_OUT_PRICE,
            }],
            potions: vec![ShopPotionEntry {
                potion_rarity: crate::shop::ShopPotionRarity::Common,
                potion_kind: Some(crate::potions::PotionKind::BlockPotion),
                price: 40,
            }],
            removal_cost: 75,
            removal_used: false,
        }
    }

    fn make_rm_in_shop_phase_with_inventory(inv: crate::shop::ShopInventory) -> RunManager {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        // RunManager::new leaves master_deck empty (the starter loader
        // lives in sts2-content). Seed a small deck so the
        // ShopRemoveCard surface has something to enumerate (v0.107.1:
        // every non-Eternal card is offered, basics included).
        for uid in 200..208u32 {
            rm.run_state.master_deck.push(crate::Card {
                uid: crate::CardUid::new(uid).expect("nz"),
                card_id: crate::cards::CardId::Anger,
                ..crate::Card::pod_defaults()
            });
        }
        rm.set_phase(Phase::Shop);
        rm.shop_inventory = Some(inv);
        rm
    }

    #[test]
    fn shop_with_no_inventory_falls_back_to_leave_shop() {
        // Skip-only behavior preserved when the callback isn't wired.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.set_phase(Phase::Shop);
        assert!(rm.shop_inventory().is_none());
        assert_eq!(rm.get_available_actions(), vec![RunAction::LeaveShop]);
    }

    #[test]
    fn shop_enumerates_only_affordable_in_stock_items_plus_leave() {
        let rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        let actions = rm.get_available_actions();
        // Affordable card 0 ✓, card 1 (999 gold) ✗, relic (sold-out) ✗,
        // potion 0 (40 gold) ✓, removal slot for each master_deck idx
        // (starter Ironclad deck has 10 cards), then LeaveShop.
        let deck_len = rm.run_state.master_deck.len();
        let mut expected: Vec<RunAction> = vec![
            RunAction::ShopBuyCard { idx: 0 },
            RunAction::ShopBuyPotion { idx: 0 },
        ];
        for deck_idx in 0..deck_len {
            expected.push(RunAction::ShopRemoveCard { deck_idx });
        }
        expected.push(RunAction::LeaveShop);
        assert_eq!(actions, expected);
    }

    #[test]
    fn shop_potion_row_not_surfaced_when_belt_full() {
        // GH #270: `buy_potion` refuses on a full belt with zero state
        // change and `do_shop_buy_potion` maps that refusal to
        // `Advanced` — a surfaced action that no-ops livelocks
        // deterministic greedy policies. The surface must drop the
        // potion row when the purchase is guaranteed to refuse.
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        rm.run_state.max_potion_slots = 2;
        rm.run_state.potions = vec![crate::potions::PotionKind::BlockPotion; 2];
        let actions = rm.get_available_actions();
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, RunAction::ShopBuyPotion { .. })),
            "full belt must not surface ShopBuyPotion: {actions:?}",
        );
        // Other rows are unaffected.
        assert!(actions.contains(&RunAction::ShopBuyCard { idx: 0 }));
        assert!(actions.contains(&RunAction::LeaveShop));
    }

    #[test]
    fn shop_potion_row_not_surfaced_under_procure_veto() {
        // GH #270: with Sozu held, `procure_potion` is vetoed via the
        // `should_procure_potion` channel, so every ShopBuyPotion is a
        // guaranteed no-op — PPO argmax repeated the identical buy until
        // the stall watchdog killed the run (8/999 corpus livelocks).
        // The real Sozu listener lives in sts2-content; register a
        // stand-in with the same shape (relic-keyed, self-guarded).
        fn veto(
            rs: &mut crate::runstate::RunState,
            cancel: &mut bool,
        ) -> crate::card_effects::EffectResult {
            if !rs.relics.contains(&crate::RelicId::Sozu) {
                return crate::card_effects::EffectResult::NotImplemented;
            }
            *cancel = true;
            crate::card_effects::EffectResult::Resolved
        }
        crate::run_effects::register_should_procure_potion(
            crate::RelicId::Sozu,
            veto as crate::run_effects::ShouldProcurePotionRunFn,
        );
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        rm.run_state.relics.push(crate::RelicId::Sozu);
        let actions = rm.get_available_actions();
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, RunAction::ShopBuyPotion { .. })),
            "Sozu veto must not surface ShopBuyPotion: {actions:?}",
        );
        assert!(actions.contains(&RunAction::ShopBuyCard { idx: 0 }));
        assert!(actions.contains(&RunAction::LeaveShop));
        // Without the relic the same registered listener self-guards
        // and the row surfaces normally.
        let rm2 = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        assert!(rm2
            .get_available_actions()
            .contains(&RunAction::ShopBuyPotion { idx: 0 }));
    }

    #[test]
    fn shop_remove_card_action_offers_basics_in_offer_order() {
        // v0.107.1: shop removal is `FromDeckForRemoval` with no extra
        // filter — basic Strike / Defend ARE offered (the old
        // `sts2-rs-hrz2m` basic-card filter was a Python-pin rule; the
        // real DLL offers the whole deck, census seeds 1042/1045/1047/
        // 1055). Enumeration follows `removal_offer_order` (curse-front,
        // Eternal-excluded), which is plain deck order for this deck.
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        rm.run_state.master_deck.clear();
        let layout = [
            crate::cards::CardId::StrikeIronclad,
            crate::cards::CardId::Anger,
            crate::cards::CardId::DefendIronclad,
            crate::cards::CardId::Anger,
            crate::cards::CardId::StrikeIronclad,
        ];
        for (i, cid) in layout.iter().enumerate() {
            rm.run_state.master_deck.push(crate::Card {
                uid: crate::CardUid::new((300 + i) as u32).expect("nz"),
                card_id: *cid,
                ..crate::Card::pod_defaults()
            });
        }
        let removal_actions: Vec<usize> = rm
            .get_available_actions()
            .into_iter()
            .filter_map(|a| match a {
                RunAction::ShopRemoveCard { deck_idx } => Some(deck_idx),
                _ => None,
            })
            .collect();
        assert_eq!(removal_actions, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn shop_purchase_card_removal_allows_basic_strike() {
        // v0.107.1: `OneOffSynchronizer.DoMerchantCardRemoval` applies no
        // basic-card filter — deleting a starter Strike at the shop is the
        // standard deck-thinning line and must succeed at the primitive too.
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        rm.run_state.master_deck.clear();
        rm.run_state.master_deck.push(crate::Card {
            uid: crate::CardUid::new(400).expect("nz"),
            card_id: crate::cards::CardId::StrikeIronclad,
            ..crate::Card::pod_defaults()
        });
        let gold_before = rm.run_state.gold;
        let res = rm.take_action(RunAction::ShopRemoveCard { deck_idx: 0 });
        assert_eq!(res, ActionResult::Advanced);
        assert!(
            rm.run_state.master_deck.is_empty(),
            "the basic Strike is removed from the master deck"
        );
        assert!(rm.run_state.gold < gold_before, "removal cost debited");
        let inv = rm.shop_inventory().expect("inv present");
        assert!(inv.removal_used, "slot consumed");
    }

    #[test]
    fn shop_buy_card_debits_gold_and_marks_slot_sold_out() {
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        let gold_before = rm.run_state.gold;
        let deck_len_before = rm.run_state.master_deck.len();
        let res = rm.take_action(RunAction::ShopBuyCard { idx: 0 });
        assert_eq!(res, ActionResult::Advanced);
        assert_eq!(rm.run_state.gold, gold_before - 50);
        assert_eq!(rm.run_state.master_deck.len(), deck_len_before + 1);
        // Slot 0 marked sold-out → no longer enumerated.
        let actions = rm.get_available_actions();
        assert!(!actions
            .iter()
            .any(|a| matches!(a, RunAction::ShopBuyCard { idx: 0 })));
    }

    /// With `TheCourier` owned, a card buy (1) charges the ×0.8-discounted
    /// price and (2) fires the restock callback for the bought slot instead
    /// of leaving it sold-out — mirroring C# `MerchantEntry.Cost` +
    /// `ShouldRefillMerchantEntry` → `RestockAfterPurchase`. The content-layer
    /// restock RNG re-roll is exercised by the `spec_index` tests; here a stub
    /// restock proves the engine plumbing (gating on ownership + callback
    /// dispatch + slot overwrite).
    #[test]
    fn shop_buy_card_with_courier_discounts_and_restocks_slot() {
        fn stub_restock(
            _rs: &mut RunState,
            inv: &mut crate::shop::ShopInventory,
            slot: crate::shop::ShopSlot,
        ) {
            // Marker: a re-stocked (available) price proves the callback ran
            // and overwrote the sold-out sentinel `buy_card` had just set.
            if let crate::shop::ShopSlot::Card(i) = slot {
                inv.cards[i].price = 42;
            }
        }
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        rm.callbacks.restock_shop_entry = Some(stub_restock);
        rm.run_state.relics.push(crate::RelicId::TheCourier);
        let gold_before = rm.run_state.gold;

        let res = rm.take_action(RunAction::ShopBuyCard { idx: 0 });
        assert_eq!(res, ActionResult::Advanced);
        // Courier 20% off the 50g base: `(int)(50 * 0.8m)` = 40.
        assert_eq!(
            rm.run_state.gold,
            gold_before - 40,
            "Courier discount applied at buy time",
        );
        let inv = rm.shop_inventory.as_ref().expect("shop inventory present");
        assert_eq!(
            inv.cards[0].price, 42,
            "restock callback re-stocked the slot"
        );
        assert!(
            crate::shop::is_shop_entry_available(inv.cards[0].price),
            "restocked slot is available, not the sold-out sentinel",
        );
    }

    #[test]
    fn shop_buy_card_unaffordable_returns_invalid_and_does_not_debit() {
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        rm.run_state.gold = 30; // can't afford the 50-gold card
        let gold_before = rm.run_state.gold;
        let res = rm.take_action(RunAction::ShopBuyCard { idx: 0 });
        assert_eq!(
            res,
            ActionResult::ShopPurchaseFailed(crate::shop::ShopPurchaseOutcome::CannotAfford),
        );
        assert_eq!(rm.run_state.gold, gold_before);
    }

    #[test]
    fn shop_buy_potion_adds_to_belt_and_debits_gold() {
        // `buy_potion` routes through `procure_potion` (queue on
        // `pending_rewards`), and `do_shop_buy_potion` then drains that
        // queue so the potion lands in the held belt immediately — a
        // shop never reaches a reward screen that would otherwise drain
        // it, so without the drain the purchase would be invisible.
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        let gold_before = rm.run_state.gold;
        let pending_before = rm.run_state.pending_rewards.len();
        let res = rm.take_action(RunAction::ShopBuyPotion { idx: 0 });
        assert_eq!(res, ActionResult::Advanced);
        assert_eq!(rm.run_state.gold, gold_before - 40);
        // Drained into the belt — pending queue back to its baseline.
        assert_eq!(rm.run_state.pending_rewards.len(), pending_before);
        assert_eq!(
            rm.run_state.potions,
            vec![crate::potions::PotionKind::BlockPotion],
        );
    }

    #[test]
    fn shop_remove_card_pops_master_deck_entry_and_consumes_slot() {
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        let gold_before = rm.run_state.gold;
        let deck_before = rm.run_state.master_deck.clone();
        let removal_cost = rm.shop_inventory().expect("inv").removal_cost;
        let target_uid = deck_before[2].uid;
        let res = rm.take_action(RunAction::ShopRemoveCard { deck_idx: 2 });
        assert_eq!(res, ActionResult::Advanced);
        assert_eq!(rm.run_state.gold, gold_before - removal_cost);
        assert_eq!(rm.run_state.master_deck.len(), deck_before.len() - 1);
        assert!(rm.run_state.master_deck.iter().all(|c| c.uid != target_uid));
        assert!(rm.shop_inventory().expect("inv").removal_used);
        // Second removal must be rejected.
        let res2 = rm.take_action(RunAction::ShopRemoveCard { deck_idx: 0 });
        assert_eq!(
            res2,
            ActionResult::ShopRemoveCardFailed(crate::shop::PurchaseCardRemovalResult::AlreadyUsed),
        );
    }

    #[test]
    fn shop_leave_clears_inventory_and_transitions_to_map_choice() {
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        let res = rm.take_action(RunAction::LeaveShop);
        assert_eq!(res, ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::MapChoice);
        assert!(rm.shop_inventory().is_none());
    }

    #[test]
    fn shop_buy_actions_invalid_outside_shop_phase() {
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        rm.set_phase(Phase::MapChoice);
        assert_eq!(
            rm.take_action(RunAction::ShopBuyCard { idx: 0 }),
            ActionResult::InvalidForPhase,
        );
        assert_eq!(
            rm.take_action(RunAction::ShopBuyRelic { idx: 0 }),
            ActionResult::InvalidForPhase,
        );
        assert_eq!(
            rm.take_action(RunAction::ShopBuyPotion { idx: 0 }),
            ActionResult::InvalidForPhase,
        );
        assert_eq!(
            rm.take_action(RunAction::ShopRemoveCard { deck_idx: 0 }),
            ActionResult::InvalidForPhase,
        );
    }

    #[test]
    fn shop_buy_card_oob_idx_is_invalid() {
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        let res = rm.take_action(RunAction::ShopBuyCard { idx: 99 });
        assert_eq!(
            res,
            ActionResult::ShopPurchaseFailed(crate::shop::ShopPurchaseOutcome::IndexOutOfBounds),
        );
    }

    // ── sts2-rs-8he0t: shop failure-mode surfacing ──────────────────

    #[test]
    fn shop_buy_card_sold_out_returns_typed_failure() {
        // Slot 0 of a fresh inventory is in-stock; pre-poison its price
        // with the sold-out sentinel so the buy attempt mirrors the
        // post-first-purchase state.
        let mut inv = make_test_shop_inventory();
        inv.cards[0].price = crate::shop::SHOP_ENTRY_SOLD_OUT_PRICE;
        let mut rm = make_rm_in_shop_phase_with_inventory(inv);
        let gold_before = rm.run_state.gold;
        let res = rm.take_action(RunAction::ShopBuyCard { idx: 0 });
        assert_eq!(
            res,
            ActionResult::ShopPurchaseFailed(crate::shop::ShopPurchaseOutcome::SoldOut),
        );
        assert_eq!(rm.run_state.gold, gold_before);
    }

    #[test]
    fn shop_buy_relic_oob_returns_typed_index_failure() {
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        let res = rm.take_action(RunAction::ShopBuyRelic { idx: 99 });
        assert_eq!(
            res,
            ActionResult::ShopPurchaseFailed(crate::shop::ShopPurchaseOutcome::IndexOutOfBounds),
        );
    }

    #[test]
    fn shop_buy_potion_oob_returns_typed_index_failure() {
        // Closes the last OOB-coverage gap flagged by code review of
        // 24b5cb9 — ShopBuyCard, ShopBuyRelic, and ShopRemoveCard
        // already have analogous OOB tests; this locks the same
        // contract for the potion variant. Bead `sts2-rs-faxtx`.
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        let res = rm.take_action(RunAction::ShopBuyPotion { idx: 99 });
        assert_eq!(
            res,
            ActionResult::ShopPurchaseFailed(crate::shop::ShopPurchaseOutcome::IndexOutOfBounds),
        );
    }

    #[test]
    fn shop_remove_card_insufficient_gold_returns_typed_failure() {
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        rm.run_state.gold = 0; // can't afford the 75-gold removal
        let res = rm.take_action(RunAction::ShopRemoveCard { deck_idx: 0 });
        assert_eq!(
            res,
            ActionResult::ShopRemoveCardFailed(
                crate::shop::PurchaseCardRemovalResult::InsufficientGold,
            ),
        );
    }

    #[test]
    fn shop_remove_card_oob_deck_idx_returns_typed_failure() {
        let mut rm = make_rm_in_shop_phase_with_inventory(make_test_shop_inventory());
        let res = rm.take_action(RunAction::ShopRemoveCard { deck_idx: 9999 });
        assert_eq!(
            res,
            ActionResult::ShopRemoveCardFailed(
                crate::shop::PurchaseCardRemovalResult::InvalidDeckIndex,
            ),
        );
    }

    // ── A3 Poverty — bead sts2-rs-6v9x1 ─────────────────────────────

    #[test]
    fn ascension_a2_keeps_full_starting_gold() {
        let rm = RunManager::with_ascension(0, CharacterId::Ironclad, 2);
        assert_eq!(rm.run_state.gold, 99);
    }

    #[test]
    fn ascension_a3_poverty_keeps_starting_gold_at_99() {
        // C# starting gold is the flat `CharacterModel.StartingGold = 99`
        // (`Player.cs:226`); Poverty scales only REWARD gold
        // (`EncounterModel.cs:52-57`, `OneOffSynchronizer.cs:110-114`).
        // The old 74 expectation came from the fictional Python-pin
        // starting-gold multiply (2026-06-12 ascension audit).
        let rm = RunManager::with_ascension(0, CharacterId::Ironclad, 3);
        assert_eq!(rm.run_state.gold, 99);
    }

    #[test]
    fn ascension_a10_starting_gold_still_99() {
        let rm = RunManager::with_ascension(0, CharacterId::Silent, 10);
        assert_eq!(rm.run_state.gold, 99);
    }

    #[test]
    fn ascension_a3_poverty_truncates_treasure_gold() {
        // C# `DoTreasureRoomRewards`: roll NextInt(42, 53) then × 0.75,
        // truncated — A3 chest gold lands in [31, 39]
        // (`OneOffSynchronizer.cs:104-117`).
        let mut rm = RunManager::with_ascension(7, CharacterId::Ironclad, 3);
        let mut shared = std::collections::BTreeMap::new();
        shared.insert(
            crate::runstate::RelicRarity::Common,
            vec![RelicId::BurningBlood],
        );
        shared.insert(
            crate::runstate::RelicRarity::Uncommon,
            vec![RelicId::Nunchaku],
        );
        shared.insert(
            crate::runstate::RelicRarity::Rare,
            vec![RelicId::RoyalPoison],
        );
        rm.run_state.shared_relic_grab_bag_by_rarity = shared;
        let before = rm.run_state.gold;
        assert_eq!(rm.resolve_treasure_room(), ActionResult::Advanced);
        let gained = rm.run_state.gold - before;
        assert!(
            (31..=39).contains(&gained),
            "A3 chest gold {gained} outside C# truncated range [31, 39]"
        );
    }

    // ── A4 TightBelt — bead sts2-rs-dms9f ───────────────────────────

    #[test]
    fn ascension_a3_keeps_full_potion_slots() {
        let rm = RunManager::with_ascension(0, CharacterId::Ironclad, 3);
        assert_eq!(rm.run_state.max_potion_slots, 3);
    }

    #[test]
    fn ascension_a4_tight_belt_drops_one_potion_slot() {
        let rm = RunManager::with_ascension(0, CharacterId::Ironclad, 4);
        assert_eq!(rm.run_state.max_potion_slots, 2);
    }

    #[test]
    fn ascension_a10_potion_slots_floor_is_one_drop() {
        // -1 only, not a per-level decrement.
        let rm = RunManager::with_ascension(0, CharacterId::Defect, 10);
        assert_eq!(rm.run_state.max_potion_slots, 2);
    }

    // ── A5 AscendersBane — bead sts2-rs-9vypz ───────────────────────

    fn count_ascenders_bane(rm: &RunManager) -> usize {
        rm.run_state
            .master_deck
            .iter()
            .filter(|c| c.card_id == crate::cards::CardId::AscendersBane)
            .count()
    }

    #[test]
    fn ascension_a4_does_not_add_ascenders_bane() {
        let rm = RunManager::with_ascension(0, CharacterId::Ironclad, 4);
        assert_eq!(count_ascenders_bane(&rm), 0);
    }

    #[test]
    fn ascension_a5_appends_one_ascenders_bane() {
        let rm = RunManager::with_ascension(0, CharacterId::Ironclad, 5);
        assert_eq!(count_ascenders_bane(&rm), 1);
        let curse = rm
            .run_state
            .master_deck
            .iter()
            .find(|c| c.card_id == crate::cards::CardId::AscendersBane)
            .expect("curse present");
        // Spec fields land on the Card POD (rest come from the
        // CardId lookup in `sts2-content`).
        assert_eq!(curse.cost, -1);
        assert_eq!(curse.card_type, crate::cards::CardType::Curse);
        assert_eq!(curse.target_type, crate::cards::TargetType::None);
        assert!(!curse.upgraded);
    }

    #[test]
    fn ascension_a10_appends_exactly_one_ascenders_bane() {
        // One curse only — not per-level.
        let rm = RunManager::with_ascension(0, CharacterId::Silent, 10);
        assert_eq!(count_ascenders_bane(&rm), 1);
    }

    // ── A7 Scarcity — bead sts2-rs-brq2l ────────────────────────────

    #[test]
    fn ascension_a6_card_rarity_odds_below_scarcity_threshold() {
        let rm = RunManager::with_ascension(0, CharacterId::Ironclad, 6);
        assert_eq!(rm.run_state.card_rarity_odds.ascension_level, 6);
        // Below SCARCITY_ASCENSION_LEVEL=7: roller matches the
        // canonical base-level roller, not the scarcity one.
        assert_eq!(
            rm.run_state.card_rarity_odds,
            crate::card_rewards::CardRarityOdds::new(6)
        );
        assert_ne!(
            rm.run_state.card_rarity_odds,
            crate::card_rewards::CardRarityOdds::new(crate::card_rewards::SCARCITY_ASCENSION_LEVEL)
        );
    }

    #[test]
    fn ascension_a7_flips_card_rarity_odds_to_scarcity() {
        let rm = RunManager::with_ascension(0, CharacterId::Ironclad, 7);
        assert_eq!(rm.run_state.card_rarity_odds.ascension_level, 7);
        // Whole-struct equality covers the scarcity-vs-base
        // *_ODDS_SCARCITY tables + rarity_growth swap.
        assert_eq!(
            rm.run_state.card_rarity_odds,
            crate::card_rewards::CardRarityOdds::new(7)
        );
        assert_ne!(
            rm.run_state.card_rarity_odds,
            crate::card_rewards::CardRarityOdds::new(0)
        );
    }

    #[test]
    fn ascension_a10_keeps_scarcity_ascension_level() {
        let rm = RunManager::with_ascension(0, CharacterId::Defect, 10);
        assert_eq!(rm.run_state.card_rarity_odds.ascension_level, 10);
    }

    // ── A10 DoubleBoss — bead sts2-rs-oyzw5 ─────────────────────────

    #[test]
    fn ascension_a9_does_not_set_double_boss() {
        let rm = RunManager::with_ascension(0, CharacterId::Ironclad, 9);
        assert!(!rm.run_state.has_double_boss);
    }

    #[test]
    fn ascension_a10_sets_double_boss() {
        let rm = RunManager::with_ascension(0, CharacterId::Ironclad, 10);
        assert!(rm.run_state.has_double_boss);
    }

    #[test]
    fn ascension_base_does_not_set_double_boss() {
        let rm = RunManager::new(0, CharacterId::Silent);
        assert!(!rm.run_state.has_double_boss);
    }

    /// A10 map-gen: the FINAL act's map carries the second-boss node
    /// (Boss-typed, one row above the boss, the boss's only child —
    /// C# `StandardActMap.cs:79-82`/`:208-210`); act 1 never does.
    #[test]
    fn ascension_a10_final_act_map_has_second_boss_node() {
        let mut rm = RunManager::with_ascension(7, CharacterId::Ironclad, 10);
        let act1_map = rm.run_state.current_act_map.as_ref().expect("act-1 map");
        assert_eq!(
            act1_map.second_boss, None,
            "act 1 is not the final act — no second boss"
        );

        rm.run_state.act_index = 3;
        rm.run_state.generate_map();
        let map = rm.run_state.current_act_map.as_ref().expect("act-3 map");
        let sb = map.second_boss.expect("A10 act-3 map has a second boss");
        let boss = map.boss.expect("boss node");
        let boss_pt = map.point(boss);
        let sb_pt = map.point(sb);
        assert_eq!(sb_pt.point_type, crate::map::MapPointType::Boss);
        assert_eq!(sb_pt.coord.col, boss_pt.coord.col, "directly above");
        assert_eq!(sb_pt.coord.row, boss_pt.coord.row + 1, "one row up");
        assert!(
            boss_pt.children.contains(&sb),
            "second boss is the boss node's child (back-to-back fights)"
        );
    }

    #[test]
    fn ascension_a9_final_act_map_has_no_second_boss_node() {
        let mut rm = RunManager::with_ascension(7, CharacterId::Ironclad, 9);
        rm.run_state.act_index = 3;
        rm.run_state.generate_map();
        let map = rm.run_state.current_act_map.as_ref().expect("act-3 map");
        assert_eq!(map.second_boss, None, "below A10 there is no second boss");
    }

    /// A10 run flow: winning the act-3 boss at the FIRST boss node does
    /// NOT end the run — full boss rewards, then back toward the map so
    /// the player can walk boss → second boss (C#
    /// `NRewardsScreen.cs:449-456`); winning at the SECOND boss node
    /// stamps the victory (`CombatManager.cs:801-806`).
    #[test]
    fn double_boss_first_act3_boss_win_is_not_run_over() {
        let mut rm = RunManager::with_ascension(0, CharacterId::Ironclad, 10);
        rm.run_state.act_index = 3;
        rm.run_state.generate_map();
        let (boss_coord, second_coord) = {
            let map = rm.run_state.current_act_map.as_ref().expect("map");
            (
                map.point(map.boss.expect("boss")).coord,
                map.point(map.second_boss.expect("second boss")).coord,
            )
        };
        // Stand on the FIRST boss node with a won combat.
        let mut room = crate::runstate::Room::new(crate::runstate::RoomType::Boss);
        room.map_coord = Some(boss_coord);
        rm.run_state.current_room = Some(room);
        rm.combat = Some(crate::CombatState {
            is_over: true,
            player_won: true,
            ..crate::CombatState::default()
        });
        rm.set_phase(Phase::Combat);
        let _ = rm.exit_combat_and_transition();
        assert_ne!(rm.phase(), Phase::RunOver, "second boss still pending");
        assert!(!rm.run_state.is_over);
        assert_eq!(
            rm.run_state.act_index, 3,
            "no act advance between the two bosses"
        );

        // Now stand on the SECOND boss node with a won combat.
        let mut room = crate::runstate::Room::new(crate::runstate::RoomType::Boss);
        room.map_coord = Some(second_coord);
        rm.run_state.current_room = Some(room);
        rm.combat = Some(crate::CombatState {
            is_over: true,
            player_won: true,
            ..crate::CombatState::default()
        });
        rm.set_phase(Phase::Combat);
        let _ = rm.exit_combat_and_transition();
        assert_eq!(rm.phase(), Phase::RunOver, "second boss win ends the run");
        assert!(rm.run_state.is_over);
        assert!(rm.run_state.player_won);
    }

    /// Below A10 the act-3 boss win stays terminal even with a map
    /// coord attached (regression guard for the awaiting-second-boss
    /// gate).
    #[test]
    fn single_boss_act3_win_with_coord_is_run_over() {
        let mut rm = RunManager::with_ascension(0, CharacterId::Ironclad, 9);
        rm.run_state.act_index = 3;
        rm.run_state.generate_map();
        let boss_coord = {
            let map = rm.run_state.current_act_map.as_ref().expect("map");
            map.point(map.boss.expect("boss")).coord
        };
        let mut room = crate::runstate::Room::new(crate::runstate::RoomType::Boss);
        room.map_coord = Some(boss_coord);
        rm.run_state.current_room = Some(room);
        rm.combat = Some(crate::CombatState {
            is_over: true,
            player_won: true,
            ..crate::CombatState::default()
        });
        rm.set_phase(Phase::Combat);
        let _ = rm.exit_combat_and_transition();
        assert_eq!(rm.phase(), Phase::RunOver);
        assert!(rm.run_state.player_won);
    }

    // ── enter_treasure — bead sts2-rs-ycria ─────────────────────────

    #[test]
    fn enter_treasure_sets_phase_to_treasure() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        let mut common = vec![RelicId::Akabeko];
        let mut uncommon = vec![];
        let mut rare = vec![];
        let _ = rm.enter_treasure(&mut common, &mut uncommon, &mut rare);
        assert_eq!(rm.phase(), Phase::Treasure);
    }

    /// Bead `sts2-rs-wt4w1`: picking a relic reward through the native
    /// run loop must fire the `after_obtained` channel (it previously
    /// did a raw `relics.push`, silently no-op'ing on-obtain listeners).
    /// Exercises both `do_relic_reward_pick` and `do_boss_relic_pick`.
    #[test]
    fn native_relic_picks_fire_after_obtained() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{register_after_obtained, AfterObtainedRunFn};
        use std::sync::atomic::{AtomicUsize, Ordering};

        // The `after_obtained` registry is a process-global keyed by
        // RelicId (register overwrites). Use relics that no other
        // sts2-core test registers a listener on or grants, so this test
        // is parallel-safe (DataDisk / FestivePopper appear only in enum
        // + name defs — never contended).
        static REWARD_HITS: AtomicUsize = AtomicUsize::new(0);
        static BOSS_HITS: AtomicUsize = AtomicUsize::new(0);
        fn reward_listener(rs: &mut RunState) -> EffectResult {
            if rs.last_obtained_relic == Some(RelicId::DataDisk) {
                REWARD_HITS.fetch_add(1, Ordering::SeqCst);
            }
            EffectResult::Resolved
        }
        fn boss_listener(rs: &mut RunState) -> EffectResult {
            if rs.last_obtained_relic == Some(RelicId::FestivePopper) {
                BOSS_HITS.fetch_add(1, Ordering::SeqCst);
            }
            EffectResult::Resolved
        }
        register_after_obtained(RelicId::DataDisk, reward_listener as AfterObtainedRunFn);
        register_after_obtained(RelicId::FestivePopper, boss_listener as AfterObtainedRunFn);

        // Relic-reward path.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.run_state
            .relic_rewards
            .push(crate::RelicReward::new(RelicId::DataDisk));
        let before = REWARD_HITS.load(Ordering::SeqCst);
        let _ = rm.do_relic_reward_pick();
        assert!(rm.run_state.relics.contains(&RelicId::DataDisk));
        assert_eq!(
            REWARD_HITS.load(Ordering::SeqCst),
            before + 1,
            "do_relic_reward_pick must fire after_obtained"
        );

        // Boss-relic path.
        let mut rm2 = RunManager::new(0, CharacterId::Ironclad);
        rm2.boss_relic_options = vec![RelicId::FestivePopper];
        let before_boss = BOSS_HITS.load(Ordering::SeqCst);
        let _ = rm2.do_boss_relic_pick(0);
        assert!(rm2.run_state.relics.contains(&RelicId::FestivePopper));
        assert_eq!(
            BOSS_HITS.load(Ordering::SeqCst),
            before_boss + 1,
            "do_boss_relic_pick must fire after_obtained"
        );
    }

    /// Regression (Silent:1018, card_reward-vs-map): an Ancient boss relic
    /// whose `after_obtained` queues follow-up CARD rewards via
    /// `RewardsCmd.OfferCustom` (Glass Eye, `GlassEye.cs:15-33`) must SURFACE
    /// those rewards as a `Phase::CardReward` after the act advances — not
    /// silently drop them and jump to the map. `do_boss_relic_pick` used to
    /// drain only `relic_rewards` (Toy Box wax relics) and then unconditionally
    /// set `Phase::MapChoice`, orphaning the 5 queued `Reward::Card` sentinels.
    #[test]
    fn boss_relic_pick_surfaces_queued_card_rewards_from_after_obtained() {
        use crate::run_effects::{register_after_obtained, AfterObtainedRunFn};
        use crate::runstate::{CardReward, Reward};

        // GlassEye is otherwise unregistered/ungranted in sts2-core tests, so
        // overriding its process-global `after_obtained` listener here is
        // parallel-safe. The stub mirrors `listener_glass_eye` (sts2-content):
        // push empty Card sentinels onto `pending_rewards` on obtain.
        fn glass_eye_stub(rs: &mut RunState) -> crate::card_effects::EffectResult {
            if rs.last_obtained_relic == Some(RelicId::GlassEye) {
                for _ in 0..5 {
                    rs.pending_rewards.push(Reward::Card(CardReward::empty()));
                }
            }
            crate::card_effects::EffectResult::Resolved
        }
        register_after_obtained(RelicId::GlassEye, glass_eye_stub as AfterObtainedRunFn);

        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(drain_test_callbacks());
        rm.boss_relic_options = vec![RelicId::GlassEye];

        let result = rm.do_boss_relic_pick(0);

        assert_eq!(result, ActionResult::Advanced);
        assert!(rm.run_state.relics.contains(&RelicId::GlassEye));
        assert_eq!(
            rm.phase(),
            Phase::CardReward,
            "queued card rewards must surface as a card-reward screen, not jump to the map"
        );
        // One sentinel materialized into a 3-card offering; the other four
        // remain queued for subsequent picks.
        assert_eq!(rm.card_reward_options.len(), 3);
        assert_eq!(rm.run_state.pending_rewards.len(), 4);
        assert!(
            rm.get_available_actions()
                .iter()
                .any(|a| matches!(a, RunAction::PickCardReward { .. })),
            "a PickCardReward action must be offered"
        );
    }

    #[test]
    fn boss_relic_act_snapshot_captures_the_new_act() {
        use crate::run_effects::{register_after_obtained, AfterObtainedRunFn};

        // Act-snapshot boss relics — FurCoat, GoldenCompass, SpoilsMap —
        // record `CurrentActIndex` in `after_obtained` and are active only
        // while `CurrentActIndex == snapshot`. C# obtains the boss relic via
        // the NEW act's opening Ancient event (`EnterAct` sets
        // `CurrentActIndex` before the grant), so the snapshot must name the
        // act we advance INTO, not the act we just cleared. Pre-fix, the obtain
        // dispatched before the advance and snapshotted the OLD act, so the
        // relic was dead the instant we entered the act it was obtained in
        // (a Regent A5 run-parity divergence). Stub mirrors sts2-content's
        // `listener_fur_coat` (which lives in a sibling crate, so it is
        // unregistered here): store `rs.act_index` into `relic_counters` on
        // obtain. Process-global registration is parallel-safe under nextest
        // (one process per test).
        //
        // (PumpkinCandle used to be in this act-snapshot family but its
        // v0.107.1 redesign replaced the act gate with a decaying KindleCount
        // — see `listener_pumpkin_candle`; it is no longer a valid example.)
        fn fur_coat_stub(rs: &mut RunState) -> crate::card_effects::EffectResult {
            if rs.last_obtained_relic == Some(RelicId::FurCoat) {
                rs.relic_counters
                    .insert(RelicId::FurCoat, i32::from(rs.act_index));
            }
            crate::card_effects::EffectResult::Resolved
        }
        register_after_obtained(RelicId::FurCoat, fur_coat_stub as AfterObtainedRunFn);

        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(drain_test_callbacks());
        rm.run_state.act_index = 1;
        rm.run_state.current_room =
            Some(crate::runstate::Room::new(crate::runstate::RoomType::Boss));
        rm.boss_relic_options = vec![RelicId::FurCoat];

        let result = rm.do_boss_relic_pick(0);

        assert_eq!(result, ActionResult::Advanced);
        assert!(rm.run_state.relics.contains(&RelicId::FurCoat));
        assert_eq!(
            rm.run_state.act_index, 2,
            "the act must have advanced 1 -> 2"
        );
        assert_eq!(
            rm.run_state.relic_counters.get(&RelicId::FurCoat),
            Some(&2),
            "boss-relic act snapshot must capture the act it is obtained in (the NEW act, 2)"
        );
    }

    #[test]
    fn enter_treasure_offers_picked_relic_via_relic_rewards() {
        // Populate every pool so any rolled rarity finds a candidate.
        // The starter relic (BurningBlood) is in rs.relics, so the
        // owned-skip path doesn't pollute.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        let mut common = vec![RelicId::Akabeko];
        let mut uncommon = vec![RelicId::BloodVial];
        let mut rare = vec![RelicId::Sozu];
        let picked = rm
            .enter_treasure(&mut common, &mut uncommon, &mut rare)
            .expect("at least one pool has a candidate");
        assert_eq!(rm.run_state.relic_rewards.len(), 1);
        assert_eq!(rm.run_state.relic_rewards[0].relic_id, picked);
    }

    #[test]
    fn collect_treasure_opens_relic_reward_screen_then_grants_on_take() {
        // Regression: CollectTreasure used to silently advance to the map,
        // either dropping the relic or auto-granting with no on-screen feedback.
        // It now opens the relic-reward screen (CardReward) so the player sees
        // the relic; taking it (PickRelicReward) grants it.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        let mut common = vec![RelicId::Akabeko];
        let mut uncommon = vec![RelicId::BloodVial];
        let mut rare = vec![RelicId::Sozu];
        let picked = rm
            .enter_treasure(&mut common, &mut uncommon, &mut rare)
            .expect("a candidate");
        assert_eq!(
            rm.take_action(RunAction::CollectTreasure),
            ActionResult::Advanced
        );
        assert_eq!(
            rm.phase(),
            Phase::CardReward,
            "chest opens the relic-reward screen"
        );
        assert!(
            !rm.run_state.relics.contains(&picked),
            "not granted until taken"
        );
        // The reward screen offers taking the relic.
        assert!(rm
            .get_available_actions()
            .contains(&RunAction::PickRelicReward));
        assert_eq!(
            rm.take_action(RunAction::PickRelicReward),
            ActionResult::Advanced
        );
        assert!(
            rm.run_state.relics.contains(&picked),
            "relic granted on take"
        );
    }

    #[test]
    fn enter_treasure_drains_picked_relic_from_pool() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        let mut common = vec![RelicId::Akabeko];
        let mut uncommon = vec![RelicId::BloodVial];
        let mut rare = vec![RelicId::Sozu];
        let before_total = common.len() + uncommon.len() + rare.len();
        let _ = rm.enter_treasure(&mut common, &mut uncommon, &mut rare);
        let after_total = common.len() + uncommon.len() + rare.len();
        assert_eq!(
            after_total,
            before_total - 1,
            "exactly one pool entry drained"
        );
    }

    #[test]
    fn enter_treasure_returns_none_when_all_pools_exhausted() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        let mut common = vec![];
        let mut uncommon = vec![];
        let mut rare = vec![];
        let picked = rm.enter_treasure(&mut common, &mut uncommon, &mut rare);
        assert!(picked.is_none());
        assert!(rm.run_state.relic_rewards.is_empty());
        // Phase still transitions even on empty pools — matches
        // Python's `_current_reward = None` branch.
        assert_eq!(rm.phase(), Phase::Treasure);
    }

    #[test]
    fn enter_treasure_skips_already_owned_relics() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        // Ironclad already owns BurningBlood from the starter pack;
        // we additionally seed every pool with BurningBlood so the
        // skip-owned path is exercised. Akabeko is unowned and
        // should be the only candidate that can be returned.
        rm.run_state.relics.push(RelicId::Akabeko);
        let mut common = vec![RelicId::BurningBlood, RelicId::Akabeko];
        let mut uncommon = vec![RelicId::BurningBlood];
        let mut rare = vec![RelicId::BurningBlood];
        let picked = rm.enter_treasure(&mut common, &mut uncommon, &mut rare);
        // Akabeko is owned now too, so all candidates are owned. None.
        assert!(picked.is_none());
        assert!(rm.run_state.relic_rewards.is_empty());
    }

    #[test]
    fn enter_treasure_falls_through_to_higher_rarities_per_python_pin() {
        // Force the COMMON path by populating only Common, and verify
        // that on the rolls that land in Uncommon / Rare buckets the
        // pool walk still finds the Common entry as a fallback per
        // `_relic_grab_bag_order(Common) = [Common, Uncommon, Rare]`.
        // We don't control the roll, so we instead test the empty-
        // target case: Common pool empty + only Rare populated must
        // still return Some(Rare's entry) IF rolled was Common.
        //
        // The bounded version: with ONLY Rare populated, the picker
        // must return that entry no matter the roll — Rare's chain
        // is [Rare], Uncommon's is [Uncommon, Rare], Common's is
        // [Common, Uncommon, Rare]; all three include Rare.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        let mut common = vec![];
        let mut uncommon = vec![];
        let mut rare = vec![RelicId::Akabeko];
        let picked = rm.enter_treasure(&mut common, &mut uncommon, &mut rare);
        assert_eq!(picked, Some(RelicId::Akabeko));
    }

    #[test]
    fn enter_treasure_is_deterministic_under_same_seed() {
        let mk = || {
            let mut rm = RunManager::new(7, CharacterId::Ironclad);
            // Reset the rng to a clean RunRngSet seed to avoid
            // CharacterStarter side effects on the rewards stream.
            rm.run_state.rng = crate::run_rng_set::RunRngSet::new(7);
            let common = vec![RelicId::Akabeko, RelicId::Anchor];
            let uncommon = vec![RelicId::BloodVial];
            let rare = vec![RelicId::Sozu];
            (rm, common, uncommon, rare)
        };
        let (mut rm_a, mut ca, mut ua, mut ra) = mk();
        let (mut rm_b, mut cb, mut ub, mut rb) = mk();
        let pa = rm_a.enter_treasure(&mut ca, &mut ua, &mut ra);
        let pb = rm_b.enter_treasure(&mut cb, &mut ub, &mut rb);
        assert_eq!(pa, pb, "same seed → same chosen relic");
        assert_eq!(ca, cb);
        assert_eq!(ua, ub);
        assert_eq!(ra, rb);
    }

    // ── roll_treasure_relic_rarity — bead sts2-rs-ycria ─────────────

    #[test]
    fn roll_treasure_relic_rarity_uses_treasure_room_stream_deterministically() {
        let mk = || crate::runstate::RunState {
            rng: crate::run_rng_set::RunRngSet::new(0x1234),
            ..crate::runstate::RunState::default()
        };
        let mut a = mk();
        let mut b = mk();
        let mut seq_a = Vec::new();
        let mut seq_b = Vec::new();
        for _ in 0..32 {
            seq_a.push(a.roll_treasure_relic_rarity());
            seq_b.push(b.roll_treasure_relic_rarity());
        }
        assert_eq!(seq_a, seq_b, "same seed → identical rarity sequence");
        // Sanity: across 32 rolls at seed 0x1234 we should see at
        // least one Common (50% bucket dominates). A purely random
        // distribution would yield Common ≥1 with prob ≈1.
        use crate::card_rewards::RewardRarity;
        assert!(seq_a.contains(&RewardRarity::Common));
    }

    // ── advance_act_on_boss_kill — bead sts2-rs-7uzk3 ───────────────

    /// Build a CombatState in the post-boss-kill shape: `is_over =
    /// true`, `player_won = true`. Everything else stays at the
    /// default sentinels.
    fn combat_after_boss_win() -> CombatState {
        CombatState {
            is_over: true,
            player_won: true,
            ..CombatState::default()
        }
    }

    /// Seed the run-side current room to Boss so the guard passes.
    fn enter_boss_room(rm: &mut RunManager) {
        rm.run_state
            .enter_room(crate::runstate::Room::new(crate::runstate::RoomType::Boss));
    }

    #[test]
    fn advance_act_on_boss_kill_advances_and_regens_map() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        enter_boss_room(&mut rm);
        assert_eq!(rm.run_state.act_index, 1, "default act = 1");
        let map = rm
            .advance_act_on_boss_kill(&combat_after_boss_win(), 0)
            .expect("guards pass → Some(map)");
        assert_eq!(rm.run_state.act_index, 2, "act 1 → 2");
        assert!(map.start.is_some(), "regenerated map has a start node");
        assert!(map.boss.is_some(), "regenerated map has a boss node");
    }

    #[test]
    fn advance_act_on_boss_kill_act_2_to_act_3() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.run_state.act_index = 2;
        enter_boss_room(&mut rm);
        let map = rm
            .advance_act_on_boss_kill(&combat_after_boss_win(), 0)
            .expect("guards pass");
        assert_eq!(rm.run_state.act_index, 3);
        assert!(map.start.is_some());
    }

    #[test]
    fn advance_act_on_boss_kill_returns_none_at_act_3() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.run_state.act_index = 3;
        enter_boss_room(&mut rm);
        let result = rm.advance_act_on_boss_kill(&combat_after_boss_win(), 0);
        assert!(result.is_none(), "act 3 → final-boss handling in 8i1kn");
        assert_eq!(rm.run_state.act_index, 3, "act_index unchanged");
    }

    #[test]
    fn advance_act_on_boss_kill_returns_none_when_combat_not_over() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        enter_boss_room(&mut rm);
        let combat = CombatState {
            is_over: false,
            player_won: true,
            ..CombatState::default()
        };
        let result = rm.advance_act_on_boss_kill(&combat, 0);
        assert!(result.is_none(), "is_over=false blocks advance");
        assert_eq!(rm.run_state.act_index, 1);
    }

    #[test]
    fn advance_act_on_boss_kill_returns_none_when_player_lost() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        enter_boss_room(&mut rm);
        let combat = CombatState {
            is_over: true,
            player_won: false,
            ..CombatState::default()
        };
        let result = rm.advance_act_on_boss_kill(&combat, 0);
        assert!(result.is_none(), "player_won=false blocks advance");
        assert_eq!(rm.run_state.act_index, 1);
    }

    #[test]
    fn advance_act_on_boss_kill_returns_none_when_not_boss_room() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.run_state.enter_room(crate::runstate::Room::new(
            crate::runstate::RoomType::Monster,
        ));
        let result = rm.advance_act_on_boss_kill(&combat_after_boss_win(), 0);
        assert!(result.is_none(), "Monster room blocks advance");
        assert_eq!(rm.run_state.act_index, 1);
    }

    #[test]
    fn advance_act_on_boss_kill_returns_none_when_no_current_room() {
        // Fresh RunManager has no current_room set.
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        assert!(rm.run_state.current_room.is_none());
        let result = rm.advance_act_on_boss_kill(&combat_after_boss_win(), 0);
        assert!(result.is_none(), "no current_room blocks advance");
        assert_eq!(rm.run_state.act_index, 1);
    }

    #[test]
    fn advance_act_on_boss_kill_fires_after_act_entered_hook() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{register_after_act_entered, AfterActEnteredRunFn};
        use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

        static FIRED: AtomicUsize = AtomicUsize::new(0);
        static LAST_ACT: AtomicI32 = AtomicI32::new(0);
        fn listener(rs: &mut RunState, act_index: u8) -> EffectResult {
            // Ownership gate — without this, every sibling test that
            // fires `dispatch_after_act_entered` (act-advance flows,
            // map-regen tests, …) increments our COUNTER and races
            // the baseline+1 assertion.
            if !rs.relics.contains(&crate::RelicId::LavaRock) {
                return EffectResult::NotImplemented;
            }
            FIRED.fetch_add(1, Ordering::SeqCst);
            LAST_ACT.store(i32::from(act_index), Ordering::SeqCst);
            EffectResult::Resolved
        }
        register_after_act_entered(crate::RelicId::LavaRock, listener as AfterActEnteredRunFn);

        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.run_state.relics.push(crate::RelicId::LavaRock);
        enter_boss_room(&mut rm);
        let baseline = FIRED.load(Ordering::SeqCst);
        let _ = rm.advance_act_on_boss_kill(&combat_after_boss_win(), 0);
        assert!(
            FIRED.load(Ordering::SeqCst) > baseline,
            "after_act_entered listener fired"
        );
        assert_eq!(
            LAST_ACT.load(Ordering::SeqCst),
            2,
            "listener received post-advance act_index"
        );
    }

    // ── carry_to_next_act — bead sts2-rs-oo78v ──────────────────────

    #[test]
    fn carry_to_next_act_preserves_run_persistent_state() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        // Seed run-persistent surfaces with non-default sentinel values
        // so we can assert they're untouched by carry_to_next_act.
        rm.run_state.current_hp = 42;
        rm.run_state.max_hp = 64;
        rm.run_state.gold = 137;
        rm.run_state.relics.push(RelicId::Akabeko);
        rm.run_state.relic_counters.insert(RelicId::Akabeko, 3);
        let mut card = crate::cards::Card::pod_defaults();
        card.card_id = crate::cards::CardId::DefendIronclad;
        rm.run_state.master_deck.push(card);
        let deck_before = rm.run_state.master_deck.clone();
        let relics_before = rm.run_state.relics.clone();
        let counters_before = rm.run_state.relic_counters.clone();

        rm.carry_to_next_act();

        assert_eq!(rm.run_state.current_hp, 42);
        assert_eq!(rm.run_state.max_hp, 64);
        assert_eq!(rm.run_state.gold, 137);
        assert_eq!(rm.run_state.master_deck, deck_before);
        assert_eq!(rm.run_state.relics, relics_before);
        assert_eq!(rm.run_state.relic_counters, counters_before);
    }

    #[test]
    fn carry_to_next_act_clears_combat_slot() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        rm.combat = Some(CombatState::default());
        assert!(rm.combat.is_some());
        rm.carry_to_next_act();
        assert!(rm.combat.is_none(), "combat-only state cleared");
    }

    #[test]
    fn carry_to_next_act_clears_current_room() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        enter_boss_room(&mut rm);
        assert!(rm.run_state.current_room.is_some());
        rm.carry_to_next_act();
        assert!(
            rm.run_state.current_room.is_none(),
            "boss room cleared; next-act start node landed by map driver"
        );
    }

    #[test]
    fn carry_to_next_act_resets_unknown_map_point_odds() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        let base_snapshot = rm.run_state.unknown_map_point_odds.clone();
        // Mutate `current` away from base via the public field so the
        // reset_to_base call has an observable effect.
        for odds in rm.run_state.unknown_map_point_odds.current.values_mut() {
            *odds += 99.0;
        }
        assert_ne!(
            rm.run_state.unknown_map_point_odds, base_snapshot,
            "mutation made odds drift from base"
        );
        rm.carry_to_next_act();
        assert_eq!(
            rm.run_state.unknown_map_point_odds, base_snapshot,
            "carry_to_next_act restored odds to base via reset_to_base"
        );
    }

    #[test]
    fn advance_act_on_boss_kill_clears_combat_and_current_room() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        enter_boss_room(&mut rm);
        rm.combat = Some(CombatState::default());
        let _ = rm.advance_act_on_boss_kill(&combat_after_boss_win(), 0);
        // After act transition, combat slot is cleared and current_room
        // is None (waiting for the new-act map driver to place the
        // player on the start node).
        assert!(rm.combat.is_none(), "combat cleared post-act");
        assert!(
            rm.run_state.current_room.is_none(),
            "current_room cleared post-act"
        );
    }

    #[test]
    fn advance_act_on_boss_kill_heals_player_to_full_between_acts() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        enter_boss_room(&mut rm);
        // Arrive at the act boss badly hurt.
        rm.run_state.current_hp = 25;
        let max = rm.run_state.max_hp;
        let _ = rm.advance_act_on_boss_kill(&combat_after_boss_win(), 0);
        assert_eq!(
            rm.run_state.current_hp, max,
            "entering the next act's Ancient event heals to full at base ascension"
        );
    }

    #[test]
    fn advance_act_on_boss_kill_weary_traveler_heals_80pct() {
        // A2 WearyTraveler run: only 80% of missing HP recovers between acts.
        let mut rm = RunManager::with_ascension(11, CharacterId::Ironclad, 2);
        enter_boss_room(&mut rm);
        rm.run_state.max_hp = 80;
        rm.run_state.current_hp = 30; // missing 50 → +floor(40) → 70.
        let _ = rm.advance_act_on_boss_kill(&combat_after_boss_win(), 2);
        assert_eq!(rm.run_state.current_hp, 70);
    }

    #[test]
    fn with_ascension_weary_traveler_starts_run_at_80pct_hp() {
        // A2 Neow zeroes HP then heals 0.8*MaxHp → run begins at 80%.
        let base = RunManager::new(1, CharacterId::Ironclad);
        let max = base.run_state.max_hp;
        let a2 = RunManager::with_ascension(1, CharacterId::Ironclad, 2);
        assert_eq!(
            a2.run_state.current_hp,
            (f64::from(max) * 0.8).floor() as i32,
            "A2 starts at 80% max HP"
        );
        // Base ascension still starts at full.
        assert_eq!(base.run_state.current_hp, max);
    }

    #[test]
    fn advance_act_on_boss_kill_preserves_deck_and_relics() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        enter_boss_room(&mut rm);
        let mut card = crate::cards::Card::pod_defaults();
        card.card_id = crate::cards::CardId::DefendIronclad;
        rm.run_state.master_deck.push(card);
        rm.run_state.relics.push(RelicId::Akabeko);
        let deck_before = rm.run_state.master_deck.clone();
        let relics_before = rm.run_state.relics.clone();
        let _ = rm.advance_act_on_boss_kill(&combat_after_boss_win(), 0);
        assert_eq!(rm.run_state.master_deck, deck_before);
        assert_eq!(rm.run_state.relics, relics_before);
    }

    #[test]
    fn advance_act_on_boss_kill_is_deterministic_under_same_seed() {
        let build = || {
            let mut rm = RunManager::new(99, CharacterId::Ironclad);
            // Reset the RNG bundle to a clean seed so the rewards
            // stream draws from the CharacterStarter don't desync.
            rm.run_state.rng = crate::run_rng_set::RunRngSet::new(99);
            enter_boss_room(&mut rm);
            rm
        };
        let mut rm_a = build();
        let mut rm_b = build();
        let map_a = rm_a
            .advance_act_on_boss_kill(&combat_after_boss_win(), 0)
            .expect("guards pass");
        let map_b = rm_b
            .advance_act_on_boss_kill(&combat_after_boss_win(), 0)
            .expect("guards pass");
        assert_eq!(map_a.points.len(), map_b.points.len());
        assert_eq!(map_a.start, map_b.start);
        assert_eq!(map_a.boss, map_b.boss);
        // Spot-check a few points are byte-identical.
        for (a, b) in map_a.points.iter().zip(map_b.points.iter()) {
            assert_eq!(a.coord, b.coord);
            assert_eq!(a.point_type, b.point_type);
            assert_eq!(a.children, b.children);
        }
    }

    // ── finish_run_on_final_boss_kill — bead sts2-rs-8i1kn ──────────

    #[test]
    fn finish_run_on_final_boss_kill_sets_terminal_flags_at_act_3() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.run_state.act_index = 3;
        enter_boss_room(&mut rm);
        assert!(!rm.run_state.is_over, "run starts in-progress");
        assert!(!rm.run_state.player_won);
        assert_ne!(rm.phase(), Phase::RunOver);

        let finished = rm.finish_run_on_final_boss_kill(&combat_after_boss_win());
        assert!(finished, "act 3 boss-win → run terminates");
        assert!(rm.run_state.is_over);
        assert!(rm.run_state.player_won);
        assert_eq!(rm.phase(), Phase::RunOver);
        assert!(rm.is_over());
    }

    #[test]
    fn finish_run_on_final_boss_kill_no_op_at_act_1_or_2() {
        for act in [1u8, 2u8] {
            let mut rm = RunManager::new(7, CharacterId::Ironclad);
            rm.run_state.act_index = act;
            enter_boss_room(&mut rm);
            let finished = rm.finish_run_on_final_boss_kill(&combat_after_boss_win());
            assert!(
                !finished,
                "act {act} → advance_act_on_boss_kill handles it, not 8i1kn"
            );
            assert!(!rm.run_state.is_over);
            assert!(!rm.run_state.player_won);
            assert_ne!(rm.phase(), Phase::RunOver);
        }
    }

    #[test]
    fn finish_run_on_final_boss_kill_no_op_when_combat_not_over() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.run_state.act_index = 3;
        enter_boss_room(&mut rm);
        let combat = CombatState {
            is_over: false,
            player_won: true,
            ..CombatState::default()
        };
        assert!(!rm.finish_run_on_final_boss_kill(&combat));
        assert!(!rm.run_state.is_over);
    }

    #[test]
    fn finish_run_on_final_boss_kill_no_op_when_player_lost() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.run_state.act_index = 3;
        enter_boss_room(&mut rm);
        let combat = CombatState {
            is_over: true,
            player_won: false,
            ..CombatState::default()
        };
        assert!(!rm.finish_run_on_final_boss_kill(&combat));
        assert!(!rm.run_state.is_over);
        assert!(!rm.run_state.player_won);
    }

    #[test]
    fn finish_run_on_final_boss_kill_no_op_when_not_boss_room() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.run_state.act_index = 3;
        rm.run_state.enter_room(crate::runstate::Room::new(
            crate::runstate::RoomType::Monster,
        ));
        assert!(!rm.finish_run_on_final_boss_kill(&combat_after_boss_win()));
        assert!(!rm.run_state.is_over);
    }

    #[test]
    fn finish_run_on_final_boss_kill_no_op_when_no_current_room() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.run_state.act_index = 3;
        assert!(rm.run_state.current_room.is_none());
        assert!(!rm.finish_run_on_final_boss_kill(&combat_after_boss_win()));
        assert!(!rm.run_state.is_over);
    }

    #[test]
    fn finish_run_on_final_boss_kill_pairs_with_advance_act_on_boss_kill() {
        // The two methods partition the act-boss-clear space: act 1/2
        // → advance_act_on_boss_kill returns Some(map); act 3 →
        // finish_run_on_final_boss_kill returns true. Verify both
        // sides of the partition at act 3.
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.run_state.act_index = 3;
        enter_boss_room(&mut rm);
        // act 3 boss-win → advance_act_on_boss_kill is a no-op (None)
        // since the final-boss terminal lives in 8i1kn, not 7uzk3.
        let advance = rm.advance_act_on_boss_kill(&combat_after_boss_win(), 0);
        assert!(advance.is_none(), "act 3 → advance returns None");
        assert_eq!(rm.run_state.act_index, 3, "act_index unchanged");
        // …and finish_run_on_final_boss_kill picks up the slack.
        let finished = rm.finish_run_on_final_boss_kill(&combat_after_boss_win());
        assert!(finished, "act 3 → finish_run_on_final_boss_kill terminates");
    }

    // ── enter_rest_site / do_rest_site_action — bead sts2-rs-76mdy ──

    #[test]
    fn enter_rest_site_transitions_phase_and_offers_base_options() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        let options = rm.enter_rest_site();
        assert_eq!(rm.phase(), Phase::RestSite);
        // Base option list: Rest + Smith. Channel listeners may add
        // more, but the two canonical entries always lead.
        assert!(options
            .iter()
            .any(|o| matches!(o, crate::runstate::RestSiteOption::Rest)));
        assert!(options
            .iter()
            .any(|o| matches!(o, crate::runstate::RestSiteOption::Smith { .. })));
    }

    #[test]
    fn do_rest_site_action_rest_heals_and_returns_to_map_choice() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        // Bring HP below max so the heal has room to bind.
        rm.run_state.current_hp = 10;
        let options = rm.enter_rest_site();
        let rest = options
            .into_iter()
            .find(|o| matches!(o, crate::runstate::RestSiteOption::Rest))
            .expect("Rest offered");
        let outcome = rm.do_rest_site_action(rest);
        assert!(matches!(
            outcome,
            crate::runstate::RestSiteActionOutcome::Rested { healed } if healed > 0
        ));
        assert_eq!(rm.phase(), Phase::MapChoice);
    }

    #[test]
    fn do_rest_site_action_disabled_still_exits_phase() {
        // Smith with `enabled: false` resolves to Disabled; the room
        // is still consumed and the run advances to MapChoice.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.phase = Phase::RestSite;
        let outcome =
            rm.do_rest_site_action(crate::runstate::RestSiteOption::Smith { enabled: false });
        assert_eq!(outcome, crate::runstate::RestSiteActionOutcome::Disabled);
        assert_eq!(rm.phase(), Phase::MapChoice);
    }

    #[test]
    fn do_rest_site_action_unsupported_variant_advances_phase() {
        // Deferred variants (Dig/Lift/etc.) surface as Unsupported but
        // still complete the rest-site visit — the engine doesn't
        // strand the run on a partially-ported handler.
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        rm.phase = Phase::RestSite;
        let outcome = rm.do_rest_site_action(crate::runstate::RestSiteOption::Dig);
        assert_eq!(outcome, crate::runstate::RestSiteActionOutcome::Unsupported);
        assert_eq!(rm.phase(), Phase::MapChoice);
    }

    #[test]
    fn do_rest_site_action_smith_awaiting_choice_keeps_rest_site_phase() {
        // With ≥2 upgradable cards Smith opens a pending_choice; the
        // phase stays at RestSite until the caller resolves the
        // upgrade out-of-band.
        use crate::cards::{Card, CardId};
        let mut rm = RunManager::new(0, CharacterId::Ironclad);
        // Replace the deck with a known-upgradable trio so the
        // assertion doesn't depend on the starter-deck pull's upgrade
        // flags.
        rm.run_state.master_deck.clear();
        for id in [CardId::Bash, CardId::PommelStrike, CardId::Anger] {
            rm.run_state.master_deck.push(Card {
                card_id: id,
                upgraded: false,
                ..Card::pod_defaults()
            });
        }
        rm.enter_rest_site();
        let outcome =
            rm.do_rest_site_action(crate::runstate::RestSiteOption::Smith { enabled: true });
        assert_eq!(
            outcome,
            crate::runstate::RestSiteActionOutcome::SmithAwaitingChoice
        );
        assert_eq!(rm.phase(), Phase::RestSite);
        assert!(rm.run_state.pending_choice.is_some());
    }

    // ----- 2h627: Unknown-room resolver at room entry -----

    #[test]
    fn enter_room_for_point_concrete_types_map_directly() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        for (pt, rt) in [
            (
                crate::map::MapPointType::Monster,
                crate::runstate::RoomType::Monster,
            ),
            (
                crate::map::MapPointType::Elite,
                crate::runstate::RoomType::Elite,
            ),
            (
                crate::map::MapPointType::Boss,
                crate::runstate::RoomType::Boss,
            ),
            (
                crate::map::MapPointType::Shop,
                crate::runstate::RoomType::Shop,
            ),
            (
                crate::map::MapPointType::RestSite,
                crate::runstate::RoomType::RestSite,
            ),
            (
                crate::map::MapPointType::Treasure,
                crate::runstate::RoomType::Treasure,
            ),
            (
                crate::map::MapPointType::Ancient,
                crate::runstate::RoomType::Event,
            ),
        ] {
            let got = rm.enter_room_for_point(pt);
            assert_eq!(got, rt, "{pt:?} → {rt:?}");
            assert_eq!(
                rm.run_state.current_room.map(|r| r.room_type),
                Some(rt),
                "current_room mirrors entered type"
            );
        }
    }

    #[test]
    fn treasure_room_auto_resolves_shared_bag_relic_plus_gold() {
        // C# (BUG-016-patched headless) treasure flow: rarity float on the
        // treasure stream + SHARED-bag front pull + Rewards gold int +
        // direct obtain — no decision surface, straight back to the map.
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        let mut shared = std::collections::BTreeMap::new();
        shared.insert(
            crate::runstate::RelicRarity::Common,
            vec![RelicId::BurningBlood],
        );
        shared.insert(
            crate::runstate::RelicRarity::Uncommon,
            vec![RelicId::Nunchaku],
        );
        shared.insert(
            crate::runstate::RelicRarity::Rare,
            vec![RelicId::RoyalPoison],
        );
        rm.run_state.shared_relic_grab_bag_by_rarity = shared;
        let gold_before = rm.run_state.gold;
        let relics_before = rm.run_state.relics.len();
        assert_eq!(rm.resolve_treasure_room(), ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::MapChoice, "no decision surface");
        assert_eq!(
            rm.run_state.relics.len(),
            relics_before + 1,
            "chest relic granted directly"
        );
        assert!(rm.run_state.relic_rewards.is_empty());
        let gained = rm.run_state.gold - gold_before;
        assert!(
            (42..=52).contains(&gained),
            "chest gold {gained} in [42,52]"
        );
        // The granted relic is drained from the shared bag.
        let granted = *rm.run_state.relics.last().expect("relic granted");
        assert!(!rm
            .run_state
            .shared_relic_grab_bag_by_rarity
            .values()
            .any(|b| b.contains(&granted)));
    }

    #[test]
    fn treasure_room_without_shared_bag_falls_back_to_legacy_phase() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        assert_eq!(rm.resolve_treasure_room(), ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::Treasure);
    }

    #[test]
    fn enter_room_for_point_unknown_resolves_via_roller() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        let got = rm.enter_room_for_point(crate::map::MapPointType::Unknown);
        // Resolver yields one of the candidate set
        // (Monster/Treasure/Shop/Event) — never `RoomType::Unknown`
        // (that's the engine-side catch-all) and never `Elite` (base
        // odds -1, no modifier-boost path lands today).
        assert!(
            matches!(
                got,
                crate::runstate::RoomType::Monster
                    | crate::runstate::RoomType::Treasure
                    | crate::runstate::RoomType::Shop
                    | crate::runstate::RoomType::Event
            ),
            "resolved RoomType must be in candidate set, got {got:?}"
        );
        assert_eq!(rm.run_state.current_room.map(|r| r.room_type), Some(got));
    }

    #[test]
    fn enter_room_for_point_unknown_is_deterministic_under_seed() {
        // Two RunManagers with the same seed and no listeners
        // registered into the modify_unknown_map_point_room_types
        // channel between them should yield the same resolved type.
        // (The channel registry is global, so we can't assert
        // listener-set neutrality from inside a unit test; this test
        // tolerates contamination as long as it's stable across the
        // two adjacent calls.)
        let mut a = RunManager::new(91, CharacterId::Ironclad);
        let mut b = RunManager::new(91, CharacterId::Ironclad);
        let ra = a.enter_room_for_point(crate::map::MapPointType::Unknown);
        let rb = b.enter_room_for_point(crate::map::MapPointType::Unknown);
        assert_eq!(ra, rb, "same seed → same resolved room type");
    }

    #[test]
    fn enter_room_for_point_sets_current_room_via_enter_room() {
        // `enter_room` is the channel that fires `after_room_entered`.
        // Sanity-check the dispatcher path lands the right RoomType
        // payload: no current_room beforehand, entry returns a definite
        // type, current_room mirrors it.
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        assert!(rm.run_state.current_room.is_none());
        let got = rm.enter_room_for_point(crate::map::MapPointType::Shop);
        assert_eq!(got, crate::runstate::RoomType::Shop);
        assert_eq!(
            rm.run_state.current_room.map(|r| r.room_type),
            Some(crate::runstate::RoomType::Shop)
        );
    }

    // ----- otj0g: initialize_run orchestration -----

    #[test]
    fn initialize_run_populates_grab_bag_and_generates_map() {
        let mut rm = RunManager::new(42, CharacterId::Ironclad);
        let common = vec![crate::RelicId::Anchor];
        let uncommon: Vec<crate::RelicId> = vec![];
        let boss: Vec<crate::RelicId> = vec![];
        let act_map = rm.initialize_run(common.clone(), uncommon, boss);
        assert_eq!(rm.run_state.relic_grab_bag, common);
        assert!(rm.run_state.relic_grab_bag_fallback.is_empty());
        assert!(rm.run_state.boss_relic_pool.is_empty());
        assert!(act_map.start.is_some(), "map has a start node");
        assert!(act_map.boss.is_some(), "map has a boss node");
    }

    #[test]
    fn initialize_run_is_deterministic_under_seed() {
        let mut a = RunManager::new(91, CharacterId::Ironclad);
        let mut b = RunManager::new(91, CharacterId::Ironclad);
        let ma = a.initialize_run(Vec::new(), Vec::new(), Vec::new());
        let mb = b.initialize_run(Vec::new(), Vec::new(), Vec::new());
        // Map RNG is seeded from the master seed; same input → same map.
        assert_eq!(ma.start, mb.start);
        assert_eq!(ma.boss, mb.boss);
        assert_eq!(
            ma.points.iter().map(|p| p.point_type).collect::<Vec<_>>(),
            mb.points.iter().map(|p| p.point_type).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn initialize_run_fires_after_act_entered_with_starting_act_index() {
        use crate::card_effects::EffectResult;
        use crate::run_effects::{register_after_act_entered, AfterActEnteredRunFn};
        use std::sync::atomic::{AtomicU8, Ordering};
        static OBSERVED: AtomicU8 = AtomicU8::new(0);
        OBSERVED.store(0, Ordering::SeqCst);
        fn listener(_rs: &mut RunState, act_index: u8) -> EffectResult {
            OBSERVED.store(act_index, Ordering::SeqCst);
            EffectResult::Resolved
        }
        register_after_act_entered(crate::RelicId::LavaRock, listener as AfterActEnteredRunFn);

        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        // Ensure LavaRock is in the owner set so the listener actually
        // fires (the dispatcher only invokes listeners whose owner is
        // owned by the run).
        if !rm.run_state.relics.contains(&crate::RelicId::LavaRock) {
            rm.run_state.relics.push(crate::RelicId::LavaRock);
        }
        let starting_act = rm.run_state.act_index;
        let _ = rm.initialize_run(Vec::new(), Vec::new(), Vec::new());
        assert_eq!(
            OBSERVED.load(Ordering::SeqCst),
            starting_act,
            "after_act_entered fired with starting act_index"
        );
    }

    #[test]
    fn initialize_run_preserves_ascension_effects_from_constructor() {
        // Construction at asc=10 should set has_double_boss; initialize_run
        // must NOT clobber it (ascension effects are NOT re-applied here).
        let mut rm = RunManager::with_ascension(7, CharacterId::Ironclad, 10);
        assert!(rm.run_state.has_double_boss, "asc 10 → double boss at ctor");
        let _ = rm.initialize_run(Vec::new(), Vec::new(), Vec::new());
        assert!(
            rm.run_state.has_double_boss,
            "initialize_run preserves has_double_boss"
        );
    }

    // ----- wk1t6: _generate_rooms event_ids shuffle -----

    #[test]
    fn initialize_run_shuffles_event_ids_into_per_act_mirror() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        assert!(
            !rm.run_state.rooms_generated,
            "rooms_generated=false on a fresh RunState"
        );
        for act in &rm.run_state.acts {
            assert!(
                act.event_ids_shuffled.is_empty(),
                "fresh act has empty event_ids_shuffled"
            );
        }
        let _ = rm.initialize_run(Vec::new(), Vec::new(), Vec::new());
        assert!(rm.run_state.rooms_generated);
        for act in &rm.run_state.acts {
            assert_eq!(
                act.event_ids_shuffled.len(),
                act.event_ids.len(),
                "shuffle preserves length"
            );
            // Same multiset of event IDs (shuffle is a permutation).
            let mut got = act.event_ids_shuffled.clone();
            let mut want: Vec<&'static str> = act.event_ids.to_vec();
            got.sort_unstable();
            want.sort_unstable();
            assert_eq!(got, want, "shuffle preserves multiset");
            assert_eq!(act.events_visited, 0, "events_visited reset to 0");
        }
    }

    #[test]
    fn initialize_run_shuffle_is_deterministic_under_seed() {
        let mut a = RunManager::new(91, CharacterId::Ironclad);
        let mut b = RunManager::new(91, CharacterId::Ironclad);
        let _ = a.initialize_run(Vec::new(), Vec::new(), Vec::new());
        let _ = b.initialize_run(Vec::new(), Vec::new(), Vec::new());
        for (act_a, act_b) in a.run_state.acts.iter().zip(b.run_state.acts.iter()) {
            assert_eq!(
                act_a.event_ids_shuffled, act_b.event_ids_shuffled,
                "same seed → same shuffled order"
            );
        }
    }

    #[test]
    fn initialize_run_event_shuffle_is_idempotent_under_rooms_generated_guard() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        let _ = rm.initialize_run(Vec::new(), Vec::new(), Vec::new());
        let first: Vec<Vec<&'static str>> = rm
            .run_state
            .acts
            .iter()
            .map(|a| a.event_ids_shuffled.clone())
            .collect();
        // Simulate per-run mutable progress on events_visited; a
        // second initialize_run must NOT re-shuffle or zero
        // events_visited (the rooms_generated guard short-circuits).
        for act in &mut rm.run_state.acts {
            act.events_visited = 5;
        }
        let _ = rm.initialize_run(Vec::new(), Vec::new(), Vec::new());
        for (act, prev) in rm.run_state.acts.iter().zip(first.iter()) {
            assert_eq!(
                act.event_ids_shuffled, *prev,
                "shuffle untouched on re-init"
            );
            assert_eq!(act.events_visited, 5, "events_visited preserved on re-init");
        }
    }

    // -----------------------------------------------------------------
    // P10/sts2-rs-1jg54 — PAELS_WING sacrifice card reward → relic
    // every 2 sacrifices. Counter lives on RunState::pael_state.
    // -----------------------------------------------------------------

    #[test]
    fn paels_wing_sacrifice_action_only_surfaces_when_relic_owned() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        rm.phase = Phase::CardReward;
        rm.card_reward_options = vec![crate::cards::CardId::StrikeIronclad];
        // No PAELS_WING yet → no SacrificeCardReward in the menu.
        let acts = rm.get_available_actions();
        assert!(
            !acts.contains(&RunAction::SacrificeCardReward),
            "without PAELS_WING the sacrifice action must not surface"
        );
        // Give the relic → now it surfaces.
        rm.run_state.relics.push(RelicId::PaelsWing);
        let acts = rm.get_available_actions();
        assert!(
            acts.contains(&RunAction::SacrificeCardReward),
            "with PAELS_WING the sacrifice action surfaces in CardReward"
        );
    }

    #[test]
    fn paels_wing_sacrifice_every_second_pulls_relic_from_grab_bag() {
        let mut rm = RunManager::new(23, CharacterId::Ironclad);
        rm.phase = Phase::CardReward;
        rm.run_state.relics.push(RelicId::PaelsWing);
        // Seed a deterministic grab-bag — two common relics ready to
        // pop. The pop walks the common bucket first, so the order
        // observed here is the insertion order.
        rm.run_state.relic_grab_bag_by_rarity.insert(
            crate::RelicRarity::Common,
            vec![RelicId::Anchor, RelicId::BronzeScales],
        );
        let owned_at_start = rm.run_state.relics.len();
        // First sacrifice — counter goes 0→1, not divisible by 2; no
        // relic pull.
        rm.card_reward_options = vec![crate::cards::CardId::StrikeIronclad];
        assert_eq!(
            rm.take_action(RunAction::SacrificeCardReward),
            ActionResult::Advanced
        );
        assert_eq!(
            rm.run_state.relics.len(),
            owned_at_start,
            "no relic on sac #1"
        );
        // Second sacrifice — counter goes 1→2; relic pulled.
        rm.phase = Phase::CardReward;
        rm.card_reward_options = vec![crate::cards::CardId::StrikeIronclad];
        assert_eq!(
            rm.take_action(RunAction::SacrificeCardReward),
            ActionResult::Advanced
        );
        assert_eq!(
            rm.run_state.relics.len(),
            owned_at_start + 1,
            "relic pulled on sac #2"
        );
        // The added relic must be one of the seeded grab-bag entries
        // (and equal `last_obtained_relic`).
        let last = rm.run_state.last_obtained_relic.unwrap();
        assert!(
            matches!(last, RelicId::Anchor | RelicId::BronzeScales),
            "last_obtained_relic is one of the seeded grab-bag entries"
        );
    }

    #[test]
    fn paels_wing_sacrifice_clears_card_reward_options() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.phase = Phase::CardReward;
        rm.run_state.relics.push(RelicId::PaelsWing);
        rm.card_reward_options = vec![
            crate::cards::CardId::StrikeIronclad,
            crate::cards::CardId::DefendIronclad,
        ];
        let _ = rm.take_action(RunAction::SacrificeCardReward);
        assert!(
            rm.card_reward_options.is_empty(),
            "sacrifice consumes the offered cards"
        );
    }

    #[test]
    fn paels_wing_sacrifice_without_offered_cards_is_invalid() {
        let mut rm = RunManager::new(7, CharacterId::Ironclad);
        rm.phase = Phase::CardReward;
        rm.run_state.relics.push(RelicId::PaelsWing);
        // No card_reward_options set up.
        let r = rm.take_action(RunAction::SacrificeCardReward);
        assert_eq!(r, ActionResult::InvalidForPhase);
    }

    #[test]
    fn initialize_run_shuffle_uses_up_front_stream_not_map_stream() {
        // Determinism guarantee separate from map gen: shuffling
        // pulls from `rng.up_front` (per Python pin). To verify the
        // shuffle isn't accidentally bound to the map stream, two
        // runs with the same seed but differing map state should
        // still yield the same event order. We exercise this by
        // pre-rolling the map RNG between the two runs.
        let mut a = RunManager::new(13, CharacterId::Ironclad);
        let mut b = RunManager::new(13, CharacterId::Ironclad);
        // Burn some map RNG on `b` only. up_front must be untouched.
        let _ = b.run_state.rng.map_rng(0).next_float(1.0);
        let _ = b.run_state.rng.map_rng(0).next_float(1.0);
        let _ = a.initialize_run(Vec::new(), Vec::new(), Vec::new());
        let _ = b.initialize_run(Vec::new(), Vec::new(), Vec::new());
        for (act_a, act_b) in a.run_state.acts.iter().zip(b.run_state.acts.iter()) {
            assert_eq!(
                act_a.event_ids_shuffled, act_b.event_ids_shuffled,
                "shuffle isolated to up_front stream — map-stream churn doesn't affect it"
            );
        }
    }

    // ── Event→combat bridge routing (bead `sts2-rs-ck9z6`, GH #166) ──────
    //
    // These tests drive the engine-side routing (do_event_choice enters
    // Phase::Combat when an event handler stages `event_combat`;
    // exit_combat_and_transition resumes / grants the victory reward) with
    // lightweight test callbacks. The real content callbacks + the actual
    // BattleFriend fight are covered in sts2-content; the timer itself is
    // covered in `combat_loop::tests::battleworn_dummy_time_limit_expires_and_flees`.

    use crate::events::{BattlewornDummySetting, EventCombatRequest, EventEncounter, EventId};

    /// `resolve_event_at` test double: stages a resuming BattlewornDummy
    /// (Setting1) fight, leaving the event open (NOT finished).
    fn resolve_stages_resuming_dummy(rs: &mut RunState, _idx: usize) -> bool {
        rs.event_combat = Some(EventCombatRequest::resuming(
            EventEncounter::BattlewornDummy(BattlewornDummySetting::Setting1),
            EventId::BattlewornDummy,
        ));
        true
    }

    /// `resolve_event_at` test double: stages a no-resume MysteriousKnight
    /// fight whose victory reward is a gold grant (a card reward would need
    /// a roller callback; gold keeps the assertion simple).
    fn resolve_stages_no_resume_knight(rs: &mut RunState, _idx: usize) -> bool {
        rs.event_combat = Some(EventCombatRequest::no_resume(
            EventEncounter::MysteriousKnight,
            EventId::TheLanternKey,
            vec![crate::runstate::Reward::Gold { amount: 0 }],
        ));
        true
    }

    /// `build_event_combat` test double: returns a trivially-resolvable
    /// combat (no enemies → already winnable). The routing tests mark the
    /// outcome by mutating `rm.combat` directly before calling
    /// `exit_combat_and_transition`, so the combat's internals don't matter.
    fn build_trivial_combat(rs: &mut RunState, _req: &EventCombatRequest) -> CombatState {
        CombatState {
            run_rng: rs.rng.clone(),
            ..CombatState::default()
        }
    }

    /// `resume_event_after_combat` test double: on a clean victory, grant a
    /// 7-gold marker so the test can observe that resume ran with the right
    /// `clean_victory` flag.
    fn resume_grants_marker_gold(
        rs: &mut RunState,
        encounter: &EventEncounter,
        clean_victory: bool,
    ) {
        assert!(matches!(
            encounter,
            EventEncounter::BattlewornDummy(BattlewornDummySetting::Setting1)
        ));
        if clean_victory {
            rs.gain_gold(7);
        }
    }

    fn bridge_callbacks(resolve: fn(&mut RunState, usize) -> bool) -> ContentCallbacks {
        ContentCallbacks {
            resolve_event_at: Some(resolve),
            build_event_combat: Some(build_trivial_combat),
            resume_event_after_combat: Some(resume_grants_marker_gold),
            ..ContentCallbacks::default()
        }
    }

    #[test]
    fn event_choice_staging_combat_enters_combat_phase() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(bridge_callbacks(resolve_stages_resuming_dummy));
        rm.run_state.enter_event_room(EventId::BattlewornDummy);
        rm.set_phase(Phase::Event);

        let result = rm.take_action(RunAction::EventChoice { idx: 0 });
        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::Combat, "staged fight enters combat");
        assert!(rm.combat.is_some(), "combat built");
        assert!(
            rm.run_state.event_combat.is_some(),
            "request stays set across the fight for the resume path"
        );
        assert_eq!(
            rm.run_state.current_event,
            Some(EventId::BattlewornDummy),
            "parent event stays open across the nested combat"
        );
    }

    #[test]
    fn resuming_combat_victory_grants_reward_and_returns_to_map() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(bridge_callbacks(resolve_stages_resuming_dummy));
        rm.run_state.enter_event_room(EventId::BattlewornDummy);
        rm.set_phase(Phase::Event);
        let gold_before = rm.run_state.gold;
        rm.take_action(RunAction::EventChoice { idx: 0 });

        // Simulate a clean victory and resolve the fight.
        let combat = rm.combat.as_mut().expect("combat set");
        combat.player_won = true;
        combat.is_over = true;
        combat.event_combat_timed_out = false;
        let result = rm.exit_combat_and_transition();

        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(rm.run_state.gold, gold_before + 7, "resume granted reward");
        assert!(rm.run_state.event_combat.is_none(), "request cleared");
        assert!(rm.run_state.current_event.is_none(), "event closed");
        assert_eq!(rm.phase(), Phase::MapChoice);
    }

    #[test]
    fn resuming_combat_timeout_grants_no_reward() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(bridge_callbacks(resolve_stages_resuming_dummy));
        rm.run_state.enter_event_room(EventId::BattlewornDummy);
        rm.set_phase(Phase::Event);
        let gold_before = rm.run_state.gold;
        rm.take_action(RunAction::EventChoice { idx: 0 });

        // Timed-out dummy: player_won (it fled) BUT event_combat_timed_out.
        let combat = rm.combat.as_mut().expect("combat set");
        combat.player_won = true;
        combat.is_over = true;
        combat.event_combat_timed_out = true;
        rm.exit_combat_and_transition();

        assert_eq!(
            rm.run_state.gold, gold_before,
            "RanOutOfTime grants no reward"
        );
        assert!(rm.run_state.event_combat.is_none());
        assert_eq!(rm.phase(), Phase::MapChoice);
    }

    #[test]
    fn event_combat_player_death_ends_run() {
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(bridge_callbacks(resolve_stages_no_resume_knight));
        rm.run_state.enter_event_room(EventId::TheLanternKey);
        rm.set_phase(Phase::Event);
        rm.take_action(RunAction::EventChoice { idx: 0 });

        // The knight kills the player.
        let combat = rm.combat.as_mut().expect("combat set");
        combat.player_won = false;
        combat.is_over = true;
        let result = rm.exit_combat_and_transition();

        assert_eq!(result, ActionResult::Advanced);
        assert_eq!(rm.phase(), Phase::RunOver, "death in event combat ends run");
        assert!(!rm.run_state.player_won);
    }

    // -----------------------------------------------------------------
    // Live-state import: `debug_place_on_map` positioning.
    // -----------------------------------------------------------------

    /// A grid-live node one row up from the start, for a run at act 1.
    fn a_row1_coord(rm: &RunManager) -> crate::map::MapCoord {
        let map = rm
            .run_state
            .current_act_map
            .as_ref()
            .expect("act-1 map generated at run start");
        map.points_in_row(1)
            .first()
            .expect("act-1 map has a row-1 node")
            .coord
    }

    #[test]
    fn debug_place_on_map_parks_at_the_node_with_its_children_offered() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        let coord = a_row1_coord(&rm);

        rm.debug_place_on_map(1, coord, vec![coord], coord.row + 1, 1)
            .expect("placement on a live node succeeds");

        assert_eq!(rm.phase(), Phase::MapChoice);
        assert_eq!(
            rm.run_state.current_room.and_then(|r| r.map_coord),
            Some(coord)
        );
        assert_eq!(rm.run_state.act_floor, coord.row + 1);
        assert_eq!(rm.run_state.total_floor, 1);
        // The whole point of placing rather than teleporting blind: the
        // moves offered are this node's own children.
        let want: Vec<i32> = {
            let map = rm.run_state.current_act_map.as_ref().unwrap();
            let mut cols: Vec<i32> = map
                .children_of(coord)
                .iter()
                .filter(|p| map.grid.contains_key(&p.coord))
                .map(|p| p.col())
                .collect();
            cols.sort_unstable();
            cols
        };
        let got: Vec<i32> = rm
            .get_available_actions()
            .into_iter()
            .filter_map(|a| match a {
                RunAction::Move { col } => Some(col),
                _ => None,
            })
            .collect();
        assert_eq!(got, want, "placed run offers the node's own children");
    }

    #[test]
    fn debug_place_on_map_appends_the_node_to_visited() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        let coord = a_row1_coord(&rm);
        // Caller passed a visited list that does not name the node itself.
        rm.debug_place_on_map(1, coord, Vec::new(), coord.row + 1, 1)
            .expect("placement succeeds");
        assert_eq!(rm.run_state.visited_map_coords, vec![coord]);
    }

    #[test]
    fn debug_place_on_map_rejects_a_coord_that_is_not_on_the_map() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        let err = rm
            .debug_place_on_map(1, crate::map::MapCoord::new(99, 99), Vec::new(), 1, 1)
            .expect_err("a coord off the map is not a position");
        assert!(err.contains("not a live node"), "unexpected error: {err}");
    }

    #[test]
    fn debug_place_on_map_rejects_going_back_an_act() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        rm.run_state.advance_act();
        rm.run_state.generate_map();
        let err = rm
            .debug_place_on_map(1, crate::map::MapCoord::new(0, 1), Vec::new(), 1, 1)
            .expect_err("a run cannot be placed into an act it has left");
        assert!(err.contains("cannot go back"), "unexpected error: {err}");
    }

    #[test]
    fn debug_place_on_map_reaches_act_two_on_the_act_two_map() {
        let mut rm = RunManager::new(11, CharacterId::Ironclad);
        // The act-2 map is a pure function of (seed, act), so the node
        // placed onto must be one the run would itself have generated.
        let mut probe = RunManager::new(11, CharacterId::Ironclad);
        probe.run_state.advance_act();
        probe.run_state.generate_map();
        let coord = probe
            .run_state
            .current_act_map
            .as_ref()
            .unwrap()
            .points_in_row(1)
            .first()
            .unwrap()
            .coord;

        rm.debug_place_on_map(2, coord, vec![coord], coord.row + 1, 17)
            .expect("placement into act 2 succeeds");

        assert_eq!(rm.run_state.act_index, 2);
        assert_eq!(rm.run_state.total_floor, 17);
        assert_eq!(
            rm.run_state.current_room.and_then(|r| r.map_coord),
            Some(coord)
        );
    }
}
