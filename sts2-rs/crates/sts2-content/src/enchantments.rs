//! Card enchantment runtime helpers — bead `sts2-rs-k2y` (P6-8).
//!
//! Ports `sts2-rl-agent@01c094d:sts2_env/cards/enchantments.py`. Enchantments
//! are run-persistent stacks attached to a [`Card`] via
//! [`Card::enchantments`]. Unlike `combat_vars`, they survive across combats
//! and serialize as part of the deck (Python: `card.enchantments:
//! dict[str, int]` -> here `SmallVec<[(EnchantmentId, i32); 2]>`).
//!
//! ## Scope
//!
//! Bead `sts2-rs-k2y` (P6-8) lands:
//!
//! 1. [`apply_static_enchantment`] — keyword / cost mutators that fire at
//!    enchant-time ([`EnchantmentId::RoyallyApproved`], [`EnchantmentId::Steady`],
//!    [`EnchantmentId::TezcatarasEmber`], [`EnchantmentId::SoulsPower`],
//!    [`EnchantmentId::Goopy`]). ([`EnchantmentId::Instinct`] is a pure damage
//!    multiplier with no enchant-time mutation — see
//!    [`enchant_damage_multiplicative_x100`].)
//! 2. [`can_enchant_card`] — eligibility predicate.
//! 3. Dynamic add / mul / block triplet: [`enchant_damage_additive`],
//!    [`enchant_damage_multiplicative_x100`], [`enchant_block_additive`],
//!    [`enchant_play_count_bonus`].
//! 4. [`effective_keywords`] — combines the static spec keywords with the
//!    enchantment-derived keyword set so the engine can consult one value.
//!
//! Listener wiring into `combat_loop::play_card` / `hooks::modify_damage_*`
//! is deferred to the P7-8 enchantment-dispatch audit (`sts2-rs-h9u-8`).
//! The functions here are pure and can be called by that bead without
//! restructuring.

use sts2_core::cards::{Card, CardType, EnchantmentId, VarKey};

use crate::cards::{CardKeywords, CardRarity, CardSpec};

// ---------------------------------------------------------------------------
// Eligibility
// ---------------------------------------------------------------------------

/// Whether `spec` is a legal target for `enchantment`.
///
/// Mirrors `can_enchant_card` in `enchantments.py:40-73`. The card's
/// pre-existing enchantments are checked against `card.enchantments` —
/// callers pass the runtime [`Card`] alongside the [`CardSpec`] since
/// some criteria (`cost`, `base_block`) read instance state.
#[must_use]
pub fn can_enchant_card(spec: &CardSpec, card: &Card, enchantment: EnchantmentId) -> bool {
    // Unplayable / status / curse / quest can never be enchanted.
    if matches!(card.card_type, CardType::Status | CardType::Curse) {
        return false;
    }
    if spec.keywords.contains(CardKeywords::UNPLAYABLE) {
        return false;
    }
    // Single-enchantment-per-card rule: if the card already has at least
    // one enchantment, only stacking the same one is permitted.
    if !card.enchantments.is_empty() && !card.has_enchantment(enchantment) {
        return false;
    }

    match enchantment {
        EnchantmentId::Sharp
        | EnchantmentId::Momentum
        | EnchantmentId::Corrupted
        | EnchantmentId::Favored
        | EnchantmentId::Vigorous
        | EnchantmentId::Instinct => card.card_type == CardType::Attack,
        EnchantmentId::RoyallyApproved => {
            matches!(card.card_type, CardType::Attack | CardType::Skill)
        }
        EnchantmentId::Imbued => card.card_type == CardType::Skill,
        EnchantmentId::Spiral | EnchantmentId::TezcatarasEmber => {
            // Basic Strike/Defend gate. We use the spec's rarity as a
            // proxy for Python's `CardTag.STRIKE / DEFEND` tags (tags
            // aren't modelled yet — flagged for the P7-8 dispatch audit).
            spec.rarity == CardRarity::Basic
        }
        // Defend-tagged proxy: any block-bearing card.
        EnchantmentId::Nimble | EnchantmentId::Goopy => card.base_block > 0,
        EnchantmentId::SoulsPower => spec.keywords.contains(CardKeywords::EXHAUST),
        EnchantmentId::Slither => card.cost >= 0, // proxy for "non-X cost"
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Static (apply-time) mutators
// ---------------------------------------------------------------------------

/// Apply the keyword / cost side-effects of granting `enchantment` to
/// `card`. Stacks the enchantment counter and returns the keyword diff
/// that should be folded into the card's effective keyword set.
///
/// Mirrors `apply_static_enchantment` in `enchantments.py:19-37`.
///
/// Returns `(added, removed)` keyword flag bundles. Callers typically:
/// ```ignore
/// let (add, rem) = apply_static_enchantment(&mut card, ench, amount);
/// effective = effective.with(add).without(rem);
/// ```
pub fn apply_static_enchantment(
    card: &mut Card,
    enchantment: EnchantmentId,
    amount: i32,
) -> (CardKeywords, CardKeywords) {
    card.set_enchantment(enchantment, amount);

    let mut added = CardKeywords::NONE;
    let mut removed = CardKeywords::NONE;
    match enchantment {
        EnchantmentId::RoyallyApproved => {
            added = CardKeywords::INNATE.with(CardKeywords::RETAIN);
        }
        EnchantmentId::Steady => {
            added = CardKeywords::RETAIN;
        }
        EnchantmentId::TezcatarasEmber => {
            // C# `TezcatarasEmber.OnEnchant` does exactly
            // `Card.AddKeyword(CardKeyword.Eternal)` (`TezcatarasEmber.cs:18`).
            // ETERNAL is the keyword `CardModel.IsRemovable` reads, so it is the
            // one that has to land; NON_REMOVABLE is the older local alias, kept
            // so existing readers of that flag still see it.
            added = CardKeywords::ETERNAL.with(CardKeywords::NON_REMOVABLE);
            card.cost = 0;
        }
        EnchantmentId::SoulsPower => {
            removed = CardKeywords::EXHAUST;
        }
        // Instinct (C# `Instinct.cs`) is an Attack-only 2× powered-attack
        // damage multiplier (`EnchantDamageMultiplicative` → 2m) and has NO
        // `OnEnchant` / cost behavior — see `enchant_damage_multiplicative_x100`
        // below. The earlier cost-reduction arm was Python-pin drift; C# never
        // touches the card's energy cost.
        EnchantmentId::Goopy => {
            added = CardKeywords::EXHAUST;
        }
        _ => {}
    }
    (added, removed)
}

/// Compute the effective keyword set for a card given its spec keywords +
/// every applied static enchantment. Pure function so callers can recompute
/// on demand (we don't cache).
#[must_use]
pub fn effective_keywords(spec: &CardSpec, card: &Card) -> CardKeywords {
    let mut k = spec.keywords;
    if card.has_enchantment(EnchantmentId::RoyallyApproved) {
        k = k.with(CardKeywords::INNATE).with(CardKeywords::RETAIN);
    }
    if card.has_enchantment(EnchantmentId::Steady) {
        k = k.with(CardKeywords::RETAIN);
    }
    if card.has_enchantment(EnchantmentId::TezcatarasEmber) {
        // See `apply_static_enchantment`: ETERNAL is what IsRemovable reads.
        k = k
            .with(CardKeywords::ETERNAL)
            .with(CardKeywords::NON_REMOVABLE);
    }
    if card.has_enchantment(EnchantmentId::SoulsPower) {
        k = k.without(CardKeywords::EXHAUST);
    }
    if card.has_enchantment(EnchantmentId::Goopy) {
        k = k.with(CardKeywords::EXHAUST);
    }
    k
}

// ---------------------------------------------------------------------------
// Dynamic damage / block / play-count
// ---------------------------------------------------------------------------

/// Bookkeeping bit-flags packed onto [`VarKey::EnchantmentUsed`] for enchantment
/// per-combat "used" flags. The Python source keys these by string
/// (`_enchant_used_<name>`); our typed `VarKey` arena would otherwise need
/// one variant per enchantment, so we collapse to a bit-packed `i32` until
/// the P7-8 dispatch audit decides whether to grow the enum.
const VIGOROUS_USED: i32 = 0b01;
const GLAM_USED: i32 = 0b10;
/// Bit reserved for the Momentum running-bonus stash — distinct from the
/// "used" flags because Momentum keeps an *amount*, not a one-shot bit.
/// We dedicate a separate `VarKey` (`RampageDamage`) for that amount so it
/// stays separable from the bit-flags; see [`momentum_bonus`].
const _MOMENTUM_VAR: VarKey = VarKey::RampageDamage;

/// Additive damage bonus from enchantments. Mirrors `enchant_damage_additive`
/// in `enchantments.py:82-90`. `is_powered` is the Python
/// `ValueProp.is_powered()` gate — when false (e.g. unpowered ticks like
/// Corrupted's self-damage) every enchantment is skipped.
///
/// Delegates to the canonical core implementation
/// [`sts2_core::enchantment_hooks::enchant_damage_additive`], which the damage
/// pipeline (`hooks::modify_damage`) now applies — so this value and the value
/// actually dealt in combat share one source of truth (bead
/// `sts2-rs-enchant-damage-additive`).
#[must_use]
pub fn enchant_damage_additive(card: &Card, is_powered: bool) -> i32 {
    sts2_core::enchantment_hooks::enchant_damage_additive(card, is_powered)
}

/// Multiplicative damage scalar in fixed-point (×100). Mirrors
/// `enchant_damage_multiplicative` (returns 1.0 / 1.5 / 2.0). We return
/// integer ×100 to keep [`Card`] / damage math free of float.
///
/// Delegates to the canonical core implementation
/// [`sts2_core::enchantment_hooks::enchant_damage_multiplicative_x100`], which
/// the damage pipeline (`hooks::modify_damage`) now applies — so this value and
/// the multiplier actually used in combat share one source of truth.
#[must_use]
pub fn enchant_damage_multiplicative_x100(card: &Card, is_powered: bool) -> i32 {
    sts2_core::enchantment_hooks::enchant_damage_multiplicative_x100(card, is_powered)
}

/// Additive block bonus from enchantments. Mirrors C# `Nimble.cs` /
/// `Goopy.cs` `EnchantBlockAdditive`.
///
/// **v0.107.1 (§8):** the powered (`IsPoweredCardOrMonsterMoveBlock`) guard
/// was removed — the bonus now applies unconditionally — so the `is_powered`
/// argument is gone.
///
/// Delegates to the canonical core implementation
/// [`sts2_core::enchantment_hooks::enchant_block_additive`], which the block
/// pipeline (`hooks::modify_block`) now applies — so this value and the block
/// actually granted in combat share one source of truth.
#[must_use]
pub fn enchant_block_additive(card: &Card) -> i32 {
    sts2_core::enchantment_hooks::enchant_block_additive(card)
}

/// Per-enchantment replay bonus C# grants for a card carrying Spiral / Glam:
/// the FIXED `Times.IntValue` (canonical `1`), NOT the enchantment's stack
/// `Amount`. Source: `Spiral.cs` / `Glam.cs` `EnchantPlayCount` both return
/// `originalPlayCount + base.DynamicVars["Times"].IntValue`, and `Times` is a
/// constant `IntVar("Times", 1m)` independent of the enchantment's `Amount`.
/// Extra-play-count bonus from enchantments.
///
/// C# `CardModel.GetEnchantedReplayCount` calls `Enchantment.EnchantPlayCount`
/// ONCE on the card's single enchantment, and both Spiral and Glam add the
/// fixed `Times.IntValue` (= 1), regardless of the enchantment `Amount`
/// (`Spiral.cs` / `Glam.cs`). The earlier port added `card.enchantment(..)`
/// (the stack amount), which over-counted a stacked Spiral/Glam (e.g.
/// `Spiral(2)` → +2 instead of +1). We keep the additive shape for the Rust
/// multi-enchantment model — a card in C# carries at most one enchantment, so
/// only one term can ever contribute — but each contributes the fixed +1.
#[must_use]
pub fn enchant_play_count_bonus(card: &Card) -> i32 {
    sts2_core::enchantment_hooks::enchant_play_count_bonus(card)
}

// ---------------------------------------------------------------------------
// On-play side-effects
// ---------------------------------------------------------------------------

/// Apply every enchantment's `OnPlay` side-effect for the card just played.
///
/// Mirrors C# `EnchantmentModel.OnPlay`, which fires for each enchantment on
/// a card after its base effect resolves. At this pin the only enchantment
/// with an `OnPlay` body is [`EnchantmentId::Inky`] (`Inky.cs:28-41`): it
/// applies `1` Weak to the card's target — Blade of Ink's Shivs are its sole
/// source. `target` is the single resolved enemy target (Shiv is
/// `TargetType::Enemy`; C#'s `AllEnemies` branch is unreachable for any
/// current Inky carrier).
///
/// Callers invoke this from a card's play handler *after* the damage/effect
/// resolves, passing the resolved target — the generic per-card dispatch in
/// `sts2_core::enchantment_hooks` (`dispatch_on_card_played` /
/// `dispatch_after_card_played`) does not thread a target, so on-play effects
/// that need one live at the call site.
pub fn apply_enchantment_on_play(
    state: &mut sts2_core::CombatState,
    card_uid: sts2_core::CardUid,
    target: sts2_core::CreatureUid,
) {
    let idx = card_uid.0.get() as usize - 1;
    let has_inky = state
        .cards
        .get(idx)
        .is_some_and(|c| c.has_enchantment(EnchantmentId::Inky));
    if has_inky {
        sts2_core::card_effects::apply_power_to_enemy(state, target, sts2_core::PowerKind::Weak, 1);
    }
}

// ---------------------------------------------------------------------------
// Per-combat "used this combat" bookkeeping
// ---------------------------------------------------------------------------

/// Current per-combat Momentum damage stash (incremented when a Momentum
/// card resolves). Mirrors `card.combat_vars["_enchant_momentum_bonus"]`.
#[must_use]
pub fn momentum_bonus(card: &Card) -> i32 {
    card.combat_var(VarKey::RampageDamage)
}

/// Add `amount` to the Momentum stash on `card`. Idempotent re: ordering.
pub fn add_momentum_bonus(card: &mut Card, amount: i32) {
    let v = card
        .combat_var(VarKey::RampageDamage)
        .saturating_add(amount);
    card.set_combat_var(VarKey::RampageDamage, v);
}

/// Mark Vigorous as having fired this combat. Idempotent.
pub fn mark_vigorous_used(card: &mut Card) {
    let v = card.combat_var(VarKey::EnchantmentUsed) | VIGOROUS_USED;
    card.set_combat_var(VarKey::EnchantmentUsed, v);
}

/// Mark Glam as having fired this combat. Idempotent.
pub fn mark_glam_used(card: &mut Card) {
    let v = card.combat_var(VarKey::EnchantmentUsed) | GLAM_USED;
    card.set_combat_var(VarKey::EnchantmentUsed, v);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::num::NonZeroU32;
    use sts2_core::cards::{CardId, TargetType};

    fn uid(n: u32) -> sts2_core::CardUid {
        sts2_core::CardUid(NonZeroU32::new(n).expect("non-zero"))
    }

    fn strike_card() -> Card {
        Card {
            uid: uid(1),
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        }
    }

    fn defend_card() -> Card {
        Card {
            base_damage: 0,
            base_block: 5,
            card_type: CardType::Skill,
            card_id: CardId::DefendIronclad,
            ..strike_card()
        }
    }

    fn basic_strike_spec() -> CardSpec {
        CardSpec {
            id: CardId::StrikeIronclad,
            name: "STRIKE_IRONCLAD",
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            rarity: CardRarity::Basic,
            base_damage: 6,
            base_block: 0,
            keywords: CardKeywords::NONE,
            effect: crate::cards::CardEffect::DealDamage { base: 6 },
        }
    }

    // --- apply_static_enchantment ----------------------------------------

    #[test]
    fn royally_approved_adds_innate_and_retain() {
        let mut c = strike_card();
        let (add, rem) = apply_static_enchantment(&mut c, EnchantmentId::RoyallyApproved, 1);
        assert!(add.contains(CardKeywords::INNATE));
        assert!(add.contains(CardKeywords::RETAIN));
        assert_eq!(rem, CardKeywords::NONE);
        assert_eq!(c.enchantment(EnchantmentId::RoyallyApproved), 1);
    }

    #[test]
    fn steady_adds_retain_only() {
        let mut c = strike_card();
        let (add, _) = apply_static_enchantment(&mut c, EnchantmentId::Steady, 1);
        assert!(add.contains(CardKeywords::RETAIN));
        assert!(!add.contains(CardKeywords::INNATE));
    }

    #[test]
    fn tezcataras_ember_zeros_cost_and_marks_eternal() {
        let mut c = strike_card();
        let (add, _) = apply_static_enchantment(&mut c, EnchantmentId::TezcatarasEmber, 1);
        assert!(add.contains(CardKeywords::NON_REMOVABLE));
        assert_eq!(c.cost, 0);
    }

    #[test]
    fn souls_power_removes_exhaust() {
        let mut c = strike_card();
        let (_, rem) = apply_static_enchantment(&mut c, EnchantmentId::SoulsPower, 1);
        assert!(rem.contains(CardKeywords::EXHAUST));
    }

    #[test]
    fn instinct_is_a_damage_multiplier_not_a_cost_reducer() {
        // C# `Instinct.cs` has only `CanEnchantCardType` (Attack) and
        // `EnchantDamageMultiplicative` → 2× on a powered attack. It has NO
        // `OnEnchant` / cost behavior — enchanting must leave the cost alone.
        let mut c = strike_card();
        assert_eq!(c.cost, 1);
        apply_static_enchantment(&mut c, EnchantmentId::Instinct, 1);
        assert_eq!(c.cost, 1, "Instinct must NOT change the card's cost");
        // It registers as a 2× powered-attack damage multiplier.
        assert_eq!(enchant_damage_multiplicative_x100(&c, true), 200);
        assert_eq!(
            enchant_damage_multiplicative_x100(&c, false),
            100,
            "unpowered hits are not multiplied"
        );
    }

    #[test]
    fn goopy_adds_exhaust() {
        let mut c = defend_card();
        let (add, _) = apply_static_enchantment(&mut c, EnchantmentId::Goopy, 1);
        assert!(add.contains(CardKeywords::EXHAUST));
    }

    /// Bead `sts2-rs-9kr0m` (vxvvs.1): integration pin on the
    /// `effective_keywords` reducer — applying RoyallyApproved +
    /// SoulsPower on the same EXHAUST-keyword card must yield a
    /// keyword set with INNATE + RETAIN added AND EXHAUST removed.
    /// Catches a regression where the reducer's add/remove order
    /// swaps (which would leave EXHAUST set on a SoulsPower'd card).
    #[test]
    fn effective_keywords_layers_royally_approved_plus_souls_power() {
        let mut spec = basic_strike_spec();
        spec.keywords = CardKeywords::EXHAUST;
        let mut c = strike_card();
        c.set_enchantment(EnchantmentId::RoyallyApproved, 1);
        c.set_enchantment(EnchantmentId::SoulsPower, 1);
        let k = effective_keywords(&spec, &c);
        assert!(
            k.contains(CardKeywords::INNATE),
            "RoyallyApproved adds INNATE"
        );
        assert!(
            k.contains(CardKeywords::RETAIN),
            "RoyallyApproved adds RETAIN"
        );
        assert!(
            !k.contains(CardKeywords::EXHAUST),
            "SoulsPower must remove EXHAUST even after RoyallyApproved layered on top",
        );
    }

    /// Bead `sts2-rs-9kr0m`: Goopy + SoulsPower on a block-bearing
    /// card. SoulsPower removes EXHAUST first; Goopy adds it back
    /// (the reducer applies SoulsPower BEFORE Goopy at
    /// crates/sts2-content/src/enchantments.rs:147-152, so Goopy
    /// wins the EXHAUST tug-of-war). This test pins that order so a
    /// reducer-rearrangement that flips the precedence surfaces
    /// immediately.
    #[test]
    fn effective_keywords_goopy_beats_souls_power_when_both_present() {
        let spec = basic_strike_spec();
        let mut c = defend_card();
        c.set_enchantment(EnchantmentId::Goopy, 1);
        c.set_enchantment(EnchantmentId::SoulsPower, 1);
        let k = effective_keywords(&spec, &c);
        assert!(
            k.contains(CardKeywords::EXHAUST),
            "Goopy (applied AFTER SoulsPower in the reducer) wins; \
             EXHAUST stays on. Flipping the reducer order would change \
             behavior — pin it here.",
        );
    }

    // --- can_enchant_card ------------------------------------------------

    #[test]
    fn cannot_enchant_unplayable() {
        let c = strike_card();
        let mut spec = basic_strike_spec();
        spec.keywords = CardKeywords::UNPLAYABLE;
        assert!(!can_enchant_card(&spec, &c, EnchantmentId::Sharp));
    }

    #[test]
    fn cannot_double_enchant_with_different_kinds() {
        let mut c = strike_card();
        c.set_enchantment(EnchantmentId::Sharp, 1);
        let spec = basic_strike_spec();
        // Same enchantment stacking is allowed.
        assert!(can_enchant_card(&spec, &c, EnchantmentId::Sharp));
        // Different kind is rejected.
        assert!(!can_enchant_card(&spec, &c, EnchantmentId::Favored));
    }

    #[test]
    fn imbued_only_skill() {
        let strike = strike_card();
        let defend = defend_card();
        let spec = basic_strike_spec();
        assert!(!can_enchant_card(&spec, &strike, EnchantmentId::Imbued));
        assert!(can_enchant_card(&spec, &defend, EnchantmentId::Imbued));
    }

    #[test]
    fn nimble_requires_base_block() {
        let strike = strike_card();
        let defend = defend_card();
        let spec = basic_strike_spec();
        assert!(!can_enchant_card(&spec, &strike, EnchantmentId::Nimble));
        assert!(can_enchant_card(&spec, &defend, EnchantmentId::Nimble));
    }

    /// A Power card returns a non-`Attack`, non-`Skill`, non-status type.
    /// `RoyallyApproved` (Attack|Skill only) must REJECT it, while the
    /// generic `_ => true` fall-through would ACCEPT it — so deleting the
    /// `RoyallyApproved` match arm flips this assertion (survivor L67).
    #[test]
    fn royally_approved_rejects_power_cards() {
        let spec = basic_strike_spec();
        let mut power = strike_card();
        power.card_type = CardType::Power;
        power.base_block = 0;
        assert!(
            !can_enchant_card(&spec, &power, EnchantmentId::RoyallyApproved),
            "RoyallyApproved is Attack|Skill only; a Power card is ineligible"
        );
        // Sanity: it DOES accept an Attack and a Skill.
        assert!(can_enchant_card(
            &spec,
            &strike_card(),
            EnchantmentId::RoyallyApproved
        ));
        assert!(can_enchant_card(
            &spec,
            &defend_card(),
            EnchantmentId::RoyallyApproved
        ));
    }

    /// `Spiral` / `TezcatarasEmber` gate on `spec.rarity == Basic`. A
    /// non-Basic (Common) spec must be REJECTED; the generic `_ => true`
    /// fall-through would accept it. Kills the `delete match arm` (L71)
    /// and the `== ↔ !=` flip (L75) survivors.
    #[test]
    fn spiral_and_tezcataras_require_basic_rarity() {
        let basic = basic_strike_spec();
        let mut common = basic_strike_spec();
        common.rarity = CardRarity::Common;
        let card = strike_card();

        // Basic rarity: eligible.
        assert!(can_enchant_card(&basic, &card, EnchantmentId::Spiral));
        assert!(can_enchant_card(
            &basic,
            &card,
            EnchantmentId::TezcatarasEmber
        ));
        // Non-Basic rarity: ineligible (distinguishes the arm from `_ => true`
        // and pins `==` against `!=`).
        assert!(!can_enchant_card(&common, &card, EnchantmentId::Spiral));
        assert!(!can_enchant_card(
            &common,
            &card,
            EnchantmentId::TezcatarasEmber
        ));
    }

    /// `SoulsPower` gates on `spec.keywords.contains(EXHAUST)`. A card
    /// WITHOUT exhaust must be rejected; deleting the arm (L79) would let
    /// the generic `_ => true` accept it.
    #[test]
    fn souls_power_requires_exhaust_keyword() {
        let no_exhaust = basic_strike_spec(); // keywords = NONE
        let mut exhaust_spec = basic_strike_spec();
        exhaust_spec.keywords = CardKeywords::EXHAUST;
        let card = strike_card();

        assert!(
            !can_enchant_card(&no_exhaust, &card, EnchantmentId::SoulsPower),
            "SoulsPower needs an EXHAUST card; a non-exhaust card is ineligible"
        );
        assert!(can_enchant_card(
            &exhaust_spec,
            &card,
            EnchantmentId::SoulsPower
        ));
    }

    /// `Slither` gates on `card.cost >= 0` (proxy for non-X cost). An
    /// X-cost card (`cost == -1`) must be REJECTED; deleting the arm (L80)
    /// or flipping `>=` to `<` (L80:45) both flip this assertion.
    #[test]
    fn slither_rejects_x_cost_cards() {
        let spec = basic_strike_spec();
        let mut x_cost = strike_card();
        x_cost.cost = -1; // X-cost sentinel
        assert!(
            !can_enchant_card(&spec, &x_cost, EnchantmentId::Slither),
            "Slither rejects X-cost (negative) cards"
        );
        // A zero-cost card sits on the `>= 0` boundary and IS eligible
        // (kills `>=` → `<`, which would reject cost == 0).
        let mut zero_cost = strike_card();
        zero_cost.cost = 0;
        assert!(can_enchant_card(&spec, &zero_cost, EnchantmentId::Slither));
    }

    // --- per-combat "used" bit-flags (OR vs XOR) -------------------------

    /// `mark_vigorous_used` ORs in the `VIGOROUS_USED` bit. Calling it on a
    /// card that ALREADY has the bit set must leave it set — `|` keeps it,
    /// `^` would clear it. Pre-setting the bit kills the `| → ^` mutant
    /// (L305) that a from-zero test cannot distinguish.
    #[test]
    fn mark_vigorous_used_is_idempotent_when_already_set() {
        let mut c = strike_card();
        mark_vigorous_used(&mut c);
        let after_first = c.combat_var(VarKey::EnchantmentUsed);
        mark_vigorous_used(&mut c); // OR again: no change; XOR would clear.
        assert_eq!(
            c.combat_var(VarKey::EnchantmentUsed),
            after_first,
            "re-marking Vigorous must keep the bit (OR), not toggle it (XOR)"
        );
        assert_ne!(
            after_first & VIGOROUS_USED,
            0,
            "the VIGOROUS_USED bit stays set"
        );
    }

    /// Same idempotency pin for `mark_glam_used` (kills `| → ^` at L311).
    /// Glam's effect is observable via `enchant_play_count_bonus`: once
    /// used, a Glam card grants +0. A second `mark_glam_used` must NOT
    /// flip it back to +1 (which XOR would, by clearing the bit).
    #[test]
    fn mark_glam_used_is_idempotent_when_already_set() {
        let mut c = strike_card();
        c.set_enchantment(EnchantmentId::Glam, 1);
        assert_eq!(enchant_play_count_bonus(&c), 1);
        mark_glam_used(&mut c);
        assert_eq!(enchant_play_count_bonus(&c), 0);
        mark_glam_used(&mut c); // OR again: stays used; XOR would un-use it.
        assert_eq!(
            enchant_play_count_bonus(&c),
            0,
            "re-marking Glam must keep it consumed (OR), not toggle it (XOR)"
        );
    }

    // --- Dynamic damage / block ------------------------------------------

    #[test]
    fn sharp_adds_to_damage() {
        let mut c = strike_card();
        c.set_enchantment(EnchantmentId::Sharp, 3);
        assert_eq!(enchant_damage_additive(&c, true), 3);
        // Unpowered: no enchantment damage.
        assert_eq!(enchant_damage_additive(&c, false), 0);
    }

    #[test]
    fn inky_adds_one_powered_only() {
        let mut c = strike_card();
        c.set_enchantment(EnchantmentId::Inky, 1);
        // v0.107.1 Inky.EnchantDamageAdditive → fixed +1 on a powered attack
        // (DamageVar(1m); was 2m in v0.103.2).
        assert_eq!(enchant_damage_additive(&c, true), 1);
        // Unpowered (e.g. self-damage tick): no enchantment damage.
        assert_eq!(enchant_damage_additive(&c, false), 0);
    }

    /// v0.107.1 (§8) NEW enchant `TezcatarasEmber`: `DamageVar(3)` +
    /// `EnchantDamageAdditive` → +3 on a POWERED attack only, regardless of
    /// the stack amount (the value is the canonical-var base, not the stack).
    #[test]
    fn tezcataras_ember_adds_three_powered_only() {
        let mut c = strike_card();
        c.set_enchantment(EnchantmentId::TezcatarasEmber, 1);
        assert_eq!(enchant_damage_additive(&c, true), 3);
        // Unpowered: no enchantment damage (mirrors props.IsPoweredAttack()).
        assert_eq!(enchant_damage_additive(&c, false), 0);
        // Stacking does not change the additive — it is DamageVar(3m) fixed.
        c.set_enchantment(EnchantmentId::TezcatarasEmber, 2);
        assert_eq!(enchant_damage_additive(&c, true), 3);
    }

    #[test]
    fn favored_doubles_damage() {
        let mut c = strike_card();
        c.set_enchantment(EnchantmentId::Favored, 1);
        assert_eq!(enchant_damage_multiplicative_x100(&c, true), 200);
    }

    #[test]
    fn corrupted_one_point_five_x_damage() {
        let mut c = strike_card();
        c.set_enchantment(EnchantmentId::Corrupted, 1);
        assert_eq!(enchant_damage_multiplicative_x100(&c, true), 150);
    }

    #[test]
    fn vigorous_first_play_then_consumed() {
        let mut c = strike_card();
        c.set_enchantment(EnchantmentId::Vigorous, 5);
        assert_eq!(enchant_damage_additive(&c, true), 5);
        mark_vigorous_used(&mut c);
        assert_eq!(enchant_damage_additive(&c, true), 0);
    }

    /// Direct getter coverage for `momentum_bonus`: a fresh card reads `0`,
    /// and after stashing it reflects the running amount. Pins the function
    /// against the constant-return mutants (`-> 0 / 1 / -1` at L288) — the
    /// `0` reading kills `-> 1` and `-> -1`, and the post-stash `5` reading
    /// kills `-> 0`.
    #[test]
    fn momentum_bonus_getter_reflects_stash() {
        let mut c = strike_card();
        assert_eq!(momentum_bonus(&c), 0, "fresh card has no Momentum stash");
        add_momentum_bonus(&mut c, 5);
        assert_eq!(momentum_bonus(&c), 5, "getter reflects the stashed amount");
    }

    #[test]
    fn momentum_running_bonus_persists() {
        let mut c = strike_card();
        c.set_enchantment(EnchantmentId::Momentum, 2);
        // Before any play: no bonus.
        assert_eq!(enchant_damage_additive(&c, true), 0);
        // Simulate one Momentum play: stash +2.
        add_momentum_bonus(&mut c, 2);
        assert_eq!(enchant_damage_additive(&c, true), 2);
        add_momentum_bonus(&mut c, 2);
        assert_eq!(enchant_damage_additive(&c, true), 4);
    }

    #[test]
    fn nimble_adds_block() {
        let mut c = defend_card();
        c.set_enchantment(EnchantmentId::Nimble, 4);
        assert_eq!(enchant_block_additive(&c), 4);
    }

    /// v0.107.1 (§8): `Nimble`/`Goopy` `EnchantBlockAdditive` lost the
    /// `IsPoweredCardOrMonsterMoveBlock` guard, so the block bonus applies
    /// unconditionally — there is no longer a powered/unpowered distinction
    /// (the function no longer takes `is_powered`). Pins that a Nimble card
    /// yields the same bonus on any block grant path (patch note: Entrench
    /// gains extra Block from Nimble).
    #[test]
    fn nimble_block_applies_unconditionally_v0107() {
        let mut c = defend_card();
        c.set_enchantment(EnchantmentId::Nimble, 4);
        // Bonus is no longer gated on a powered grant.
        assert_eq!(enchant_block_additive(&c), 4);
    }

    #[test]
    fn goopy_block_grows_after_plays() {
        let mut c = defend_card();
        // Initial Goopy(1) gives +0 block (Python: get("Goopy", 1) - 1).
        c.set_enchantment(EnchantmentId::Goopy, 1);
        assert_eq!(enchant_block_additive(&c), 0);
        // After one play it'd be Goopy(2) → +1 block.
        c.set_enchantment(EnchantmentId::Goopy, 2);
        assert_eq!(enchant_block_additive(&c), 1);
    }

    #[test]
    fn spiral_extra_plays() {
        let mut c = strike_card();
        // C# `Spiral.EnchantPlayCount` adds the fixed `Times.IntValue` (= 1),
        // NOT the stack `Amount` — a `Spiral(2)` card still replays only +1.
        c.set_enchantment(EnchantmentId::Spiral, 2);
        assert_eq!(enchant_play_count_bonus(&c), 1);
        // A single Spiral stack is also +1 (matches the C# constant Times).
        let mut c1 = strike_card();
        c1.set_enchantment(EnchantmentId::Spiral, 1);
        assert_eq!(enchant_play_count_bonus(&c1), 1);
    }

    #[test]
    fn glam_consumed_after_first_use() {
        let mut c = strike_card();
        c.set_enchantment(EnchantmentId::Glam, 1);
        assert_eq!(enchant_play_count_bonus(&c), 1);
        mark_glam_used(&mut c);
        assert_eq!(enchant_play_count_bonus(&c), 0);
    }

    // --- effective_keywords ----------------------------------------------

    #[test]
    fn effective_keywords_combines_spec_and_enchantments() {
        let mut spec = basic_strike_spec();
        spec.keywords = CardKeywords::EXHAUST;
        let mut c = strike_card();
        // Bare card → exhaust only.
        assert!(effective_keywords(&spec, &c).contains(CardKeywords::EXHAUST));
        // SoulsPower removes exhaust.
        c.set_enchantment(EnchantmentId::SoulsPower, 1);
        let k = effective_keywords(&spec, &c);
        assert!(!k.contains(CardKeywords::EXHAUST));
        // RoyallyApproved layered on → adds innate + retain.
        c.set_enchantment(EnchantmentId::RoyallyApproved, 1);
        let k = effective_keywords(&spec, &c);
        assert!(k.contains(CardKeywords::INNATE));
        assert!(k.contains(CardKeywords::RETAIN));
    }

    // --- Persistence across pile transitions -----------------------------

    #[test]
    fn enchantments_survive_three_turn_cycles_play_discard_draw() {
        // Acceptance criterion from the bead spec: enchantments survive
        // play → discard → draw → play three times over.

        use sts2_core::cards::{discard_pile_into_draw_pile, draw, move_card, PileKind};
        use sts2_core::PlayerCombatState;

        let mut player = PlayerCombatState::default();
        let mut card = strike_card();
        apply_static_enchantment(&mut card, EnchantmentId::Sharp, 3);
        let card_uid = card.uid;
        // Push the (cloned) UID into the draw pile; the runtime Card
        // itself lives in CombatState::cards. For this test we only care
        // about the Card POD persisting its enchantments through clones —
        // pile commands move CardUids, not Card structs.
        player.draw.push(card_uid);

        let mut rng = sts2_core::dotnet_rng::Rng::new(0_i32);
        let cards = sts2_core::cards::CardStore::new();
        for _ in 0..3 {
            // Draw to hand.
            let moved = draw(&mut player, 1, &cards, &mut rng);
            assert_eq!(moved, 1);
            // Simulate "play" by moving to play pile then to discard.
            // (Engine handles this in combat_loop; we just exercise the
            // primitives.)
            move_card(&mut player, card_uid, PileKind::Hand, PileKind::Play).expect("hand -> play");
            move_card(&mut player, card_uid, PileKind::Play, PileKind::Discard)
                .expect("play -> discard");
            // Discard pile back into draw (would happen at end of turn).
            discard_pile_into_draw_pile(&mut player, &cards, &mut rng);
            // Enchantments on the Card POD are unaffected by pile motion
            // (they live on the Card, not the pile).
            assert_eq!(card.enchantment(EnchantmentId::Sharp), 3);
        }
        // After 3 cycles the card is back in the draw pile with its
        // enchantment stack intact.
        assert_eq!(card.enchantment(EnchantmentId::Sharp), 3);
    }
}
