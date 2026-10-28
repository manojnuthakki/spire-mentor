//! Neow-pool Ancient relics — bead `sts2-rs-4rwv4` (W9).
//!
//! These relics live in the C# Neow option pool (`Neow.cs:67-103`) but
//! are absent from the pinned Python sim, so they have no factory in the
//! Python-ported pools (`common` / `uncommon` / `rare` / `shop_event` /
//! `event_reward`). Their `RelicId` variants are appended via
//! `scripts/scrape_relic_ids.py`'s `EXTRA_CSHARP_ONLY` list and ported
//! against the authoritative C# `RelicModel`s under
//! `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Relics/`.
//!
//! Coverage so far (W9a-e):
//! - **`NeowsTalisman`** — `after_obtained`: upgrade the first Basic
//!   Strike and first Basic Defend in the deck (`NeowsTalisman.cs`).
//! - **`WingedBoots`** — no pickup effect; a free-travel map relic
//!   (`WingedBoots.cs`). Granting it to `RunState.relics` is parity-clean
//!   at pickup; the free-travel counter is deferred (W9b) until the
//!   per-floor path harness needs it, so this module registers no
//!   listener for it.
//! - **`NeowsBones`** — `after_obtained`: shuffle the Neow valid-relic
//!   pool with `PlayerRng.Rewards`, grant the first 2 (RewardsSet with
//!   skipping disallowed = obtain all), then add 1 curse drawn via
//!   `Rng.Niche` (`NeowsBones.cs`). Bead `sts2-rs-wjuoi` (W9e).
//! - **`HeftyTablet`** — `after_obtained`: roll 3 distinct Rare cards
//!   from the character pool via `PlayerRng.Rewards`, offer a skippable
//!   1-of-3 select through [`RunState::pending_card_selects`], and always
//!   add 1 Injury curse (`HeftyTablet.cs`). Bead `sts2-rs-w6hdj` (W9d).
//! - **`PhialHolster`** — `after_obtained`: gain +1 max potion slot, then
//!   roll 2 random out-of-combat potions via the `CombatPotionGeneration`
//!   RNG and procure each (`PhialHolster.cs`). Bead `sts2-rs-zgy5m` (W9c).
//!
//! All five W9 Neow-pool relics now map to a `RelicId` and (where they
//! have a pickup effect) fire it; the W9 divergence at the Neow choice is
//! closed.

use sts2_core::card_effects::EffectResult;
use sts2_core::cards::{Card, CardId, EnchantmentId};
use sts2_core::relics::RelicId;
use sts2_core::run_effects::{
    register_after_obtained, register_modify_card_reward_options_late, AfterObtainedRunFn,
    ModifyCardRewardOptionsLateRunFn,
};
use sts2_core::runstate::{PotionReward, RelicReward, RunState};
use sts2_core::NeowRelic;

use crate::cards::spec_index::{
    card_pod_from_id, rarity_for_card_id, sample_distinct_cards_of_rarity,
};
use crate::cards::CardRarity;
use crate::potions::all::create_random_potions_out_of_combat;

/// Per-character Basic Strike ids. C# `NeowsTalisman` filters Deck cards
/// by `Rarity == Basic && Tags.Contains(CardTag.Strike)`; the Rust port
/// does not populate the `Strike`/`Defend` `CardTag` metadata on card
/// specs at this pin, so we match the canonical Basic Strike `CardId`s
/// instead (each is inherently `CardRarity::Basic`). Documented
/// divergence: a modded/non-standard Basic Strike-tagged card would not
/// be picked up — none exist in the v0.103.2 character pools.
const BASIC_STRIKE_IDS: &[CardId] = &[
    CardId::StrikeIronclad,
    CardId::StrikeSilent,
    CardId::StrikeDefect,
    CardId::StrikeRegent,
    CardId::StrikeNecrobinder,
];

/// Per-character Basic Defend ids — see [`BASIC_STRIKE_IDS`].
const BASIC_DEFEND_IDS: &[CardId] = &[
    CardId::DefendIronclad,
    CardId::DefendSilent,
    CardId::DefendDefect,
    CardId::DefendRegent,
    CardId::DefendNecrobinder,
];

/// Register the W9 Neow-relic run-side listeners. Called from
/// [`crate::relics::dispatch::install_default_registry`]. Idempotent.
pub fn install_pool() {
    register_after_obtained(
        RelicId::NeowsTalisman,
        listener_neows_talisman as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::NeowsBones,
        listener_neows_bones as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::HeftyTablet,
        listener_hefty_tablet as AfterObtainedRunFn,
    );
    register_after_obtained(
        RelicId::PhialHolster,
        listener_phial_holster as AfterObtainedRunFn,
    );
    // Bead `sts2-rs-v0107-content-port-wb8lk.6`: SilkenTress (v0.107.1
    // Ancient/Neow curse relic). AfterObtained loses all gold; the first
    // card reward afterward is Glam-enchanted (one-time `IsUsedUp` latch).
    register_after_obtained(
        RelicId::SilkenTress,
        listener_silken_tress as AfterObtainedRunFn,
    );
    register_modify_card_reward_options_late(
        RelicId::SilkenTress,
        listener_silken_tress_card_reward as ModifyCardRewardOptionsLateRunFn,
    );
    // Kaleidoscope (v0.107.1 Ancient/Neow positive relic) is offered at
    // Neow (see `sts2_core::neow_offers::POSITIVE_POOL`) but its
    // `AfterObtained` — gain 2 card rewards each drawn from 3 random
    // *other*-character pools — is DEFERRED pending the
    // `CardRewardAlternative` reward surface (port doc §0). No listener is
    // registered, so picking it currently grants no card rewards; do NOT
    // fake the cross-character pool draw.
}

/// `NeowsTalisman::after_obtained` — upgrade the **last** Basic Strike and
/// the **last** Basic Defend in the deck (C#
/// `MegaCrit.Sts2.Core.Models.Relics/NeowsTalisman.cs::AfterObtained`).
///
/// v0.107.1 delta (`sts2-rs-v0107-content-port-wb8lk.9` §7): the per-tag
/// pick changed `FirstOrDefault` → **`LastOrDefault`**, so the upgrade now
/// lands on the LAST Basic Strike / Defend in deck order (relevant only
/// when the deck holds more than one Basic Strike/Defend — e.g. the
/// starter four Strikes). C# takes `PileType.Deck` cards with
/// `Rarity == Basic`, then `LastOrDefault` on `Tags.Contains(Strike)` and
/// on `Tags.Contains(Defend)`, and `CardCmd.Upgrade`s each non-null result.
fn listener_neows_talisman(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::NeowsTalisman) {
        return EffectResult::NotImplemented;
    }
    upgrade_last_basic(rs, BASIC_STRIKE_IDS);
    upgrade_last_basic(rs, BASIC_DEFEND_IDS);
    EffectResult::Resolved
}

/// Upgrade the **last** `master_deck` card whose id is in `ids` and whose
/// rarity is Basic. Mirrors C# (v0.107.1) `Where(Basic).LastOrDefault(tag)`
/// then `CardCmd.Upgrade`. No-op when none match. `iter_mut().rev().find()`
/// walks from the deck tail, so it returns the LAST matching card.
///
/// Must call [`crate::cards::Card::upgrade`] — NOT a bare `upgraded =
/// true` flag flip. Combat reads the card's stored POD stats
/// (`base_damage`, `base_block`, `cost`), and `Card::upgrade` applies the
/// generated upgrade *delta* to them (e.g. Basic Strike 6 → 9 damage). A
/// flag-only flip leaves `base_damage` at 6, so the "upgraded" Strike
/// still hits for 6 in combat — the silent divergence behind the
/// full-run parity miss (run-parity sweep, `NEOWS_TALISMAN` runs:
/// C# played a 9-damage Strike+, Rust a 6-damage Strike). `upgrade()`
/// is a no-op on an already-upgraded card, matching C#'s `CardCmd.Upgrade`.
fn upgrade_last_basic(rs: &mut RunState, ids: &[CardId]) {
    if let Some(card) = rs.master_deck.iter_mut().rev().find(|c| {
        ids.contains(&c.card_id) && rarity_for_card_id(c.card_id) == Some(CardRarity::Basic)
    }) {
        card.upgrade();
    }
}

/// `SilkenTress::AfterObtained` (v0.107.1 `SilkenTress.cs:42-45`):
/// `PlayerCmd.LoseGold(Owner.Gold, Owner)` — lose ALL current gold.
fn listener_silken_tress(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::SilkenTress) {
        return EffectResult::NotImplemented;
    }
    let all_gold = rs.gold;
    rs.gain_gold(-all_gold);
    EffectResult::Resolved
}

/// `SilkenTress::TryModifyCardRewardOptionsLate` (v0.107.1
/// `SilkenTress.cs:47-73`) + the `AfterModifyingCardRewardOptions`
/// one-time `IsUsed` latch (`:75-83`). On the FIRST card reward after
/// pickup, enchant every Glam-eligible card with `Glam(1)`, then disable
/// the relic so later rewards are untouched.
///
/// The C# checks `glam.CanEnchant(card)` and enchants a clone; here the
/// reward `cards` are already per-reward `Card` instances, so we enchant
/// in place (mirrors the egg `modify_card_reward_options_late` listeners).
/// The `IsUsedUp` latch is modelled with `rs.relic_counters[SilkenTress]`
/// (`0` = unused, `1` = used). C#'s `AfterModifyingCardRewardOptions`
/// latches on the first card-reward pass regardless of whether any card
/// was actually enchantable, so we latch whenever the listener fires on a
/// card reward while owned + unused.
fn listener_silken_tress_card_reward(rs: &mut RunState, cards: &mut Vec<Card>) -> EffectResult {
    if !rs.owns_active_relic(RelicId::SilkenTress) {
        return EffectResult::NotImplemented;
    }
    // `IsUsed` gate (`SilkenTress.cs:57`): only the first card reward fires.
    if rs.relic_counters.get(&RelicId::SilkenTress).copied() == Some(1) {
        return EffectResult::NotImplemented;
    }
    let pool = crate::names::deck_card_spec_pool();
    for card in cards.iter_mut() {
        let Some(spec) = pool.iter().find(|s| s.id == card.card_id) else {
            continue;
        };
        // `glam.CanEnchant(card)` (`SilkenTress.cs:65`).
        if crate::enchantments::can_enchant_card(spec, card, EnchantmentId::Glam) {
            // `CardCmd.Enchant<Glam>(card, 1m)` (`SilkenTress.cs:68`).
            let _ = crate::enchantments::apply_static_enchantment(card, EnchantmentId::Glam, 1);
        }
    }
    // `AfterModifyingCardRewardOptions` → `IsUsed = true` (`:75-83`).
    rs.relic_counters.insert(RelicId::SilkenTress, 1);
    EffectResult::Resolved
}

/// `NeowsBones::GetValidRelics` pool, in C# `Neow.AllPossibleOptions`
/// order (`Neow.cs:53-63`): `CurseOptions`, then `PositiveOptions`, then
/// the seven single add-in options. C# keeps every option relic with
/// `IsAllowedAtNeow(owner) && !(relic is NeowsBones)` (v0.107.1
/// `NeowsBones.cs:35`; v0.103.2 used `IsAllowed(state)`).
///
/// In single-player the only filter that fires is `MassiveScroll`
/// (`MassiveScroll.cs:19` — `Players.Count > 1` → false), so it is
/// dropped here. The only v0.107.1 `IsAllowedAtNeow` overrides among pool
/// relics are `SilverCrucible`/`WingedBoots` (→ `Players.Count == 1`, true)
/// and `ScrollBoxes` (→ `CanGenerateBundles`, always true for the shipped
/// SP pool), and the base is unconditionally `true` — notably it does
/// *not* exclude already-owned relics. So the SP valid pool is exactly
/// these 26, fixed regardless of run state. `NeowsBones` itself is
/// excluded per the `!(o.Relic is NeowsBones)` clause.
///
/// Order verified against v0.107.1 `Neow.AllPossibleOptions`
/// (`Neow.cs:49-64`): `CurseOptions` (8, `:99-108`), then `PositiveOptions`
/// (14, `:66-80`), then the six single add-in options (`:55-60`). SP drops
/// `NeowsBones` (curse) and `MassiveScroll` (`MassiveScroll.cs:21`
/// `Players.Count > 1`), leaving 7 + 13 + 6 = 26. v0.107.1 added
/// `FishingRod`/`Kaleidoscope` to the positives and `SilkenTress` to the
/// curses, and relocated `ScrollBoxes` from the curse pool into the
/// positives (between `PreciseScissors` and `WingedBoots`).
const NEOWS_BONES_VALID_POOL: &[NeowRelic] = &[
    // CurseOptions (Neow.cs:99-108) minus NeowsBones.
    NeowRelic::CursedPearl,
    NeowRelic::HeftyTablet,
    NeowRelic::LargeCapsule,
    NeowRelic::LeafyPoultice,
    NeowRelic::PrecariousShears,
    NeowRelic::SilkenTress,
    NeowRelic::SilverCrucible,
    // PositiveOptions (Neow.cs:66-80) minus MassiveScroll (multiplayer).
    NeowRelic::ArcaneScroll,
    NeowRelic::BoomingConch,
    NeowRelic::FishingRod,
    NeowRelic::GoldenPearl,
    NeowRelic::Kaleidoscope,
    NeowRelic::LeadPaperweight,
    NeowRelic::LostCoffer,
    NeowRelic::NeowsTorment,
    NeowRelic::NewLeaf,
    NeowRelic::PhialHolster,
    NeowRelic::PreciseScissors,
    NeowRelic::ScrollBoxes,
    NeowRelic::WingedBoots,
    // Single add-in options (Neow.cs:55-60), in AddRange order.
    NeowRelic::LavaRock,
    NeowRelic::NeowsTalisman,
    NeowRelic::NutritiousOyster,
    NeowRelic::Pomander,
    NeowRelic::SmallCapsule,
    NeowRelic::StoneHumidifier,
];

/// Curse `CardId`s eligible for the `NeowsBones` curse draw, in
/// `CurseCardPool.GenerateAllCards` order (`CurseCardPool.cs`), filtered
/// to those with `CanBeGeneratedByModifiers == true`. The 8 pool curses
/// that override it to `false` — AscendersBane, BadLuck, CurseOfTheBell,
/// Enthralled, Folly, Greed, PoorSleep, SporeMind — are excluded. The
/// remaining 10 below match the C#
/// `GetUnlockedCards(...).Where(CanBeGeneratedByModifiers)` set that
/// `Rng.Niche.NextItem` samples (`NeowsBones.cs:48-54`).
/// Number of curses `NeowsBones` adds (`Curses` DynamicVar = 1,
/// `NeowsBones.cs:28`).
const NEOWS_BONES_CURSES: usize = 1;

const NEOWS_BONES_CURSE_POOL: &[CardId] = &[
    CardId::Clumsy,
    CardId::Debt,
    CardId::Decay,
    CardId::Doubt,
    CardId::Guilty,
    CardId::Injury,
    CardId::Normality,
    CardId::Regret,
    CardId::Shame,
    CardId::Writhe,
];

/// `NeowsBones::AfterObtained` (`NeowsBones.cs:40-71`): shuffle the valid
/// Neow-relic pool with the player `Rewards` RNG, grant the first 2
/// (offered as a `RewardsSet` with skipping disallowed → all obtained),
/// then add 1 curse drawn from the generatable curse pool via the
/// `Niche` RNG.
///
/// ## Faithfulness (bead `sts2-rs-wjuoi`)
///
/// - **Niche curse order:** v0.107.1 `NeowsBones.cs:45` replaced the
///   v0.103.2 `ToHashSet()` with a deterministic `orderby c.Id` List, so
///   the curse pool is now PROVABLY ordered (ordinal `ModelId.Entry` ==
///   our slug-alpha const order). The old "samples a HashSet, needs a
///   scraped corpus anchor" caveat no longer applies — the order is
///   determined and matches `NEOWS_BONES_CURSE_POOL`.
///
/// (W9d/W9c: granting `HeftyTablet` or `PhialHolster` from this pool now
/// fires its own `after_obtained` via the registered listener, so the
/// former "grant skipped" gaps for both are closed.)
fn listener_neows_bones(rs: &mut RunState) -> EffectResult {
    // Self-only + recursion guard. This listener grants relics by
    // re-entering `accept_relic_reward` (which re-fires the
    // `after_obtained` walk). Gating on `relics.contains(NeowsBones)`
    // would recurse forever; `last_obtained_relic` is set to the
    // just-granted id by each inner `accept_relic_reward`, so the
    // re-entrant dispatch sees `!= NeowsBones` and bails.
    if rs.last_obtained_relic != Some(RelicId::NeowsBones) {
        return EffectResult::NotImplemented;
    }

    // (1) Shuffle the valid pool with PlayerRng.Rewards (NeowsBones.cs:43).
    let mut pool: Vec<NeowRelic> = NEOWS_BONES_VALID_POOL.to_vec();
    rs.rng.rewards.shuffle(&mut pool);

    // (2) Grant the first 2, ONE AT A TIME. C# `RewardsSet(...).Offer()`
    //     awaits each relic's `AfterObtained` — including any interactive
    //     choice it surfaces (NewLeaf's transform select, ScrollBoxes'
    //     bundle pick) — before obtaining the next relic. So if granting the
    //     first relic leaves an unresolved pick on the queues, the remaining
    //     relic(s) must NOT be granted now: queue them on
    //     `pending_post_pick_relic_grants` for the run loop to grant once the
    //     pick resolves (`RunManager::drain_post_pick_relic_grants_if_settled`).
    //     Granting both synchronously made the 2nd relic appear during the
    //     1st's own pick (PARITY22 deck_size, PARITY69 bundle relics).
    //     (Unmapped variants would be skipped; all 23 pool relics now resolve
    //     to a RelicId, so this never elides a grant.)
    let to_grant: Vec<RelicId> = pool
        .iter()
        .take(2)
        .filter_map(|nr| nr.to_relic_id())
        .collect();
    if rs.interactive_rewards {
        // sts2-agent fork: the two relics are a set the player claims, built
        // `WithSkippingDisallowed()` (every relic must be claimed). A set a
        // claimed relic offers (Small Capsule's) opens over it. The curse is
        // rolled once every offered set is done (C#: after `Offer()` returns).
        let set = to_grant
            .into_iter()
            .map(|rid| sts2_core::OfferedReward::Relic(RelicReward::new(rid)))
            .collect();
        rs.offer_custom_rewards(set, false);
        rs.pending_neows_bones_curses = rs
            .pending_neows_bones_curses
            .saturating_add(NEOWS_BONES_CURSES as u32);
        return EffectResult::Resolved;
    }
    let mut remaining = to_grant.into_iter();
    if let Some(first) = remaining.next() {
        rs.relic_rewards.push(RelicReward::new(first));
        let idx = rs.relic_rewards.len() - 1;
        let _ = rs.accept_relic_reward(idx);
    }
    for rid in remaining {
        if rs.has_pending_grant_choice() || !rs.pending_post_pick_relic_grants.is_empty() {
            // A prior grant surfaced an interactive pick — defer the rest,
            // preserving grant order.
            rs.pending_post_pick_relic_grants.push(rid);
        } else {
            rs.relic_rewards.push(RelicReward::new(rid));
            let idx = rs.relic_rewards.len() - 1;
            let _ = rs.accept_relic_reward(idx);
        }
    }

    // Restore the pickup marker the inner grants cleared, so any later
    // listener in this same dispatch walk still observes NeowsBones.
    rs.last_obtained_relic = Some(RelicId::NeowsBones);

    // (3) Add 1 curse via the Niche stream (NeowsBones.cs:54). C# adds it only
    //     AFTER `await RewardsSet.Offer()` returns — i.e. after each granted
    //     relic's own `AfterObtained` (a Pomander's `FromDeckForUpgrade` pick,
    //     an ArcaneScroll's card offer, …) has resolved.
    //
    //     The Niche ROLL must wait too, not just the card-add: a granted relic
    //     can itself read Niche during its interactive `AfterObtained` (NewLeaf
    //     transforms the picked card via `CardCmd.TransformToRandom(_,
    //     Rng.Niche)` when the player resolves the select). Rolling the curse
    //     here — before that pick is even surfaced — consumed the Niche value
    //     C# gives the transform, flipping the transformed card (RP135:
    //     BONESHARDS vs DEATHBRINGER). Defer the ROLL as well: bump the
    //     pending-curse count; `RunManager::drain_post_pick_deck_cards_if_settled`
    //     rolls + adds each curse via the `roll_neows_bones_curse` callback once
    //     the whole grant batch (relics + their picks/rewards) settles, so the
    //     curse is drawn LAST, matching C#.
    let curse_count = NEOWS_BONES_CURSES as u32;
    rs.pending_neows_bones_curses = rs.pending_neows_bones_curses.saturating_add(curse_count);

    EffectResult::Resolved
}

/// Roll ONE `NeowsBones` curse on the `Niche` stream and return its POD,
/// mirroring C# `Rng.Niche.NextItem(availableCurses)` (`NeowsBones.cs:54`).
/// Wired as [`sts2_core::run_manager::ContentCallbacks::roll_neows_bones_curse`]
/// so the run loop can defer the roll until the grant batch settles — after any
/// granted relic's own interactive `Niche` read (NewLeaf's transform), matching
/// C#'s "curse drawn last" order. The pool/index logic lives here because the
/// curse pool is content-owned; core only owns the `Niche` cursor.
#[must_use]
pub fn roll_neows_bones_curse(rs: &mut RunState) -> Option<sts2_core::cards::Card> {
    let last = NEOWS_BONES_CURSE_POOL.len().checked_sub(1)?;
    let idx = rs.rng.niche.next_int(0, last as i32) as usize;
    card_pod_from_id(NEOWS_BONES_CURSE_POOL[idx], false)
}

/// Number of Rare card options HeftyTablet offers (`CardsVar(3)` in
/// `HeftyTablet.cs:23`).
const HEFTY_TABLET_CARD_COUNT: usize = 3;

/// `HeftyTablet::AfterObtained` (`HeftyTablet.cs:27-46`): roll 3 distinct
/// Rare cards from the character pool (uniform odds, `NoUpgradeRoll`) via
/// `CardFactory.CreateForReward`, offer them as a skippable 1-of-N
/// card-select (`CardSelectCmd.FromChooseACardScreen(canSkip: true)`),
/// and **always** add 1 `Injury` curse to the deck (the chosen card, if
/// any, is added by the run loop when the select is resolved).
///
/// Because the after_obtained listener runs synchronously on
/// `&mut RunState` and cannot reach `RunManager`'s `card_reward_options`,
/// the rolled options are enqueued on [`RunState::pending_card_selects`];
/// `RunManager::advance_after_card_reward` drains them into a normal
/// `PickCardReward` fan-out (skip always offered). Bead `sts2-rs-w6hdj`
/// (W9d).
///
/// Faithfulness caveat: the rolled-card *identity* depends on the
/// per-character spec-slice order matching C# `CardPool.GenerateAllCards`;
/// the RNG draw count is faithful regardless. The Rare candidate pool
/// excludes `MultiplayerOnly` cards (via
/// [`sample_distinct_cards_of_rarity`], matching C#
/// `CardFactory.FilterForPlayerCount`) — otherwise an MP-only Rare in the
/// character pool shifts the whole roll. The unpicked-option card-choice
/// *history* (`HeftyTablet.cs:39-45`, cosmetic run-history breadcrumbs) is
/// not modeled.
fn listener_hefty_tablet(rs: &mut RunState) -> EffectResult {
    // Self-only: this listener queues a choice + always adds a curse, so
    // it must fire exactly once — on HeftyTablet's own pickup. Gating on
    // `relics.contains()` would re-fire on every subsequent relic obtain.
    if rs.last_obtained_relic != Some(RelicId::HeftyTablet) {
        return EffectResult::NotImplemented;
    }

    // (1) Roll 3 distinct Rare cards from the character pool via the
    //     Rewards RNG (CreateForReward: Uniform odds, Rarity==Rare,
    //     NoUpgradeRoll).
    let options = sample_distinct_cards_of_rarity(
        rs,
        rs.character_id,
        CardRarity::Rare,
        HEFTY_TABLET_CARD_COUNT,
    );

    // (2) Queue the skippable 1-of-N choice for the run loop to surface,
    //     with the Injury curse deferred to AFTER the screen resolves:
    //     C# builds `[chosen?, Injury]` and adds both in ONE
    //     `CardPileCmd.Add(list, Deck)` once the select returns
    //     (`HeftyTablet.cs:33-39`), so the deck-tail order — load-bearing
    //     as the combat-start shuffle input — is chosen-then-Injury, and
    //     the Injury still lands on a skip. Adding the Injury at obtain
    //     time (the old shape) reversed that tail order.
    let post_add: Vec<sts2_core::Card> = card_pod_from_id(CardId::Injury, false)
        .into_iter()
        .collect();
    if options.is_empty() {
        // No Rare cards to offer (unreachable for shipped pools) — the
        // screen would show nothing; C# still always adds the Injury.
        rs.master_deck.extend(post_add);
    } else {
        rs.pending_card_selects
            .push(sts2_core::runstate::PendingCardSelect::with_post_add(
                options, post_add,
            ));
    }

    EffectResult::Resolved
}

/// Max potion slots PhialHolster grants (`PotionSlots(1)` in
/// `PhialHolster.cs:22`).
const PHIAL_HOLSTER_POTION_SLOTS: i32 = 1;

/// Number of random potions PhialHolster procures (`Potions(2)` in
/// `PhialHolster.cs:23`).
const PHIAL_HOLSTER_POTION_COUNT: usize = 2;

/// `PhialHolster::AfterObtained` (`PhialHolster.cs:26-34`): grant +1 max
/// potion slot, then create 2 random out-of-combat potions via the
/// `CombatPotionGeneration` RNG (`PotionFactory.CreateRandomPotionsOutOfCombat`)
/// and procure each into the potion belt.
///
/// `procure_potion` routes each potion through the `should_procure_potion`
/// veto channel (the Sozu surface), matching C# `PotionCmd.TryToProcure`,
/// and enqueues it on `RunState.pending_rewards` (the established
/// reward-drain surface).
///
/// ## Faithfulness caveats (documented in `tests/parity/known_divergences.md`)
///
/// - **Unlock epochs assumed revealed:** the option pool
///   ([`create_random_potions_out_of_combat`]) uses the full
///   character + shared pools without `UnlockState`/epoch gating, which is
///   the steady state for any account in the scraped corpus. A
///   not-yet-unlocked account would draw from a smaller pool.
/// - **Potion identity vs scraped corpus:** the pool order mirrors the C#
///   `<Character>4Epoch ++ SharedPotionPool` declaration order; the RNG
///   draw count (`2 * Potions`) is faithful regardless. Needs a scraped
///   corpus anchor to confirm the selected-potion identity byte-for-byte.
fn listener_phial_holster(rs: &mut RunState) -> EffectResult {
    // Self-only + recursion guard: this listener can be reached via a
    // NeowsBones grant (which re-enters the after_obtained walk), so gate
    // on the just-obtained relic rather than `relics.contains()`.
    if rs.last_obtained_relic != Some(RelicId::PhialHolster) {
        return EffectResult::NotImplemented;
    }

    // (1) +1 max potion slot (PhialHolster.cs:28).
    rs.gain_potion_slots(PHIAL_HOLSTER_POTION_SLOTS);

    // (2) Roll 2 random out-of-combat potions on the CombatPotionGeneration
    //     stream and procure each (PhialHolster.cs:29-33).
    let potions = create_random_potions_out_of_combat(
        rs.rng.combat_potion_mut(),
        rs.character_id,
        PHIAL_HOLSTER_POTION_COUNT,
    );
    for kind in potions {
        // Sozu (should_procure_potion veto) may drop a procure; ignore the
        // bool, matching C# TryToProcure's fire-and-forget semantics.
        let _ = rs.procure_potion(PotionReward::with_kind(kind));
    }

    EffectResult::Resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use sts2_core::run_effects::dispatch_after_obtained;

    fn card(id: CardId) -> sts2_core::cards::Card {
        let mut c = sts2_core::cards::Card::pod_defaults();
        c.card_id = id;
        c
    }

    /// `NeowsTalisman::after_obtained` upgrades exactly the LAST Basic
    /// Strike and LAST Basic Defend, leaving Bash and the FIRST Strike
    /// untouched. Mirrors C# v0.107.1 `NeowsTalisman.cs` (LastOrDefault per
    /// tag) — the upgrade lands on the second Strike (index 1), not the
    /// first.
    #[test]
    fn neows_talisman_upgrades_last_basic_strike_and_defend() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck = vec![
            card(CardId::StrikeIronclad),
            card(CardId::StrikeIronclad),
            card(CardId::DefendIronclad),
            card(CardId::Bash),
        ];
        rs.relics.push(RelicId::NeowsTalisman);
        rs.last_obtained_relic = Some(RelicId::NeowsTalisman);
        let _ = dispatch_after_obtained(&mut rs);

        assert!(!rs.master_deck[0].upgraded, "first Strike untouched (Last)");
        assert!(rs.master_deck[1].upgraded, "last Strike upgraded");
        assert!(rs.master_deck[2].upgraded, "last Defend upgraded");
        assert!(!rs.master_deck[3].upgraded, "Bash untouched");
    }

    /// Regression (run-parity sweep, `NEOWS_TALISMAN` seeds): the upgrade
    /// must apply the *stat delta*, not just flip the flag. Combat reads
    /// the stored POD `base_damage`/`base_block`, so a flag-only flip left
    /// the "upgraded" Basic Strike hitting for 6 (C# played a 9-damage
    /// Strike+). Build the cards via the real factory so the deltas land.
    #[test]
    fn neows_talisman_applies_upgrade_stat_delta_not_just_flag() {
        crate::cards::dispatch::install_default_registry();
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck = vec![
            card_pod_from_id(CardId::StrikeIronclad, false).expect("strike pod"),
            card_pod_from_id(CardId::DefendIronclad, false).expect("defend pod"),
        ];
        let base_strike_dmg = rs.master_deck[0].base_damage;
        let up_strike_dmg = card_pod_from_id(CardId::StrikeIronclad, true)
            .expect("strike+ pod")
            .base_damage;
        let base_defend_block = rs.master_deck[1].base_block;
        let up_defend_block = card_pod_from_id(CardId::DefendIronclad, true)
            .expect("defend+ pod")
            .base_block;
        // The upgrade must actually change the stats, else the assertion
        // below is vacuous.
        assert!(up_strike_dmg > base_strike_dmg, "Strike+ deals more");
        assert!(up_defend_block > base_defend_block, "Defend+ blocks more");

        rs.relics.push(RelicId::NeowsTalisman);
        rs.last_obtained_relic = Some(RelicId::NeowsTalisman);
        let _ = dispatch_after_obtained(&mut rs);

        assert_eq!(
            rs.master_deck[0].base_damage, up_strike_dmg,
            "upgraded Strike carries the upgraded base_damage into combat"
        );
        assert_eq!(
            rs.master_deck[1].base_block, up_defend_block,
            "upgraded Defend carries the upgraded base_block into combat"
        );
    }

    /// No-op (no panic) when the deck has no Basic Strike/Defend.
    #[test]
    fn neows_talisman_noop_without_basics() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck = vec![card(CardId::Bash)];
        rs.relics.push(RelicId::NeowsTalisman);
        rs.last_obtained_relic = Some(RelicId::NeowsTalisman);
        let _ = dispatch_after_obtained(&mut rs);
        assert!(!rs.master_deck[0].upgraded);
    }

    /// `NeowsBones::after_obtained` grants the first 2 shuffled valid
    /// relics and adds exactly 1 generatable curse — without recursing
    /// (the grant path re-enters `accept_relic_reward`). The expected
    /// grant is derived by replaying the same `Rewards` shuffle on a
    /// cloned stream, so the test pins both the count and the identity
    /// without hardcoding a seed-specific outcome.
    #[test]
    fn neows_bones_grants_two_relics_and_one_curse() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();

        // Predict the shuffle by replaying it on a clone of the stream.
        let mut probe = rs.rng.rewards.clone();
        let mut pool = NEOWS_BONES_VALID_POOL.to_vec();
        probe.shuffle(&mut pool);
        let expected_granted: Vec<RelicId> = pool
            .iter()
            .take(2)
            .filter_map(|nr| nr.to_relic_id())
            .collect();

        rs.relic_rewards.push(RelicReward::new(RelicId::NeowsBones));
        let _ = rs.accept_relic_reward(0);

        assert!(
            rs.relics.contains(&RelicId::NeowsBones),
            "NeowsBones itself recorded"
        );
        // Each predicted top-2 relic is granted — synchronously (in `relics`)
        // OR, when an earlier grant surfaces an interactive pick, deferred to
        // `pending_post_pick_relic_grants` (the run loop grants it once the pick
        // resolves; see `neows_bones_defers_second_grant_when_pending_list_nonempty`).
        // Under the v0.107.1 PlayerRngSet seed (player term 0, not the stale
        // NetId=1 — bead `sts2-rs-ms8mm` family) the seed-0 top-2 is
        // [LostCoffer, LeafyPoultice], and LostCoffer's AfterObtained surfaces a
        // pick, so LeafyPoultice defers.
        for rid in &expected_granted {
            assert!(
                rs.relics.contains(rid) || rs.pending_post_pick_relic_grants.contains(rid),
                "expected granted-or-deferred relic {rid:?}"
            );
        }
        // No pool relic grants further relics on obtain, so the count is exact:
        // NeowsBones + the (Some-mapping) top-2, split across granted + deferred.
        assert_eq!(
            rs.relics.len() + rs.pending_post_pick_relic_grants.len(),
            1 + expected_granted.len()
        );

        assert!(
            rs.pending_post_pick_cards.is_empty(),
            "curse roll is deferred as a count, not an eager card"
        );
        assert_eq!(
            rs.pending_neows_bones_curses, 1,
            "exactly one curse deferred for post-grant settle (rolled at drain)"
        );
    }

    /// Same starting state ⇒ identical grants + curse (RNG determinism).
    #[test]
    fn neows_bones_deterministic_for_same_state() {
        crate::relics::dispatch::ensure_installed();
        let run = || {
            let mut rs = RunState::new();
            rs.relic_rewards.push(RelicReward::new(RelicId::NeowsBones));
            let _ = rs.accept_relic_reward(0);
            let deck: Vec<CardId> = rs.master_deck.iter().map(|c| c.card_id).collect();
            let deferred: Vec<CardId> = rs
                .pending_post_pick_cards
                .iter()
                .map(|c| c.card_id)
                .collect();
            (rs.relics.clone(), deck, deferred)
        };
        assert_eq!(run(), run());
    }

    /// `HeftyTablet::after_obtained` queues a skippable 3-distinct-Rare
    /// card-select and always adds exactly one Injury — without recursing.
    #[test]
    fn hefty_tablet_queues_three_rare_select_and_adds_injury() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new(); // default character: Ironclad
        rs.relic_rewards
            .push(RelicReward::new(RelicId::HeftyTablet));
        let _ = rs.accept_relic_reward(0);

        // The Injury is DEFERRED to after the select resolves (C# adds
        // `[chosen?, Injury]` in one post-select CardPileCmd.Add —
        // `HeftyTablet.cs:33-39` — so the deck tail is chosen-then-Injury).
        assert_eq!(rs.master_deck.len(), 0, "no immediate deck add");
        assert_eq!(rs.pending_card_selects[0].post_add.len(), 1);
        assert_eq!(
            rs.pending_card_selects[0].post_add[0].card_id,
            CardId::Injury
        );

        // One pending card-select with 3 distinct Rare options.
        assert_eq!(rs.pending_card_selects.len(), 1);
        let opts = rs.pending_card_selects[0].options.clone();
        assert_eq!(opts.len(), 3, "three rare options, got {opts:?}");
        let mut distinct = opts.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), 3, "options distinct, got {opts:?}");
        for id in &opts {
            assert_eq!(
                rarity_for_card_id(*id),
                Some(CardRarity::Rare),
                "option {id:?} must be Rare"
            );
        }
    }

    /// Same starting state ⇒ identical rolled options + curse.
    #[test]
    fn hefty_tablet_deterministic_for_same_state() {
        crate::relics::dispatch::ensure_installed();
        let run = || {
            let mut rs = RunState::new();
            rs.relic_rewards
                .push(RelicReward::new(RelicId::HeftyTablet));
            let _ = rs.accept_relic_reward(0);
            (
                rs.pending_card_selects[0].options.clone(),
                rs.master_deck.iter().map(|c| c.card_id).collect::<Vec<_>>(),
            )
        };
        assert_eq!(run(), run());
    }

    /// `PhialHolster::after_obtained` grants +1 max potion slot and procures
    /// exactly 2 random potions into `pending_rewards` — without recursing.
    #[test]
    fn phial_holster_gains_slot_and_procures_two_potions() {
        use sts2_core::runstate::Reward;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new(); // default character: Ironclad
        let slots_before = rs.max_potion_slots;

        rs.relic_rewards
            .push(RelicReward::new(RelicId::PhialHolster));
        let _ = rs.accept_relic_reward(0);

        assert!(rs.relics.contains(&RelicId::PhialHolster));
        assert_eq!(
            rs.max_potion_slots,
            slots_before + PHIAL_HOLSTER_POTION_SLOTS,
            "one extra potion slot"
        );

        let potions: Vec<_> = rs
            .pending_rewards
            .iter()
            .filter_map(|r| match r {
                Reward::Potion(p) => p.potion_kind,
                _ => None,
            })
            .collect();
        assert_eq!(potions.len(), 2, "two potions procured, got {potions:?}");
    }

    /// `roll_neows_bones_curse` rolls a real curse POD from the
    /// generatable curse pool (never `None`). Kills the `-> None` body
    /// replacement.
    #[test]
    fn roll_neows_bones_curse_returns_a_pool_curse() {
        crate::cards::dispatch::install_default_registry();
        let mut rs = RunState::new();
        let card = roll_neows_bones_curse(&mut rs).expect("a curse is always rolled");
        assert!(
            NEOWS_BONES_CURSE_POOL.contains(&card.card_id),
            "rolled curse {:?} must come from the curse pool",
            card.card_id
        );
    }

    /// `NeowsBones` defers the 2nd grant when a grant is already pending
    /// (`has_pending_grant_choice() || !pending_post_pick_relic_grants
    /// .is_empty()`). Pre-seeding `pending_post_pick_relic_grants` makes
    /// the right operand true while the left stays false: under `||` the
    /// 2nd relic must be DEFERRED (pushed to the pending list, not
    /// granted). An `&&` mutation would grant it immediately.
    #[test]
    fn neows_bones_defers_second_grant_when_pending_list_nonempty() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();

        // Predict the top-2 grants by replaying the shuffle.
        let mut probe = rs.rng.rewards.clone();
        let mut pool = NEOWS_BONES_VALID_POOL.to_vec();
        probe.shuffle(&mut pool);
        let expected_granted: Vec<RelicId> = pool
            .iter()
            .take(2)
            .filter_map(|nr| nr.to_relic_id())
            .collect();
        assert_eq!(expected_granted.len(), 2, "two relics map to RelicIds");
        let second = expected_granted[1];

        // Seed the pending-grant list with an unrelated marker so the
        // `!is_empty()` operand is true (left operand stays false: no
        // pending choice). Choose a relic that is not the second grant.
        let marker = if second == RelicId::Anchor {
            RelicId::Lantern
        } else {
            RelicId::Anchor
        };
        rs.pending_post_pick_relic_grants.push(marker);
        assert!(
            !rs.has_pending_grant_choice(),
            "no interactive choice pending (left operand of the || is false)"
        );

        rs.relic_rewards.push(RelicReward::new(RelicId::NeowsBones));
        let _ = rs.accept_relic_reward(0);

        // The 2nd grant must be deferred onto the pending list, NOT in
        // the owned relics. (The 1st grant is always immediate.)
        assert!(
            !rs.relics.contains(&second),
            "2nd grant {second:?} must be deferred (||), not granted (&&)"
        );
        assert!(
            rs.pending_post_pick_relic_grants.contains(&second),
            "2nd grant {second:?} queued onto pending_post_pick_relic_grants"
        );
    }

    /// Same starting state ⇒ identical rolled potions (RNG determinism).
    #[test]
    fn phial_holster_deterministic_for_same_state() {
        use sts2_core::runstate::Reward;
        crate::relics::dispatch::ensure_installed();
        let run = || {
            let mut rs = RunState::new();
            rs.relic_rewards
                .push(RelicReward::new(RelicId::PhialHolster));
            let _ = rs.accept_relic_reward(0);
            rs.pending_rewards
                .iter()
                .filter_map(|r| match r {
                    Reward::Potion(p) => p.potion_kind,
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    /// `SilkenTress::AfterObtained` loses ALL gold on pickup (v0.107.1
    /// `SilkenTress.cs:42-45`). Bead `sts2-rs-v0107-content-port-wb8lk.6`.
    #[test]
    fn silken_tress_after_obtained_zeroes_all_gold() {
        let mut rs = RunState::new();
        rs.gold = 137;
        rs.relics.push(RelicId::SilkenTress);
        rs.last_obtained_relic = Some(RelicId::SilkenTress);
        assert_eq!(listener_silken_tress(&mut rs), EffectResult::Resolved);
        assert_eq!(rs.gold, 0, "SilkenTress loses all gold");
    }

    /// `SilkenTress` only fires for a different relic's pickup if it is the
    /// last-obtained relic — the `after_obtained` self-gate.
    #[test]
    fn silken_tress_after_obtained_self_gated() {
        let mut rs = RunState::new();
        rs.gold = 50;
        rs.relics.push(RelicId::SilkenTress);
        rs.last_obtained_relic = Some(RelicId::Anchor);
        assert_eq!(listener_silken_tress(&mut rs), EffectResult::NotImplemented);
        assert_eq!(
            rs.gold, 50,
            "gold untouched when SilkenTress was not just picked"
        );
    }

    /// `SilkenTress::TryModifyCardRewardOptionsLate` Glam-enchants the FIRST
    /// card reward, then the `IsUsed` latch leaves every later reward
    /// untouched (v0.107.1 `SilkenTress.cs:47-83`).
    #[test]
    fn silken_tress_glams_first_card_reward_then_latches() {
        let mut rs = RunState::new();
        rs.relics.push(RelicId::SilkenTress);

        // First card reward: every enchantable card gains Glam(1).
        let mut first = vec![
            card_pod_from_id(CardId::StrikeIronclad, false).expect("Strike POD"),
            card_pod_from_id(CardId::Bash, false).expect("Bash POD"),
        ];
        assert_eq!(
            listener_silken_tress_card_reward(&mut rs, &mut first),
            EffectResult::Resolved
        );
        assert!(
            first.iter().all(|c| c.has_enchantment(EnchantmentId::Glam)),
            "first reward's cards are Glam-enchanted"
        );
        assert_eq!(
            rs.relic_counters.get(&RelicId::SilkenTress).copied(),
            Some(1),
            "IsUsed latch set after the first card reward"
        );

        // Second card reward: latched -> untouched.
        let mut second = vec![card_pod_from_id(CardId::StrikeIronclad, false).expect("Strike POD")];
        assert_eq!(
            listener_silken_tress_card_reward(&mut rs, &mut second),
            EffectResult::NotImplemented
        );
        assert!(
            !second[0].has_enchantment(EnchantmentId::Glam),
            "later rewards are not enchanted (one-time IsUsedUp)"
        );
    }

    /// Without owning SilkenTress, the card-reward listener is a no-op.
    #[test]
    fn silken_tress_card_reward_noop_when_not_owned() {
        let mut rs = RunState::new();
        let mut cards = vec![card_pod_from_id(CardId::StrikeIronclad, false).expect("Strike POD")];
        assert_eq!(
            listener_silken_tress_card_reward(&mut rs, &mut cards),
            EffectResult::NotImplemented
        );
        assert!(!cards[0].has_enchantment(EnchantmentId::Glam));
    }
}
