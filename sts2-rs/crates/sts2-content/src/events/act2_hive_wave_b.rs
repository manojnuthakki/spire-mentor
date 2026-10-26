//! Act-2-pool events wave B (R-Z). Bead `sts2-rs-6ynt.4.7` ships 9
//! events from `sts2-rl-agent@01c094d:sts2_env/events/act2.py`:
//!
//! `RelicTrader`, `SlipperyBridge`, `SpiralingWhirlpool`,
//! `StoneOfAllTime`, `Symbiote`, `TheFutureOfPotions`,
//! `WaterloggedScriptorium`, `WelcomeToWongos`, `WhisperingHollow`.
//!
//! Wave A (`6ynt.4.6`) owns the alphabet-first half through
//! `RanwidTheElder`; the M–R boundary is bead-bookkeeping, not a clean
//! alphabet split.
//!
//! Port pin: `sts2-rl-agent@01c094d:sts2_env/events/act2.py:763`+
//! (`RelicTrader` through `WhisperingHollow`).
//!
//! ## Deferred surfaces
//!
//! **Status corrected 2026-06-26** (verified against current bodies; the
//! old `6ynt.4.*` tokens were never filed — remaining work lives under
//! umbrella `sts2-rs-m3n0g`):
//!
//! - **Act-gating**: **DONE.** Every `is_allowed` is wired
//!   (`allowed_act_2_plus`, `future_of_potions_allowed`, `relic_trader_allowed`,
//!   `slippery_bridge_allowed`, `stone_of_all_time_allowed`, `wongo_is_allowed`,
//!   `waterlogged_is_allowed`, `whispering_is_allowed`).
//! - **`lose_gold`**: works today via the `lose_gold_saturating` helper
//!   (this file); only a named `RunState::lose_gold` surface is outstanding.
//! - **`Reward::Potion`**: **LANDED** (used by `whispering_gold`). Only
//!   `TheFutureOfPotions` still queues a card placeholder (`sts2-rs-m3n0g.4`).
//! - **Deck mutation** (transform / enchant / downgrade): the surface
//!   (`pending_deck_{transforms,enchants,upgrades}`) EXISTS and some handlers
//!   use it (`whispering_hug`, `morphic_grove`); the leftover placeholder
//!   handlers just aren't wired (`sts2-rs-m3n0g.2`).
//! - **Multi-card-choice fan-out** (`request_card_choice`): still missing —
//!   `SpiralingWhirlpool.observe` / `Symbiote.*` / `Waterlogged*` queue card
//!   placeholders (was `6ynt.4.choice`; folded into `sts2-rs-m3n0g.2`/`.3`).
//! - **Relic lose-half** (`relics.pop` from an event): still deferred —
//!   `RelicTrader.trade_*` ships only the gain-half (`sts2-rs-m3n0g.1`).
//! - **`wongo_points` + customer-badge milestone**: still not modelled.
//! - **`SlipperyBridge` multi-page escalation**: **LANDED 2026-07-18**
//!   (was `sts2-rs-m3n0g.3` "collapses to a single 3-damage step") — HoldOn
//!   now escalates (3+N unblockable), rerolls the offered card with the C#
//!   `SkippedRemovals` no-repeat set, and loops via `MultiPageEventState`
//!   until Overcome. Surfaced by the 2026-07-18 shadow-replay census
//!   (4+/60 runs first-diverged here).

use crate::cards::CardRarity;
use sts2_core::cards::{CardId, CardType};
use sts2_core::{
    CardReward, EventId, EventOptionInfo, EventOutcome, EventVarSpec, RelicId, Reward, RunState,
};

use super::{
    allowed_act_2_plus, any_enchantable_deck_card, default_option_info_from_spec,
    future_of_potions_allowed, relic_trader_allowed, slippery_bridge_allowed,
    stone_of_all_time_allowed, EventOptionSpec, EventSpec,
};

// ── shared helpers ───────────────────────────────────────────────────

/// Subtract `amount` from `state.gold`, saturating at 0. Inlined here
/// pending a dedicated `RunState::lose_gold` surface (same pattern as
/// [`super::act1_overgrowth`]).
fn lose_gold_saturating(state: &mut RunState, amount: i32) {
    state.gold = state.gold.saturating_sub(amount);
}

/// Queue a `Reward::Card(empty)` placeholder so the run-loop sees the
/// event produced a card reward. Concrete card-pool resolution lives
/// in the (future) reward dispatcher.
fn queue_card_reward(state: &mut RunState) {
    state
        .pending_rewards
        .push(Reward::Card(CardReward::empty()));
}

// ── RelicTrader ──────────────────────────────────────────────────────

/// `RelicTrader` trade-slot count. Port pin: `act2.py:781`.
pub const RELIC_TRADER_TRADE_COUNT: usize = 3;

/// `RelicTrader`'s `Id.Entry` (L10N key root) — names its own event stream.
const RELIC_TRADER_STREAM: &str = "RELIC_TRADER";

/// The three relics of yours on the table, top → bottom.
///
/// C# `RelicTrader.OwnedRelics`: `GetValidRelics(Owner).ToList()
/// .StableShuffle(base.Rng).Take(3)`, where `GetValidRelics` is
/// `Relics.Where(r => r.IsTradable)` ([`relic_is_tradable`]).
///
/// `StableShuffle` sorts before shuffling, and a relic's `CompareTo` is
/// `ModelId.CompareTo` — an ORDINAL compare of `Entry`, the UPPER_SNAKE id
/// [`RelicId::to_python_name`] returns. Sorting by the enum's own `Ord`
/// (declaration order) would shuffle a differently-ordered list and hand back
/// different slots, so the comparator is explicit.
///
/// Recomputed rather than cached: the event draws nothing else from its
/// stream, so a fresh stream at cursor 0 gives the same three relics whether
/// asked while presenting the options or while resolving one.
fn relic_trader_owned(state: &RunState) -> Vec<RelicId> {
    let mut valid: Vec<RelicId> = state
        .relics
        .iter()
        .copied()
        .filter(|&id| crate::relics::tradable::relic_is_tradable(id))
        .collect();
    let mut rng = sts2_core::dotnet_rng::Rng::with_name(state.rng.seed, RELIC_TRADER_STREAM);
    rng.stable_shuffle_by(&mut valid, &|a: RelicId, b: RelicId| {
        a.to_python_name().cmp(b.to_python_name())
    });
    valid.truncate(RELIC_TRADER_TRADE_COUNT);
    valid
}

/// The three relics the trader offers back, for the options that name them.
///
/// C# `RelicTrader.NewRelics` is three `PullNextRelicFromFront(owner)` calls,
/// each rolling its own rarity off the rewards stream. Peeking must not
/// advance that stream — the option list is rebuilt on every observation — so
/// this rolls against a COPY and feeds each peeked id back as an exclusion,
/// which is what draining the bag would have done.
fn relic_trader_stock(state: &RunState) -> Vec<RelicId> {
    let mut rewards = state.rng.rewards.clone();
    let mut stock: Vec<RelicId> = Vec::with_capacity(RELIC_TRADER_TRADE_COUNT);
    for _ in 0..RELIC_TRADER_TRADE_COUNT {
        let rarity = sts2_core::runstate::roll_relic_rarity(&mut rewards);
        let Some(id) = state.peek_relic_from_grab_bag_where(rarity, |_| true, &stock) else {
            break;
        };
        stock.push(id);
    }
    stock
}

/// Commit what [`relic_trader_stock`] previewed.
///
/// All three come out of the bag even though you leave with one, because C#
/// pulls all three in `GenerateInitialOptions` — the two you turn down are
/// spent for the rest of the run. Every option resolves exactly one trade, so
/// doing the pulls here leaves the bag where C# leaves it.
fn relic_trader_take_stock(state: &mut RunState) -> Vec<RelicId> {
    let mut stock: Vec<RelicId> = Vec::with_capacity(RELIC_TRADER_TRADE_COUNT);
    for _ in 0..RELIC_TRADER_TRADE_COUNT {
        let rarity = state.roll_reward_relic_rarity();
        let Some(id) = state.pop_relic_from_grab_bag(rarity) else {
            break;
        };
        stock.push(id);
    }
    stock
}

/// C# `RelicTrader.Trade(index)`: `RelicCmd.Remove(OwnedRelics[index])` then
/// `RelicCmd.Obtain(NewRelics[index])`.
fn relic_trader_trade(state: &mut RunState, slot: usize) -> EventOutcome {
    let owned = relic_trader_owned(state);
    let stock = relic_trader_take_stock(state);
    let (Some(&given), Some(&taken)) = (owned.get(slot), stock.get(slot)) else {
        // Neither can be short in a run — `relic_trader_allowed` demands 5
        // tradable relics and the bag is far from empty in Act 2 — but a
        // force-entered fixture can present a slot with nothing behind it.
        return EventOutcome::finished("The trader had nothing to offer.");
    };
    state.lose_relic(given);
    state.grant_event_relic(taken);
    EventOutcome::finished(format!(
        "Traded {} for {}.",
        given.to_python_name(),
        taken.to_python_name()
    ))
}

/// `{TopRelicOwned}` / `{TopRelicNew}` and friends: the pair of relic titles
/// the slot's option text names. Empty when the slot has no pair, so the
/// option falls back to the engine shorthand rather than naming half a trade.
fn relic_trader_vars(
    state: &RunState,
    slot: usize,
    owned_key: &'static str,
    new_key: &'static str,
) -> Vec<(&'static str, sts2_core::EventVar)> {
    let title = |id: RelicId| sts2_core::EventVar::Text(format!("{}.title", id.to_python_name()));
    match (
        relic_trader_owned(state).get(slot),
        relic_trader_stock(state).get(slot),
    ) {
        (Some(&given), Some(&taken)) => vec![(owned_key, title(given)), (new_key, title(taken))],
        _ => Vec::new(),
    }
}

const RELIC_TRADER_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "trade_0",
        loc_key: "RELIC_TRADER.pages.INITIAL.options.TOP",
        default_label: "Trade Relic 1",
        default_description: "Swap an owned relic for a new one.",
        default_vars: &[],
        vars_fn: Some(|s| relic_trader_vars(s, 0, "TopRelicOwned", "TopRelicNew")),
        title_vars_fn: None,
        handler: |s| relic_trader_trade(s, 0),
    },
    EventOptionSpec {
        option_id: "trade_1",
        loc_key: "RELIC_TRADER.pages.INITIAL.options.MIDDLE",
        default_label: "Trade Relic 2",
        default_description: "Swap an owned relic for a new one.",
        default_vars: &[],
        vars_fn: Some(|s| relic_trader_vars(s, 1, "MiddleRelicOwned", "MiddleRelicNew")),
        title_vars_fn: None,
        handler: |s| relic_trader_trade(s, 1),
    },
    EventOptionSpec {
        option_id: "trade_2",
        loc_key: "RELIC_TRADER.pages.INITIAL.options.BOTTOM",
        default_label: "Trade Relic 3",
        default_description: "Swap an owned relic for a new one.",
        default_vars: &[],
        vars_fn: Some(|s| relic_trader_vars(s, 2, "BottomRelicOwned", "BottomRelicNew")),
        title_vars_fn: None,
        handler: |s| relic_trader_trade(s, 2),
    },
];

pub const RELIC_TRADER: EventSpec = EventSpec {
    id: EventId::RelicTrader,
    python_name: "RelicTrader",
    // C# `CurrentActIndex != 0 && GetValidRelics(p).Count() >= 5`.
    is_allowed: relic_trader_allowed,
    generate_options: default_option_info_from_spec,
    options: RELIC_TRADER_OPTIONS,
};

// ── SlipperyBridge ───────────────────────────────────────────────────

/// `SlipperyBridge.hold_on` base damage. Port pin: `act2.py:859`.
pub const SLIPPERY_BRIDGE_BASE_DAMAGE: i32 = 3;

/// `SlipperyBridge` event-RNG stream name (C# `Id.Entry`, UPPER_SNAKE), same
/// construction as the `DARV` / `DOLL_ROOM` streams. v0.107.1 single-player
/// event seed: additive player term is 0 (`EventModel.cs:238` GetPlayerSlotIndex
/// == 0; v0.103.2 used `Owner.NetId == 1`, `EventModel.cs:193`).
const SLIPPERY_BRIDGE_STREAM: &str = "SLIPPERY_BRIDGE";

/// Take the persisted event RNG out of the multi-page scratch, creating it on
/// first use (same take/store-back contract as `tinker_time_take_rng` — the
/// caller MUST store it back so the counter advances across HoldOn rerolls,
/// matching C# `base.Rng` living for the whole event).
fn slippery_bridge_take_rng(state: &mut RunState) -> sts2_core::dotnet_rng::Rng {
    state.event_state.rng.take().unwrap_or_else(|| {
        sts2_core::dotnet_rng::Rng::with_name(state.rng.seed, SLIPPERY_BRIDGE_STREAM)
    })
}

/// Replay C# `SlipperyBridge.GetNewRandomCard` on its FIRST invocation
/// (`RandomCardToLose == null`, the state Overcome resolves from): the candidate
/// pool is the deck's non-Basic removable cards, falling back to ALL removable
/// cards when that's empty (`DollRoom.cs`-style), then `Rng.NextItem(list)`.
/// Returns the chosen master-deck index, or `None` on a fully-unremovable deck.
///
/// C# rolls this once in `GenerateInitialOptions` (at event entry); recomputing
/// it here off the fresh event stream (counter 0) reproduces the same draw — the
/// TinkerTime/Darv recompute pattern — because no other event-RNG draw precedes
/// it.
fn slippery_bridge_card_to_lose(
    state: &RunState,
    rng: &mut sts2_core::dotnet_rng::Rng,
) -> Option<usize> {
    use crate::cards::spec_index::{card_is_removable_instance, rarity_for_card_id};
    use crate::cards::CardRarity;
    // C# `CardModel.IsRemovable => !Keywords.Contains(Eternal)` — basic
    // Strike/Defend ARE removable here (only the Eternal keyword blocks it),
    // but a TezcatarasEmber'd one is not, so this reads the instance.
    let removable: Vec<usize> = state
        .master_deck
        .iter()
        .enumerate()
        .filter(|(_, c)| card_is_removable_instance(c))
        .map(|(i, _)| i)
        .collect();
    let mut candidates: Vec<usize> = removable
        .iter()
        .copied()
        .filter(|&i| rarity_for_card_id(state.master_deck[i].card_id) != Some(CardRarity::Basic))
        .collect();
    if candidates.is_empty() {
        candidates = removable;
    }
    if candidates.is_empty() {
        return None;
    }
    let pick = rng.next_int_exclusive(0, candidates.len() as i32) as usize;
    Some(candidates[pick])
}

/// Scratch layout for the multi-page state (C# private fields):
/// `scratch[0]` = current `RandomCardToLose` master-deck index,
/// `scratch[1..]` = `SkippedRemovals` (indices of previously-offered cards),
/// `page` = `NumberOfHoldOns`. HoldOn never mutates the deck, so indices
/// stay valid for the whole event; Overcome removes exactly one and exits.
///
/// Rolls the initial card lazily in whichever handler first needs it —
/// C# rolls in `GenerateInitialOptions` (event entry), but no other
/// event-RNG draw precedes the first handler, so counter-0 reproduces the
/// same pick (the TinkerTime/Darv recompute pattern).
fn slippery_bridge_current_card(state: &mut RunState) -> Option<usize> {
    if state.event_state.scratch.is_empty() {
        let mut rng = slippery_bridge_take_rng(state);
        let rolled = slippery_bridge_card_to_lose(state, &mut rng);
        state.event_state.rng = Some(rng);
        let idx = rolled?;
        state
            .event_state
            .scratch
            .push(i16::try_from(idx).unwrap_or(0));
    }
    usize::try_from(state.event_state.scratch[0]).ok()
}

fn slippery_bridge_overcome(state: &mut RunState) -> EventOutcome {
    // C# `SlipperyBridge.Overcome`: `CardPileCmd.RemoveFromDeck(RandomCardToLose)`
    // — the specific card the event RNG rolled (NOT a player pick), so remove
    // that exact master-deck slot rather than the deck front.
    if let Some(idx) = slippery_bridge_current_card(state) {
        state.master_deck.remove(idx);
        EventOutcome::finished("Crossed the bridge, losing a card.")
    } else {
        EventOutcome::finished("Crossed the bridge.")
    }
}

fn slippery_bridge_hold_on(state: &mut RunState) -> EventOutcome {
    // C# `SlipperyBridge.HoldOn`: unblockable damage of `CurrentHpLoss`
    // (3 + NumberOfHoldOns, BEFORE the increment), then `NumberOfHoldOns++`,
    // then `GetNewRandomCard()` rerolls the offer and the event presents the
    // next 2-option page (`SetEventState`, NOT finished) — the escalation
    // loops until Overcome. `SlipperyBridge.cs:139-153`.
    use crate::cards::spec_index::card_is_removable_instance;
    let dmg = SLIPPERY_BRIDGE_BASE_DAMAGE + i32::from(state.event_state.page);
    let lost = state.lose_hp(dmg);

    if let Some(cur_idx) = slippery_bridge_current_card(state) {
        // Reroll per C# `GetNewRandomCard` (non-null branch,
        // `SlipperyBridge.cs:107-131`): `SkippedRemovals.Add(current)`; pool =
        // deck cards of a DIFFERENT card class (`GetType() !=
        // cur.GetType()` — i.e. a different CardId; upgrades share the
        // class), minus non-removable (`IsRemovable` = not Eternal) and
        // minus everything in `SkippedRemovals`; on an empty pool fall back
        // to ALL removable cards (skip/type filters dropped).
        let cur_id = state.master_deck[cur_idx].card_id;
        let skipped: Vec<usize> = state
            .event_state
            .scratch
            .iter()
            .skip(1)
            .filter_map(|&v| usize::try_from(v).ok())
            .collect();
        let mut candidates: Vec<usize> = state
            .master_deck
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                c.card_id != cur_id && card_is_removable_instance(c) && !skipped.contains(i)
            })
            .map(|(i, _)| i)
            .collect();
        if candidates.is_empty() {
            candidates = state
                .master_deck
                .iter()
                .enumerate()
                .filter(|(_, c)| card_is_removable_instance(c))
                .map(|(i, _)| i)
                .collect();
        }
        let mut rng = slippery_bridge_take_rng(state);
        let new_idx = if candidates.is_empty() {
            cur_idx
        } else {
            let pick = rng.next_int_exclusive(0, i32::try_from(candidates.len()).unwrap_or(1));
            candidates[usize::try_from(pick).unwrap_or(0)]
        };
        state.event_state.rng = Some(rng);
        state
            .event_state
            .scratch
            .push(i16::try_from(cur_idx).unwrap_or(0));
        state.event_state.scratch[0] = i16::try_from(new_idx).unwrap_or(0);
    }

    state.event_state.page = state.event_state.page.saturating_add(1);
    EventOutcome {
        finished: false,
        description: format!("Held on, taking {lost} damage."),
        effects: Vec::new(),
    }
}

/// Loc key for the title of the card Overcome will remove — C#
/// `SlipperyBridge`'s `RandomCard` StringVar, which its option description
/// prints ("Strike is removed from your Deck.").
///
/// Read-only twin of [`slippery_bridge_current_card`]: once the event has
/// rolled, the pick is already in `scratch[0]`; before that it re-rolls off a
/// COPY of the event stream, so previewing the option never advances the
/// counter the handler will draw from. Returns a `<CARD_ID>.title` key rather
/// than prose because card titles live in the display layer's loc tables — the
/// same contract the Rust backend uses for every other card name it emits.
fn slippery_bridge_card_preview(state: &RunState) -> Option<String> {
    let idx = if state.event_state.scratch.is_empty() {
        let mut rng = state.event_state.rng.clone().unwrap_or_else(|| {
            sts2_core::dotnet_rng::Rng::with_name(state.rng.seed, SLIPPERY_BRIDGE_STREAM)
        });
        slippery_bridge_card_to_lose(state, &mut rng)?
    } else {
        usize::try_from(state.event_state.scratch[0]).ok()?
    };
    let card = state.master_deck.get(idx)?;
    Some(format!("{}.title", card.card_id.to_python_name()))
}

const SLIPPERY_BRIDGE_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "overcome",
        loc_key: "SLIPPERY_BRIDGE.pages.INITIAL.options.OVERCOME",
        default_label: "Overcome",
        default_description: "Lose a random card.",
        default_vars: &[],
        vars_fn: Some(|s| {
            slippery_bridge_card_preview(s)
                .map(|key| ("RandomCard", sts2_core::EventVar::Text(key)))
                .into_iter()
                .collect()
        }),
        title_vars_fn: None,
        handler: slippery_bridge_overcome,
    },
    EventOptionSpec {
        option_id: "hold_on",
        loc_key: "SLIPPERY_BRIDGE.pages.INITIAL.options.HOLD_ON_0",
        default_label: "Hold On",
        default_description: "Take 3 damage, reroll card.",
        default_vars: &[(
            "HpLoss",
            EventVarSpec::Int(SLIPPERY_BRIDGE_BASE_DAMAGE as i64),
        )],
        vars_fn: Some(|s| vec![("HpLoss", slippery_bridge_damage(s).into())]),
        title_vars_fn: None,
        handler: slippery_bridge_hold_on,
    },
];

/// Render `hold_on`'s description at the CURRENT page's cost (GH #302).
///
/// C# re-binds the `HpLoss` DynamicVar to `CurrentHpLoss` on every page
/// (`SlipperyBridge.cs:143`, `3 + NumberOfHoldOns`), so the text the player
/// reads escalates with the loop. The static spec string said "Take 3
/// damage" forever, which is the number every description-parsing consumer
/// then believed: `ppo.encoder.event_payload`'s regex fallback and the
/// GH #300 Python no-suicide mask (`realgame_ppo._event_option_hp_cost`)
/// both read the option text, so on page 9 they priced a 12-damage click at
/// 3. The engine-side consumers (the structured `hp_delta` payload of
/// GH #237 and the Rust `veto_lethal_event_choice`) preview the option and
/// were always exact — this closes the gap for the text-reading ones.
/// Hold On's HP cost for the page the bridge is currently on — it climbs by
/// one every time you hold. Split out so the option can print the live number.
fn slippery_bridge_damage(state: &RunState) -> i32 {
    SLIPPERY_BRIDGE_BASE_DAMAGE + i32::from(state.event_state.page)
}

fn slippery_bridge_options(state: &RunState, spec: &EventSpec) -> Vec<EventOptionInfo> {
    let dmg = SLIPPERY_BRIDGE_BASE_DAMAGE + i32::from(state.event_state.page);
    default_option_info_from_spec(state, spec)
        .into_iter()
        .map(|mut info| {
            if info.option_id == "hold_on" {
                info.description = format!("Take {dmg} damage, reroll card.");
            }
            info
        })
        .collect()
}

pub const SLIPPERY_BRIDGE: EventSpec = EventSpec {
    id: EventId::SlipperyBridge,
    python_name: "SlipperyBridge",
    // C# `TotalFloor > 6 && deck.Any(IsRemovable)` (Rust total_floor + 1).
    is_allowed: slippery_bridge_allowed,
    generate_options: slippery_bridge_options,
    options: SLIPPERY_BRIDGE_OPTIONS,
};

// ── SpiralingWhirlpool ───────────────────────────────────────────────

/// `SpiralingWhirlpool.drink` heal fraction (33% of max_hp). Port pin:
/// `act2.py:889`.
pub const SPIRALING_WHIRLPOOL_HEAL_PCT: i32 = 33;

fn spiraling_whirlpool_observe(state: &mut RunState) -> EventOutcome {
    // Python enchants a deck card with Spiral. Enchant + card-choice
    // deferred; queue a card-reward placeholder so the run-loop sees
    // the intent.
    queue_card_reward(state);
    EventOutcome::finished("Queued enchant-with-Spiral reward.")
}

/// The Drink heal the option advertises: `SPIRALING_WHIRLPOOL_HEAL_PCT`% of
/// max HP. Split out so the choice screen can print the number.
fn spiraling_whirlpool_heal(state: &RunState) -> i32 {
    state.max_hp * SPIRALING_WHIRLPOOL_HEAL_PCT / 100
}

fn spiraling_whirlpool_drink(state: &mut RunState) -> EventOutcome {
    let heal_amount = state.max_hp * SPIRALING_WHIRLPOOL_HEAL_PCT / 100;
    let healed = state.heal(heal_amount);
    EventOutcome::finished(format!("Healed {healed} HP."))
}

fn spiraling_whirlpool_options(state: &RunState, _spec: &EventSpec) -> Vec<EventOptionInfo> {
    let heal_amount = state.max_hp * SPIRALING_WHIRLPOOL_HEAL_PCT / 100;
    vec![
        EventOptionInfo {
            option_id: "observe",
            loc_key: "SPIRALING_WHIRLPOOL.pages.INITIAL.options.OBSERVE",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Observe the Spiral".to_string(),
            description: "Enchant a card with Spiral.".to_string(),
            enabled: true,
        },
        EventOptionInfo {
            option_id: "drink",
            loc_key: "SPIRALING_WHIRLPOOL.pages.INITIAL.options.DRINK",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Drink".to_string(),
            description: format!("Heal {heal_amount} HP."),
            enabled: true,
        },
    ]
}

const SPIRALING_WHIRLPOOL_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "observe",
        loc_key: "SPIRALING_WHIRLPOOL.pages.INITIAL.options.OBSERVE",
        default_label: "Observe the Spiral",
        default_description: "Enchant a card with Spiral.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: spiraling_whirlpool_observe,
    },
    EventOptionSpec {
        option_id: "drink",
        loc_key: "SPIRALING_WHIRLPOOL.pages.INITIAL.options.DRINK",
        default_label: "Drink",
        default_description: "Heal 33% of Max HP.",
        default_vars: &[],
        vars_fn: Some(|s| vec![("Heal", spiraling_whirlpool_heal(s).into())]),
        title_vars_fn: None,
        handler: spiraling_whirlpool_drink,
    },
];

pub const SPIRALING_WHIRLPOOL: EventSpec = EventSpec {
    id: EventId::SpiralingWhirlpool,
    python_name: "SpiralingWhirlpool",
    // C# `deck.Any(Spiral.CanEnchant)` — base enchant filter approximation.
    is_allowed: any_enchantable_deck_card,
    generate_options: spiraling_whirlpool_options,
    options: SPIRALING_WHIRLPOOL_OPTIONS,
};

// ── StoneOfAllTime ───────────────────────────────────────────────────

/// `StoneOfAllTime.lift` Max-HP gain. Port pin: `act2.py:960`.
pub const STONE_LIFT_MAX_HP: i32 = 10;
/// `StoneOfAllTime.push` HP cost. Port pin: `act2.py:963`.
pub const STONE_PUSH_DAMAGE: i32 = 6;

/// `StoneOfAllTime`'s `Id.Entry` (L10N key root) — names its own event stream.
const STONE_OF_ALL_TIME_STREAM: &str = "STONE_OF_ALL_TIME";

/// Belt index of the potion the stone drinks — C# `DrinkAndLiftPotion`,
/// `base.Rng.NextItem(base.Owner.Potions)` in `GenerateInitialOptions`.
///
/// Read-only, and recomputed rather than cached: `StoneOfAllTime` draws
/// nothing from its event stream before this pick (`CalculateVars` is not
/// overridden), so a fresh stream at cursor 0 reproduces it whether asked at
/// present time or at resolve time — the same recompute contract
/// `slippery_bridge_card_to_lose` documents. `None` on an empty belt, which
/// `stone_of_all_time_allowed` already forbids (C# shows `LIFT_LOCKED` there).
fn stone_lift_potion(state: &RunState) -> Option<usize> {
    if state.potions.is_empty() {
        return None;
    }
    let mut rng = sts2_core::dotnet_rng::Rng::with_name(state.rng.seed, STONE_OF_ALL_TIME_STREAM);
    let len = i32::try_from(state.potions.len()).unwrap_or(1);
    Some(usize::try_from(rng.next_int_exclusive(0, len)).unwrap_or(0))
}

fn stone_lift(state: &mut RunState) -> EventOutcome {
    // C# `StoneOfAllTime.Lift` (`StoneOfAllTime.cs:91-99`):
    // `PotionCmd.Discard(DrinkAndLiftPotion)` THEN `GainMaxHp(10)`. The
    // discard was deferred here for a long time, which made Lift a free +10
    // Max HP — the single most over-taken event option in the run policy.
    let drunk = stone_lift_potion(state).map(|idx| state.remove_potion_from_belt(idx));
    state.gain_max_hp(STONE_LIFT_MAX_HP);
    match drunk {
        Some(kind) => EventOutcome::finished(format!(
            "The stone drank your {}. Gained {STONE_LIFT_MAX_HP} Max HP.",
            kind.python_name()
        )),
        None => EventOutcome::finished(format!("Gained {STONE_LIFT_MAX_HP} Max HP.")),
    }
}

fn stone_push(state: &mut RunState) -> EventOutcome {
    let lost = state.lose_hp(STONE_PUSH_DAMAGE);
    // Python enchants a deck card with Vigorous +8. Enchant deferred.
    EventOutcome::finished(format!(
        "Took {lost} damage — Vigorous +8 enchant deferred."
    ))
}

const STONE_OF_ALL_TIME_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "lift",
        loc_key: "STONE_OF_ALL_TIME.pages.INITIAL.options.LIFT",
        default_label: "Lift",
        default_description: "Discard a potion, gain 10 Max HP.",
        default_vars: &[(
            "DrinkMaxHpGain",
            EventVarSpec::Int(STONE_LIFT_MAX_HP as i64),
        )],
        vars_fn: Some(|s| {
            stone_lift_potion(s)
                .map(|idx| {
                    (
                        "DrinkRandomPotion",
                        sts2_core::EventVar::Text(format!(
                            "{}.title",
                            s.potions[idx].python_name()
                        )),
                    )
                })
                .into_iter()
                .collect()
        }),
        title_vars_fn: None,
        handler: stone_lift,
    },
    EventOptionSpec {
        option_id: "push",
        loc_key: "STONE_OF_ALL_TIME.pages.INITIAL.options.PUSH",
        default_label: "Push",
        default_description: "Take 6 damage, enchant a card with Vigorous +8.",
        default_vars: &[
            ("PushHpLoss", EventVarSpec::Int(STONE_PUSH_DAMAGE as i64)),
            ("PushVigorousAmount", EventVarSpec::Int(8)),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: stone_push,
    },
];

pub const STONE_OF_ALL_TIME: EventSpec = EventSpec {
    id: EventId::StoneOfAllTime,
    python_name: "StoneOfAllTime",
    // C# `CurrentActIndex == 1 && Potions.Any()` (Act 2 + ≥1 potion). Bead `sts2-rs-1hiht`.
    is_allowed: stone_of_all_time_allowed,
    generate_options: default_option_info_from_spec,
    options: STONE_OF_ALL_TIME_OPTIONS,
};

// ── Symbiote ─────────────────────────────────────────────────────────

/// `Symbiote` event-RNG stream name (C# `Id.Entry`, UPPER_SNAKE).
const SYMBIOTE_STREAM: &str = "SYMBIOTE";

/// C# `Symbiote.CanEnchant(card) => Corrupted.CanEnchant(card)`. Corrupted has
/// no `CanEnchant` override, so this is the base `EnchantmentModel.CanEnchant`:
/// not Status/Curse, no existing enchantment, concrete cost (X-cost stays
/// eligible) — identical to [`super::shared_wave_b`]'s `sown_can_enchant`.
fn symbiote_can_enchant(c: &sts2_core::cards::Card) -> bool {
    use sts2_core::cards::CardType;
    !matches!(c.card_type, CardType::Status | CardType::Curse)
        && c.enchantments.is_empty()
        && (c.cost >= 0 || c.is_x_cost())
}

fn symbiote_approach(state: &mut RunState) -> EventOutcome {
    // C# `Symbiote.Approach`: `CardSelectCmd.FromDeckForEnchantment(Corrupted, 1)`
    // pick-1 → `CardCmd.Enchant<Corrupted>(card, 1)`. Queue the interactive
    // enchant pick (the run loop surfaces it as a `card_select`); the grid
    // auto-takes at <=1 eligible, so fall through to an immediate enchant then.
    use sts2_core::cards::EnchantmentId;
    let eligible = state
        .master_deck
        .iter()
        .filter(|c| symbiote_can_enchant(c))
        .count();
    if state.interactive_card_transform && eligible > 1 {
        state
            .pending_deck_enchants
            .push(sts2_core::PendingDeckEnchant {
                enchantment: EnchantmentId::Corrupted,
                amount: 1,
                type_restriction: None,
                secondary_type: None,
            });
        EventOutcome::finished("Pick a card to enchant with Corrupted.")
    } else {
        if let Some(card) = state
            .master_deck
            .iter_mut()
            .find(|c| symbiote_can_enchant(c))
        {
            card.enchantments.push((EnchantmentId::Corrupted, 1));
        }
        EventOutcome::finished("Enchanted a card with Corrupted.")
    }
}

fn symbiote_kill_fire(state: &mut RunState) -> EventOutcome {
    // C# `Symbiote.KillWithFire`: `FromDeckForTransformation(CardsVar=1)` pick →
    // `CardCmd.TransformToRandom(item, base.Rng)`. `base.Rng` is the SYMBIOTE
    // event stream with no prior draw (no CalculateVars roll precedes it), so
    // `skip_reads = 0`. Queue the interactive transform pick → `card_select`;
    // auto mode transforms the first deck card off the same stream.
    if state.interactive_card_transform {
        state
            .pending_deck_transforms
            .push(sts2_core::PendingDeckTransform {
                event_stream_roll: Some((SYMBIOTE_STREAM, 0)),
                ..Default::default()
            });
        EventOutcome::finished("Pick a card to transform.")
    } else {
        if let Some(original) = state.master_deck.first().map(|c| c.card_id) {
            if let Some(new_id) = crate::cards::spec_index::roll_transform_replacement_event_stream(
                state,
                state.character_id,
                original,
                SYMBIOTE_STREAM,
                0,
            ) {
                if let Some(card) = crate::cards::spec_index::card_pod_from_id(new_id, false) {
                    state.master_deck.remove(0);
                    state.master_deck.push(card);
                }
            }
        }
        EventOutcome::finished("Transformed a card.")
    }
}

const SYMBIOTE_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "approach",
        loc_key: "SYMBIOTE.pages.INITIAL.options.APPROACH",
        default_label: "Approach",
        default_description: "Enchant a card with Corrupted.",
        default_vars: &[("Enchantment", EventVarSpec::Text("Corrupted"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: symbiote_approach,
    },
    EventOptionSpec {
        option_id: "kill_fire",
        loc_key: "SYMBIOTE.pages.INITIAL.options.KILL_WITH_FIRE",
        default_label: "Kill with Fire",
        default_description: "Transform 1 card.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: symbiote_kill_fire,
    },
];

pub const SYMBIOTE: EventSpec = EventSpec {
    id: EventId::Symbiote,
    python_name: "Symbiote",
    // C# `CurrentActIndex > 0` (Act 2+). Bead `sts2-rs-1hiht`.
    is_allowed: allowed_act_2_plus,
    generate_options: default_option_info_from_spec,
    options: SYMBIOTE_OPTIONS,
};

// ── TheFutureOfPotions ───────────────────────────────────────────────

/// `TheFutureOfPotions`'s `Id.Entry` (L10N key root) — its event stream.
const THE_FUTURE_OF_POTIONS_STREAM: &str = "THE_FUTURE_OF_POTIONS";

/// Slots the machine has: C# `Mathf.Min(3, Owner.Potions.Count)`.
pub const THE_FUTURE_OF_POTIONS_SLOTS: usize = 3;

/// The card rarity a potion buys — C# `GetCardRarity`.
fn future_of_potions_card_rarity(kind: sts2_core::potions::PotionKind) -> CardRarity {
    use sts2_core::potions::PotionRarity;
    match crate::potions::all::spec_for(kind).rarity {
        PotionRarity::Rare | PotionRarity::Event => CardRarity::Rare,
        PotionRarity::Uncommon => CardRarity::Uncommon,
        // Common, Token and the `None` guard. C# throws on `None`; a potion
        // with no rarity cannot reach the belt, so Common is the safe read.
        _ => CardRarity::Common,
    }
}

/// The card TYPE each held potion buys — C# `PotionToCardType`.
///
/// One `base.Rng.NextItem` per potion on the belt, in belt order, over
/// `[Attack, Skill, Power]` minus `Power` for a Common or Token potion. Built
/// for the WHOLE belt even though only three slots are shown, because the
/// draws for potions 4+ still come off the same stream in C# — asking only
/// about slot 0 must still consume the draws that precede it.
fn future_of_potions_card_types(state: &RunState) -> Vec<CardType> {
    use sts2_core::potions::PotionRarity;
    let mut rng =
        sts2_core::dotnet_rng::Rng::with_name(state.rng.seed, THE_FUTURE_OF_POTIONS_STREAM);
    state
        .potions
        .iter()
        .map(|&kind| {
            let rarity = crate::potions::all::spec_for(kind).rarity;
            let choices: &[CardType] =
                if matches!(rarity, PotionRarity::Common | PotionRarity::Token) {
                    &[CardType::Attack, CardType::Skill]
                } else {
                    &[CardType::Attack, CardType::Skill, CardType::Power]
                };
            let len = i32::try_from(choices.len()).unwrap_or(1);
            choices[usize::try_from(rng.next_int_exclusive(0, len)).unwrap_or(0)]
        })
        .collect()
}

/// Every card in the run's own pool of exactly this rarity AND type — C#
/// `CardCreationOptions.ForNonCombatWithUniformOdds(Character.CardPool,
/// c => c.Rarity == targetRarity && c.Type == cardType)` under
/// `NoRarityModification | NoCardPoolModifications`.
fn future_of_potions_pool(
    state: &RunState,
    rarity: CardRarity,
    card_type: CardType,
) -> Vec<CardId> {
    crate::cards::spec_index::specs_for_character(state.character_id)
        .iter()
        .map(|factory| factory())
        .filter(|spec| {
            spec.rarity == rarity
                && spec.card_type == card_type
                && !crate::cards::spec_index::is_multiplayer_only(spec.id)
        })
        .map(|spec| spec.id)
        .collect()
}

/// C# `TheFutureOfPotions.Trade(potion)`: discard it, then offer three cards
/// of the matched rarity and type, every one UPGRADED.
///
/// The offer goes on `pending_card_selects` rather than
/// `pending_rewards`, because that is the only surface carrying per-option
/// upgrade flags — a `CardReward` is a bare id list and would hand the player
/// an unupgraded card. The run loop moves it into `card_reward_options` and
/// fans it out as the usual pick-or-skip, so the decision the policy sees is
/// unchanged in kind (`RewardsCmd.OfferCustom` is skippable in C# too).
fn the_future_of_potions_trade(state: &mut RunState, slot: usize) -> EventOutcome {
    let Some(&kind) = state.potions.get(slot) else {
        return EventOutcome::finished("That slot was empty.");
    };
    let rarity = future_of_potions_card_rarity(kind);
    let card_type = future_of_potions_card_types(state)[slot];
    state.remove_potion_from_belt(slot);

    // C# uniform odds: three DISTINCT picks from the filtered pool, on the
    // rewards stream (`CardReward` rolls there). `NoRarityModification` means
    // no rarity roll precedes each pick, so it is one draw per card.
    let mut pool = future_of_potions_pool(state, rarity, card_type);
    let mut options: Vec<CardId> = Vec::with_capacity(3);
    for _ in 0..3 {
        if pool.is_empty() {
            break;
        }
        let len = i32::try_from(pool.len()).unwrap_or(1);
        let idx = usize::try_from(state.rng.rewards.next_int_exclusive(0, len)).unwrap_or(0);
        options.push(pool.remove(idx));
    }
    if options.is_empty() {
        return EventOutcome::finished("The machine had nothing to give.");
    }
    let count = options.len();
    state
        .pending_card_selects
        .push(sts2_core::runstate::PendingCardSelect {
            upgraded: vec![true; count],
            options,
            post_add: Vec::new(),
            can_skip: true,
        });
    EventOutcome::finished(format!(
        "Fed the machine a potion for {count} upgraded cards to choose from."
    ))
}

/// `{Potion}` / `{Rarity}` / `{Type}` for one slot.
///
/// **`Rarity` means two different things.** The TITLE takes the POTION's
/// rarity ("Insert Rare Potion") and the description takes the CARD's
/// ("Obtain an Upgraded Rare Power"), and C# renders them from two separate
/// `LocString` var bags. They agree except for Event and Token potions, which
/// buy Rare and Common cards respectively — so `title_vars` carries the
/// potion's, overriding the shared bag when the title is rendered.
fn future_of_potions_vars(
    state: &RunState,
    slot: usize,
) -> Vec<(&'static str, sts2_core::EventVar)> {
    let Some(&kind) = state.potions.get(slot) else {
        return Vec::new();
    };
    let text = |s: &str| sts2_core::EventVar::Text(s.to_string());
    vec![
        (
            "Potion",
            sts2_core::EventVar::Text(format!("{}.title", kind.python_name())),
        ),
        (
            "Rarity",
            text(card_rarity_word(future_of_potions_card_rarity(kind))),
        ),
        (
            "Type",
            text(card_type_word(future_of_potions_card_types(state)[slot])),
        ),
    ]
}

/// The potion's OWN rarity, for the option title. See
/// [`future_of_potions_vars`] on why this is a separate bag.
fn future_of_potions_title_vars(
    state: &RunState,
    slot: usize,
) -> Vec<(&'static str, sts2_core::EventVar)> {
    let Some(&kind) = state.potions.get(slot) else {
        return Vec::new();
    };
    let word = potion_rarity_word(crate::potions::all::spec_for(kind).rarity);
    vec![("Rarity", sts2_core::EventVar::Text(word.to_string()))]
}

/// C# `CardRarity.ToLocString()` — the words the option text prints.
fn card_rarity_word(rarity: CardRarity) -> &'static str {
    match rarity {
        CardRarity::Basic => "Basic",
        CardRarity::Common => "Common",
        CardRarity::Uncommon => "Uncommon",
        CardRarity::Rare => "Rare",
        CardRarity::Ancient => "Ancient",
        CardRarity::Status => "Status",
        CardRarity::Curse => "Curse",
        CardRarity::Event => "Event",
        CardRarity::Token => "Token",
        CardRarity::Quest => "Quest",
    }
}

/// C# `CardType.ToLocString()`.
fn card_type_word(card_type: CardType) -> &'static str {
    match card_type {
        CardType::Attack => "Attack",
        CardType::Skill => "Skill",
        CardType::Power => "Power",
        CardType::Status => "Status",
        CardType::Curse => "Curse",
    }
}

/// C# `PotionRarity.ToLocString()`.
fn potion_rarity_word(rarity: sts2_core::potions::PotionRarity) -> &'static str {
    use sts2_core::potions::PotionRarity;
    match rarity {
        PotionRarity::Common => "Common",
        PotionRarity::Uncommon => "Uncommon",
        PotionRarity::Rare => "Rare",
        PotionRarity::Event => "Event",
        PotionRarity::Token => "Token",
        PotionRarity::None => "",
    }
}

const THE_FUTURE_OF_POTIONS_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "trade_0",
        loc_key: "THE_FUTURE_OF_POTIONS.pages.INITIAL.options.POTION",
        default_label: "Trade Potion 1",
        default_description: "Discard potion for upgraded card reward.",
        default_vars: &[],
        vars_fn: Some(|s| future_of_potions_vars(s, 0)),
        title_vars_fn: Some(|s| future_of_potions_title_vars(s, 0)),
        handler: |s| the_future_of_potions_trade(s, 0),
    },
    EventOptionSpec {
        option_id: "trade_1",
        loc_key: "THE_FUTURE_OF_POTIONS.pages.INITIAL.options.POTION",
        default_label: "Trade Potion 2",
        default_description: "Discard potion for upgraded card reward.",
        default_vars: &[],
        vars_fn: Some(|s| future_of_potions_vars(s, 1)),
        title_vars_fn: Some(|s| future_of_potions_title_vars(s, 1)),
        handler: |s| the_future_of_potions_trade(s, 1),
    },
    EventOptionSpec {
        option_id: "trade_2",
        loc_key: "THE_FUTURE_OF_POTIONS.pages.INITIAL.options.POTION",
        default_label: "Trade Potion 3",
        default_description: "Discard potion for upgraded card reward.",
        default_vars: &[],
        vars_fn: Some(|s| future_of_potions_vars(s, 2)),
        title_vars_fn: Some(|s| future_of_potions_title_vars(s, 2)),
        handler: |s| the_future_of_potions_trade(s, 2),
    },
];

/// One slot per potion you carry, capped at three — C# `Mathf.Min(3,
/// list2.Count)`. The static spec always listed three, so a two-potion belt
/// (the `IsAllowed` minimum) offered a slot with nothing behind it.
fn the_future_of_potions_options(state: &RunState, spec: &EventSpec) -> Vec<EventOptionInfo> {
    let slots = state.potions.len().min(THE_FUTURE_OF_POTIONS_SLOTS);
    default_option_info_from_spec(state, spec)
        .into_iter()
        .take(slots)
        .collect()
}

pub const THE_FUTURE_OF_POTIONS: EventSpec = EventSpec {
    id: EventId::TheFutureOfPotions,
    python_name: "TheFutureOfPotions",
    // C# `Potions.Count() >= 2`. Bead `sts2-rs-1hiht`.
    is_allowed: future_of_potions_allowed,
    generate_options: the_future_of_potions_options,
    options: THE_FUTURE_OF_POTIONS_OPTIONS,
};

// ── WaterloggedScriptorium ───────────────────────────────────────────

/// `WaterloggedScriptorium.bloody_ink` Max-HP gain. Port pin: `act2.py:1193`.
pub const WATERLOGGED_INK_MAX_HP: i32 = 6;
/// `WaterloggedScriptorium.tentacle_quill` gold cost — C#
/// `WaterloggedScriptorium.cs` `CanonicalVars[1] = GoldVar(55)`
/// (Python pin `act2.py:1196`'s 65 drifted). Bead `sts2-rs-rcner`.
pub const WATERLOGGED_QUILL_COST: i32 = 55;
/// `WaterloggedScriptorium.prickly_sponge` gold cost — C#
/// `CanonicalVars[2] = GoldVar("PricklySpongeGold", 99)` (Python
/// pin `act2.py:1223`'s 155 drifted). Bead `sts2-rs-rcner`.
pub const WATERLOGGED_SPONGE_COST: i32 = 99;

fn waterlogged_bloody_ink(state: &mut RunState) -> EventOutcome {
    state.gain_max_hp(WATERLOGGED_INK_MAX_HP);
    EventOutcome::finished("Gained 6 Max HP.")
}

fn waterlogged_tentacle_quill(state: &mut RunState) -> EventOutcome {
    lose_gold_saturating(state, WATERLOGGED_QUILL_COST);
    queue_card_reward(state);
    EventOutcome::finished("Paid 55g, queued enchant-1-card-with-Steady reward.")
}

fn waterlogged_prickly_sponge(state: &mut RunState) -> EventOutcome {
    lose_gold_saturating(state, WATERLOGGED_SPONGE_COST);
    queue_card_reward(state);
    EventOutcome::finished("Paid 99g, queued enchant-2-cards-with-Steady reward.")
}

fn waterlogged_options(state: &RunState, _spec: &EventSpec) -> Vec<EventOptionInfo> {
    let gold = state.gold;
    vec![
        EventOptionInfo {
            option_id: "bloody_ink",
            loc_key: "WATERLOGGED_SCRIPTORIUM.pages.INITIAL.options.BLOODY_INK",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Bloody Ink".to_string(),
            description: "Gain 6 Max HP.".to_string(),
            enabled: true,
        },
        EventOptionInfo {
            option_id: "tentacle_quill",
            loc_key: "WATERLOGGED_SCRIPTORIUM.pages.INITIAL.options.TENTACLE_QUILL",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: if gold >= WATERLOGGED_QUILL_COST {
                "Tentacle Quill (55g)".to_string()
            } else {
                "Tentacle Quill".to_string()
            },
            description: "Enchant 1 card with Steady.".to_string(),
            enabled: gold >= WATERLOGGED_QUILL_COST,
        },
        EventOptionInfo {
            option_id: "prickly_sponge",
            loc_key: "WATERLOGGED_SCRIPTORIUM.pages.INITIAL.options.PRICKLY_SPONGE",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: if gold >= WATERLOGGED_SPONGE_COST {
                "Prickly Sponge (99g)".to_string()
            } else {
                "Prickly Sponge".to_string()
            },
            description: "Enchant 2 cards with Steady.".to_string(),
            enabled: gold >= WATERLOGGED_SPONGE_COST,
        },
    ]
}

const WATERLOGGED_SCRIPTORIUM_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "bloody_ink",
        loc_key: "WATERLOGGED_SCRIPTORIUM.pages.INITIAL.options.BLOODY_INK",
        default_label: "Bloody Ink",
        default_description: "Gain 6 Max HP.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: waterlogged_bloody_ink,
    },
    EventOptionSpec {
        option_id: "tentacle_quill",
        loc_key: "WATERLOGGED_SCRIPTORIUM.pages.INITIAL.options.TENTACLE_QUILL",
        default_label: "Tentacle Quill",
        default_description: "Enchant 1 card with Steady.",
        default_vars: &[("Gold", EventVarSpec::Int(WATERLOGGED_QUILL_COST as i64))],
        vars_fn: None,
        title_vars_fn: None,
        handler: waterlogged_tentacle_quill,
    },
    EventOptionSpec {
        option_id: "prickly_sponge",
        loc_key: "WATERLOGGED_SCRIPTORIUM.pages.INITIAL.options.PRICKLY_SPONGE",
        default_label: "Prickly Sponge",
        default_description: "Enchant 2 cards with Steady.",
        default_vars: &[
            (
                "PricklySpongeGold",
                EventVarSpec::Int(WATERLOGGED_SPONGE_COST as i64),
            ),
            ("Cards", EventVarSpec::Int(2)),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: waterlogged_prickly_sponge,
    },
];

fn waterlogged_is_allowed(state: &RunState) -> bool {
    state.gold >= WATERLOGGED_QUILL_COST
}

pub const WATERLOGGED_SCRIPTORIUM: EventSpec = EventSpec {
    id: EventId::WaterloggedScriptorium,
    python_name: "WaterloggedScriptorium",
    is_allowed: waterlogged_is_allowed,
    generate_options: waterlogged_options,
    options: WATERLOGGED_SCRIPTORIUM_OPTIONS,
};

// ── WelcomeToWongos ──────────────────────────────────────────────────

/// `WelcomeToWongos.bargain_bin` gold cost. Port pin: `act2.py:1324`.
pub const WONGO_BARGAIN_COST: i32 = 100;
/// `WelcomeToWongos.featured` gold cost. Port pin: `act2.py:1340`.
pub const WONGO_FEATURED_COST: i32 = 200;
/// `WelcomeToWongos.mystery` gold cost. Port pin: `act2.py:1355`.
pub const WONGO_MYSTERY_COST: i32 = 300;
/// Relics [`RelicId::WongosMysteryTicket`] eventually pays out, and the number
/// of combats it waits first. C# keeps them on the EVENT
/// (`WelcomeToWongos.CanonicalVars`: `MysteryBoxRelicCount` 3,
/// `MysteryBoxCombatCount` 5) purely so the Mystery Box option can describe the
/// ticket it sells; the relic itself carries the behaviour.
pub const WONGO_MYSTERY_RELIC_COUNT: i32 = 3;
/// Combats [`RelicId::WongosMysteryTicket`] waits before paying out. See
/// [`WONGO_MYSTERY_RELIC_COUNT`].
pub const WONGO_MYSTERY_COMBAT_COUNT: i32 = 5;

/// The rare relic sitting on Wongo's counter — C# `FeaturedItem`,
/// `PullNextRelicFromFront(owner, RelicRarity.Rare, r => r.IsAllowedInShops)`.
///
/// Read-only, for the option that names it. [`wongo_take_featured`] commits
/// the same pull.
fn wongo_featured_relic(state: &RunState) -> Option<RelicId> {
    state.peek_relic_from_grab_bag_where(
        sts2_core::runstate::RelicRarity::Rare,
        sts2_core::shop::is_relic_allowed_in_shops,
        &[],
    )
}

/// Consume the featured relic from the bag, whether or not it is bought.
///
/// C# pulls it in `GenerateInitialOptions` — i.e. on ENTERING the event — so
/// it leaves the relic pool even if you walk out with the Mystery Box or
/// nothing at all, shifting every later relic pull by one. Our option list is
/// re-derived on every observation and must stay side-effect free, so the pull
/// is committed here instead and **every** Wongo handler calls it. Exactly one
/// handler runs per visit, so the bag ends up in the state C# leaves it in.
fn wongo_take_featured(state: &mut RunState) -> Option<RelicId> {
    state.pop_relic_from_grab_bag_where(
        sts2_core::runstate::RelicRarity::Rare,
        sts2_core::shop::is_relic_allowed_in_shops,
    )
}

fn wongo_bargain_bin(state: &mut RunState) -> EventOutcome {
    // The featured item is off the counter before any option resolves — see
    // `wongo_take_featured`. It goes first, matching the C# pull order.
    wongo_take_featured(state);
    lose_gold_saturating(state, WONGO_BARGAIN_COST);
    // C# `BuyBargainBin`: `PullNextRelicFromFront(owner, RelicRarity.Common,
    // r => r.IsAllowedInShops)` then `RelicCmd.Obtain`.
    let relic = state.pop_relic_from_grab_bag_where(
        sts2_core::runstate::RelicRarity::Common,
        sts2_core::shop::is_relic_allowed_in_shops,
    );
    match relic {
        Some(id) => {
            state.grant_event_relic(id);
            EventOutcome::finished(format!("Paid {WONGO_BARGAIN_COST}g for the bargain bin."))
        }
        None => EventOutcome::finished(format!(
            "Paid {WONGO_BARGAIN_COST}g — the bargain bin was empty."
        )),
    }
}

fn wongo_featured(state: &mut RunState) -> EventOutcome {
    // C# `BuyFeaturedItem`: `RelicCmd.Obtain(FeaturedItem)` — the rare relic
    // pulled when the event opened, not a fresh roll.
    let featured = wongo_take_featured(state);
    lose_gold_saturating(state, WONGO_FEATURED_COST);
    match featured {
        Some(id) => {
            state.grant_event_relic(id);
            EventOutcome::finished(format!(
                "Paid {WONGO_FEATURED_COST}g for the featured item."
            ))
        }
        None => EventOutcome::finished(format!(
            "Paid {WONGO_FEATURED_COST}g — the counter was bare."
        )),
    }
}

fn wongo_mystery(state: &mut RunState) -> EventOutcome {
    wongo_take_featured(state);
    lose_gold_saturating(state, WONGO_MYSTERY_COST);
    // C# `BuyMysteryBox`: `RelicCmd.Obtain<WongosMysteryTicket>` — obtained
    // outright, not offered as a reward to confirm.
    state.grant_event_relic(RelicId::WongosMysteryTicket);
    EventOutcome::finished(format!("Paid {WONGO_MYSTERY_COST}g for the mystery box."))
}

/// `WelcomeToWongos` event-RNG stream name (C# `Id.Entry`, UPPER_SNAKE).
/// `WelcomeToWongos` does not override `IsShared`, so the single-player player
/// term applies. v0.107.1 additive player term is 0 (`EventModel.cs:238`
/// GetPlayerSlotIndex == 0; v0.103.2 used `Owner.NetId == 1`,
/// `EventModel.cs:193`). Same construction as the `SLIPPERY_BRIDGE` stream.
const WONGO_STREAM: &str = "WELCOME_TO_WONGOS";

fn wongo_rng(state: &RunState) -> sts2_core::dotnet_rng::Rng {
    sts2_core::dotnet_rng::Rng::with_name(state.rng.seed, WONGO_STREAM)
}

fn wongo_leave(state: &mut RunState) -> EventOutcome {
    // C# `WelcomeToWongos.Leave`:
    //   base.Rng.NextItem(owner.Deck.Cards.Where(c => c.IsUpgraded))  → Downgrade
    // The event RNG picks ONE upgraded card (in deck order) off a fresh
    // per-event stream — `WelcomeToWongos` has no `CalculateVars`, so no draws
    // precede this pick (counter 0) — then downgrades that exact card. NOT a
    // rewards-stream pick, so `downgrade_random_cards` (which reads
    // `rng.rewards`) is wrong here; we replicate the event-local NextItem the
    // same way `slippery_bridge_overcome` does. On a deck with no upgraded
    // cards, C#'s `NextItem` returns null and the leg no-ops.
    wongo_take_featured(state);
    let upgraded: Vec<usize> = state
        .master_deck
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.upgraded.then_some(i))
        .collect();
    if upgraded.is_empty() {
        return EventOutcome::finished("Left Wongo's — no upgraded card to downgrade.");
    }
    let mut rng = wongo_rng(state);
    let pick = rng.next_int_exclusive(0, upgraded.len() as i32) as usize;
    state.master_deck[upgraded[pick]].downgrade();
    EventOutcome::finished("Left Wongo's, downgrading a card.")
}

fn wongo_options(state: &RunState, _spec: &EventSpec) -> Vec<EventOptionInfo> {
    let gold = state.gold;
    vec![
        EventOptionInfo {
            option_id: "bargain_bin",
            loc_key: "WELCOME_TO_WONGOS.pages.INITIAL.options.BARGAIN_BIN",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: if gold >= WONGO_BARGAIN_COST {
                "Bargain Bin (100g)".to_string()
            } else {
                "Bargain Bin".to_string()
            },
            description: "Common relic.".to_string(),
            enabled: gold >= WONGO_BARGAIN_COST,
        },
        EventOptionInfo {
            option_id: "featured",
            loc_key: "WELCOME_TO_WONGOS.pages.INITIAL.options.FEATURED_ITEM",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: if gold >= WONGO_FEATURED_COST {
                "Featured Item (200g)".to_string()
            } else {
                "Featured Item".to_string()
            },
            description: "Rare relic.".to_string(),
            enabled: gold >= WONGO_FEATURED_COST,
        },
        EventOptionInfo {
            option_id: "mystery",
            loc_key: "WELCOME_TO_WONGOS.pages.INITIAL.options.MYSTERY_BOX",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: if gold >= WONGO_MYSTERY_COST {
                "Mystery Box (300g)".to_string()
            } else {
                "Mystery Box".to_string()
            },
            description: "Mystery ticket relic.".to_string(),
            enabled: gold >= WONGO_MYSTERY_COST,
        },
        EventOptionInfo {
            option_id: "leave",
            loc_key: "WELCOME_TO_WONGOS.pages.INITIAL.options.LEAVE",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Leave".to_string(),
            description: "Downgrade a random upgraded card.".to_string(),
            enabled: true,
        },
    ]
}

const WELCOME_TO_WONGOS_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "bargain_bin",
        loc_key: "WELCOME_TO_WONGOS.pages.INITIAL.options.BARGAIN_BIN",
        default_label: "Bargain Bin",
        default_description: "Common relic.",
        default_vars: &[(
            "BargainBinCost",
            EventVarSpec::Int(WONGO_BARGAIN_COST as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: wongo_bargain_bin,
    },
    EventOptionSpec {
        option_id: "featured",
        loc_key: "WELCOME_TO_WONGOS.pages.INITIAL.options.FEATURED_ITEM",
        default_label: "Featured Item",
        default_description: "Rare relic.",
        default_vars: &[(
            "FeaturedItemCost",
            EventVarSpec::Int(WONGO_FEATURED_COST as i64),
        )],
        vars_fn: Some(|s| {
            wongo_featured_relic(s)
                .map(|id| {
                    (
                        "RandomRelic",
                        sts2_core::EventVar::Text(format!("{}.title", id.to_python_name())),
                    )
                })
                .into_iter()
                .collect()
        }),
        title_vars_fn: None,
        handler: wongo_featured,
    },
    EventOptionSpec {
        option_id: "mystery",
        loc_key: "WELCOME_TO_WONGOS.pages.INITIAL.options.MYSTERY_BOX",
        default_label: "Mystery Box",
        default_description: "Mystery ticket relic.",
        default_vars: &[
            (
                "MysteryBoxCost",
                EventVarSpec::Int(WONGO_MYSTERY_COST as i64),
            ),
            (
                "MysteryBoxRelicCount",
                EventVarSpec::Int(WONGO_MYSTERY_RELIC_COUNT as i64),
            ),
            (
                "MysteryBoxCombatCount",
                EventVarSpec::Int(WONGO_MYSTERY_COMBAT_COUNT as i64),
            ),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: wongo_mystery,
    },
    EventOptionSpec {
        option_id: "leave",
        loc_key: "WELCOME_TO_WONGOS.pages.INITIAL.options.LEAVE",
        default_label: "Leave",
        default_description: "Downgrade a random upgraded card.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: wongo_leave,
    },
];

fn wongo_is_allowed(state: &RunState) -> bool {
    // C# `WelcomeToWongos.IsAllowed`: `CurrentActIndex == 1 && Gold >= 100`
    // (0-based act → Rust Act 2). Bead `sts2-rs-1hiht`.
    state.act_index == 2 && state.gold >= WONGO_BARGAIN_COST
}

pub const WELCOME_TO_WONGOS: EventSpec = EventSpec {
    id: EventId::WelcomeToWongos,
    python_name: "WelcomeToWongos",
    // Python: `act==1 && gold>=100`. Act deferred.
    is_allowed: wongo_is_allowed,
    generate_options: wongo_options,
    options: WELCOME_TO_WONGOS_OPTIONS,
};

// ── WhisperingHollow ─────────────────────────────────────────────────

/// C# `WhisperingHollow._baseGold`.
pub const WHISPERING_GOLD_BASE: i32 = 35;
/// C# `WhisperingHollow._goldVariance`: `CalculateVars` adds
/// `Rng.NextInt(-9, 10)` (max-exclusive → [-9, 9]) to the base.
pub const WHISPERING_GOLD_VARIANCE: i32 = 9;
/// C# `IsAllowed`: every player's gold `>= 44` (base + variance, so the
/// rolled cost is always payable) — NOT the rolled cost itself.
pub const WHISPERING_MIN_GOLD: i32 = 44;
/// `WhisperingHollow.Gold` potion count (`RewardsCmd.OfferCustom` with
/// two `PotionReward`s).
pub const WHISPERING_POTION_COUNT: usize = 2;
/// `WhisperingHollow.Hug` HP cost (`HpLossVar(9)`, unblockable, applied
/// AFTER the transform pick resolves).
pub const WHISPERING_HUG_DAMAGE: i32 = 9;
/// `Id.Entry` for the event's own RNG stream. NON-shared event
/// (`EventModel.IsShared` default false); v0.107.1 seeds it with the
/// single-player `GetPlayerSlotIndex == 0` additive term (i.e. none) — see
/// [`crate::cards::spec_index::roll_transform_replacement_event_stream`].
pub const WHISPERING_HOLLOW_STREAM: &str = "WHISPERING_HOLLOW";

/// The event-entry `CalculateVars` gold roll: `35 + NextInt(-9, 10)` =
/// read #1 of the event's own stream. Re-derived per call (the stream is
/// private to this event, so re-deriving preserves in-stream order).
fn whispering_rolled_gold(state: &RunState) -> i32 {
    // v0.107.1 single-player event seed: additive player term is 0
    // (`EventModel.cs:238` GetPlayerSlotIndex == 0; v0.103.2 used NetId == 1).
    let mut rng = sts2_core::dotnet_rng::Rng::with_name(state.rng.seed, WHISPERING_HOLLOW_STREAM);
    WHISPERING_GOLD_BASE + rng.next_int(-WHISPERING_GOLD_VARIANCE, WHISPERING_GOLD_VARIANCE)
}

fn whispering_gold(state: &mut RunState) -> EventOutcome {
    // C# Gold: `PlayerCmd.LoseGold(DynamicVars.Gold.IntValue)` (the
    // CalculateVars roll), then `RewardsCmd.OfferCustom` with two
    // `PotionReward`s — sentinels roll on `PlayerRng.Rewards` at drain
    // time in queue order.
    let cost = whispering_rolled_gold(state);
    lose_gold_saturating(state, cost);
    for _ in 0..WHISPERING_POTION_COUNT {
        state
            .pending_rewards
            .push(Reward::Potion(sts2_core::PotionReward::empty()));
    }
    EventOutcome::finished(format!("Paid {cost} gold, gained 2 potion rewards."))
}

fn whispering_hug(state: &mut RunState) -> EventOutcome {
    // C# Hug: `CardSelectCmd.FromDeckForTransformation` (pick 1) →
    // `CardCmd.TransformToRandom(item, base.Rng)` — the EVENT stream,
    // read #2 (CalculateVars' gold roll consumed read #1 at entry) —
    // then 9 unblockable damage only after the transform.
    if state.interactive_card_transform {
        state
            .pending_deck_transforms
            .push(sts2_core::PendingDeckTransform {
                event_stream_roll: Some((WHISPERING_HOLLOW_STREAM, 1)),
                post_hp_loss: WHISPERING_HUG_DAMAGE,
                ..Default::default()
            });
        EventOutcome::finished("Pick a card to transform (9 damage follows).")
    } else {
        // Auto mode (parity-fuzz / headless): transform the first deck
        // card with the same event-stream roll, then take the damage.
        if let Some(original) = state.master_deck.first().map(|c| c.card_id) {
            if let Some(new_id) = crate::cards::spec_index::roll_transform_replacement_event_stream(
                state,
                state.character_id,
                original,
                WHISPERING_HOLLOW_STREAM,
                1,
            ) {
                if let Some(card) = crate::cards::spec_index::card_pod_from_id(new_id, false) {
                    state.master_deck.remove(0);
                    state.master_deck.push(card);
                }
            }
        }
        let lost = state.lose_hp(WHISPERING_HUG_DAMAGE);
        EventOutcome::finished(format!("Transformed a card, took {lost} damage."))
    }
}

const WHISPERING_HOLLOW_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "gold",
        loc_key: "WHISPERING_HOLLOW.pages.INITIAL.options.GOLD",
        default_label: "Pay Gold (35±9)",
        default_description: "Gain 2 potions.",
        default_vars: &[],
        vars_fn: Some(|s| vec![("Gold", whispering_rolled_gold(s).into())]),
        title_vars_fn: None,
        handler: whispering_gold,
    },
    EventOptionSpec {
        option_id: "hug",
        loc_key: "WHISPERING_HOLLOW.pages.INITIAL.options.HUG",
        default_label: "Hug",
        default_description: "Take 9 damage, transform 1 card.",
        default_vars: &[("HpLoss", EventVarSpec::Int(WHISPERING_HUG_DAMAGE as i64))],
        vars_fn: None,
        title_vars_fn: None,
        handler: whispering_hug,
    },
];

fn whispering_is_allowed(state: &RunState) -> bool {
    state.gold >= WHISPERING_MIN_GOLD
}

pub const WHISPERING_HOLLOW: EventSpec = EventSpec {
    id: EventId::WhisperingHollow,
    python_name: "WhisperingHollow",
    is_allowed: whispering_is_allowed,
    generate_options: default_option_info_from_spec,
    options: WHISPERING_HOLLOW_OPTIONS,
};

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> RunState {
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 40;
        rs.gold = 500;
        rs
    }

    fn run(spec: &EventSpec, opt_id: &str, state: &mut RunState) -> EventOutcome {
        let handler = spec
            .options
            .iter()
            .find(|o| o.option_id == opt_id)
            .unwrap_or_else(|| panic!("option {opt_id} missing in {}", spec.python_name))
            .handler;
        handler(state)
    }

    /// A trader run-state: five tradable relics owned, a stocked bag.
    fn trader_state() -> RunState {
        let mut rs = fresh();
        rs.act_index = 2;
        rs.relics = vec![
            RelicId::Anchor,
            RelicId::BloodVial,
            RelicId::HappyFlower,
            RelicId::Lantern,
            RelicId::Pendulum,
        ];
        rs.relic_grab_bag_by_rarity.insert(
            sts2_core::runstate::RelicRarity::Common,
            vec![RelicId::BagOfMarbles, RelicId::Gorget, RelicId::MealTicket],
        );
        rs.relic_grab_bag_by_rarity.insert(
            sts2_core::runstate::RelicRarity::Uncommon,
            vec![RelicId::Kifuda, RelicId::Shuriken, RelicId::Tingsha],
        );
        rs.relic_grab_bag_by_rarity.insert(
            sts2_core::runstate::RelicRarity::Rare,
            vec![RelicId::Brimstone, RelicId::Astrolabe, RelicId::CallingBell],
        );
        rs
    }

    /// The trade actually swaps: the picked relic leaves, its partner arrives.
    #[test]
    fn relic_trader_trade_swaps_the_slot_it_was_offered() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = trader_state();
        rs.direct_event_relic_grant = true;
        let given = relic_trader_owned(&rs)[1];
        let taken = relic_trader_stock(&rs)[1];
        assert_ne!(given, taken);
        run(&RELIC_TRADER, "trade_1", &mut rs);
        assert!(!rs.relics.contains(&given), "{given:?} should be gone");
        assert!(rs.relics.contains(&taken), "{taken:?} should be owned");
        assert_eq!(rs.relics.len(), 5, "one out, one in");
    }

    /// The option text names the pair the handler then swaps — a preview that
    /// rolled its own stock would advertise a relic you do not get.
    #[test]
    fn relic_trader_preview_matches_what_the_trade_delivers() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = trader_state();
        rs.direct_event_relic_grant = true;
        let vars = relic_trader_vars(&rs, 0, "TopRelicOwned", "TopRelicNew");
        let taken = relic_trader_stock(&rs)[0];
        assert_eq!(
            vars,
            vec![
                (
                    "TopRelicOwned",
                    sts2_core::EventVar::Text(format!(
                        "{}.title",
                        relic_trader_owned(&rs)[0].to_python_name()
                    ))
                ),
                (
                    "TopRelicNew",
                    sts2_core::EventVar::Text(format!("{}.title", taken.to_python_name()))
                ),
            ]
        );
        run(&RELIC_TRADER, "trade_0", &mut rs);
        assert!(rs.relics.contains(&taken));
    }

    /// C# pulls all three replacements when the event opens, so the two you
    /// turn down are spent for the rest of the run.
    #[test]
    fn relic_trader_consumes_all_three_replacements() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = trader_state();
        rs.direct_event_relic_grant = true;
        let stock = relic_trader_stock(&rs);
        assert_eq!(stock.len(), RELIC_TRADER_TRADE_COUNT);
        run(&RELIC_TRADER, "trade_0", &mut rs);
        for id in &stock[1..] {
            assert!(
                !rs.relic_grab_bag_by_rarity.values().any(|b| b.contains(id)),
                "{id:?} was pulled and declined — it must not return to the bag"
            );
        }
    }

    /// `IsAllowed` counts TRADABLE relics: a shelf of starter/event relics is
    /// not five valid ones, and C# never spawns the trader there.
    #[test]
    fn relic_trader_allowed_ignores_untradable_relics() {
        let mut rs = fresh();
        rs.act_index = 2;
        rs.relics = vec![
            RelicId::BurningBlood,        // Starter
            RelicId::ForgottenSoul,       // Event
            RelicId::WongosMysteryTicket, // Event
            RelicId::Anchor,
            RelicId::BloodVial,
        ];
        assert!(
            !super::super::relic_trader_allowed(&rs),
            "only 2 of the 5 are tradable"
        );
        rs.relics
            .extend([RelicId::HappyFlower, RelicId::Lantern, RelicId::Pendulum]);
        assert!(super::super::relic_trader_allowed(&rs), "now 5 tradable");
    }

    #[test]
    fn slippery_bridge_hold_on_takes_base_damage() {
        let mut rs = fresh();
        run(&SLIPPERY_BRIDGE, "hold_on", &mut rs);
        assert_eq!(rs.current_hp, 40 - SLIPPERY_BRIDGE_BASE_DAMAGE);
    }

    #[test]
    fn spiraling_whirlpool_drink_heals_one_third_max_hp() {
        let mut rs = fresh();
        run(&SPIRALING_WHIRLPOOL, "drink", &mut rs);
        // 80 * 33 / 100 = 26 heal; current_hp 40 + 26 = 66.
        assert_eq!(rs.current_hp, 66);
    }

    #[test]
    fn spiraling_whirlpool_options_interpolates_heal_amount() {
        let rs = fresh();
        let opts = (SPIRALING_WHIRLPOOL.generate_options)(&rs, &SPIRALING_WHIRLPOOL);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[1].description, "Heal 26 HP.");
    }

    /// Lift is a TRADE — the stone drinks one of your potions. It read as a
    /// free +10 Max HP for as long as the discard was deferred.
    #[test]
    fn stone_lift_drinks_a_potion_for_the_max_hp() {
        use sts2_core::potions::PotionKind;
        let mut rs = fresh();
        rs.potions = vec![
            PotionKind::BlockPotion,
            PotionKind::FirePotion,
            PotionKind::StrengthPotion,
        ];
        rs.potion_slot_indices = vec![0, 1, 2];
        let before = rs.max_hp;
        let drunk = rs.potions[stone_lift_potion(&rs).expect("a potion on the belt")];
        run(&STONE_OF_ALL_TIME, "lift", &mut rs);
        assert_eq!(rs.max_hp, before + STONE_LIFT_MAX_HP);
        assert_eq!(rs.potions.len(), 2, "exactly one potion drunk");
        assert!(
            !rs.potions.contains(&drunk) || rs.potions.iter().filter(|&&p| p == drunk).count() == 0,
            "the previewed potion is the one that went"
        );
    }

    /// The belt can't be empty in a real run (`stone_of_all_time_allowed`
    /// demands a potion), but a force-entered fixture must not panic.
    #[test]
    fn stone_lift_on_an_empty_belt_still_grants_the_max_hp() {
        let mut rs = fresh();
        let before = rs.max_hp;
        run(&STONE_OF_ALL_TIME, "lift", &mut rs);
        assert_eq!(rs.max_hp, before + STONE_LIFT_MAX_HP);
    }

    #[test]
    fn stone_push_takes_damage() {
        let mut rs = fresh();
        run(&STONE_OF_ALL_TIME, "push", &mut rs);
        assert_eq!(rs.current_hp, 40 - STONE_PUSH_DAMAGE);
    }

    #[test]
    fn symbiote_both_options_open_deck_card_selects() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::{CardId, EnchantmentId};
        let mut rs = fresh();
        rs.interactive_card_transform = true;
        // >1 enchantable card so the grid does NOT auto-take (C# MinSelect == 1).
        for _ in 0..3 {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false).unwrap(),
            );
        }
        run(&SYMBIOTE, "approach", &mut rs);
        assert_eq!(rs.pending_deck_enchants.len(), 1);
        assert_eq!(
            rs.pending_deck_enchants[0].enchantment,
            EnchantmentId::Corrupted
        );
        run(&SYMBIOTE, "kill_fire", &mut rs);
        assert_eq!(rs.pending_deck_transforms.len(), 1);
        assert_eq!(
            rs.pending_deck_transforms[0].event_stream_roll,
            Some(("SYMBIOTE", 0))
        );
        // No reward-queue placeholder — these are deck card_selects, not rewards.
        assert!(rs.pending_rewards.is_empty());
    }

    /// The machine EATS the potion and pays in upgraded cards of one matched
    /// rarity and type. It used to keep the potion and queue a generic reward.
    #[test]
    fn the_future_of_potions_trade_eats_the_potion_for_upgraded_matched_cards() {
        use sts2_core::potions::PotionKind;
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh();
        rs.character_id = sts2_core::potions::CharacterId::Ironclad;
        // Blood Potion is Common → a Common card, Attack or Skill (never
        // Power: C# drops Power for Common/Token potions).
        rs.potions = vec![PotionKind::BloodPotion, PotionKind::FirePotion];
        rs.potion_slot_indices = vec![0, 1];
        let card_type = future_of_potions_card_types(&rs)[0];
        assert!(
            matches!(card_type, CardType::Attack | CardType::Skill),
            "a Common potion never buys a Power, got {card_type:?}"
        );

        run(&THE_FUTURE_OF_POTIONS, "trade_0", &mut rs);
        assert_eq!(rs.potions, vec![PotionKind::FirePotion], "slot 0 consumed");
        assert!(rs.pending_rewards.is_empty(), "not a generic reward");
        let select = rs.pending_card_selects.first().expect("an offer");
        assert!(!select.options.is_empty());
        assert!(
            select.upgraded.iter().all(|&u| u),
            "C# upgrades every card in the offer"
        );
        for &id in &select.options {
            let spec = crate::cards::spec_index::specs_for_character(rs.character_id)
                .iter()
                .map(|f| f())
                .find(|s| s.id == id)
                .expect("from the run's own pool");
            assert_eq!(spec.rarity, CardRarity::Common, "{id:?} rarity matched");
            assert_eq!(spec.card_type, card_type, "{id:?} type matched");
        }
    }

    /// One slot per potion, capped at three — the static spec always showed
    /// three, so a two-potion belt (the `IsAllowed` minimum) offered a slot
    /// with nothing behind it.
    #[test]
    fn the_future_of_potions_shows_one_slot_per_potion() {
        use sts2_core::potions::PotionKind;
        let mut rs = fresh();
        rs.potions = vec![PotionKind::BloodPotion, PotionKind::FirePotion];
        rs.potion_slot_indices = vec![0, 1];
        let opts = (THE_FUTURE_OF_POTIONS.generate_options)(&rs, &THE_FUTURE_OF_POTIONS);
        assert_eq!(opts.len(), 2);
        rs.potions.push(PotionKind::StrengthPotion);
        rs.potion_slot_indices.push(2);
        rs.potions.push(PotionKind::BlockPotion);
        rs.potion_slot_indices.push(3);
        let opts = (THE_FUTURE_OF_POTIONS.generate_options)(&rs, &THE_FUTURE_OF_POTIONS);
        assert_eq!(opts.len(), 3, "capped at three slots");
    }

    /// `{Rarity}` means the POTION's rarity in the title and the CARD's in the
    /// description. They only differ for Event and Token potions — Foul is
    /// Event rarity and buys a RARE card.
    #[test]
    fn the_future_of_potions_title_and_description_rarities_can_disagree() {
        use sts2_core::potions::PotionKind;
        let mut rs = fresh();
        rs.potions = vec![PotionKind::FoulPotion, PotionKind::FirePotion];
        rs.potion_slot_indices = vec![0, 1];
        let vars = future_of_potions_vars(&rs, 0);
        let title_vars = future_of_potions_title_vars(&rs, 0);
        assert_eq!(
            vars.iter().find(|(n, _)| *n == "Rarity").map(|(_, v)| v),
            Some(&sts2_core::EventVar::Text("Rare".into())),
            "the description names the CARD rarity",
        );
        assert_eq!(
            title_vars.as_slice(),
            [("Rarity", sts2_core::EventVar::Text("Event".into()))],
            "the title names the POTION rarity",
        );
    }

    #[test]
    fn waterlogged_bloody_ink_gains_max_hp() {
        let mut rs = fresh();
        let before = rs.max_hp;
        run(&WATERLOGGED_SCRIPTORIUM, "bloody_ink", &mut rs);
        assert_eq!(rs.max_hp, before + WATERLOGGED_INK_MAX_HP);
    }

    #[test]
    fn waterlogged_quill_spends_and_queues_reward() {
        let mut rs = fresh();
        run(&WATERLOGGED_SCRIPTORIUM, "tentacle_quill", &mut rs);
        assert_eq!(rs.gold, 500 - WATERLOGGED_QUILL_COST);
        assert_eq!(rs.pending_rewards.len(), 1);
    }

    #[test]
    fn waterlogged_sponge_spends_and_queues_reward() {
        let mut rs = fresh();
        run(&WATERLOGGED_SCRIPTORIUM, "prickly_sponge", &mut rs);
        assert_eq!(rs.gold, 500 - WATERLOGGED_SPONGE_COST);
        assert_eq!(rs.pending_rewards.len(), 1);
    }

    #[test]
    fn waterlogged_options_disable_when_gold_insufficient() {
        let mut rs = fresh();
        rs.gold = 70;
        let opts = (WATERLOGGED_SCRIPTORIUM.generate_options)(&rs, &WATERLOGGED_SCRIPTORIUM);
        assert!(opts[0].enabled);
        assert!(opts[1].enabled);
        assert!(!opts[2].enabled);
    }

    /// A stocked Wongo's: a rare shop-legal relic on the counter, commons in
    /// the bin. `Brimstone` leads the rare bucket and is shop-legal;
    /// `AmethystAubergine` is NOT (`is_relic_allowed_in_shops`), so it also
    /// pins the filter.
    fn wongo_state() -> RunState {
        let mut rs = fresh();
        rs.act_index = 2;
        rs.direct_event_relic_grant = true;
        rs.relic_grab_bag_by_rarity.insert(
            sts2_core::runstate::RelicRarity::Common,
            vec![RelicId::AmethystAubergine, RelicId::Anchor],
        );
        rs.relic_grab_bag_by_rarity.insert(
            sts2_core::runstate::RelicRarity::Rare,
            vec![RelicId::Brimstone, RelicId::Astrolabe],
        );
        rs
    }

    #[test]
    fn wongo_bargain_bin_buys_a_shop_legal_common() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = wongo_state();
        run(&WELCOME_TO_WONGOS, "bargain_bin", &mut rs);
        assert_eq!(rs.gold, 400);
        // AmethystAubergine fronts the bucket but is shop-banned, so the
        // filter skips to Anchor rather than handing over a Circlet.
        assert!(rs.relics.contains(&RelicId::Anchor));
        assert!(!rs.relics.contains(&RelicId::AmethystAubergine));
    }

    #[test]
    fn wongo_featured_buys_the_relic_the_option_named() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = wongo_state();
        let featured = wongo_featured_relic(&rs).expect("a rare on the counter");
        run(&WELCOME_TO_WONGOS, "featured", &mut rs);
        assert_eq!(rs.gold, 300);
        assert!(rs.relics.contains(&featured), "{featured:?} bought");
    }

    /// C# pulls the featured relic when the event OPENS, so it is spent even
    /// if you buy something else or walk out.
    #[test]
    fn wongo_featured_relic_leaves_the_bag_whatever_you_pick() {
        crate::cards::dispatch::ensure_installed();
        for option in ["bargain_bin", "mystery", "leave"] {
            let mut rs = wongo_state();
            let featured = wongo_featured_relic(&rs).expect("a rare on the counter");
            run(&WELCOME_TO_WONGOS, option, &mut rs);
            assert!(
                !rs.relic_grab_bag_by_rarity
                    .values()
                    .any(|b| b.contains(&featured)),
                "{option}: the featured relic must not go back in the bag"
            );
            assert!(!rs.relics.contains(&featured), "{option}: not bought");
        }
    }

    #[test]
    fn wongo_mystery_obtains_the_mystery_ticket() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = wongo_state();
        run(&WELCOME_TO_WONGOS, "mystery", &mut rs);
        assert_eq!(rs.gold, 200);
        assert!(rs.relics.contains(&RelicId::WongosMysteryTicket));
    }

    #[test]
    fn wongo_options_label_enables_by_gold_threshold() {
        let mut rs = fresh();
        rs.gold = 150;
        let opts = (WELCOME_TO_WONGOS.generate_options)(&rs, &WELCOME_TO_WONGOS);
        assert!(opts[0].enabled, "bargain_bin (100g) enabled at 150g");
        assert!(!opts[1].enabled, "featured (200g) disabled at 150g");
        assert!(!opts[2].enabled, "mystery (300g) disabled at 150g");
        assert!(opts[3].enabled, "leave always enabled");
    }

    #[test]
    fn whispering_gold_spends_rolled_cost_and_queues_two_potion_rewards() {
        let mut rs = fresh();
        let expected_cost = {
            // Mirror the production v0.107.1 single-player event seed (additive
            // player term 0 — `EventModel.cs:238` GetPlayerSlotIndex == 0).
            let mut rng =
                sts2_core::dotnet_rng::Rng::with_name(rs.rng.seed, WHISPERING_HOLLOW_STREAM);
            WHISPERING_GOLD_BASE + rng.next_int(-WHISPERING_GOLD_VARIANCE, WHISPERING_GOLD_VARIANCE)
        };
        run(&WHISPERING_HOLLOW, "gold", &mut rs);
        assert_eq!(rs.gold, 500 - expected_cost);
        assert!((WHISPERING_GOLD_BASE - WHISPERING_GOLD_VARIANCE
            ..=WHISPERING_GOLD_BASE + WHISPERING_GOLD_VARIANCE)
            .contains(&expected_cost));
        assert_eq!(rs.pending_rewards.len(), 2);
        assert!(rs
            .pending_rewards
            .iter()
            .all(|r| matches!(r, Reward::Potion(_))));
    }

    #[test]
    fn whispering_hug_interactive_queues_event_stream_transform_with_post_damage() {
        let mut rs = fresh();
        rs.interactive_card_transform = true;
        run(&WHISPERING_HOLLOW, "hug", &mut rs);
        // Damage is deferred to the transform resolve (C# fires it only
        // after the pick), so HP is untouched here.
        assert_eq!(rs.current_hp, 40);
        assert_eq!(rs.pending_deck_transforms.len(), 1);
        let pending = &rs.pending_deck_transforms[0];
        assert_eq!(
            pending.event_stream_roll,
            Some((WHISPERING_HOLLOW_STREAM, 1))
        );
        assert_eq!(pending.post_hp_loss, WHISPERING_HUG_DAMAGE);
        assert!(pending.fixed_target.is_none());
        assert!(rs.pending_rewards.is_empty());
    }

    #[test]
    fn whispering_hug_auto_transforms_first_card_and_takes_damage() {
        let mut rs = fresh();
        rs.character_id = sts2_core::CharacterId::Ironclad;
        let mut strike = sts2_core::cards::Card::pod_defaults();
        strike.card_id = sts2_core::cards::CardId::StrikeIronclad;
        rs.master_deck.push(strike);
        run(&WHISPERING_HOLLOW, "hug", &mut rs);
        assert_eq!(rs.current_hp, 40 - WHISPERING_HUG_DAMAGE);
        assert_eq!(rs.master_deck.len(), 1);
        assert_ne!(
            rs.master_deck[0].card_id,
            sts2_core::cards::CardId::StrikeIronclad
        );
    }

    #[test]
    fn whispering_is_allowed_requires_44_gold() {
        let mut rs = fresh();
        rs.gold = 43;
        assert!(!(WHISPERING_HOLLOW.is_allowed)(&rs));
        rs.gold = 44;
        assert!((WHISPERING_HOLLOW.is_allowed)(&rs));
    }

    // ── mutant-coverage pins (sts2-content cargo-mutants survivors) ──

    /// Kills `slippery_bridge_overcome -> Default::default()` (line 144):
    /// the flavor-only handler still closes the event (`finished: true`),
    /// which the default (`finished: false`) would not.
    #[test]
    fn slippery_bridge_overcome_finishes() {
        let mut rs = fresh();
        let outcome = slippery_bridge_overcome(&mut rs);
        assert!(
            outcome.finished,
            "overcome closes the event (not a default no-op outcome)"
        );
    }

    #[test]
    fn slippery_bridge_overcome_removes_one_rng_chosen_card() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::CardId;
        let mut rs = fresh();
        // A fresh-Ironclad-style deck: 5 Strike, 4 Defend, 1 Bash. All Basic +
        // non-Eternal, so the candidate pool is all 10 (C# IsRemovable).
        for _ in 0..5 {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false).unwrap(),
            );
        }
        for _ in 0..4 {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(CardId::DefendIronclad, false).unwrap(),
            );
        }
        rs.master_deck
            .push(crate::cards::spec_index::card_pod_from_id(CardId::Bash, false).unwrap());
        let before = rs.master_deck.len();
        let outcome = slippery_bridge_overcome(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.master_deck.len(), before - 1, "exactly one card removed");
    }

    /// The Overcome option names the card it will take, and it must name the
    /// SAME card the handler then removes — a preview that rolls its own pick
    /// would advertise one card and eat another.
    #[test]
    fn slippery_bridge_preview_names_the_card_overcome_removes() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::CardId;
        let mut rs = fresh();
        for id in [
            CardId::StrikeIronclad,
            CardId::DefendIronclad,
            CardId::Bash,
            CardId::Anger,
            CardId::Armaments,
        ] {
            rs.master_deck
                .push(crate::cards::spec_index::card_pod_from_id(id, false).unwrap());
        }
        let previewed = slippery_bridge_card_preview(&rs).expect("a removable card");
        // Previewing must not consume the event stream: ask twice, and the
        // handler afterwards must still agree.
        assert_eq!(
            slippery_bridge_card_preview(&rs).as_deref(),
            Some(&*previewed)
        );
        let before = rs.master_deck.clone();
        slippery_bridge_overcome(&mut rs);
        let removed = before
            .iter()
            .find(|c| {
                before.iter().filter(|b| b.card_id == c.card_id).count()
                    > rs.master_deck
                        .iter()
                        .filter(|a| a.card_id == c.card_id)
                        .count()
            })
            .expect("one card left the deck");
        assert_eq!(
            previewed,
            format!("{}.title", removed.card_id.to_python_name())
        );
    }

    #[test]
    fn slippery_bridge_card_to_lose_is_deterministic_under_same_seed() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::CardId;
        let mut rs = fresh();
        rs.rng.seed = 12345;
        for _ in 0..5 {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false).unwrap(),
            );
        }
        rs.master_deck
            .push(crate::cards::spec_index::card_pod_from_id(CardId::Bash, false).unwrap());
        let mut a = sts2_core::dotnet_rng::Rng::with_name(rs.rng.seed, SLIPPERY_BRIDGE_STREAM);
        let mut b = sts2_core::dotnet_rng::Rng::with_name(rs.rng.seed, SLIPPERY_BRIDGE_STREAM);
        assert_eq!(
            slippery_bridge_card_to_lose(&rs, &mut a),
            slippery_bridge_card_to_lose(&rs, &mut b),
            "same seed ⇒ same chosen index (fresh event stream, counter 0)"
        );
    }

    /// Multi-page escalation (C# `SlipperyBridge.HoldOn`): each HoldOn is
    /// NOT finished, damages 3 + NumberOfHoldOns, bumps the page, and
    /// rerolls the offered card without repeating a skipped one.
    #[test]
    fn slippery_bridge_hold_on_escalates_and_loops() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::CardId;
        let mut rs = fresh();
        rs.rng.seed = 777;
        rs.current_hp = 60;
        for _ in 0..5 {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false).unwrap(),
            );
        }
        rs.master_deck
            .push(crate::cards::spec_index::card_pod_from_id(CardId::Bash, false).unwrap());

        let first = slippery_bridge_hold_on(&mut rs);
        assert!(!first.finished, "HoldOn presents the next page");
        assert_eq!(rs.event_state.page, 1);
        assert_eq!(rs.current_hp, 57, "first HoldOn costs 3");

        let offered_after_first = rs.event_state.scratch[0];
        let skipped = rs.event_state.scratch[1];
        assert_ne!(
            offered_after_first, skipped,
            "reroll must not re-offer the just-skipped card"
        );

        let second = slippery_bridge_hold_on(&mut rs);
        assert!(!second.finished);
        assert_eq!(rs.event_state.page, 2);
        assert_eq!(rs.current_hp, 53, "second HoldOn costs 3+1");

        let before = rs.master_deck.len();
        let done = slippery_bridge_overcome(&mut rs);
        assert!(done.finished, "Overcome exits the loop");
        assert_eq!(rs.master_deck.len(), before - 1);
    }

    /// GH #302: the presented `hold_on` text must quote the CURRENT page's
    /// cost, the way C# re-binds `HpLoss` to `CurrentHpLoss` each page — a
    /// static "Take 3 damage" makes every description-parsing consumer
    /// (`ppo.encoder.event_payload`, the GH #300 Python suicide mask) price
    /// a 12-damage click at 3.
    #[test]
    fn slippery_bridge_options_quote_the_current_page_cost() {
        let mut rs = fresh();
        let render = |rs: &RunState| {
            let opts = (SLIPPERY_BRIDGE.generate_options)(rs, &SLIPPERY_BRIDGE);
            assert_eq!(opts.len(), 2);
            assert_eq!(opts[0].description, "Lose a random card.");
            opts[1].description.clone()
        };
        assert_eq!(render(&rs), "Take 3 damage, reroll card.");
        rs.event_state.page = 9;
        assert_eq!(render(&rs), "Take 12 damage, reroll card.");
    }

    /// Kills `spiraling_whirlpool_observe -> Default::default()` (line 191):
    /// observe queues a card-reward placeholder AND finishes; the default
    /// outcome would skip the reward and leave `finished` false.
    #[test]
    fn spiraling_whirlpool_observe_queues_reward_and_finishes() {
        let mut rs = fresh();
        let outcome = spiraling_whirlpool_observe(&mut rs);
        assert!(outcome.finished);
        assert_eq!(
            rs.pending_rewards.len(),
            1,
            "observe queues one card-reward placeholder"
        );
    }

    /// Kills `wongo_featured -> Default::default()`: featured spends 200g AND
    /// hands over a relic, neither of which the default outcome would do.
    #[test]
    fn wongo_featured_spends_and_grants_a_relic() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = wongo_state();
        let outcome = wongo_featured(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, 500 - WONGO_FEATURED_COST);
        assert_eq!(rs.relics.len(), 1);
    }

    /// `wongo_leave` closes the event and (per C# `Leave`) downgrades one
    /// event-RNG-chosen upgraded deck card. On a deck with no upgraded cards
    /// it is a no-op leg but still finishes.
    #[test]
    fn wongo_leave_finishes() {
        let mut rs = fresh();
        let outcome = wongo_leave(&mut rs);
        assert!(
            outcome.finished,
            "leave closes the event (not a default no-op outcome)"
        );
    }

    /// `wongo_leave` downgrades exactly one upgraded card via the event RNG,
    /// leaving the rest of the deck (and any non-upgraded cards) untouched.
    #[test]
    fn wongo_leave_downgrades_one_upgraded_card() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::CardId;
        let mut rs = fresh();
        rs.rng.seed = 4242;
        // Two upgraded cards + one base card. Exactly one should downgrade.
        rs.master_deck
            .push(crate::cards::spec_index::card_pod_from_id(CardId::Bash, true).unwrap());
        rs.master_deck.push(
            crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false).unwrap(),
        );
        rs.master_deck
            .push(crate::cards::spec_index::card_pod_from_id(CardId::Bash, true).unwrap());
        let upgraded_before = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(upgraded_before, 2);
        let outcome = wongo_leave(&mut rs);
        assert!(outcome.finished);
        let upgraded_after = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(
            upgraded_after, 1,
            "exactly one upgraded card was downgraded"
        );
        assert_eq!(
            rs.master_deck.len(),
            3,
            "deck size unchanged by a downgrade"
        );
    }

    /// The downgrade target is deterministic under a fixed seed: the event RNG
    /// is a fresh per-event stream (counter 0, no `CalculateVars` draws), so
    /// the same seed always picks the same upgraded card.
    #[test]
    fn wongo_leave_pick_is_deterministic_under_same_seed() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::CardId;
        let build = || {
            let mut rs = fresh();
            rs.rng.seed = 99;
            for _ in 0..4 {
                rs.master_deck
                    .push(crate::cards::spec_index::card_pod_from_id(CardId::Bash, true).unwrap());
            }
            rs
        };
        let mut a = build();
        let mut b = build();
        wongo_leave(&mut a);
        wongo_leave(&mut b);
        let downgraded_idx = |rs: &RunState| {
            rs.master_deck
                .iter()
                .position(|c| !c.upgraded)
                .expect("one card downgraded")
        };
        assert_eq!(
            downgraded_idx(&a),
            downgraded_idx(&b),
            "same seed ⇒ same downgrade target (fresh event stream, counter 0)"
        );
    }

    /// Kills the `waterlogged_is_allowed` mutants (line 452: `-> true` /
    /// `-> false`, `>=` → `<`): the gate is `gold >= 55`, so 55g is allowed
    /// and 54g is not. Boundary + both-sides coverage rules out the
    /// constant-return and flipped-comparison mutants.
    #[test]
    fn waterlogged_is_allowed_gates_on_quill_cost() {
        let mut rs = fresh();
        rs.gold = WATERLOGGED_QUILL_COST; // exactly 55
        assert!(
            waterlogged_is_allowed(&rs),
            "exactly 55g is allowed (>= boundary, not const false / <)"
        );
        rs.gold = WATERLOGGED_QUILL_COST - 1; // 54
        assert!(
            !waterlogged_is_allowed(&rs),
            "54g is NOT allowed (>= boundary, not const true / <)"
        );
    }

    /// Kills the `wongo_is_allowed` mutants (line 572: `-> false` / `-> true`,
    /// `&&` → `||`, `>=` → `<`, `==` → `!=`). The gate is
    /// `act_index == 2 && gold >= 100`:
    ///  - act 2 + 100g  → allowed (rules out const-false, the `>=` flip at
    ///    the boundary, and the `==`→`!=` flip on the act side),
    ///  - act 2 + 0g    → NOT allowed (rules out const-true and `&&`→`||`,
    ///    since only the left clause holds),
    ///  - act 1 + 500g  → NOT allowed (rules out `==`→`!=`, which would
    ///    accept any non-2 act).
    #[test]
    fn wongo_is_allowed_requires_act_2_and_100_gold() {
        let mut rs = fresh();
        rs.act_index = 2;
        rs.gold = WONGO_BARGAIN_COST; // exactly 100
        assert!(
            wongo_is_allowed(&rs),
            "act 2 + 100g is allowed (>= boundary; not const false; not == flip)"
        );
        rs.gold = 0;
        assert!(
            !wongo_is_allowed(&rs),
            "act 2 + 0g is NOT allowed (&& not ||; not const true)"
        );
        rs.act_index = 1;
        rs.gold = 500;
        assert!(
            !wongo_is_allowed(&rs),
            "act 1 + 500g is NOT allowed (== not !=)"
        );
    }

    #[test]
    fn all_specs_carry_matching_python_name() {
        for spec in [
            &RELIC_TRADER,
            &SLIPPERY_BRIDGE,
            &SPIRALING_WHIRLPOOL,
            &STONE_OF_ALL_TIME,
            &SYMBIOTE,
            &THE_FUTURE_OF_POTIONS,
            &WATERLOGGED_SCRIPTORIUM,
            &WELCOME_TO_WONGOS,
            &WHISPERING_HOLLOW,
        ] {
            assert_eq!(spec.python_name, spec.id.python_name());
            assert!(
                !spec.options.is_empty(),
                "{} has no options",
                spec.python_name
            );
        }
    }
}
