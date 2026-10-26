// port-pin-skip — dispatch registry glue (no Python counterpart).
//! Event-spec lookup + choice resolution. Bead `sts2-rs-6ynt.4.1`
//! (P7 events framework).
//!
//! Mirrors the registry side of
//! `sts2-rl-agent@01c094d:sts2_env/run/events.py` (`_EVENT_REGISTRY`,
//! `register_event`, `get_event`). The Rust registry is a static
//! match on [`EventId`] rather than a runtime dict — simpler, no
//! init-order pitfalls, and the per-variant exhaustiveness check
//! catches missing entries at compile time.

use sts2_core::events::EventEncounter;
use sts2_core::{apply_event_effects, EventId, EventOptionInfo, EventOutcome, RunState};

use super::act1_overgrowth::{BRAIN_LEECH, ROOM_FULL_OF_CHEESE, TEA_MASTER, THE_LEGENDS_WERE_TRUE};
use super::act2_hive::{
    CRYSTAL_SPHERE, DOLL_ROOM, ENDLESS_CONVEYOR, FAKE_MERCHANT, FIELD_OF_MAN_SIZED_HOLES,
    JUNGLE_MAZE_ADVENTURE, LUMINOUS_CHOIR, MORPHIC_GROVE, POTION_COURIER, RANWID_THE_ELDER,
};
use super::act2_hive_wave_b::{
    RELIC_TRADER, SLIPPERY_BRIDGE, SPIRALING_WHIRLPOOL, STONE_OF_ALL_TIME, SYMBIOTE,
    THE_FUTURE_OF_POTIONS, WATERLOGGED_SCRIPTORIUM, WELCOME_TO_WONGOS, WHISPERING_HOLLOW,
};
use super::act3_glory::{
    DEPRECATED_ANCIENT_EVENT, DEPRECATED_EVENT, NEOW, THE_ARCHITECT, WAR_HISTORIAN_REPY,
};
use super::neow_blessing::NEOWS_BLESSING;
use super::shared::{
    ABYSSAL_BATHS, AMALGAMATOR, AROMA_OF_CHAOS, BATTLEWORN_DUMMY, BUGSLAYER, BYRDONIS_NEST,
    COLORFUL_PHILOSOPHERS, COLOSSAL_FLOWER, DARV, DENSE_VEGETATION, DOORS_OF_LIGHT_AND_DARK,
    DROWNING_BEACON, GRAVE_OF_THE_FORGOTTEN, HUNGRY_FOR_MUSHROOMS, INFESTED_AUTOMATON, LOST_WISP,
    NONUPEIPE, OROBAS, PAEL, PUNCH_OFF, TRASH_HEAP,
};
use super::shared_wave_b::{
    REFLECTIONS, ROUND_TEA_PARTY, SAPPHIRE_SEED, SELF_HELP_BOOK, SPIRIT_GRAFTER, SUNKEN_STATUE,
    SUNKEN_TREASURY, TABLET_OF_TRUTH, TANX, TEZCATARA, THE_LANTERN_KEY, THIS_OR_THAT, TINKER_TIME,
    TRIAL, UNREST_SITE, VAKUU, WELLSPRING, WOOD_CARVINGS, ZEN_WEAVER,
};
use super::EventSpec;

/// Resolve an [`EventId`] to its static [`EventSpec`].
///
/// Panics if `id` has no registered spec — every [`EventId`] variant
/// MUST be wired in this match. The
/// [`tests::every_event_id_has_a_spec`] regression guard enforces
/// this so a new per-act sub-bead can't accidentally land an
/// [`EventId`] variant without a spec.
#[must_use]
pub fn spec_for(id: EventId) -> &'static EventSpec {
    match id {
        EventId::LostWisp => &LOST_WISP,
        EventId::TrashHeap => &TRASH_HEAP,
        EventId::TheArchitect => &THE_ARCHITECT,
        EventId::WarHistorianRepy => &WAR_HISTORIAN_REPY,
        EventId::Neow => &NEOW,
        // Bead `sts2-rs-8tkle` — Neow's Blessing (run-start 5-boon).
        EventId::NeowsBlessing => &NEOWS_BLESSING,
        EventId::DeprecatedEvent => &DEPRECATED_EVENT,
        EventId::DeprecatedAncientEvent => &DEPRECATED_ANCIENT_EVENT,
        // Bead `sts2-rs-6ynt.4.2` — 4 act1-pool events.
        EventId::BrainLeech => &BRAIN_LEECH,
        EventId::RoomFullOfCheese => &ROOM_FULL_OF_CHEESE,
        EventId::TheLegendsWereTrue => &THE_LEGENDS_WERE_TRUE,
        EventId::TeaMaster => &TEA_MASTER,
        // Bead `sts2-rs-6ynt.4.4` — 19 shared-pool events wave A
        // (A–N order from shared.py).
        EventId::AbyssalBaths => &ABYSSAL_BATHS,
        EventId::Amalgamator => &AMALGAMATOR,
        EventId::AromaOfChaos => &AROMA_OF_CHAOS,
        EventId::BattlewornDummy => &BATTLEWORN_DUMMY,
        EventId::Bugslayer => &BUGSLAYER,
        EventId::ByrdonisNest => &BYRDONIS_NEST,
        EventId::ColorfulPhilosophers => &COLORFUL_PHILOSOPHERS,
        EventId::ColossalFlower => &COLOSSAL_FLOWER,
        EventId::Darv => &DARV,
        EventId::DenseVegetation => &DENSE_VEGETATION,
        EventId::DoorsOfLightAndDark => &DOORS_OF_LIGHT_AND_DARK,
        EventId::DrowningBeacon => &DROWNING_BEACON,
        EventId::GraveOfTheForgotten => &GRAVE_OF_THE_FORGOTTEN,
        EventId::HungryForMushrooms => &HUNGRY_FOR_MUSHROOMS,
        EventId::InfestedAutomaton => &INFESTED_AUTOMATON,
        EventId::Nonupeipe => &NONUPEIPE,
        EventId::Orobas => &OROBAS,
        EventId::Pael => &PAEL,
        EventId::PunchOff => &PUNCH_OFF,
        // Bead `sts2-rs-6ynt.4.5` — 19 shared-pool events wave B
        // (R–Z order from shared.py).
        EventId::Reflections => &REFLECTIONS,
        EventId::RoundTeaParty => &ROUND_TEA_PARTY,
        EventId::SapphireSeed => &SAPPHIRE_SEED,
        EventId::SelfHelpBook => &SELF_HELP_BOOK,
        EventId::SpiritGrafter => &SPIRIT_GRAFTER,
        EventId::SunkenStatue => &SUNKEN_STATUE,
        EventId::SunkenTreasury => &SUNKEN_TREASURY,
        EventId::TabletOfTruth => &TABLET_OF_TRUTH,
        EventId::Tanx => &TANX,
        EventId::Tezcatara => &TEZCATARA,
        EventId::TheLanternKey => &THE_LANTERN_KEY,
        EventId::ThisOrThat => &THIS_OR_THAT,
        EventId::TinkerTime => &TINKER_TIME,
        EventId::Trial => &TRIAL,
        EventId::UnrestSite => &UNREST_SITE,
        EventId::Vakuu => &VAKUU,
        EventId::Wellspring => &WELLSPRING,
        EventId::WoodCarvings => &WOOD_CARVINGS,
        EventId::ZenWeaver => &ZEN_WEAVER,
        // Bead `sts2-rs-6ynt.4.6` — 10 act2-pool events wave A
        // (alphabetical first half of act2.py).
        EventId::CrystalSphere => &CRYSTAL_SPHERE,
        EventId::DollRoom => &DOLL_ROOM,
        EventId::EndlessConveyor => &ENDLESS_CONVEYOR,
        EventId::FakeMerchant => &FAKE_MERCHANT,
        EventId::FieldOfManSizedHoles => &FIELD_OF_MAN_SIZED_HOLES,
        EventId::JungleMazeAdventure => &JUNGLE_MAZE_ADVENTURE,
        EventId::LuminousChoir => &LUMINOUS_CHOIR,
        EventId::MorphicGrove => &MORPHIC_GROVE,
        EventId::PotionCourier => &POTION_COURIER,
        EventId::RanwidTheElder => &RANWID_THE_ELDER,
        // Bead `sts2-rs-6ynt.4.7` — 9 act2-pool events wave B
        // (R–Z order from act2.py).
        EventId::RelicTrader => &RELIC_TRADER,
        EventId::SlipperyBridge => &SLIPPERY_BRIDGE,
        EventId::SpiralingWhirlpool => &SPIRALING_WHIRLPOOL,
        EventId::StoneOfAllTime => &STONE_OF_ALL_TIME,
        EventId::Symbiote => &SYMBIOTE,
        EventId::TheFutureOfPotions => &THE_FUTURE_OF_POTIONS,
        EventId::WaterloggedScriptorium => &WATERLOGGED_SCRIPTORIUM,
        EventId::WelcomeToWongos => &WELCOME_TO_WONGOS,
        EventId::WhisperingHollow => &WHISPERING_HOLLOW,
    }
}

/// Present the option list for the event currently active in
/// [`RunState::current_event`]. Returns the dynamically-built
/// [`EventOptionInfo`] list via the spec's
/// [`super::EventGenerateOptionsFn`].
///
/// Returns `None` if no event is active.
#[must_use]
pub fn present(state: &RunState) -> Option<Vec<EventOptionInfo>> {
    let id = state.current_event?;
    let spec = spec_for(id);
    let mut opts = (spec.generate_options)(state, spec);
    // Backfill the localized-template values from the spec. Roughly half the
    // events present themselves with a hand-written `generate_options` that
    // builds `EventOptionInfo` literals directly — those never see
    // `default_vars` / `vars_fn`, so without this an event's values would
    // silently depend on HOW it presents rather than on what it declares, and
    // the display layer would fall back to the engine's shorthand for exactly
    // those events. A builder that set `vars` itself still wins.
    for info in &mut opts {
        if !info.vars.is_empty() {
            continue;
        }
        if let Some(opt) = spec.options.iter().find(|o| o.option_id == info.option_id) {
            info.vars = super::option_vars(state, opt);
            info.title_vars = super::option_title_vars(state, opt);
        }
    }
    Some(opts)
}

/// Resolve a choice for the event currently active in
/// [`RunState::current_event`] by `option_id`. Applies the matching
/// handler, clears `current_event` on a finished outcome, and
/// returns the outcome.
///
/// Returns `None` if (a) no event is active, or (b) no option with
/// `option_id` exists in the spec.
pub fn resolve(state: &mut RunState, option_id: &str) -> Option<EventOutcome> {
    let id = state.current_event?;
    let spec = spec_for(id);
    let opt = spec.options.iter().find(|o| o.option_id == option_id)?;
    let outcome = (opt.handler)(state);
    // Bead sts2-rs-bv816: declarative effects fire BEFORE the
    // current_event slot is cleared so a hypothetical mid-chain
    // hook reading current_event still sees the right id.
    apply_event_effects(state, &outcome.effects);
    if outcome.finished {
        state.exit_event_room();
    }
    Some(outcome)
}

/// Resolve a choice by 0-based option index instead of `option_id`.
/// Mirrors the agent-facing surface where the policy network picks
/// the option by its position in the generated list.
pub fn resolve_by_index(state: &mut RunState, option_idx: usize) -> Option<EventOutcome> {
    let id = state.current_event?;
    let spec = spec_for(id);
    // Index into the PRESENTED (current-page) option list, not the flat
    // static `spec.options`. For a multi-page event the static list holds
    // every page's options, so a raw `spec.options[idx]` would pick the
    // wrong page's handler. For single-page events the default builder
    // presents `spec.options` in order, so this is identical. The presented
    // `option_id` then keys the handler in the static list. Bead
    // `sts2-rs-2guyj`.
    let presented = (spec.generate_options)(state, spec);
    let info = presented.get(option_idx)?;
    let opt = spec
        .options
        .iter()
        .find(|o| o.option_id == info.option_id)?;
    let outcome = (opt.handler)(state);
    apply_event_effects(state, &outcome.effects);
    if outcome.finished {
        state.exit_event_room();
    }
    Some(outcome)
}

/// Structured per-option effect deltas for the PPO run-policy encoder's
/// event payload features (GH #237). The encoder historically derived
/// `gold_delta` / `hp_delta` by regex-scraping the option's English
/// description; those numbers are already known to the engine, so this
/// surfaces them directly.
///
/// Computed by cloning the run state, resolving the option on the clone,
/// and diffing the scalar pools — so it captures BOTH declarative
/// [`sts2_core::EventEffect`]s and direct-mutation handlers (the legacy
/// `6ynt.4.x` style), and never touches the live state's RNG, reward
/// queues, or `current_event`.
/// The counts are GAINED/LOST pairs rather than one signed number because
/// three events trade: Relic Trader takes a relic and gives one back, and a
/// deck transform removes a card and adds one. Netting those to zero is what
/// the payload used to do implicitly by not carrying them at all — the option
/// then featurized identically to one that does nothing, and Stone of All
/// Time's "drink a potion for +10 Max HP" looked exactly like a free +10.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OptionEffectDelta {
    /// Signed gold change (gain +, cost/pay −).
    pub gold_delta: i32,
    /// Signed current-HP change (heal +, damage −). A `GainMaxHp` also
    /// bumps current HP by its contract, so a max-HP gain shows here too.
    pub hp_delta: i32,
    /// Signed max-HP change (independent of `hp_delta`).
    pub max_hp_delta: i32,
    /// Relics the option puts in your hands or on offer.
    pub relics_gained: i32,
    /// Relics it takes off you.
    pub relics_lost: i32,
    /// Potions gained or offered.
    pub potions_gained: i32,
    /// Potions drunk, discarded or traded away.
    pub potions_lost: i32,
    /// Cards added to the deck, plus card rewards put on offer (a
    /// choose-1-of-3 counts as the one card it can become).
    pub cards_gained: i32,
    /// Cards removed from the deck.
    pub cards_lost: i32,
}

/// How many entries `after` has that `before` does not, counting duplicates —
/// i.e. `|after \ before|` over multisets.
///
/// Used both ways round to split a change into gained and lost, so a SWAP
/// (one out, one in) reports `(1, 1)` instead of netting to nothing.
fn multiset_added<T: Ord + Copy>(before: &[T], after: &[T]) -> i32 {
    let mut remaining: std::collections::BTreeMap<T, i32> = std::collections::BTreeMap::new();
    for item in before {
        *remaining.entry(*item).or_insert(0) += 1;
    }
    let mut added = 0;
    for item in after {
        match remaining.get_mut(item) {
            Some(n) if *n > 0 => *n -= 1,
            _ => added += 1,
        }
    }
    added
}

/// Preview the structured effect of resolving option `idx` of the
/// currently-active event WITHOUT mutating `state`. Returns `None` when
/// no event is active or `idx` is out of range for the presented option
/// list (same guards as [`resolve_by_index`], which does the resolve on
/// the throwaway clone).
///
/// Resolving on a clone is deterministic w.r.t. the live state: the
/// preview advances only the clone's RNG, so a later real resolution of
/// the same option (drawing from the untouched live RNG) yields the same
/// deltas for RNG-dependent handlers. GH #237.
#[must_use]
pub fn preview_option_effect(state: &RunState, idx: usize) -> Option<OptionEffectDelta> {
    let mut clone = state.clone();
    resolve_by_index(&mut clone, idx)?;
    // "In your hands or on offer": under `direct_event_relic_grant` (what the
    // arena and the PPO env both run) an event relic lands straight in
    // `relics`, but the Python-parity flow queues it as a reward to confirm.
    // Counting both keeps the feature reading the same number either way, and
    // they cannot double-count — `grant_event_relic` takes one branch or the
    // other. The two are appended so a queued reward reads as a gain, never a
    // loss.
    let relics = |s: &RunState| {
        let mut v = s.relics.clone();
        v.extend(s.relic_rewards.iter().map(|r| r.relic_id));
        v
    };
    let potions = |s: &RunState| {
        let mut v: Vec<i32> = s.potions.iter().map(|p| *p as i32).collect();
        // Potion rewards are frequently `PotionReward::empty()` sentinels
        // rolled at drain time, so they carry no identity to match on. A
        // negative sentinel keeps them out of the owned multiset while still
        // counting one apiece.
        v.extend(
            s.pending_rewards
                .iter()
                .filter(|r| matches!(r, sts2_core::Reward::Potion(_)))
                .enumerate()
                .map(|(i, _)| -1 - i32::try_from(i).unwrap_or(0)),
        );
        v
    };
    let cards = |s: &RunState| {
        let mut v: Vec<i32> = s.master_deck.iter().map(|c| c.card_id as i32).collect();
        v.extend(
            s.pending_rewards
                .iter()
                .filter(|r| matches!(r, sts2_core::Reward::Card(_)))
                .enumerate()
                .map(|(i, _)| -1 - i32::try_from(i).unwrap_or(0)),
        );
        v
    };
    let (before_relics, after_relics) = (relics(state), relics(&clone));
    let (before_potions, after_potions) = (potions(state), potions(&clone));
    let (before_cards, after_cards) = (cards(state), cards(&clone));
    Some(OptionEffectDelta {
        gold_delta: clone.gold - state.gold,
        hp_delta: clone.current_hp - state.current_hp,
        max_hp_delta: clone.max_hp - state.max_hp,
        relics_gained: multiset_added(&before_relics, &after_relics),
        relics_lost: multiset_added(&after_relics, &before_relics),
        potions_gained: multiset_added(&before_potions, &after_potions),
        potions_lost: multiset_added(&after_potions, &before_potions),
        cards_gained: multiset_added(&before_cards, &after_cards),
        cards_lost: multiset_added(&after_cards, &before_cards),
    })
}

/// Pick the next allowed event from the current act's event pool.
/// Mirrors Python `pick_event` at
/// `sts2-rl-agent@01c094d:sts2_env/run/events.py:241`.
///
/// Walks the pool starting at the current act's `events_visited`
/// offset (wrapping modulo `pool.len()`), skipping any candidate
/// whose spec returns `is_allowed(state) == false` or whose
/// `EventId` is already present in `state.visited_event_ids`. On
/// success, marks the picked id as visited, increments
/// `act.events_visited`, and returns the [`EventId`].
///
/// `pool` defaults to the current act's `event_ids_shuffled` (the
/// per-run shuffled mirror populated by `RunManager::initialize_run`),
/// falling back to the static `event_ids` slice when the shuffled
/// list is empty (e.g. before `initialize_run` has run).
///
/// Returns `None` when the pool is empty OR every candidate was
/// filtered out (and the pool is a caller-supplied override — when
/// using the default act-event order, Python falls back to the
/// `events_visited`-th slot regardless; this driver mirrors that
/// fallback by returning the slot's `EventId` even if filtered).
///
/// Bead `sts2-rs-b0hjz` (P9.2.w event-framework scaffolding).
///
/// Design note: Python carries an `EventModel` trait object registry
/// keyed by `event_id`. This port instead reaches `is_allowed` via
/// the existing static [`EventSpec`] table (`spec_for`). Semantics
/// are equivalent; the registry/get_event/all_events accessors the
/// bead description called for already exist in spec-shape under
/// [`spec_for`] + [`sts2_core::EventId::all`].
#[must_use]
pub fn pick_event(state: &mut RunState, pool: Option<&[&str]>) -> Option<EventId> {
    use sts2_core::EventId;

    // Pool selection: caller override > shuffled act pool > static
    // act pool. The `uses_act_event_order` flag tracks whether we
    // walk under the act's `events_visited` cursor or restart at 0
    // (matches Python's `pool is act.event_ids` identity check).
    let uses_act_event_order = pool.is_none();
    let act_idx = state.act_index() as usize;
    debug_assert!(
        act_idx >= 1 && act_idx <= state.acts.len(),
        "act_index out of range"
    );
    let act = &state.acts[act_idx - 1];

    // Materialize the working pool as an owned Vec<EventId> so we
    // drop the borrow on `state.acts` before mutating anywhere else.
    let pool_ids: Vec<EventId> = if let Some(p) = pool {
        p.iter()
            .filter_map(|name| EventId::from_python_name(name))
            .collect()
    } else {
        // C#-faithful source (bead `sts2-rs-1hiht`): the per-act
        // `RoomSet.events` shuffled order recorded by `run_room_gen`
        // (`AllEvents ++ AllSharedEvents` UnstableShuffled off `up_front`).
        // Falls back to the legacy in-`acts` mirror, then the static slice,
        // for raw-core test paths that skip the `generated_rooms` setup.
        let gen_events: &[&str] = state
            .generated_rooms
            .get(act_idx - 1)
            .map(|g| &g.events_shuffled[..])
            .filter(|s| !s.is_empty())
            .unwrap_or(if act.event_ids_shuffled.is_empty() {
                act.event_ids
            } else {
                &act.event_ids_shuffled[..]
            });
        gen_events
            .iter()
            .filter_map(|n| EventId::from_python_name(n))
            .collect()
    };

    if pool_ids.is_empty() {
        return None;
    }

    let start = if uses_act_event_order {
        act.events_visited as usize
    } else {
        0
    };

    let mut event_index = start;
    let mut picked: Option<EventId> = None;
    for _ in 0..pool_ids.len() {
        let candidate = pool_ids[event_index % pool_ids.len()];
        if !state.visited_event_ids.contains(&candidate) && (spec_for(candidate).is_allowed)(state)
        {
            picked = Some(candidate);
            break;
        }
        event_index += 1;
    }

    // Python fallback: when walking the act event order and nothing
    // passed the filters, the cursor's current slot is returned anyway
    // (the act event-room must produce *something*). Caller-supplied
    // pools get strict `None` semantics.
    if picked.is_none() && uses_act_event_order {
        picked = Some(pool_ids[event_index % pool_ids.len()]);
    }

    // Mutate state only after picking (avoids borrow conflicts).
    if let Some(id) = picked {
        state.visited_event_ids.insert(id);
        if uses_act_event_order {
            event_index = event_index.wrapping_add(1);
            state.acts[act_idx - 1].events_visited = event_index as u32;
        }
    }
    picked
}

/// `resolve_event_at` callback for [`sts2_core::run_manager::ContentCallbacks`].
///
/// End-to-end Event-phase driver:
/// 1. If [`sts2_core::RunState::current_event`] is `None`, calls
///    [`pick_event`] with `pool = None` to roll the next allowed event
///    from the current act's `event_ids_shuffled` cursor (with the
///    per-run visited-set + `is_allowed` filters already applied),
///    then [`sts2_core::RunState::enter_event_room`] to bind the
///    picked id. If `current_event` is already set (e.g.
///    [`sts2_core::run_manager::RunManager::with_neow_start`] pre-binds
///    Neow before the agent gets the first action), the pick + enter
///    steps are skipped.
/// 2. Calls [`resolve_by_index`] with `option_idx = idx` to apply the
///    picked option's handler. The spec's handler returns
///    [`EventOutcome::finished`] for terminal options, which causes
///    [`sts2_core::RunState::exit_event_room`] to clear
///    `current_event` on the way out.
///
/// Returns `true` if a choice was applied, `false` if no event could
/// be picked (empty pool / all filtered out under a caller-supplied
/// override) OR if `idx` is out of range for the active event's
/// option list.
///
/// `None` from [`pick_event`] is a no-op — the caller stays in
/// `Phase::Event` with no `current_event`. With the current act-1 pool
/// + the act-1-events shipped by `sts2-rs-6ynt.4.2` + the shared-pool
/// events shipped by `sts2-rs-6ynt.4.4` / `6ynt.4.5`, the pool is
/// never empty for an Ironclad run, so the `None` path only triggers
/// under custom test scaffolding.
///
/// Bead `sts2-rs-nzdz1` (multi-option idx plumbing on top of the
/// `sts2-rs-81xxo` PARTITION-C single-option callback this replaces).
pub fn resolve_event_at_callback(state: &mut RunState, idx: usize) -> bool {
    if state.current_event.is_none() {
        let Some(event_id) = pick_event(state, None) else {
            return false;
        };
        state.enter_event_room(event_id);
    }
    resolve_by_index(state, idx).is_some()
}

/// `event_option_count` callback for [`sts2_core::run_manager::ContentCallbacks`].
///
/// Returns the number of options the currently-active event offers.
/// Returns 0 when no event is active (`current_event == None`). The
/// engine uses this in `get_available_actions` to enumerate one
/// `RunAction::EventChoice { idx }` per option. Bead `sts2-rs-nzdz1`.
#[must_use]
pub fn event_option_count_callback(state: &RunState) -> usize {
    present(state).map_or(0, |opts| opts.len())
}

/// `resume_event_after_combat` callback for
/// [`sts2_core::run_manager::ContentCallbacks`].
///
/// Dispatches a finished event→combat back to its parent event's resume
/// handler (C# `EventModel.Resume(room)`). Only `resume`-flagged encounters
/// reach here: `BattlewornDummy` grants its per-setting reward via
/// [`super::shared::battleworn_dummy_resume`] (gated on `clean_victory` —
/// `RanOutOfTime` → no reward). `MysteriousKnight` (TheLanternKey) and
/// `FakeMerchant` are no-resume — their victory rewards are granted directly
/// by the run loop — so they are no-ops here. Bead `sts2-rs-ck9z6` (GH #166).
pub fn resume_event_after_combat_callback(
    state: &mut RunState,
    encounter: &EventEncounter,
    clean_victory: bool,
) {
    match encounter {
        EventEncounter::BattlewornDummy(setting) => {
            super::shared::battleworn_dummy_resume(state, *setting, clean_victory);
        }
        EventEncounter::MysteriousKnight | EventEncounter::FakeMerchant => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_id_has_a_spec() {
        // Sentinel: every EventId variant must round-trip through
        // spec_for without panicking. New sub-beads adding EventId
        // variants will fail this test until they wire the spec.
        for id in EventId::all() {
            let spec = spec_for(*id);
            assert_eq!(spec.id, *id);
            assert_eq!(spec.python_name, id.python_name());
            assert!(!spec.options.is_empty(), "{id:?} has no options");
        }
    }

    #[test]
    fn present_returns_none_outside_event_room() {
        let rs = RunState::new();
        assert!(present(&rs).is_none());
    }

    #[test]
    fn enter_event_room_then_present_returns_options() {
        let mut rs = RunState::new();
        rs.enter_event_room(EventId::TrashHeap);
        let opts = present(&rs).expect("event active");
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].option_id, "dive_in");
        assert_eq!(opts[1].option_id, "grab");
        // current_event remains set until resolve fires.
        assert_eq!(rs.current_event, Some(EventId::TrashHeap));
    }

    #[test]
    fn resolve_finished_outcome_clears_current_event() {
        let mut rs = RunState::new();
        rs.enter_event_room(EventId::TrashHeap);
        let outcome = resolve(&mut rs, "grab").expect("event active");
        assert!(outcome.finished);
        assert!(rs.current_event.is_none());
        assert_eq!(rs.gold, 99 + 100);
    }

    #[test]
    fn resolve_by_index_matches_resolve_by_option_id() {
        let mut rs_a = RunState::new();
        rs_a.enter_event_room(EventId::LostWisp);
        let out_a = resolve(&mut rs_a, "search").expect("event active");

        let mut rs_b = RunState::new();
        rs_b.enter_event_room(EventId::LostWisp);
        // "search" is the second option (index 1).
        let out_b = resolve_by_index(&mut rs_b, 1).expect("event active");

        assert_eq!(out_a, out_b);
        assert_eq!(rs_a.gold, rs_b.gold);
    }

    #[test]
    fn resolve_unknown_option_id_returns_none() {
        let mut rs = RunState::new();
        rs.enter_event_room(EventId::TrashHeap);
        assert!(resolve(&mut rs, "bogus").is_none());
        // current_event MUST stay set when resolve fails.
        assert_eq!(rs.current_event, Some(EventId::TrashHeap));
    }

    #[test]
    fn resolve_outside_event_room_returns_none() {
        let mut rs = RunState::new();
        assert!(resolve(&mut rs, "anything").is_none());
        assert!(resolve_by_index(&mut rs, 0).is_none());
    }

    // ── pick_event tests (sts2-rs-b0hjz) ─────────────────────────────

    #[test]
    fn event_id_from_python_name_round_trips() {
        for id in EventId::all() {
            let name = id.python_name();
            assert_eq!(EventId::from_python_name(name), Some(*id));
        }
        assert_eq!(EventId::from_python_name("BogusUnknown"), None);
    }

    #[test]
    fn pick_event_returns_none_for_empty_caller_pool() {
        let mut rs = RunState::new();
        assert!(pick_event(&mut rs, Some(&[])).is_none());
    }

    #[test]
    fn pick_event_filters_out_already_visited() {
        let mut rs = RunState::new();
        // Pin a 2-event caller pool; mark LostWisp visited.
        rs.visited_event_ids.insert(EventId::LostWisp);
        let pool: [&str; 2] = ["LostWisp", "TrashHeap"];
        let picked = pick_event(&mut rs, Some(&pool));
        assert_eq!(picked, Some(EventId::TrashHeap));
        // pick_event must add the picked id to visited_event_ids.
        assert!(rs.visited_event_ids.contains(&EventId::TrashHeap));
    }

    #[test]
    fn pick_event_filters_out_never_allowed() {
        // Neow / TheArchitect / DeprecatedEvent all have
        // is_allowed == never_allowed; pool containing only these
        // returns None for a caller-supplied pool (strict mode).
        let mut rs = RunState::new();
        let pool: [&str; 3] = ["Neow", "TheArchitect", "DeprecatedEvent"];
        let picked = pick_event(&mut rs, Some(&pool));
        assert_eq!(picked, None);
        // visited_event_ids untouched.
        assert!(rs.visited_event_ids.is_empty());
    }

    #[test]
    fn pick_event_rp3_post_neow_cursor_lands_on_room_full_of_cheese() {
        // Regression pin for the RP3 Ironclad run-parity divergence: the
        // act-1 shuffled order below is the live `events_shuffled` for seed
        // "RP3" (identical in C# `dump_rooms`). After the Neow room entry
        // (C# MarkRoomVisited ⇒ cursor 1), the first event room must skip
        // WarHistorianRepy (`IsAllowed => false`) and land on
        // RoomFullOfCheese at slot 2 — NOT walk any further.
        let mut rs = RunState::new();
        rs.acts[0].event_ids_shuffled = vec![
            "JungleMazeAdventure",
            "WarHistorianRepy",
            "RoomFullOfCheese",
            "CrystalSphere",
            "DollRoom",
            "SlipperyBridge",
            "PotionCourier",
            "WhisperingHollow",
            "Symbiote",
            "TheLegendsWereTrue",
            "SapphireSeed",
            "TheFutureOfPotions",
            "UnrestSite",
            "Wellspring",
            "FakeMerchant",
            "WoodCarvings",
            "RelicTrader",
            "ByrdonisNest",
            "DenseVegetation",
            "TeaMaster",
            "SelfHelpBook",
            "LuminousChoir",
            "StoneOfAllTime",
            "WelcomeToWongos",
            "ThisOrThat",
            "AromaOfChaos",
            "BrainLeech",
            "RanwidTheElder",
            "MorphicGrove",
            "SunkenStatue",
            "TabletOfTruth",
        ];
        rs.acts[0].events_visited = 1;
        let picked = pick_event(&mut rs, None);
        assert_eq!(picked, Some(EventId::RoomFullOfCheese));
        assert_eq!(rs.acts[0].events_visited, 3, "cursor = picked slot + 1");
    }

    #[test]
    fn pick_event_act_order_advances_events_visited() {
        let mut rs = RunState::new();
        // Default pool = act_1's event_ids (shuffled empty before
        // initialize_run, falls back to static slice).
        let before = rs.acts[0].events_visited;
        let picked = pick_event(&mut rs, None);
        assert!(picked.is_some(), "act_1 event pool must yield a pick");
        assert_eq!(
            rs.acts[0].events_visited,
            before + 1,
            "events_visited must advance by 1 on a successful pick"
        );
        // Picked id is now in visited_event_ids.
        assert!(rs.visited_event_ids.contains(&picked.unwrap()));
    }

    #[test]
    fn pick_event_act_order_is_deterministic() {
        // Two fresh RunStates pick identically when the pool is the
        // static fallback (event_ids_shuffled empty).
        let mut a = RunState::new();
        let mut b = RunState::new();
        for _ in 0..3 {
            assert_eq!(pick_event(&mut a, None), pick_event(&mut b, None));
        }
    }

    #[test]
    fn enter_event_room_routes_through_after_room_entered() {
        // enter_event_room should set current_event AND mark the
        // room as RoomType::Event (so after_room_entered listeners
        // — MawBank, etc. — fire just as for any other room).
        use sts2_core::RoomType;
        let mut rs = RunState::new();
        rs.enter_event_room(EventId::LostWisp);
        assert_eq!(rs.current_event, Some(EventId::LostWisp));
        let room = rs.current_room.as_ref().expect("room set");
        assert_eq!(room.room_type, RoomType::Event);
    }

    // ── resolve_event_at_callback (sts2-rs-nzdz1, was sts2-rs-81xxo) ─

    #[test]
    fn resolve_event_at_callback_picks_enters_and_resolves_first_option() {
        // End-to-end: an empty rs (no visited events, no current
        // event) gets driven through pick→enter→resolve, leaving
        // `current_event` cleared (finished-outcome path) and the
        // picked event recorded in visited_event_ids.
        let mut rs = RunState::new();
        assert!(rs.current_event.is_none());
        let before_visited = rs.visited_event_ids.len();

        assert!(resolve_event_at_callback(&mut rs, 0));

        assert!(
            rs.current_event.is_none(),
            "first-option handler should be a finished outcome that exits the event room"
        );
        assert_eq!(
            rs.visited_event_ids.len(),
            before_visited + 1,
            "pick_event must mark exactly one new event visited",
        );
    }

    #[test]
    fn resolve_event_at_callback_is_deterministic_under_same_seed() {
        // Same starting RunState → same picked event (the per-act
        // event_ids_shuffled mirror is empty at this pin, so pick_event
        // falls back to the static `event_ids` slice — deterministic
        // by index). Re-runs must mark the same id visited.
        let mut a = RunState::new();
        let mut b = RunState::new();
        assert!(resolve_event_at_callback(&mut a, 0));
        assert!(resolve_event_at_callback(&mut b, 0));
        assert_eq!(a.visited_event_ids, b.visited_event_ids);
    }

    #[test]
    fn resolve_event_at_callback_repeats_pick_different_event_each_call() {
        // Two consecutive callback firings must visit two distinct
        // events — `pick_event` filters out already-visited ids, so
        // the second call lands on the next slot in the act pool.
        let mut rs = RunState::new();
        assert!(resolve_event_at_callback(&mut rs, 0));
        let first_visited: Vec<EventId> = rs.visited_event_ids.iter().copied().collect();
        assert_eq!(first_visited.len(), 1, "{first_visited:?}");

        assert!(resolve_event_at_callback(&mut rs, 0));
        assert_eq!(
            rs.visited_event_ids.len(),
            2,
            "second call should visit a second distinct event",
        );
    }

    // ─── Bead `sts2-rs-8z3fl.8` — event-resolution outcome categories ──
    //
    // One end-to-end `dispatch::resolve()` test per outcome category
    // (heal / hp-cost / gain-card / gain-relic / lose-card). Each test
    // drives the canonical full dispatch path:
    //   1. `rs.enter_event_room(EventId::X)` (sets current_event).
    //   2. `dispatch::resolve(&mut rs, option_id)` (fires handler,
    //      applies effects, clears current_event on finished).
    //   3. Assert post-resolve `RunState` delta + that
    //      `current_event == None` (the bus shut down cleanly).
    //
    // Each test pins the Python pin commit
    // (`sts2-rl-agent@01c094d`) and the source event/option name
    // from `sts2_env/events/*.py` so future divergences surface
    // here rather than in act-1-replay corpus runs.

    /// **heal**: `AbyssalBaths.abstain` heals
    /// [`super::shared::ABYSSAL_BATHS_ABSTAIN_HEAL`] = 10 HP.
    /// Pin: `sts2-rl-agent@01c094d:sts2_env/events/shared.py::AbyssalBaths`.
    #[test]
    fn dispatch_heal_category_abyssal_baths_abstain_heals_ten_hp() {
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 50; // missing 30 HP so the heal lands fully.
        rs.enter_event_room(EventId::AbyssalBaths);
        let out = resolve(&mut rs, "abstain").expect("event active");
        assert!(out.finished, "abstain finishes (no linger)");
        assert_eq!(rs.current_hp, 60, "+10 HP per ABYSSAL_BATHS_ABSTAIN_HEAL");
        assert_eq!(rs.max_hp, 80, "max_hp unchanged");
        assert!(rs.current_event.is_none(), "event bus cleared on finished");
    }

    /// **hp-cost**: `BrainLeech.rip` deducts
    /// [`super::act1_overgrowth::BRAIN_LEECH_RIP_DAMAGE`] = 5 HP and queues a
    /// `CardReward`. Pin:
    /// `sts2-rl-agent@01c094d:sts2_env/events/act1.py::BrainLeech`.
    #[test]
    fn dispatch_hp_cost_category_brain_leech_rip_loses_five_hp() {
        let mut rs = RunState::new();
        let hp_before = rs.current_hp;
        let rewards_before = rs.pending_rewards.len();
        rs.enter_event_room(EventId::BrainLeech);
        let out = resolve(&mut rs, "rip").expect("event active");
        assert!(out.finished);
        assert_eq!(
            rs.current_hp,
            hp_before - 5,
            "Brain Leech rip costs BRAIN_LEECH_RIP_DAMAGE=5"
        );
        assert_eq!(
            rs.pending_rewards.len(),
            rewards_before + 1,
            "card reward queued"
        );
        assert!(matches!(
            rs.pending_rewards.last().expect("non-empty"),
            sts2_core::runstate::Reward::Card(_)
        ));
        assert!(rs.current_event.is_none());
    }

    /// **gain-card**: `BrainLeech.share_knowledge` queues a
    /// `CardReward` for 1-of-5 character cards (full fan-out lives
    /// in the reward dispatcher; the event-side contract is "queue
    /// the reward, no HP delta"). Pin: same as the hp-cost test.
    #[test]
    fn dispatch_gain_card_category_brain_leech_share_knowledge_queues_card_reward() {
        let mut rs = RunState::new();
        let hp_before = rs.current_hp;
        let deck_before = rs.master_deck.len();
        let rewards_before = rs.pending_rewards.len();
        rs.enter_event_room(EventId::BrainLeech);
        let out = resolve(&mut rs, "share_knowledge").expect("event active");
        assert!(out.finished);
        assert_eq!(rs.current_hp, hp_before, "share_knowledge costs no HP");
        // Card is gained via the queued pick-1-of-5 select, not via
        // master_deck mutation in-handler.
        assert_eq!(
            rs.master_deck.len(),
            deck_before,
            "master_deck not mutated in-handler (gain happens on pick resolve)"
        );
        assert_eq!(rs.pending_rewards.len(), rewards_before);
        assert_eq!(rs.pending_card_selects.len(), 1, "pick-1-of-5 grid queued");
        assert_eq!(rs.pending_card_selects[0].options.len(), 5);
        // C# `BrainLeech.cs:67-70`: `Cancelable = false`, no skip (S5).
        assert!(!rs.pending_card_selects[0].can_skip);
        assert!(rs.current_event.is_none());
    }

    /// **gain-relic**: `TeaMaster.bone_tea` deducts 50 gold and
    /// queues a `RelicReward` for BoneTea. Pin:
    /// `sts2-rl-agent@01c094d:sts2_env/events/act1.py::TeaMaster`.
    #[test]
    fn dispatch_gain_relic_category_tea_master_bone_tea_queues_bone_tea_reward() {
        let mut rs = RunState::new();
        rs.gold = 100; // Need at least TEA_MASTER_BONE_COST=50.
        let relics_before = rs.relics.len();
        let relic_rewards_before = rs.relic_rewards.len();
        rs.enter_event_room(EventId::TeaMaster);
        let out = resolve(&mut rs, "bone_tea").expect("event active");
        assert!(out.finished);
        assert_eq!(rs.gold, 50, "100 − TEA_MASTER_BONE_COST(50) = 50");
        // Relic granted via the RelicReward queue (run-loop drains it
        // through accept_relic_reward so on-obtain hooks fire).
        assert_eq!(
            rs.relic_rewards.len(),
            relic_rewards_before + 1,
            "BoneTea relic reward queued"
        );
        assert_eq!(
            rs.relic_rewards.last().expect("non-empty").relic_id,
            sts2_core::RelicId::BoneTea
        );
        assert_eq!(
            rs.relics.len(),
            relics_before,
            "relics list NOT mutated in-handler (gain happens via accept_relic_reward)"
        );
        assert!(rs.current_event.is_none());
    }

    /// **lose-card**: `NeowsBlessing.remove_card` is the canonical
    /// card-removal event at the Python pin. The handler is now wired
    /// through the deck-mutation surface (bead `sts2-rs-m3n0g.2`): the
    /// default (parity) host removes the deck front immediately, so a
    /// resolve through the dispatcher shrinks `master_deck` by one. Pin:
    /// `sts2-rl-agent@01c094d:sts2_env/events/neow_blessing.py`.
    #[test]
    fn dispatch_lose_card_category_neows_blessing_remove_card_removes_one() {
        let mut rs = RunState::new();
        // Seed a few cards so the handler has something to remove.
        rs.master_deck.push(sts2_core::cards::Card::pod_defaults());
        rs.master_deck.push(sts2_core::cards::Card::pod_defaults());
        let deck_before = rs.master_deck.len();
        rs.enter_event_room(EventId::NeowsBlessing);
        let out = resolve(&mut rs, "remove_card").expect("event active");
        assert!(out.finished, "resolve finishes the event");
        assert_eq!(
            rs.master_deck.len(),
            deck_before - 1,
            "card-removal wired through the deck-mutation surface \
             (front-of-deck auto-removal in the parity host)"
        );
        assert!(rs.current_event.is_none(), "bus cleared after resolve");
    }

    // ── pick_event generated_rooms indexing (mutant coverage) ────────

    /// Populate `generated_rooms` with two acts whose `events_shuffled`
    /// lists differ, so the act-index arithmetic and the empty-filter on
    /// the working pool are observable.
    fn rs_with_generated_event_orders(
        act0: Vec<&'static str>,
        act1: Vec<&'static str>,
    ) -> RunState {
        let mut rs = RunState::new();
        let g0 = sts2_core::run_room_gen::GeneratedActRooms {
            events_shuffled: act0,
            ..Default::default()
        };
        let g1 = sts2_core::run_room_gen::GeneratedActRooms {
            events_shuffled: act1,
            ..Default::default()
        };
        rs.generated_rooms = vec![g0, g1];
        rs
    }

    #[test]
    fn pick_event_reads_current_act_generated_event_order() {
        // act_index == 1 ⇒ generated_rooms[act_idx - 1] == [0]. The
        // distinctive single-event order at index 0 must be the source.
        //   `- → +`: get(2) → None → static fallback (not TrashHeap).
        //   `- → /`: act_idx/1 == 1 → get(1) → LostWisp, not TrashHeap.
        // Both differ from the original TrashHeap pick.
        let mut rs = rs_with_generated_event_orders(vec!["TrashHeap"], vec!["LostWisp"]);
        rs.acts[0].events_visited = 0;
        let picked = pick_event(&mut rs, None);
        assert_eq!(
            picked,
            Some(EventId::TrashHeap),
            "index-0 order is the source"
        );
    }

    #[test]
    fn pick_event_skips_empty_generated_order_only_when_nonempty() {
        // Kills `delete !` on `.filter(|s| !s.is_empty())`: a non-empty
        // index-0 order must be USED (TrashHeap), not filtered out. Under
        // the mutant the non-empty list is dropped → static fallback, which
        // does not lead with TrashHeap.
        let mut rs = rs_with_generated_event_orders(vec!["TrashHeap"], vec!["LostWisp"]);
        rs.acts[0].events_visited = 0;
        // Sanity: the static fallback for act 1 does NOT start with TrashHeap,
        // so a fallback would yield a different pick.
        assert_ne!(
            rs.acts[0].event_ids.first().copied(),
            Some("TrashHeap"),
            "fallback must differ so the filter behavior is observable",
        );
        let picked = pick_event(&mut rs, None);
        assert_eq!(picked, Some(EventId::TrashHeap));
    }

    #[test]
    fn pick_event_act_order_fallback_uses_modulo_wraparound() {
        // Kills `event_index % pool_ids.len()` → `+` / `/` in the
        // "nothing allowed" fallback. Two never-allowed events ⇒ the loop
        // exhausts (event_index advances to len==2), then the fallback
        // returns `pool_ids[2 % 2] == pool_ids[0]` (Neow).
        //   `% → /`: 2 / 2 == 1 → pool_ids[1] (TheArchitect), differs.
        //   `% → +`: 2 + 2 == 4 → out-of-bounds panic.
        let mut rs = rs_with_generated_event_orders(vec!["Neow", "TheArchitect"], vec!["LostWisp"]);
        rs.acts[0].events_visited = 0;
        let picked = pick_event(&mut rs, None);
        assert_eq!(
            picked,
            Some(EventId::Neow),
            "fallback wraps to slot 0 via modulo",
        );
    }

    // ── preview_option_effect (GH #237) ──────────────────────────────

    /// The reason the resource slots exist: Stone of All Time's Lift costs a
    /// potion, and gold/HP/max-HP alone report `0 / +10 / +10` — byte-identical
    /// to a free +10 Max HP.
    #[test]
    fn preview_option_effect_reports_the_potion_lift_drinks() {
        use sts2_core::potions::PotionKind;
        let mut rs = RunState::new();
        rs.potions = vec![PotionKind::BlockPotion, PotionKind::FirePotion];
        rs.potion_slot_indices = vec![0, 1];
        rs.enter_event_room(EventId::StoneOfAllTime);
        let idx = present(&rs)
            .expect("options")
            .iter()
            .position(|o| o.option_id == "lift")
            .expect("lift present");
        let delta = preview_option_effect(&rs, idx).expect("previewable");
        assert_eq!((delta.hp_delta, delta.max_hp_delta), (10, 10));
        assert_eq!(delta.potions_lost, 1, "Lift drinks one potion");
        assert_eq!(delta.potions_gained, 0);
        assert_eq!(rs.potions.len(), 2, "preview must not drink it for real");
    }

    /// A SWAP has to read `(1, 1)`, not net to zero — netting is exactly how
    /// Relic Trader used to featurize as an option that does nothing.
    #[test]
    fn preview_option_effect_reports_a_relic_trade_as_one_in_one_out() {
        use sts2_core::relics::RelicId;
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.act_index = 2;
        rs.direct_event_relic_grant = true;
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
            sts2_core::runstate::RelicRarity::Rare,
            vec![RelicId::Brimstone, RelicId::Astrolabe, RelicId::CallingBell],
        );
        rs.enter_event_room(EventId::RelicTrader);
        let delta = preview_option_effect(&rs, 0).expect("previewable");
        assert_eq!((delta.relics_gained, delta.relics_lost), (1, 1));
        assert_eq!(rs.relics.len(), 5, "preview must not trade for real");
    }

    #[test]
    fn preview_option_effect_reads_gold_gain_without_mutating() {
        // TrashHeap option 1 ("grab") gains 100 gold (see
        // `resolve_finished_outcome_clears_current_event`).
        let mut rs = RunState::new();
        rs.enter_event_room(EventId::TrashHeap);
        let gold_before = rs.gold;
        let delta = preview_option_effect(&rs, 1).expect("option present");
        assert_eq!(delta.gold_delta, 100);
        assert_eq!(delta.hp_delta, 0);
        assert_eq!(delta.max_hp_delta, 0);
        // Preview must leave the live state untouched: gold and the bound
        // event both unchanged, so the real resolve still fires later.
        assert_eq!(rs.gold, gold_before);
        assert_eq!(rs.current_event, Some(EventId::TrashHeap));
    }

    #[test]
    fn preview_option_effect_reads_hp_cost() {
        // TrashHeap option 0 ("dive_in") loses 8 HP (and grants a relic —
        // no gold change).
        let mut rs = RunState::new();
        rs.enter_event_room(EventId::TrashHeap);
        let delta = preview_option_effect(&rs, 0).expect("option present");
        assert_eq!(delta.gold_delta, 0);
        assert_eq!(delta.hp_delta, -8);
        assert_eq!(delta.max_hp_delta, 0);
        assert_eq!(rs.current_event, Some(EventId::TrashHeap));
    }

    #[test]
    fn preview_option_effect_reads_heal() {
        // AbyssalBaths option 1 ("abstain") heals 10 HP; seed missing HP so
        // the heal lands fully rather than clamping at max.
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 50;
        rs.enter_event_room(EventId::AbyssalBaths);
        let delta = preview_option_effect(&rs, 1).expect("option present");
        assert_eq!(delta.hp_delta, 10);
        assert_eq!(delta.gold_delta, 0);
        assert_eq!(rs.current_hp, 50, "preview must not heal the live state");
    }

    #[test]
    fn preview_option_effect_none_out_of_range_or_no_event() {
        // No active event → None.
        let rs = RunState::new();
        assert!(preview_option_effect(&rs, 0).is_none());
        // Active event but idx past the presented option list → None.
        let mut rs = RunState::new();
        rs.enter_event_room(EventId::TrashHeap);
        assert!(preview_option_effect(&rs, 99).is_none());
    }
}
