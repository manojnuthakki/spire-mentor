//! Act3-pool event specs. Bead `sts2-rs-6ynt.4.3`.
//!
//! Port pin: `sts2-rl-agent@01c094d:sts2_env/events/act3.py`.
//!
//! All 5 act3 events have `is_allowed = False` in Python — none spawn
//! from the random room-roll. They are reachable only via special
//! game logic:
//!
//! - `TheArchitect` — act-transition end-of-run event (the run-loop
//!   driver will materialize it before the final boss).
//! - `WarHistorianRepy` — gated on owning a `LANTERN_KEY` card, which
//!   the Rust port doesn't yet model.
//! - `Neow` — run-start relic-selection event, dropped into the
//!   room by the start-of-run code path.
//! - `DeprecatedEvent` / `DeprecatedAncientEvent` — legacy stubs that
//!   exist in the registry to round-trip parity dumps.
//!
//! For now the Rust port lands the static `EventSpec` shape for each.
//! Reward/queue side-effects that depend on unported surfaces
//! (multi-deck card-remove for `LANTERN_KEY`, `PotionReward` variant,
//! `RunState.rng` for Neow's dynamic relic pool) ship as documented
//! divergences with sentinel handler behaviour.

use sts2_core::{EventId, PotionReward, RelicId, RelicReward, Reward, RunState};

use super::{
    default_option_info_from_spec, never_allowed, EventOptionSpec, EventOutcome, EventSpec,
};

// ── TheArchitect ─────────────────────────────────────────────────────

/// `TheArchitect::proceed` — flavor-only outcome. Python's
/// `EventResult` carries `event_combat_setup="the_architect"` +
/// `post_combat_phase="RUN_OVER"`, which are run-loop driver fields
/// not yet ported. When the act-transition surface lands, this
/// handler will write a hand-off marker into `RunState` so the
/// driver can launch the final combat. Today it just signals
/// finished — the run-loop driver short-circuits before the option
/// is actually presentable in pool play. Port pin:
/// `act3.py:41-47`.
fn the_architect_proceed(_state: &mut RunState) -> EventOutcome {
    EventOutcome::finished("The Architect emerges.")
}

const THE_ARCHITECT_OPTIONS: &[EventOptionSpec] = &[EventOptionSpec {
    option_id: "proceed",
    loc_key: "",
    default_label: "Proceed",
    default_description: "The Architect emerges.",
    default_vars: &[],
    vars_fn: None,
    title_vars_fn: None,
    handler: the_architect_proceed,
}];

/// `TheArchitect` static spec. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/act3.py:27`.
pub const THE_ARCHITECT: EventSpec = EventSpec {
    id: EventId::TheArchitect,
    python_name: "TheArchitect",
    is_allowed: never_allowed,
    generate_options: default_option_info_from_spec,
    options: THE_ARCHITECT_OPTIONS,
};

// ── WarHistorianRepy ─────────────────────────────────────────────────

/// `WarHistorianRepy::unlock_cage` — free Repy + grant
/// `HISTORY_COURSE`. Python additionally sets
/// `run_state.extra_fields["freed_repy"] = True` and walks the deck
/// to drop every `LANTERN_KEY` copy
/// (`act3.py::WarHistorianRepy._remove_lantern_key`). Both surfaces
/// are unported (`RunState.extra_fields` and run-side deck mutation
/// API), documented divergences — see sub-bead `6ynt.4.deck-add`.
/// The relic grant lands via the existing `offer_relic_rewards`
/// path; the `_should_defer_event_rewards` branch (Python's reward-
/// rerouting helper) is treated as always-defer here, mirroring the
/// `LostWisp::claim` precedent in `shared.rs`. Port pin:
/// `act3.py:87-98`.
fn war_historian_repy_unlock_cage(state: &mut RunState) -> EventOutcome {
    state.offer_relic_rewards(RelicId::HistoryCourse, 1);
    EventOutcome::finished(
        "Freed Repy, gained History Course relic \
         (LANTERN_KEY removal deferred to 6ynt.4.deck-add).",
    )
}

/// `WarHistorianRepy::unlock_chest` — gain 2 potion-rewards + 2
/// relic-rewards. Python pushes 2 `PotionReward` + 2 `RelicReward`
/// (`act3.py:100-111`). The Rust `Reward` enum lacks a `Potion`
/// variant today (see `runstate.rs::Reward` doc-comment listing
/// follow-up variants); we ship 2 `Reward::Relic` placeholders for
/// the potions and 2 for the relics, with a doc divergence. The
/// dispatcher overwrites the placeholder `relic_id`s post-roll once
/// the rarity / RNG surfaces land — same convention as
/// `TrashHeap::dive_in`. Port pin: `act3.py:99-111`.
fn war_historian_repy_unlock_chest(state: &mut RunState) -> EventOutcome {
    if state.interactive_rewards {
        // C# `UnlockChest`: `OfferCustom([Potion, Potion, Relic, Relic])`, each
        // rolled when offered, in that order.
        let relic = sts2_core::OfferedReward::RandomRelic {
            rarity: None,
            is_wax: false,
        };
        state.offer_custom_rewards(
            vec![
                sts2_core::OfferedReward::Potion(None),
                sts2_core::OfferedReward::Potion(None),
                relic.clone(),
                relic,
            ],
            true,
        );
        return EventOutcome::finished("Unlocked chest, gained 2 potions and 2 relics.");
    }
    // 2 potion-rewards — `Reward::Potion` variant landed via
    // `sts2-rs-ib04`; `PotionReward::empty()` matches Python's
    // unspecified-potion shape (dispatcher rolls at drain time).
    state
        .pending_rewards
        .push(Reward::Potion(PotionReward::empty()));
    state
        .pending_rewards
        .push(Reward::Potion(PotionReward::empty()));
    // 2 relic-rewards (relic_id rolled at reward-population time).
    state
        .pending_rewards
        .push(Reward::Relic(RelicReward::new(RelicId::HistoryCourse)));
    state
        .pending_rewards
        .push(Reward::Relic(RelicReward::new(RelicId::HistoryCourse)));
    EventOutcome::finished("Unlocked chest, gained 2 potions and 2 relics.")
}

const WAR_HISTORIAN_REPY_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "unlock_cage",
        loc_key: "WAR_HISTORIAN_REPY.pages.INITIAL.options.UNLOCK_CAGE",
        default_label: "Unlock Cage",
        default_description: "Free Repy, gain History Course relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: war_historian_repy_unlock_cage,
    },
    EventOptionSpec {
        option_id: "unlock_chest",
        loc_key: "WAR_HISTORIAN_REPY.pages.INITIAL.options.UNLOCK_CHEST",
        default_label: "Unlock Chest",
        default_description: "Gain 2 potions + 2 relics.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: war_historian_repy_unlock_chest,
    },
];

/// `WarHistorianRepy` static spec. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/act3.py:55`.
pub const WAR_HISTORIAN_REPY: EventSpec = EventSpec {
    id: EventId::WarHistorianRepy,
    python_name: "WarHistorianRepy",
    is_allowed: never_allowed,
    generate_options: default_option_info_from_spec,
    options: WAR_HISTORIAN_REPY_OPTIONS,
};

// ── Neow ─────────────────────────────────────────────────────────────

/// Fallback positive relic for `Neow::positive_1` when
/// [`RunState::neow_offers`] hasn't been rolled yet (e.g. fresh
/// `RunState::new()` in a unit test that constructs the spec
/// surface manually). Once sub-bead `sts2-rs-g24zv` lands the
/// run-start phase wiring, the cache is always populated before any
/// option is presented, so this fallback only matters for legacy
/// tests that hit the handler without going through the run-loop.
const NEOW_FALLBACK_POSITIVE_1: RelicId = RelicId::ArcaneScroll;
/// Fallback positive relic for `Neow::positive_2`. See
/// [`NEOW_FALLBACK_POSITIVE_1`].
const NEOW_FALLBACK_POSITIVE_2: RelicId = RelicId::BoomingConch;
/// Fallback cursed relic for `Neow::cursed`. See
/// [`NEOW_FALLBACK_POSITIVE_1`].
const NEOW_FALLBACK_CURSED: RelicId = RelicId::CursedPearl;

/// Resolve a cached Neow offer to its [`RelicId`]. Every `NeowRelic`
/// variant now maps 1-to-1 via [`sts2_core::NeowRelic::to_relic_id`]
/// (W9a-e: PhialHolster / WingedBoots / NeowsTalisman — `sts2-rs-4rwv4`;
/// NeowsBones — `sts2-rs-wjuoi`; HeftyTablet — `sts2-rs-w6hdj`), so the
/// former safe-placeholder fallback is gone. The `Option` return on
/// `to_relic_id` is retained for forward-compat with any future Neow
/// option that lands ahead of its `RelicId`; if that ever happens this
/// `expect` flags it loudly rather than silently mis-granting.
pub(crate) fn resolve_neow_relic(nr: sts2_core::NeowRelic) -> RelicId {
    nr.to_relic_id()
        .expect("every NeowRelic variant maps to a RelicId (W9a-e)")
}

/// Grant a chosen Neow relic. With [`RunState::direct_event_relic_grant`]
/// set (the workbench), add it immediately via [`RunState::obtain_relic`]
/// — matching C#, where picking the blessing grants the relic with no
/// follow-up "Take relic / Skip" step (and on-obtain effects like Precise
/// Scissors' card removal fire right away). Otherwise queue it as a
/// relic-reward for a separate `PickRelicReward`, the Python-parity flow.
fn grant_neow_relic(state: &mut RunState, relic: RelicId) {
    state.grant_event_relic(relic);
}

fn neow_positive_1(state: &mut RunState) -> EventOutcome {
    let relic = state.neow_offers.map_or(NEOW_FALLBACK_POSITIVE_1, |o| {
        resolve_neow_relic(o.positive_1)
    });
    grant_neow_relic(state, relic);
    EventOutcome::finished("Gained a positive relic from Neow.")
}

fn neow_positive_2(state: &mut RunState) -> EventOutcome {
    let relic = state.neow_offers.map_or(NEOW_FALLBACK_POSITIVE_2, |o| {
        resolve_neow_relic(o.positive_2)
    });
    grant_neow_relic(state, relic);
    EventOutcome::finished("Gained a positive relic from Neow.")
}

fn neow_cursed(state: &mut RunState) -> EventOutcome {
    let relic = state
        .neow_offers
        .map_or(NEOW_FALLBACK_CURSED, |o| resolve_neow_relic(o.cursed));
    grant_neow_relic(state, relic);
    EventOutcome::finished("Gained a cursed relic from Neow.")
}

/// Dynamic option-list builder for Neow: labels reflect the cached
/// rolled relic names from [`sts2_core::RunState::neow_offers`]. If
/// the cache is unset (e.g. the run-loop hasn't called
/// [`sts2_core::RunState::roll_neow_offers_if_needed`] yet), falls
/// back to the static labels — sub-bead `sts2-rs-g24zv` wires the
/// roll into the run-start path so this branch only fires in
/// legacy / unit-test code paths.
///
/// Bead `sts2-rs-14cid`.
fn neow_generate_options(state: &RunState, spec: &EventSpec) -> Vec<sts2_core::EventOptionInfo> {
    let Some(offers) = state.neow_offers else {
        return super::default_option_info_from_spec(state, spec);
    };
    let labels = [
        offers.positive_1.snake_case_id(),
        offers.positive_2.snake_case_id(),
        offers.cursed.snake_case_id(),
    ];
    spec.options
        .iter()
        .zip(labels)
        .map(|(opt, label)| sts2_core::EventOptionInfo {
            option_id: opt.option_id,
            loc_key: opt.loc_key,
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: label.to_string(),
            description: opt.default_description.to_string(),
            enabled: true,
        })
        .collect()
}

const NEOW_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "positive_1",
        loc_key: "",
        default_label: "Arcane Scroll",
        default_description: "Gain a positive relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: neow_positive_1,
    },
    EventOptionSpec {
        option_id: "positive_2",
        loc_key: "",
        default_label: "Booming Conch",
        default_description: "Gain a positive relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: neow_positive_2,
    },
    EventOptionSpec {
        option_id: "cursed",
        loc_key: "",
        default_label: "Cursed Pearl",
        default_description: "Gain a cursed relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: neow_cursed,
    },
];

/// `Neow` static spec. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/act3.py:119`.
pub const NEOW: EventSpec = EventSpec {
    id: EventId::Neow,
    python_name: "Neow",
    is_allowed: never_allowed,
    generate_options: neow_generate_options,
    options: NEOW_OPTIONS,
};

// ── DeprecatedEvent / DeprecatedAncientEvent ─────────────────────────

/// Shared no-op handler for the two deprecated act3 stubs. Mirrors
/// Python's `return EventResult(finished=True)` body
/// (`act3.py:228-229`, `act3.py:248-249`).
fn deprecated_finish(_state: &mut RunState) -> EventOutcome {
    EventOutcome::finished("")
}

/// Synthetic single-option list for the two deprecated event stubs.
/// Python has `generate_initial_options` returning `[]`, but the
/// Rust dispatcher's `every_event_id_has_a_spec` sentinel requires
/// at least one option per spec (so a new EventId variant can't
/// silently ship without resolution logic). The option is unreachable
/// in pool play because `is_allowed = never_allowed`; it exists only
/// so the spec round-trips through the registry.
const DEPRECATED_OPTIONS: &[EventOptionSpec] = &[EventOptionSpec {
    option_id: "finish",
    loc_key: "",
    default_label: "Finish",
    default_description: "Deprecated event — no effect.",
    default_vars: &[],
    vars_fn: None,
    title_vars_fn: None,
    handler: deprecated_finish,
}];

/// `DeprecatedEvent` static spec. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/act3.py:217`.
pub const DEPRECATED_EVENT: EventSpec = EventSpec {
    id: EventId::DeprecatedEvent,
    python_name: "DeprecatedEvent",
    is_allowed: never_allowed,
    generate_options: default_option_info_from_spec,
    options: DEPRECATED_OPTIONS,
};

/// `DeprecatedAncientEvent` static spec. Same shape as
/// [`DEPRECATED_EVENT`]. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/act3.py:237`.
pub const DEPRECATED_ANCIENT_EVENT: EventSpec = EventSpec {
    id: EventId::DeprecatedAncientEvent,
    python_name: "DeprecatedAncientEvent",
    is_allowed: never_allowed,
    generate_options: default_option_info_from_spec,
    options: DEPRECATED_OPTIONS,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_architect_spec_matches_python_pin() {
        assert_eq!(THE_ARCHITECT.python_name, "TheArchitect");
        assert_eq!(THE_ARCHITECT.id.python_name(), "TheArchitect");
        assert_eq!(THE_ARCHITECT.options.len(), 1);
        assert_eq!(THE_ARCHITECT.options[0].option_id, "proceed");
        let rs = RunState::new();
        assert!(!(THE_ARCHITECT.is_allowed)(&rs));
    }

    #[test]
    fn the_architect_proceed_finishes() {
        let mut rs = RunState::new();
        let outcome = (THE_ARCHITECT.options[0].handler)(&mut rs);
        assert!(outcome.finished);
    }

    #[test]
    fn war_historian_repy_spec_matches_python_pin() {
        assert_eq!(WAR_HISTORIAN_REPY.python_name, "WarHistorianRepy");
        assert_eq!(WAR_HISTORIAN_REPY.options.len(), 2);
        assert_eq!(WAR_HISTORIAN_REPY.options[0].option_id, "unlock_cage");
        assert_eq!(WAR_HISTORIAN_REPY.options[1].option_id, "unlock_chest");
        let rs = RunState::new();
        assert!(!(WAR_HISTORIAN_REPY.is_allowed)(&rs));
    }

    #[test]
    fn war_historian_repy_unlock_cage_grants_history_course() {
        let mut rs = RunState::new();
        let outcome = (WAR_HISTORIAN_REPY.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, RelicId::HistoryCourse);
    }

    #[test]
    fn war_historian_repy_unlock_chest_queues_4_rewards() {
        let mut rs = RunState::new();
        let outcome = (WAR_HISTORIAN_REPY.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        // 2 potion-rewards + 2 relic-rewards (Python:
        // PotionReward × 2 + RelicReward × 2).
        assert_eq!(rs.pending_rewards.len(), 4);
        let potions = rs
            .pending_rewards
            .iter()
            .filter(|r| matches!(r, Reward::Potion(_)))
            .count();
        let relics = rs
            .pending_rewards
            .iter()
            .filter(|r| matches!(r, Reward::Relic(_)))
            .count();
        assert_eq!(potions, 2, "two potion-rewards");
        assert_eq!(relics, 2, "two relic-rewards");
    }

    #[test]
    fn neow_spec_matches_python_pin() {
        assert_eq!(NEOW.python_name, "Neow");
        assert_eq!(NEOW.options.len(), 3);
        assert_eq!(NEOW.options[0].option_id, "positive_1");
        assert_eq!(NEOW.options[1].option_id, "positive_2");
        assert_eq!(NEOW.options[2].option_id, "cursed");
        let rs = RunState::new();
        assert!(!(NEOW.is_allowed)(&rs));
    }

    #[test]
    fn neow_positive_grants_positive_relic() {
        let mut rs = RunState::new();
        let outcome = (NEOW.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, NEOW_FALLBACK_POSITIVE_1);
    }

    #[test]
    fn neow_cursed_grants_cursed_relic() {
        let mut rs = RunState::new();
        let outcome = (NEOW.options[2].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, NEOW_FALLBACK_CURSED);
    }

    #[test]
    fn neow_roll_caches_offers_idempotently() {
        // Bead `sts2-rs-14cid`: `roll_neow_offers_if_needed` populates
        // `neow_offers` exactly once.
        let mut rs = RunState::new();
        assert!(rs.neow_offers.is_none());
        rs.roll_neow_offers_if_needed();
        let first = rs.neow_offers;
        assert!(first.is_some());
        // Mutating the cache to a marker proves the second call is a
        // no-op (idempotent).
        rs.roll_neow_offers_if_needed();
        assert_eq!(rs.neow_offers, first);
    }

    #[test]
    fn neow_dynamic_options_label_with_rolled_relics() {
        // Bead `sts2-rs-14cid`: the dynamic `generate_options` reads
        // the cached offers and emits SCREAMING_SNAKE_CASE labels for
        // each option, including the curse in slot 3.
        let mut rs = RunState::new();
        rs.roll_neow_offers_if_needed();
        let offers = rs.neow_offers.expect("roll populates the cache");
        let opts = (NEOW.generate_options)(&rs, &NEOW);
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0].label, offers.positive_1.snake_case_id());
        assert_eq!(opts[1].label, offers.positive_2.snake_case_id());
        assert_eq!(opts[2].label, offers.cursed.snake_case_id());
        for opt in &opts {
            assert!(opt.enabled);
        }
    }

    #[test]
    fn neow_dynamic_handler_grants_cached_relic() {
        // Bead `sts2-rs-14cid`: when the cache is populated, the
        // handler grants the rolled relic — not the legacy
        // fallback. We populate the cache with a known shape
        // (positive_1 = MassiveScroll, positive_2 = BoomingConch,
        // cursed = LeafyPoultice) so we don't depend on the
        // particular RNG output for the assertion.
        let mut rs = RunState::new();
        rs.neow_offers = Some(sts2_core::NeowOffers {
            positive_1: sts2_core::NeowRelic::MassiveScroll,
            positive_2: sts2_core::NeowRelic::BoomingConch,
            cursed: sts2_core::NeowRelic::LeafyPoultice,
        });

        // positive_1 → MassiveScroll
        let outcome = (NEOW.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(
            rs.relic_rewards.last().unwrap().relic_id,
            RelicId::MassiveScroll
        );

        // cursed → LeafyPoultice
        let outcome = (NEOW.options[2].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(
            rs.relic_rewards.last().unwrap().relic_id,
            RelicId::LeafyPoultice
        );
    }

    #[test]
    fn neow_handler_falls_back_to_constants_when_cache_unset() {
        // Bead `sts2-rs-14cid`: legacy code paths that hit the
        // handler without rolling first still work (Pomander/
        // ArcaneScroll/CursedPearl fallbacks).
        let mut rs = RunState::new();
        assert!(rs.neow_offers.is_none());
        let outcome = (NEOW.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.relic_rewards[0].relic_id, NEOW_FALLBACK_POSITIVE_1);
    }

    #[test]
    fn resolve_neow_relic_maps_all_w9_variants_to_their_own_relic_id() {
        // W9a-c (sts2-rs-4rwv4) + W9e (sts2-rs-wjuoi) + W9d
        // (sts2-rs-w6hdj): all five C#-only Neow relics now resolve to
        // their own RelicId variant — no placeholder routing left.
        assert_eq!(
            resolve_neow_relic(sts2_core::NeowRelic::PhialHolster),
            RelicId::PhialHolster
        );
        assert_eq!(
            resolve_neow_relic(sts2_core::NeowRelic::WingedBoots),
            RelicId::WingedBoots
        );
        assert_eq!(
            resolve_neow_relic(sts2_core::NeowRelic::NeowsTalisman),
            RelicId::NeowsTalisman
        );
        assert_eq!(
            resolve_neow_relic(sts2_core::NeowRelic::NeowsBones),
            RelicId::NeowsBones
        );
        assert_eq!(
            resolve_neow_relic(sts2_core::NeowRelic::HeftyTablet),
            RelicId::HeftyTablet
        );
    }

    #[test]
    fn deprecated_event_spec_matches_python_pin() {
        assert_eq!(DEPRECATED_EVENT.python_name, "DeprecatedEvent");
        assert_eq!(
            DEPRECATED_ANCIENT_EVENT.python_name,
            "DeprecatedAncientEvent"
        );
        // Both ship a synthetic `finish` option (Python has []).
        assert_eq!(DEPRECATED_EVENT.options.len(), 1);
        assert_eq!(DEPRECATED_ANCIENT_EVENT.options.len(), 1);
        let rs = RunState::new();
        assert!(!(DEPRECATED_EVENT.is_allowed)(&rs));
        assert!(!(DEPRECATED_ANCIENT_EVENT.is_allowed)(&rs));
    }

    #[test]
    fn deprecated_finish_is_no_op_finished() {
        let mut rs = RunState::new();
        let gold_before = rs.gold;
        let hp_before = rs.current_hp;
        let outcome = (DEPRECATED_EVENT.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, gold_before);
        assert_eq!(rs.current_hp, hp_before);
        assert!(rs.relic_rewards.is_empty());
        assert!(rs.pending_rewards.is_empty());
    }
}
