//! Shop-inventory populator — bead `sts2-rs-80apg` (P9.2.x).
//!
//! `sts2-core::shop::generate_shop_inventory` lands the *rarity + price*
//! shape of a merchant inventory. This module wires the content-side
//! sampling that fills the still-`None` slots:
//!
//! - `ShopCardEntry.card`: rolled from the run's character pool (or
//!   the colorless pool for the colorless row) by `(rarity, card_type)`.
//! - `ShopRelicEntry.relic_id`: popped off the run's
//!   per-rarity relic grab-bag via
//!   [`sts2_core::runstate::RunState::pop_relic_from_grab_bag`]
//!   (bead `sts2-rs-l9vkh` landed the bag).
//! - `ShopPotionEntry.potion_kind`: rolled from the standard
//!   normal-pool [`PotionKind`] list filtered by `(rarity,
//!   character_lock, !is_event_only)`.
//!
//! Pin: `sts2-rl-agent@01c094d:sts2_env/run/shop.py:399-455`
//! (`generate_shop_inventory`) + 201-353 (`_create_*` helpers).
//!
//! ## RNG order divergence (documented)
//!
//! Python interleaves rarity-roll → candidate-pick → price for each
//! slot. The Rust port currently splits these phases: rarity + price
//! roll first in `sts2_core::shop::generate_shop_inventory`, then
//! candidate-pick here. Net RNG-call count matches but ordering
//! differs, so dotnet-stream-level parity will require a follow-up
//! reorder. Tracked as a divergence note on this bead's close reason.

use crate::cards::spec_index::{card_pod_from_id, COLORLESS_SPECS};
use crate::cards::types::{CardRarity, CardSpec};

use sts2_core::card_rewards::RewardRarity;
use sts2_core::cards::{CardId, CardType};
use sts2_core::potions::{CharacterId, PotionKind, PotionRarity};
use sts2_core::runstate::RunState;
use sts2_core::shop::{ShopInventory, ShopPotionRarity, ShopRelicRarity};
use sts2_core::RelicRarity;

// ── Rarity conversions (local to keep deps thin) ─────────────────────

const fn reward_to_card_rarity(r: RewardRarity) -> CardRarity {
    match r {
        RewardRarity::Common => CardRarity::Common,
        RewardRarity::Uncommon => CardRarity::Uncommon,
        RewardRarity::Rare => CardRarity::Rare,
    }
}

const fn shop_to_relic_rarity(r: ShopRelicRarity) -> RelicRarity {
    match r {
        ShopRelicRarity::Common => RelicRarity::Common,
        ShopRelicRarity::Uncommon => RelicRarity::Uncommon,
        ShopRelicRarity::Rare => RelicRarity::Rare,
        ShopRelicRarity::Shop => RelicRarity::Shop,
    }
}

const fn shop_to_potion_rarity(r: ShopPotionRarity) -> PotionRarity {
    match r {
        ShopPotionRarity::Common => PotionRarity::Common,
        ShopPotionRarity::Uncommon => PotionRarity::Uncommon,
        ShopPotionRarity::Rare => PotionRarity::Rare,
    }
}

// ── Character-card sampling ──────────────────────────────────────────

fn specs_for_character(character: CharacterId) -> &'static [fn() -> CardSpec] {
    // Delegate to the canonical `spec_index::specs_for_character` so the shop
    // sells Regent/Necrobinder cards (sts2-rs-m3n0g.7). For Ironclad / Silent /
    // Defect this returns the SAME `*_SPECS` constants the local list used, so
    // the shop's `next_int_exclusive` candidate draws are unchanged — no parity
    // shift. Regent/Necrobinder go from the empty `_ => &[]` slice (slot left
    // `None`) to their real pools.
    crate::cards::spec_index::specs_for_character(character)
}

fn eligible_character_card_ids(
    character: CharacterId,
    rarity: CardRarity,
    card_type: CardType,
) -> Vec<CardId> {
    let strict: Vec<CardId> = specs_for_character(character)
        .iter()
        .filter_map(|f| {
            let spec = f();
            if spec.rarity == rarity && spec.card_type == card_type {
                Some(spec.id)
            } else {
                None
            }
        })
        .collect();
    if !strict.is_empty() {
        return strict;
    }
    // Python fallback (`shop.py:223-230`): when no candidate matches
    // (rarity, type), drop the rarity filter and sample over the full
    // type pool. Keeps "Common Power" slots fillable for IRONCLAD's
    // pool (which has no rarity-Common Powers).
    specs_for_character(character)
        .iter()
        .filter_map(|f| {
            let spec = f();
            if spec.card_type == card_type {
                Some(spec.id)
            } else {
                None
            }
        })
        .collect()
}

fn eligible_colorless_card_ids(rarity: CardRarity) -> Vec<CardId> {
    COLORLESS_SPECS
        .iter()
        .filter_map(|f| {
            let spec = f();
            if spec.rarity == rarity {
                Some(spec.id)
            } else {
                None
            }
        })
        .collect()
}

// ── Potion-pool sampling ─────────────────────────────────────────────

fn eligible_potion_kinds(rarity: PotionRarity, character: CharacterId) -> Vec<PotionKind> {
    PotionKind::ALL
        .iter()
        .copied()
        .filter(|k| {
            let spec = crate::potions::all::spec_for(*k);
            if spec.is_event_only {
                return false;
            }
            if spec.rarity != rarity {
                return false;
            }
            // Character-locked potions only spawn for their character.
            !matches!(spec.character_lock, Some(c) if c != character)
        })
        .collect()
}

// ── Main populator ───────────────────────────────────────────────────

/// Fill in the per-slot `card` / `relic_id` / `potion_kind` fields of
/// an inventory previously shaped by
/// [`sts2_core::shop::generate_shop_inventory`].
///
/// Walks every slot in declaration order on `rs.rng.shops`:
///
/// 1. Character cards — sample from
///    `specs_for_character(rs.character_id).filter(rarity, card_type)`.
/// 2. Colorless cards — sample from `COLORLESS_SPECS.filter(rarity)`.
/// 3. Relics — `rs.pop_relic_from_grab_bag(rarity)` (per-rarity bag,
///    with Python's fallback chain). The mapped `RelicRarity::Shop`
///    case uses the shop-pool bucket if populated, else falls back per
///    the standard fallback chain.
/// 4. Potions — sample from `PotionKind::ALL` filtered by `(rarity,
///    character_lock, !is_event_only)`.
///
/// Slots where the eligible pool is empty stay `None`. The two
/// rolled-rarity relic slots may collide with one another (the bag
/// drains, so the second pop sees the first picked id removed; no
/// in-this-shop dedup logic required beyond that).
///
/// Bead `sts2-rs-80apg`. Pin
/// `sts2-rl-agent@01c094d:sts2_env/run/shop.py:399-455`.
pub fn populate_shop_inventory(rs: &mut RunState, inv: &mut ShopInventory) {
    let character = rs.character_id;

    // 1. Character cards.
    for entry in &mut inv.cards {
        let cr = reward_to_card_rarity(entry.rarity);
        let candidates = eligible_character_card_ids(character, cr, entry.card_type);
        if candidates.is_empty() {
            continue;
        }
        let idx = rs.rng.shops.next_int_exclusive(0, candidates.len() as i32) as usize;
        let id = candidates[idx];
        entry.card = card_pod_from_id(id, false);
    }

    // 2. Colorless cards.
    for entry in &mut inv.colorless_cards {
        let cr = reward_to_card_rarity(entry.rarity);
        let candidates = eligible_colorless_card_ids(cr);
        if candidates.is_empty() {
            continue;
        }
        let idx = rs.rng.shops.next_int_exclusive(0, candidates.len() as i32) as usize;
        let id = candidates[idx];
        entry.card = card_pod_from_id(id, false);
    }

    // 3. Relics — drain via the per-rarity grab-bag with fallback
    //    chain. The bag is shared with reward-screen relic picks, so
    //    drawing here naturally prevents the same relic from
    //    appearing in two rooms.
    for entry in &mut inv.relics {
        let rr = shop_to_relic_rarity(entry.relic_rarity);
        entry.relic_id = rs.pop_relic_from_grab_bag(rr);
    }

    // 4. Potions.
    for entry in &mut inv.potions {
        let pr = shop_to_potion_rarity(entry.potion_rarity);
        let candidates = eligible_potion_kinds(pr, character);
        if candidates.is_empty() {
            continue;
        }
        let idx = rs.rng.shops.next_int_exclusive(0, candidates.len() as i32) as usize;
        entry.potion_kind = Some(candidates[idx]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::spec_index::{
        DEFECT_SPECS, IRONCLAD_SPECS, NECROBINDER_SPECS, REGENT_SPECS, SILENT_SPECS,
    };
    use sts2_core::shop::generate_shop_inventory as core_generate;

    fn fresh_ironclad_runstate() -> RunState {
        let mut rs = RunState::new();
        rs.character_id = CharacterId::Ironclad;
        rs
    }

    #[test]
    fn populator_fills_every_character_card_slot_for_ironclad() {
        let mut rs = fresh_ironclad_runstate();
        let mut inv = core_generate(&mut rs.rng.shops.clone(), &rs.card_rarity_odds, 0);
        populate_shop_inventory(&mut rs, &mut inv);

        assert_eq!(inv.cards.len(), 5, "5 character-card slots");
        // Every slot should resolve since the Ironclad pool has at
        // least one Common Attack/Skill and Common/Uncommon/Rare Power.
        for entry in &inv.cards {
            assert!(
                entry.card.is_some(),
                "character card slot {:?}/{:?} must resolve",
                entry.card_type,
                entry.rarity
            );
        }
    }

    #[test]
    fn populator_fills_every_colorless_card_slot() {
        let mut rs = fresh_ironclad_runstate();
        let mut inv = core_generate(&mut rs.rng.shops.clone(), &rs.card_rarity_odds, 0);
        populate_shop_inventory(&mut rs, &mut inv);

        assert_eq!(inv.colorless_cards.len(), 2, "2 colorless slots");
        for entry in &inv.colorless_cards {
            assert!(
                entry.card.is_some(),
                "colorless card slot {:?} must resolve",
                entry.rarity
            );
        }
    }

    #[test]
    fn populator_fills_potion_slots() {
        let mut rs = fresh_ironclad_runstate();
        let mut inv = core_generate(&mut rs.rng.shops.clone(), &rs.card_rarity_odds, 0);
        populate_shop_inventory(&mut rs, &mut inv);

        assert_eq!(inv.potions.len(), 3, "3 potion slots");
        // The normal pool has plenty of common/uncommon/rare potions
        // for Ironclad, so every slot must resolve.
        for entry in &inv.potions {
            assert!(
                entry.potion_kind.is_some(),
                "potion slot {:?} must resolve",
                entry.potion_rarity
            );
        }
    }

    #[test]
    fn populator_pulls_relics_from_grab_bag() {
        let mut rs = fresh_ironclad_runstate();
        // Seed the bag with three distinct relics across rarities.
        use sts2_core::relics::RelicId;
        rs.relic_grab_bag_by_rarity
            .entry(RelicRarity::Common)
            .or_default()
            .push(RelicId::Anchor);
        rs.relic_grab_bag_by_rarity
            .entry(RelicRarity::Uncommon)
            .or_default()
            .push(RelicId::BloodVial);
        rs.relic_grab_bag_by_rarity
            .entry(RelicRarity::Rare)
            .or_default()
            .push(RelicId::Strawberry);
        rs.relic_grab_bag_by_rarity
            .entry(RelicRarity::Shop)
            .or_default()
            .push(RelicId::DollysMirror);
        rs.relic_grab_bag = vec![
            RelicId::Anchor,
            RelicId::BloodVial,
            RelicId::Strawberry,
            RelicId::DollysMirror,
        ];

        let mut inv = core_generate(&mut rs.rng.shops.clone(), &rs.card_rarity_odds, 0);
        populate_shop_inventory(&mut rs, &mut inv);

        // 3 relic slots; whichever rarities rolled, each must have
        // pulled SOMETHING from the seeded bag (fallback chain
        // guarantees this since every bucket above is non-empty).
        for entry in &inv.relics {
            assert!(
                entry.relic_id.is_some(),
                "relic slot {:?} must pull from seeded grab-bag",
                entry.relic_rarity
            );
        }
        // All four bag entries must now be drained or partially
        // drained — at least 3 pops happened.
        let remaining: usize = rs
            .relic_grab_bag_by_rarity
            .values()
            .map(Vec::len)
            .sum::<usize>()
            + rs.relic_grab_bag_fallback.len();
        assert!(remaining <= 1, "at most 1 relic left after 3 shop pops");
    }

    #[test]
    fn populator_is_deterministic_for_fixed_seed() {
        let a = fresh_ironclad_runstate();
        let b = fresh_ironclad_runstate();

        let mut a = a;
        let mut b = b;
        let mut inv_a = core_generate(&mut a.rng.shops.clone(), &a.card_rarity_odds, 0);
        let mut inv_b = core_generate(&mut b.rng.shops.clone(), &b.card_rarity_odds, 0);
        populate_shop_inventory(&mut a, &mut inv_a);
        populate_shop_inventory(&mut b, &mut inv_b);

        let card_ids_a: Vec<_> = inv_a
            .cards
            .iter()
            .map(|e| e.card.as_ref().map(|c| c.card_id))
            .collect();
        let card_ids_b: Vec<_> = inv_b
            .cards
            .iter()
            .map(|e| e.card.as_ref().map(|c| c.card_id))
            .collect();
        assert_eq!(card_ids_a, card_ids_b);

        let potions_a: Vec<_> = inv_a.potions.iter().map(|e| e.potion_kind).collect();
        let potions_b: Vec<_> = inv_b.potions.iter().map(|e| e.potion_kind).collect();
        assert_eq!(potions_a, potions_b);
    }

    // REMOVED with the sixth `CharacterId` variant (2026-08-08):
    // `populator_empty_pool_leaves_slot_unfilled` asserted that an empty
    // character card pool leaves a shop slot `None` instead of panicking. Its
    // only empty-pool fixture was a character STS2 does not have, and every
    // real character now has a populated pool, so the path is unreachable
    // through `CharacterId`. The defensive code in `populate_shop_inventory`
    // is deliberately left in place; if an empty (rarity, type) bucket becomes
    // reachable some other way, re-add coverage through that instead.

    // ── specs_for_character: per-character arm coverage ──────────────────

    use std::collections::HashSet;

    fn ids_of(specs: &[fn() -> CardSpec]) -> HashSet<CardId> {
        specs.iter().map(|f| f().id).collect()
    }

    /// `specs_for_character(Silent)` must return the SILENT pool, not the
    /// empty `_ => &[]` slice. Deleting the `CharacterId::Silent` arm (L73)
    /// drops it to empty → these candidates would vanish, and any returned
    /// id would no longer be a Silent id. We assert the result is non-empty
    /// AND every id is a genuine Silent card disjoint from the Ironclad
    /// pool (so the arm cannot be silently swapped for Ironclad's).
    #[test]
    fn specs_for_character_silent_returns_silent_pool() {
        let silent_ids = ids_of(SILENT_SPECS);
        let ironclad_ids = ids_of(IRONCLAD_SPECS);
        // Abrasive is a Rare Power in the Silent pool (probed).
        let got =
            eligible_character_card_ids(CharacterId::Silent, CardRarity::Rare, CardType::Power);
        assert!(!got.is_empty(), "Silent has at least one Rare Power");
        for id in &got {
            assert!(silent_ids.contains(id), "{id:?} must be a Silent card");
            assert!(
                !ironclad_ids.contains(id),
                "{id:?} must NOT come from the Ironclad pool"
            );
        }
    }

    /// `specs_for_character(Defect)` must return the DEFECT pool, not the
    /// empty `_ => &[]` slice (kills the `CharacterId::Defect` arm delete,
    /// L74). Defect's pool is non-empty and disjoint from Ironclad's.
    #[test]
    fn specs_for_character_defect_returns_defect_pool() {
        let defect_ids = ids_of(DEFECT_SPECS);
        let ironclad_ids = ids_of(IRONCLAD_SPECS);
        assert!(!defect_ids.is_empty(), "Defect pool is ported");
        // Pick any (rarity, type) the Defect pool actually contains by
        // scanning for the first spec.
        let first = DEFECT_SPECS[0]();
        let got = eligible_character_card_ids(CharacterId::Defect, first.rarity, first.card_type);
        assert!(
            !got.is_empty(),
            "a (rarity,type) present in the Defect pool must resolve"
        );
        for id in &got {
            assert!(defect_ids.contains(id), "{id:?} must be a Defect card");
            assert!(
                !ironclad_ids.contains(id),
                "{id:?} must NOT come from the Ironclad pool"
            );
        }
    }

    /// Regression for sts2-rs-m3n0g.7: the shop populator must now sell Regent
    /// and Necrobinder cards. Before delegating to `spec_index`, both fell
    /// through the local `_ => &[]` arm and `specs_for_character` returned an
    /// empty slice — so no Regent/Necrobinder card was ever offered. We assert
    /// both pools resolve a real card for a (rarity, type) they actually
    /// contain.
    #[test]
    fn specs_for_character_sells_regent_and_necrobinder_cards() {
        for (character, specs) in [
            (CharacterId::Regent, REGENT_SPECS),
            (CharacterId::Necrobinder, NECROBINDER_SPECS),
        ] {
            assert!(!specs.is_empty(), "{character:?} pool is ported");
            let pool_ids = ids_of(specs);
            let probe = specs[0]();
            let got = eligible_character_card_ids(character, probe.rarity, probe.card_type);
            assert!(
                !got.is_empty(),
                "{character:?} shop must offer a card for a (rarity,type) in its pool"
            );
            for id in &got {
                assert!(pool_ids.contains(id), "{id:?} must be a {character:?} card");
            }
        }
    }

    // ── eligible_character_card_ids: predicate-boundary coverage ─────────

    /// The strict filter is `spec.rarity == rarity && spec.card_type ==
    /// card_type`. Anger is a Common Attack in the Ironclad pool; BodySlam
    /// is too. We pin all three mutations on L91:
    ///   * `== ↔ !=` on rarity (L91:28): a Common request must NOT surface
    ///     an Uncommon/Rare card.
    ///   * `== ↔ !=` on type (L91:56): a Common Attack request must NOT
    ///     surface a Skill/Power.
    ///   * `&& ↔ ||` (L91:38): with `||`, a card matching EITHER rarity OR
    ///     type would slip in — so we assert every result matches BOTH.
    #[test]
    fn eligible_character_strict_filter_requires_both_rarity_and_type() {
        let got = eligible_character_card_ids(
            CharacterId::Ironclad,
            CardRarity::Common,
            CardType::Attack,
        );
        assert!(!got.is_empty(), "Ironclad has Common Attacks");
        let by_id: std::collections::HashMap<CardId, CardSpec> = IRONCLAD_SPECS
            .iter()
            .map(|f| {
                let s = f();
                (s.id, s)
            })
            .collect();
        for id in &got {
            let spec = &by_id[id];
            assert_eq!(
                spec.rarity,
                CardRarity::Common,
                "{id:?} surfaced for a Common request but isn't Common (kills == → != on rarity, \
                 and && → || which would admit a non-Common Attack)"
            );
            assert_eq!(
                spec.card_type,
                CardType::Attack,
                "{id:?} surfaced for an Attack request but isn't an Attack (kills == → != on type, \
                 and && → || which would admit a Common non-Attack)"
            );
        }
    }

    /// The rarity-drop fallback (`shop.py:223-230`) filters on
    /// `spec.card_type == card_type` only (L109). Ironclad has NO
    /// rarity-`Common` Power, so a (Common, Power) request hits the
    /// fallback and must return ONLY Powers. Flipping `==` to `!=` (L109)
    /// would return every NON-Power instead.
    #[test]
    fn eligible_character_fallback_keeps_only_requested_type() {
        // Ironclad has no Common Power → strict pool empty → fallback over
        // all Powers regardless of rarity.
        let got =
            eligible_character_card_ids(CharacterId::Ironclad, CardRarity::Common, CardType::Power);
        assert!(
            !got.is_empty(),
            "fallback must surface Ironclad Powers when no Common Power exists"
        );
        let by_id: std::collections::HashMap<CardId, CardSpec> = IRONCLAD_SPECS
            .iter()
            .map(|f| {
                let s = f();
                (s.id, s)
            })
            .collect();
        for id in &got {
            assert_eq!(
                by_id[id].card_type,
                CardType::Power,
                "fallback must keep only Powers (kills == → != at L109)"
            );
        }
    }

    // ── eligible_colorless_card_ids: rarity filter ───────────────────────

    /// Colorless filter is `spec.rarity == rarity` (L123). The colorless
    /// pool has Uncommon and Rare cards (no Common/Basic). An Uncommon
    /// request must surface ONLY Uncommon cards; `== → !=` would surface
    /// the Rare ones instead.
    #[test]
    fn eligible_colorless_keeps_only_requested_rarity() {
        let got = eligible_colorless_card_ids(CardRarity::Uncommon);
        assert!(!got.is_empty(), "colorless pool has Uncommon cards");
        let by_id: std::collections::HashMap<CardId, CardSpec> = COLORLESS_SPECS
            .iter()
            .map(|f| {
                let s = f();
                (s.id, s)
            })
            .collect();
        for id in &got {
            assert_eq!(
                by_id[id].rarity,
                CardRarity::Uncommon,
                "colorless filter must keep only the requested rarity (kills == → != at L123)"
            );
        }
    }

    // ── eligible_potion_kinds: rarity + character-lock filters ───────────

    /// `spec.rarity != rarity` early-rejects mismatched rarities (L143).
    /// A Common request must include a Common potion and EXCLUDE an
    /// Uncommon one. Flipping `!= → ==` keeps only mismatches, so the
    /// Common potion would vanish and the Uncommon one would appear.
    #[test]
    fn eligible_potion_kinds_filters_by_rarity() {
        let common = eligible_potion_kinds(PotionRarity::Common, CharacterId::Ironclad);
        assert!(
            common.contains(&PotionKind::BloodPotion),
            "Common Ironclad-locked BloodPotion must appear for a Common Ironclad request"
        );
        // Ashwater is an Uncommon Ironclad-locked potion — wrong rarity for
        // a Common request, so it must be absent (kills != → ==).
        assert!(
            !common.contains(&PotionKind::Ashwater),
            "an Uncommon potion must NOT appear in a Common request"
        );
    }

    /// The character-lock guard is `!matches!(spec.character_lock, Some(c)
    /// if c != character)` (L147): keep a potion UNLESS it's locked to a
    /// different character. BloodPotion is Ironclad-locked, so it is
    /// eligible for Ironclad but NOT for Silent. Deleting the `!` inverts
    /// the guard (keep ONLY foreign-locked potions), flipping both
    /// assertions.
    #[test]
    fn eligible_potion_kinds_respects_character_lock() {
        let for_ironclad = eligible_potion_kinds(PotionRarity::Common, CharacterId::Ironclad);
        assert!(
            for_ironclad.contains(&PotionKind::BloodPotion),
            "Ironclad-locked BloodPotion is eligible for the Ironclad"
        );
        let for_silent = eligible_potion_kinds(PotionRarity::Common, CharacterId::Silent);
        assert!(
            !for_silent.contains(&PotionKind::BloodPotion),
            "Ironclad-locked BloodPotion must NOT be eligible for the Silent (kills delete-! at L147)"
        );
    }
}
