//! RelicId listener-completeness census — bead `sts2-rs-j87a7`.
//!
//! Replaces the per-pool `install_pool_registers_every_*` marker tests
//! deleted alongside the vestigial `on_trigger_*` marker layer (see
//! `docs/plans/2026-05-30-relic-marker-layer-collapse.md`). The old
//! tests asserted "every relic resolves through the generic
//! `relic_effects::dispatch` registry" — but that registry only ever
//! held `EffectResult::Resolved` markers with zero behaviour and zero
//! non-test callers, so it masked unported relics as "handled".
//!
//! This census is strictly stronger: after
//! `install_default_registry()`, every [`RelicId`] must fall into
//! exactly one accounted-for bucket:
//!
//! 1. **Typed channel** — resolves to >=1 typed-channel listener on any
//!    of the `relic_effects::*` combat-hook channels or the
//!    `run_effects::*` run-hook channels (each channel macro generates a
//!    `lookup_<hook>` fn; we probe all of them).
//! 2. **`INLINE_HANDLED`** — implemented inline in
//!    `hooks.rs`/`combat_loop.rs`/`run_manager.rs` via direct
//!    `RelicKind::X` checks (no registry entry by design).
//! 3. **`BEHAVIORLESS_RELICS`** — genuinely behaviourless in the C#
//!    decomp (only a `Rarity` override).
//! 4. **`KNOWN_UNPORTED`** — truly unported, each tagged with its bead.
//!
//! Porting a gap moves it off `KNOWN_UNPORTED` (it starts resolving via
//! a typed channel). A newly-added unported relic that lands in none of
//! the buckets trips this test.

use sts2_content::relics::dispatch::install_default_registry;
use sts2_core::relics::RelicId;

/// Relics implemented inline via direct `RelicKind::X` checks in
/// `hooks.rs`/`combat_loop.rs`/`run_manager.rs` rather than through a
/// registry channel. Behaviour exists; there is intentionally no
/// listener registration. (Plan §"Gaps surfaced": 10 relics.)
const INLINE_HANDLED: &[RelicId] = &[
    // Bookmark is the game's only `AfterFlush` override; the player's flush
    // tail calls it inline at that point, before the turn-end cost cleanup
    // (F6 step32, `combat_loop::end_player_turn_phase_two_flush_tail`).
    RelicId::Bookmark,
    RelicId::Bread,
    RelicId::IceCream,
    RelicId::LizardTail,
    // PaelsFlesh's +1 max energy from turn 3 is a `relic_modify_max_energy`
    // arm (sts2-agent fork).
    RelicId::PaelsFlesh,
    RelicId::PaelsWing,
    RelicId::PrismaticGem,
    RelicId::RingingTriangle,
    RelicId::RunicPyramid,
    // GH #742: SpikedGauntlets' +1 Power-card cost is a cost modifier in
    // `hooks::dispatch_modify_card_cost` / `effective_card_cost_inst_with_mask`
    // and its +1 max energy is a `relic_modify_max_energy` arm.
    RelicId::SpikedGauntlets,
    RelicId::UndyingSigil,
    RelicId::VitruvianMinion,
    RelicId::WhisperingEarring,
    // v0.107.1 FishingRod (bead `sts2-rs-v0107-content-port-wb8lk.6`):
    // every-3rd-normal-combat deck upgrade is implemented inline in
    // `run_bridge::exit_combat` (combat-end → run deck mutation), not via
    // a registry channel.
    RelicId::FishingRod,
];

/// Relics that are genuinely behaviourless in the authoritative C#
/// decomp (only a `Rarity` override, no hook virtuals). They never need
/// a listener.
///
/// The plan enumerated 2 (`FakeMerchantsRug`,
/// `WongoCustomerAppreciationBadge`). The census surfaced 3 more that
/// the plan's marker-vs-typed cross-reference missed because they were
/// never in any `*_POOL` marker table — all three are display-only /
/// placeholder relics that carry zero hook behaviour and therefore
/// belong here rather than in `KNOWN_UNPORTED`:
///
/// - `Circlet` — `RelicPool.FALLBACK` stackable placeholder shown when
///   no other relics are available; no listener methods (Python pin
///   `shop_event.py:3102`, `RelicEffect::Noop`, common.rs:90).
/// - `DeprecatedRelic` — `RelicPool.DEPRECATED` stackable placeholder
///   so legacy relic ids round-trip without a missing-factory panic; no
///   listener methods (common.rs:98, `RelicEffect::Noop`).
/// - `WingedBoots` — free-travel map relic with no pickup/combat/run
///   hook; map-layer-only (neow_relics.rs:14, `WingedBoots.cs`).
const BEHAVIORLESS_RELICS: &[RelicId] = &[
    RelicId::FakeMerchantsRug,
    RelicId::WongoCustomerAppreciationBadge,
    // Surfaced by the census beyond the plan's enumerated 2:
    RelicId::Circlet,
    RelicId::DeprecatedRelic,
    RelicId::WingedBoots,
];

/// Relics that are truly unported — masked as `Resolved` markers under
/// the old layer. Each carries its tracking bead id. Porting any of
/// these (wiring a real typed-channel listener) lets you drop it from
/// this list.
///
/// The plan's §"Gaps surfaced" enumerated 8 relics here; all 8 have since
/// been ported (BeltBuckle/BoneTea/FakeVenerableTeaSet/ReptileTrinket/
/// PaperPhrog/Byrdpip/PaelsLegion/HistoryCourse), so the list is now
/// empty. A newly-added unported relic that lands in none of the buckets
/// trips `every_relic_id_is_accounted_for`; add it here (with its bead)
/// to acknowledge the gap.
// Kaleidoscope (v0.107.1 Ancient/Neow positive relic, bead `sts2-rs-utgn4`):
// `AfterObtained` offers 2 card rewards each drawn from 3 random
// *other*-character pools — now PORTED (`listener_kaleidoscope`, Niche-shuffled
// non-owner pools + base-odds Rewards card creation), so it is no longer
// unported. The list is empty again.
const KNOWN_UNPORTED: &[RelicId] = &[];

/// True iff `id` resolves to at least one registered typed-channel
/// listener on any of the combat-hook (`relic_effects`) or run-hook
/// (`run_effects`) channels. Enumerates every `lookup_<hook>` fn
/// generated by the channel macros in
/// `sts2-core/src/relic_effects/channels.rs` and
/// `sts2-core/src/run_effects.rs`.
fn has_typed_listener(id: RelicId) -> bool {
    sts2_core::relic_effects::lookup_after_block_cleared(id).is_some()
        || sts2_core::relic_effects::lookup_after_block_gained(id).is_some()
        || sts2_core::relic_effects::lookup_after_card_discarded(id).is_some()
        || sts2_core::relic_effects::lookup_after_card_entered_combat(id).is_some()
        || sts2_core::relic_effects::lookup_after_card_exhausted(id).is_some()
        || sts2_core::relic_effects::lookup_after_card_played(id).is_some()
        || sts2_core::relic_effects::lookup_after_combat_end(id).is_some()
        || sts2_core::relic_effects::lookup_after_combat_victory(id).is_some()
        || sts2_core::relic_effects::lookup_after_damage_given(id).is_some()
        || sts2_core::relic_effects::lookup_after_damage_received(id).is_some()
        || sts2_core::relic_effects::lookup_after_death(id).is_some()
        || sts2_core::relic_effects::lookup_after_died_to_doom(id).is_some()
        || sts2_core::relic_effects::lookup_after_energy_reset(id).is_some()
        || sts2_core::relic_effects::lookup_after_hand_emptied(id).is_some()
        || sts2_core::relic_effects::lookup_after_max_hp_changed(id).is_some()
        || sts2_core::relic_effects::lookup_after_max_hp_gained(id).is_some()
        || sts2_core::relic_effects::lookup_after_modifying_power_amount_given(id).is_some()
        || sts2_core::relic_effects::lookup_after_modifying_power_amount_received(id).is_some()
        || sts2_core::relic_effects::lookup_after_obtained(id).is_some()
        || sts2_core::relic_effects::lookup_after_player_turn_start(id).is_some()
        || sts2_core::relic_effects::lookup_after_potion_used(id).is_some()
        || sts2_core::relic_effects::lookup_after_room_entered(id).is_some()
        || sts2_core::relic_effects::lookup_after_shuffle_bus(id).is_some()
        || sts2_core::relic_effects::lookup_after_side_turn_start(id).is_some()
        || sts2_core::relic_effects::lookup_after_turn_end(id).is_some()
        || sts2_core::relic_effects::lookup_before_card_played(id).is_some()
        || sts2_core::relic_effects::lookup_before_combat_start(id).is_some()
        || sts2_core::relic_effects::lookup_before_hand_draw(id).is_some()
        || sts2_core::relic_effects::lookup_before_play_phase_start(id).is_some()
        || sts2_core::relic_effects::lookup_before_power_amount_changed(id).is_some()
        || sts2_core::relic_effects::lookup_before_side_turn_start(id).is_some()
        || sts2_core::relic_effects::lookup_before_turn_end(id).is_some()
        || sts2_core::relic_effects::lookup_count_pets(id).is_some()
        || sts2_core::relic_effects::lookup_modify_block_multiplicative(id).is_some()
        || sts2_core::relic_effects::lookup_modify_card_reward_options_late(id).is_some()
        || sts2_core::relic_effects::lookup_modify_damage_additive(id).is_some()
        || sts2_core::relic_effects::lookup_modify_damage_multiplicative(id).is_some()
        || sts2_core::relic_effects::lookup_modify_vulnerable_multiplier(id).is_some()
        || sts2_core::relic_effects::lookup_modify_hand_draw(id).is_some()
        || sts2_core::relic_effects::lookup_modify_hp_lost(id).is_some()
        || sts2_core::relic_effects::lookup_modify_power_amount_given(id).is_some()
        || sts2_core::relic_effects::lookup_modify_power_amount_received(id).is_some()
        || sts2_core::relic_effects::lookup_modify_rest_site_heal_rewards(id).is_some()
        || sts2_core::relic_effects::lookup_modify_rewards(id).is_some()
        || sts2_core::relic_effects::lookup_modify_shop_price(id).is_some()
        || sts2_core::relic_effects::lookup_modify_x_value(id).is_some()
        || sts2_core::relic_effects::lookup_on_card_added_to_deck(id).is_some()
        || sts2_core::relic_effects::lookup_on_orb_channeled(id).is_some()
        || sts2_core::relic_effects::lookup_on_pet_summoned(id).is_some()
        || sts2_core::relic_effects::lookup_on_stars_spent(id).is_some()
        || sts2_core::relic_effects::lookup_relic_modify_card_play_count(id).is_some()
        || sts2_core::relic_effects::lookup_should_clear_block(id).is_some()
        || sts2_core::relic_effects::lookup_should_die_late(id).is_some()
        || sts2_core::run_effects::lookup_after_act_entered(id).is_some()
        || sts2_core::run_effects::lookup_after_combat_end(id).is_some()
        || sts2_core::run_effects::lookup_after_combat_victory_run(id).is_some()
        || sts2_core::run_effects::lookup_after_max_hp_changed(id).is_some()
        || sts2_core::run_effects::lookup_after_obtained(id).is_some()
        || sts2_core::run_effects::lookup_after_rest_site_heal(id).is_some()
        || sts2_core::run_effects::lookup_after_room_entered(id).is_some()
        || sts2_core::run_effects::lookup_after_shop_inventory_generated(id).is_some()
        || sts2_core::run_effects::lookup_allow_card_reward_reroll(id).is_some()
        || sts2_core::run_effects::lookup_modify_card_being_added_to_deck(id).is_some()
        || sts2_core::run_effects::lookup_modify_card_reward_creation_options(id).is_some()
        || sts2_core::run_effects::lookup_modify_card_reward_options_late(id).is_some()
        || sts2_core::run_effects::lookup_modify_generated_map(id).is_some()
        || sts2_core::run_effects::lookup_modify_generated_map_late(id).is_some()
        || sts2_core::run_effects::lookup_modify_merchant_card_creation_results(id).is_some()
        || sts2_core::run_effects::lookup_modify_rest_site_heal_amount(id).is_some()
        || sts2_core::run_effects::lookup_modify_rest_site_heal_rewards(id).is_some()
        || sts2_core::run_effects::lookup_modify_rest_site_options(id).is_some()
        || sts2_core::run_effects::lookup_modify_rewards(id).is_some()
        || sts2_core::run_effects::lookup_modify_unknown_map_point_room_types(id).is_some()
        || sts2_core::run_effects::lookup_on_card_added_to_deck(id).is_some()
        || sts2_core::run_effects::lookup_on_gold_gained(id).is_some()
        || sts2_core::run_effects::lookup_on_item_purchased(id).is_some()
        || sts2_core::run_effects::lookup_should_disable_remaining_rest_site_options(id).is_some()
        || sts2_core::run_effects::lookup_should_force_potion_reward(id).is_some()
        || sts2_core::run_effects::lookup_should_gain_gold(id).is_some()
        || sts2_core::run_effects::lookup_should_generate_treasure(id).is_some()
        || sts2_core::run_effects::lookup_should_procure_potion(id).is_some()
}

/// Every `RelicId` is accounted for by exactly one bucket: a typed
/// listener, an inline handler, the behaviourless set, or the
/// known-unported set. Any `RelicId` in none of these buckets is an
/// unmasked gap and fails the test.
#[test]
fn every_relic_id_is_accounted_for() {
    install_default_registry();

    let mut unaccounted: Vec<RelicId> = Vec::new();
    for &id in RelicId::all() {
        let accounted = has_typed_listener(id)
            || INLINE_HANDLED.contains(&id)
            || BEHAVIORLESS_RELICS.contains(&id)
            || KNOWN_UNPORTED.contains(&id);
        if !accounted {
            unaccounted.push(id);
        }
    }

    assert!(
        unaccounted.is_empty(),
        "RelicId values are not accounted for by any completeness bucket \
         (typed listener / INLINE_HANDLED / BEHAVIORLESS_RELICS / KNOWN_UNPORTED). \
         Either wire a typed-channel listener for them or add them to the \
         appropriate bucket in this test (with a bead id for unported relics): {unaccounted:?}"
    );
}

/// The accounted-for bucket constants are mutually disjoint — a relic
/// must not be double-listed (e.g. both inline and known-unported).
/// Catches stale entries left behind when a gap is ported.
#[test]
fn buckets_are_disjoint() {
    for &id in INLINE_HANDLED {
        assert!(
            !BEHAVIORLESS_RELICS.contains(&id) && !KNOWN_UNPORTED.contains(&id),
            "RelicId::{id:?} is in INLINE_HANDLED and another bucket"
        );
    }
    for &id in BEHAVIORLESS_RELICS {
        assert!(
            !KNOWN_UNPORTED.contains(&id),
            "RelicId::{id:?} is in BEHAVIORLESS_RELICS and KNOWN_UNPORTED"
        );
    }
}

/// A relic listed in `KNOWN_UNPORTED` must NOT already resolve to a
/// typed listener — once it's ported, drop it from the list so the
/// census stays honest about what's actually missing.
#[test]
fn known_unported_relics_have_no_typed_listener() {
    install_default_registry();
    let stale: Vec<RelicId> = KNOWN_UNPORTED
        .iter()
        .copied()
        .filter(|&id| has_typed_listener(id))
        .collect();
    assert!(
        stale.is_empty(),
        "these KNOWN_UNPORTED relics now resolve to a typed listener and \
         should be removed from KNOWN_UNPORTED: {stale:?}"
    );
}
