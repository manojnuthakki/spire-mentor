//! `NeowsBlessing` — run-start 5-boon menu. Bead `sts2-rs-8tkle`
//! (P9.2.e.2).
//!
//! Demonstrates the [`super::EventOptionSpec`] surface wired through
//! the declarative [`sts2_core::EventEffect`] dispatcher from
//! `sts2-rs-bv816`. The boon handlers return an
//! [`sts2_core::EventOutcome`] carrying a `Vec<EventEffect>`; the
//! [`super::dispatch::resolve`] driver applies each effect before
//! clearing `current_event`, so the same code path covers gold /
//! HP / max-HP / relic / curse mutations without per-handler
//! `&mut RunState` plumbing.
//!
//! Port stance: STS2's canonical `Neow` (see
//! `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Events/Neow.cs`)
//! is a relic-grab-bag — that variant ships as [`super::act3_glory::NEOW`]
//! and stays the source-of-truth port. `NeowsBlessing` here is the
//! lighter run-start blessing menu used by the run-loop driver
//! before any room is rolled (matches the STS1-style boon shape the
//! umbrella `sts2-rs-2zp4e` specced for the simulator stub). The
//! two coexist; pool-spawn paths use `Neow`, run-bootstrap uses
//! `NeowsBlessing`. Neither is allowed to spawn from the room pool
//! (`is_allowed` returns `false` for both).

use sts2_core::{EventEffect, EventId, EventOutcome, RelicId, RunState};

use super::{default_option_info_from_spec, never_allowed, EventOptionSpec, EventSpec};

/// `NeowsBlessing::max_hp` boon magnitude. Pinned to +8 to match the
/// classic STS1 blessing menu's "Max HP" option.
pub const NEOWS_BLESSING_MAX_HP: i32 = 8;

/// `NeowsBlessing::gain_gold` boon magnitude. Pinned to +50 to match
/// the classic STS1 blessing menu's "Money" option.
pub const NEOWS_BLESSING_GOLD: i32 = 50;

/// Default relic granted by `NeowsBlessing::gain_relic`. Held at
/// `ArcaneScroll` to match the `NEOW_FALLBACK_POSITIVE_1` pin in
/// `super::act3_glory` so the two Neow surfaces stay aligned during early
/// P9. (The Neow event itself now rolls dynamically via
/// `sts2_core::roll_neow_offers` — bead `sts2-rs-qq9og` — but the
/// `NeowsBlessing` simulator-stub menu still grants a fixed positive
/// relic until its own dynamic roll lands.)
pub const NEOWS_BLESSING_RELIC: RelicId = RelicId::ArcaneScroll;

fn neows_blessing_max_hp(_state: &mut RunState) -> EventOutcome {
    EventOutcome::finished(format!("Gained {NEOWS_BLESSING_MAX_HP} Max HP."))
        .with_effects(vec![EventEffect::GainMaxHp(NEOWS_BLESSING_MAX_HP)])
}

fn neows_blessing_gain_gold(_state: &mut RunState) -> EventOutcome {
    EventOutcome::finished(format!("Gained {NEOWS_BLESSING_GOLD} gold."))
        .with_effects(vec![EventEffect::GainGold(NEOWS_BLESSING_GOLD)])
}

fn neows_blessing_gain_relic(_state: &mut RunState) -> EventOutcome {
    EventOutcome::finished("Gained a positive relic.")
        .with_effects(vec![EventEffect::GainRelic(NEOWS_BLESSING_RELIC)])
}

/// `NeowsBlessing::remove_card` — one-card removal from the master
/// deck, wired through the existing deck-mutation surface
/// (`sts2-rs-m3n0g.2`). Interactive hosts (the workbench / `run_parity`
/// lockstep) queue a removal `card_select` via
/// [`RunState::remove_cards_from_deck`]'s `pending_deck_removals` path;
/// the headless / parity-fuzz default removes the deck front. Same
/// shape as `zen_weaver_remove` / `wellspring_bathe`.
fn neows_blessing_remove_card(state: &mut RunState) -> EventOutcome {
    let removed = state.remove_cards_from_deck(1);
    if state.interactive_card_removal {
        EventOutcome::finished("Choose a card to remove from your deck.")
    } else {
        EventOutcome::finished(format!("Removed {removed} card from your deck."))
    }
}

/// `NeowsBlessing::transform_card` — one-card transform, wired through
/// the deck-mutation surface (`sts2-rs-m3n0g.2`). Interactive hosts
/// queue a transform `card_select` ([`RunState::pending_deck_transforms`])
/// and roll the replacement only after the pick; the headless / parity
/// path transforms the deck front via the basic-Niche roll. Same shape
/// as `morphic_grove_group`.
fn neows_blessing_transform_card(state: &mut RunState) -> EventOutcome {
    if state.interactive_card_transform {
        state
            .pending_deck_transforms
            .push(sts2_core::PendingDeckTransform::default());
        return EventOutcome::finished("Choose a card to transform.");
    }
    if let Some(original) = state.master_deck.first().map(|c| c.card_id) {
        if let Some(new_id) = crate::cards::spec_index::roll_transform_replacement_basic_niche(
            state,
            state.character_id,
            original,
        ) {
            if let Some(card) = crate::cards::spec_index::card_pod_from_id(new_id, false) {
                state.master_deck.remove(0);
                state.master_deck.push(card);
            }
        }
    }
    EventOutcome::finished("Transformed a card in your deck.")
}

const NEOWS_BLESSING_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "max_hp",
        loc_key: "",
        default_label: "Max HP +8",
        default_description: "Gain 8 Max HP.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: neows_blessing_max_hp,
    },
    EventOptionSpec {
        option_id: "gain_gold",
        loc_key: "",
        default_label: "+50 Gold",
        default_description: "Gain 50 gold.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: neows_blessing_gain_gold,
    },
    EventOptionSpec {
        option_id: "gain_relic",
        loc_key: "",
        default_label: "Gain a Relic",
        default_description: "Gain a positive relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: neows_blessing_gain_relic,
    },
    EventOptionSpec {
        option_id: "remove_card",
        loc_key: "",
        default_label: "Remove a Card",
        default_description: "Remove a card from your deck.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: neows_blessing_remove_card,
    },
    EventOptionSpec {
        option_id: "transform_card",
        loc_key: "",
        default_label: "Transform a Card",
        default_description: "Transform a card in your deck.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: neows_blessing_transform_card,
    },
];

/// `NeowsBlessing` static spec. Bead `sts2-rs-8tkle`. `is_allowed`
/// is [`never_allowed`] — the run-loop driver triggers this event
/// directly at run start, never via the room-pool roll.
pub const NEOWS_BLESSING: EventSpec = EventSpec {
    id: EventId::NeowsBlessing,
    python_name: "NeowsBlessing",
    is_allowed: never_allowed,
    generate_options: default_option_info_from_spec,
    options: NEOWS_BLESSING_OPTIONS,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_matches_python_pin() {
        assert_eq!(NEOWS_BLESSING.python_name, "NeowsBlessing");
        assert_eq!(NEOWS_BLESSING.id.python_name(), "NeowsBlessing");
        assert_eq!(NEOWS_BLESSING.options.len(), 5);
        assert_eq!(NEOWS_BLESSING.options[0].option_id, "max_hp");
        assert_eq!(NEOWS_BLESSING.options[1].option_id, "gain_gold");
        assert_eq!(NEOWS_BLESSING.options[2].option_id, "gain_relic");
        assert_eq!(NEOWS_BLESSING.options[3].option_id, "remove_card");
        assert_eq!(NEOWS_BLESSING.options[4].option_id, "transform_card");
        let rs = RunState::new();
        assert!(!(NEOWS_BLESSING.is_allowed)(&rs));
    }

    #[test]
    fn max_hp_boon_attaches_gain_max_hp_effect() {
        let mut rs = RunState::new();
        let out = (NEOWS_BLESSING.options[0].handler)(&mut rs);
        assert!(out.finished);
        assert_eq!(out.effects.len(), 1);
        assert!(matches!(out.effects[0], EventEffect::GainMaxHp(8)));
    }

    #[test]
    fn gain_gold_boon_attaches_gain_gold_effect() {
        let mut rs = RunState::new();
        let out = (NEOWS_BLESSING.options[1].handler)(&mut rs);
        assert!(out.finished);
        assert_eq!(out.effects.len(), 1);
        assert!(matches!(out.effects[0], EventEffect::GainGold(50)));
    }

    #[test]
    fn gain_relic_boon_attaches_gain_relic_effect() {
        let mut rs = RunState::new();
        let out = (NEOWS_BLESSING.options[2].handler)(&mut rs);
        assert!(out.finished);
        assert_eq!(out.effects.len(), 1);
        assert!(matches!(
            out.effects[0],
            EventEffect::GainRelic(RelicId::ArcaneScroll)
        ));
    }

    #[test]
    fn remove_card_boon_removes_one_deck_card() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck.push(sts2_core::cards::Card::pod_defaults());
        rs.master_deck.push(sts2_core::cards::Card::pod_defaults());
        let before = rs.master_deck.len();
        let out = (NEOWS_BLESSING.options[3].handler)(&mut rs);
        assert!(out.finished);
        // Default (parity) host removes the deck front immediately.
        assert_eq!(
            rs.master_deck.len(),
            before - 1,
            "remove_card now removes one card via the deck-mutation surface"
        );
    }

    #[test]
    fn remove_card_boon_queues_pick_for_interactive_host() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.interactive_card_removal = true;
        // Two removable cards: one to choose (with one, C#'s FromDeckGeneric
        // removes it without a choice; deck-removal-candidate-count).
        rs.master_deck.push(sts2_core::cards::Card::pod_defaults());
        rs.master_deck.push(sts2_core::cards::Card::pod_defaults());
        let before = rs.master_deck.len();
        let out = (NEOWS_BLESSING.options[3].handler)(&mut rs);
        assert!(out.finished);
        // Interactive host defers to a removal card_select instead of
        // auto-removing the front.
        assert_eq!(rs.master_deck.len(), before, "interactive defers removal");
        assert_eq!(rs.pending_deck_removals, 1, "one removal pick queued");
    }

    #[test]
    fn transform_card_boon_transforms_one_deck_card() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::CardId;
        let mut rs = RunState::new();
        rs.master_deck.push(
            crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false).unwrap(),
        );
        let before = rs.master_deck.len();
        let out = (NEOWS_BLESSING.options[4].handler)(&mut rs);
        assert!(out.finished);
        // Deck size is preserved (remove original + append replacement).
        assert_eq!(
            rs.master_deck.len(),
            before,
            "transform preserves deck size"
        );
    }

    #[test]
    fn transform_card_boon_queues_pick_for_interactive_host() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.interactive_card_transform = true;
        let out = (NEOWS_BLESSING.options[4].handler)(&mut rs);
        assert!(out.finished);
        assert_eq!(
            rs.pending_deck_transforms.len(),
            1,
            "interactive host queues one transform pick"
        );
    }

    #[test]
    fn dispatcher_applies_max_hp_effect_through_resolve() {
        // End-to-end: dispatch::resolve walks the EventEffect list
        // before clearing current_event. Pinning the +8 Max HP boon
        // through the full resolve path guards against future
        // dispatcher changes that might drop the effect-application
        // step.
        use crate::events::dispatch::resolve;
        let mut rs = RunState::new();
        rs.enter_event_room(EventId::NeowsBlessing);
        let starting_max = rs.max_hp;
        let out = resolve(&mut rs, "max_hp").expect("event active");
        assert!(out.finished);
        assert_eq!(rs.max_hp, starting_max + NEOWS_BLESSING_MAX_HP);
        assert!(rs.current_event.is_none(), "exit_event_room fired");
    }

    #[test]
    fn dispatcher_applies_gain_gold_effect_through_resolve() {
        use crate::events::dispatch::resolve;
        let mut rs = RunState::new();
        rs.enter_event_room(EventId::NeowsBlessing);
        let starting_gold = rs.gold;
        let out = resolve(&mut rs, "gain_gold").expect("event active");
        assert!(out.finished);
        assert_eq!(rs.gold, starting_gold + NEOWS_BLESSING_GOLD);
    }

    #[test]
    fn dispatcher_applies_gain_relic_effect_through_resolve() {
        use crate::events::dispatch::resolve;
        let mut rs = RunState::new();
        rs.enter_event_room(EventId::NeowsBlessing);
        let starting_relic_count = rs.relics.len();
        let out = resolve(&mut rs, "gain_relic").expect("event active");
        assert!(out.finished);
        assert_eq!(rs.relics.len(), starting_relic_count + 1);
        assert_eq!(rs.last_obtained_relic, Some(NEOWS_BLESSING_RELIC));
    }
}
