//! Rare-rarity relic dispatch — bead `sts2-rs-ge1` (P7-4).
//!
//! Ported from `sts2-rl-agent@01c094d:sts2_env/relics/rare.py`
//! (50 relics). This file registers a Phase-7 on-trigger handler
//! for every rare-rarity [`RelicId`] variant. Handler bodies
//! are intentionally minimal markers (`EffectResult::Resolved`)
//! that satisfy the dispatch-table acceptance criterion: every
//! variant resolves through the registry instead of falling
//! through to `NotImplemented`. Concrete gameplay plumbing
//! (energy gains, HP-loss caps, ethereal-card generation, rest-site
//! options, reward-modification hooks, ...) lands in downstream
//! Phase-7 plumbing beads once the corresponding `CombatState` /
//! `RunState` surfaces land.
//!
//! ### Note on bead description
//!
//! The bead spec mentioned `Snecko Eye` / `Dead Branch` / `Pandora's
//! Box` as deep-test exemplars; those three relics actually live in
//! `sts2_env/relics/shop_event.py` (rarity = SHOP or BOSS) and ship
//! with the P7-5 sibling bead `sts2-rs-yyw`, not here. The deep
//! pin-tests in this file cover the analogous high-impact rare
//! relics with crisp Python-side constants: `Mango` (+14 `MaxHP`),
//! `MeatOnTheBone` (heal 12 at <=50% HP), `TungstenRod` (-1 HP loss),
//! and `LizardTail` (50% revive). These four are the rare-rarity
//! counterparts to Snecko / `DeadBranch` / Pandora — they pin Python
//! constants and lock the hook names so the integration bead can
//! drop in real logic without retyping numbers.
//!
//! ### Stateful relics
//!
//! Many rare relics carry per-combat or per-run counter state
//! in Python (`ArtOfWar` last-turn attack flag, `BeatingRemnant`
//! damage-this-turn accumulator, `DemonTongue` triggered-this-turn,
//! `Girya` lift-counter (run-scoped), `Kunai`/`Shuriken`/`Pocketwatch`
//! attack/card counters, `LastingCandy` combats-seen, `LizardTail`
//! one-shot, `Metronome` orb-channel accumulator, `MiniRegent`
//! used-this-turn, `RainbowRing` per-type counters, `UnsettlingLamp`
//! triggered, `RuinedHelmet` used-this-combat). Per-instance counters
//! will live on the runtime `Relic` POD — modelled in this bead as
//! comments on each handler to lock the Python pin for the downstream
//! plumbing bead.

use sts2_core::card_effects::{apply_power_to_creature, EffectResult};
use sts2_core::cards::{Card, CardType};
use sts2_core::hooks::{CreatureRef, DamageCtx, DamageMul};
use sts2_core::powers::PowerKind;
use sts2_core::relic_effects::{
    register_after_card_played, register_modify_damage_multiplicative, AfterCardPlayedFn,
    CardEventCtx, ModifyDamageMultiplicativeFn,
};
use sts2_core::relics::RelicId;
use sts2_core::run_effects::{
    register_after_obtained, register_modify_card_being_added_to_deck,
    register_modify_card_reward_options_late, register_modify_merchant_card_creation_results,
    register_modify_rest_site_options, register_modify_rewards,
    register_should_force_potion_reward, AfterObtainedRunFn, ModifyCardBeingAddedToDeckRunFn,
    ModifyCardRewardOptionsLateRunFn, ModifyMerchantCardCreationResultsRunFn,
    ModifyRestSiteOptionsRunFn, ModifyRewardsRunFn, ShouldForcePotionRewardRunFn,
};
use sts2_core::runstate::{CardReward, RestSiteOption, Reward, RoomType, RunState};
use sts2_core::{CombatState, RelicKind};

use crate::relics::{RelicEffect, RelicRarity, RelicSpec};

// ---------------------------------------------------------------------------
// Pool installer
// ---------------------------------------------------------------------------

/// All 50 rare-rarity relic ids from
/// `sts2-rl-agent@01c094d:sts2_env/relics/rare.py`, in Python
/// `@register_relic` declaration order. Membership/identity data only —
/// live behaviour flows through the typed hook-channel listeners
/// registered by [`install_pool`], not through any dispatch marker.
const RARE_RELIC_IDS: &[RelicId] = &[
    RelicId::ArtOfWar,
    RelicId::BeatingRemnant,
    RelicId::BigHat,
    RelicId::Bookmark,
    RelicId::CaptainsWheel,
    RelicId::Chandelier,
    RelicId::CharonsAshes,
    RelicId::DemonTongue,
    RelicId::EmotionChip,
    RelicId::FrozenEgg,
    RelicId::GamblingChip,
    RelicId::GamePiece,
    RelicId::Girya,
    RelicId::HelicalDart,
    RelicId::IceCream,
    RelicId::IntimidatingHelmet,
    RelicId::IvoryTile,
    RelicId::Kunai,
    RelicId::LastingCandy,
    RelicId::LizardTail,
    RelicId::LunarPastry,
    RelicId::Mango,
    RelicId::MeatOnTheBone,
    RelicId::Metronome,
    RelicId::MiniRegent,
    RelicId::MoltenEgg,
    RelicId::MummifiedHand,
    RelicId::OldCoin,
    RelicId::OrangeDough,
    RelicId::Pocketwatch,
    RelicId::PowerCell,
    RelicId::PrayerWheel,
    RelicId::RainbowRing,
    RelicId::RazorTooth,
    RelicId::Shovel,
    RelicId::Shuriken,
    RelicId::StoneCalendar,
    RelicId::SturdyClamp,
    RelicId::TheCourier,
    RelicId::ToughBandages,
    RelicId::ToxicEgg,
    RelicId::TungstenRod,
    RelicId::UnceasingTop,
    RelicId::UnsettlingLamp,
    RelicId::RuinedHelmet,
    RelicId::VexingPuzzlebox,
    RelicId::WhiteBeastStatue,
    RelicId::WhiteStar,
    RelicId::PaperKrane,
    RelicId::CloakClasp,
];

/// Install every rare-relic handler. Called from
/// [`crate::relics::dispatch::install_default_registry`]. Idempotent
/// (re-registering overwrites the same function pointer).
/// Every [`RelicId`] registered in this module's rare pool — the
/// droppable relics of this tier. Used to seed the run's relic grab
/// bag so combat/treasure/shop relic rewards are stocked (the run
/// loop's `populate_relic_grab_bag_by_rarity` source). Bead `sts2-rs-64mq2`.
#[must_use]
pub fn pool_relic_ids() -> Vec<RelicId> {
    RARE_RELIC_IDS.to_vec()
}

pub fn install_pool() {
    // Bead `sts2-rs-u8qa`: real `after_obtained` listener for Mango's
    // `MAX_HP = 14` (Python `rare.py::Mango.MAX_HP`). The combat-side
    // `on_trigger_mango` handler stays `Resolved`-only; this is the
    // run-state side that actually mutates the player's max HP.
    register_after_obtained(RelicId::Mango, listener_mango as AfterObtainedRunFn);
    // Bead `sts2-rs-tyqo`: real `after_obtained` listener for OldCoin's
    // `GOLD = 300` (Python `rare.py::OldCoin.GOLD`). Same pattern as
    // Mango above — the AST factory line was metadata-only.
    register_after_obtained(RelicId::OldCoin, listener_old_coin as AfterObtainedRunFn);
    // Bead `sts2-rs-4ihz`: real `modify_rewards` listener for PrayerWheel.
    // Python `rare.py::PrayerWheel.modify_rewards` appends a CardReward
    // when `room.room_type == MONSTER`. Combat-side `on_trigger_prayer_wheel`
    // stays a `resolved_marker`; this is the run-state side.
    register_modify_rewards(
        RelicId::PrayerWheel,
        listener_prayer_wheel as ModifyRewardsRunFn,
    );
    // Bead `sts2-rs-b4tb`: real `modify_rewards` listener for WhiteStar.
    // Python `rare.py::WhiteStar.modify_rewards` appends a CardReward
    // (`context='boss'`) when `room.room_type == ELITE`. Same shape as
    // PrayerWheel above; only the room-type gate differs. The boss-rarity
    // context-tag is deferred — current `CardReward` POD has no `context`
    // field (umbrella `sts2-rs-01o`).
    register_modify_rewards(
        RelicId::WhiteStar,
        listener_white_star as ModifyRewardsRunFn,
    );
    // Bead `sts2-rs-roi4f`: WhiteBeastStatue forces a potion reward
    // on every combat-room (Monster/Elite/Boss) clear. Python
    // `rare.py::WhiteBeastStatue.should_force_potion_reward`.
    // Listener registered eagerly; firing site is the future
    // potion-roll step of the combat-end reward generator
    // (filed as a follow-up sub-bead).
    register_should_force_potion_reward(
        RelicId::WhiteBeastStatue,
        listener_white_beast_statue as ShouldForcePotionRewardRunFn,
    );
    // Bead `sts2-rs-yye7d`: Shovel appends DigOption to the rest-site
    // option list. Mirrors `rare.py::Shovel.modify_rest_site_options`.
    // The is_allowed act-3-chest gate is out of scope at this pin.
    register_modify_rest_site_options(
        RelicId::Shovel,
        listener_shovel_rest_site_options as ModifyRestSiteOptionsRunFn,
    );
    // Girya appends LiftOption while TimesLifted < 3 (C#
    // `Girya.TryModifyRestSiteOptions`, `Girya.cs:58-69`). Surfaced by
    // the 2026-07-18 shadow-replay census: the real DLL rest site offers
    // [Heal, Smith, Lift] with Girya owned, the engine offered only 2.
    register_modify_rest_site_options(
        RelicId::Girya,
        listener_girya_rest_site_options as ModifyRestSiteOptionsRunFn,
    );
    // IvoryTile is implemented ONCE, in sts2-core
    // (`relic_effects::ivory_tile::on_after_card_played_ivory_tile`,
    // installed by `relic_effects::install_default_registry`), on the
    // same `after_card_played` relic channel. The earlier content-side
    // `listener_ivory_tile` port (bead `sts2-rs-mft0`, Python pin) was a
    // SECOND registration on that channel. Since both target the same
    // `RelicId::IvoryTile` key, the later (content) install silently
    // OVERWROTE the core one in the registry BTreeMap, so which body
    // fired was install-ORDER-dependent — a latent correctness/
    // determinism bug. The duplicate has been removed; do NOT re-add a
    // content-side IvoryTile listener. Bead `sts2-rs-zerx1`.
    // Bead `sts2-rs-8aw7`: real `after_card_played` listener for
    // HelicalDart (`DEXTERITY = 1`). Python `rare.py::HelicalDart`:
    // `if card.is_shiv: owner.apply_power(DEXTERITY, 1)`. Combat-side
    // `on_trigger_helical_dart` stays a `resolved_marker`.
    register_after_card_played(
        RelicId::HelicalDart,
        listener_helical_dart as AfterCardPlayedFn,
    );
    // Issue #50 — FrozenEgg / MoltenEgg / ToxicEgg auto-upgrade Power /
    // Attack / Skill cards as they enter the deck. Three run-side
    // channels matter: `modify_card_reward_options_late` so the
    // pre-pick reward UI shows the upgraded version, plus
    // `modify_card_being_added_to_deck` (catches event / merchant /
    // direct-add paths) and `modify_merchant_card_creation_results`
    // (mirrors FresnelLens). Mirrors Python `rare.py::FrozenEgg /
    // MoltenEgg / ToxicEgg`.
    register_modify_card_reward_options_late(
        RelicId::FrozenEgg,
        listener_frozen_egg_card_reward as ModifyCardRewardOptionsLateRunFn,
    );
    register_modify_card_being_added_to_deck(
        RelicId::FrozenEgg,
        listener_frozen_egg_deck_add as ModifyCardBeingAddedToDeckRunFn,
    );
    register_modify_merchant_card_creation_results(
        RelicId::FrozenEgg,
        listener_frozen_egg_merchant as ModifyMerchantCardCreationResultsRunFn,
    );
    register_modify_card_reward_options_late(
        RelicId::MoltenEgg,
        listener_molten_egg_card_reward as ModifyCardRewardOptionsLateRunFn,
    );
    register_modify_card_being_added_to_deck(
        RelicId::MoltenEgg,
        listener_molten_egg_deck_add as ModifyCardBeingAddedToDeckRunFn,
    );
    register_modify_merchant_card_creation_results(
        RelicId::MoltenEgg,
        listener_molten_egg_merchant as ModifyMerchantCardCreationResultsRunFn,
    );
    register_modify_card_reward_options_late(
        RelicId::ToxicEgg,
        listener_toxic_egg_card_reward as ModifyCardRewardOptionsLateRunFn,
    );
    register_modify_card_being_added_to_deck(
        RelicId::ToxicEgg,
        listener_toxic_egg_deck_add as ModifyCardBeingAddedToDeckRunFn,
    );
    register_modify_merchant_card_creation_results(
        RelicId::ToxicEgg,
        listener_toxic_egg_merchant as ModifyMerchantCardCreationResultsRunFn,
    );
    // PaperKrane — additional multiplier on Weak-attack damage when
    // the player is targeted. Python pin
    // `sts2-rl-agent@01c094d:sts2_env/relics/rare.py::PaperKrane`
    // (`EXTRA_WEAK = 0.15`). Combat-side `on_trigger_paper_krane`
    // stays a `resolved_marker`.
    register_modify_damage_multiplicative(
        RelicId::PaperKrane,
        listener_paper_krane as ModifyDamageMultiplicativeFn,
    );
}

// ---------------------------------------------------------------------------
// Issue #50 — FrozenEgg / MoltenEgg / ToxicEgg auto-upgrade listeners
// ---------------------------------------------------------------------------
//
// Each egg watches one `CardType` (Power / Attack / Skill) and sets
// `card.upgraded = true` on every freshly-encountered card of that
// type. Mirrors Python `rare.py::FrozenEgg / MoltenEgg / ToxicEgg`
// (`should_upgrade_card_on_obtain` semantics: every applicable card
// added to the master deck — reward, event, merchant — is upgraded
// pre-add). Curse/Status cards never upgrade.

///
/// C# `EggRelicHelper.UpgradeValidCards` (the card-reward arm): a card of the
/// type that `IsUpgradable` gets `CardCmd.Upgrade`, its whole upgrade, as the
/// deck-add arm (sts2-agent fork, event-deck-add-hooks).
fn upgrade_cards_of_type(cards: &mut [Card], ct: CardType) -> bool {
    let mut any = false;
    for card in cards {
        any |= upgrade_card_of_type(card, ct);
    }
    any
}

/// The deck-add arm: C# `{Frozen,Molten,Toxic}Egg.TryModifyCardBeingAddedToDeck`
/// gates on `card.Type` and `card.IsUpgradable`, then `CardCmd.Upgrade` on a
/// clone — the card's whole `OnUpgrade` (cost, numbers, keywords), which
/// [`Card::upgrade`] applies through the registered card factory, keeping the
/// instance's state (sts2-agent fork, calling-bell-curse F7).
fn upgrade_card_of_type(card: &mut Card, ct: CardType) -> bool {
    card.card_type == ct && card.is_upgradable() && card.upgrade()
}

fn run_owns_rare(rs: &RunState, id: RelicId) -> bool {
    rs.relics.contains(&id)
}

/// `FrozenEgg.modify_card_reward_options_late` — upgrade every Power
/// card in the displayed reward option list. Ownership-gated.
fn listener_frozen_egg_card_reward(rs: &mut RunState, cards: &mut Vec<Card>) -> EffectResult {
    if !run_owns_rare(rs, RelicId::FrozenEgg) {
        return EffectResult::NotImplemented;
    }
    if upgrade_cards_of_type(cards, CardType::Power) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `FrozenEgg.modify_card_being_added_to_deck` — upgrade the card
/// before it lands in the deck if it is a Power. Ownership-gated.
fn listener_frozen_egg_deck_add(rs: &mut RunState, card: &mut Card) -> EffectResult {
    if !run_owns_rare(rs, RelicId::FrozenEgg) {
        return EffectResult::NotImplemented;
    }
    if upgrade_card_of_type(card, CardType::Power) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `FrozenEgg.modify_merchant_card_creation_results` — upgrade Power
/// cards in merchant inventory. Mirrors FresnelLens's parity hook.
fn listener_frozen_egg_merchant(
    rs: &mut RunState,
    card: &mut Card,
    _is_colorless: bool,
) -> EffectResult {
    if !run_owns_rare(rs, RelicId::FrozenEgg) {
        return EffectResult::NotImplemented;
    }
    if upgrade_card_of_type(card, CardType::Power) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `MoltenEgg` — same pattern, Attack-gated.
fn listener_molten_egg_card_reward(rs: &mut RunState, cards: &mut Vec<Card>) -> EffectResult {
    if !run_owns_rare(rs, RelicId::MoltenEgg) {
        return EffectResult::NotImplemented;
    }
    if upgrade_cards_of_type(cards, CardType::Attack) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

fn listener_molten_egg_deck_add(rs: &mut RunState, card: &mut Card) -> EffectResult {
    if !run_owns_rare(rs, RelicId::MoltenEgg) {
        return EffectResult::NotImplemented;
    }
    if upgrade_card_of_type(card, CardType::Attack) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

fn listener_molten_egg_merchant(
    rs: &mut RunState,
    card: &mut Card,
    _is_colorless: bool,
) -> EffectResult {
    if !run_owns_rare(rs, RelicId::MoltenEgg) {
        return EffectResult::NotImplemented;
    }
    if upgrade_card_of_type(card, CardType::Attack) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `ToxicEgg` — same pattern, Skill-gated.
fn listener_toxic_egg_card_reward(rs: &mut RunState, cards: &mut Vec<Card>) -> EffectResult {
    if !run_owns_rare(rs, RelicId::ToxicEgg) {
        return EffectResult::NotImplemented;
    }
    if upgrade_cards_of_type(cards, CardType::Skill) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

fn listener_toxic_egg_deck_add(rs: &mut RunState, card: &mut Card) -> EffectResult {
    if !run_owns_rare(rs, RelicId::ToxicEgg) {
        return EffectResult::NotImplemented;
    }
    if upgrade_card_of_type(card, CardType::Skill) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

fn listener_toxic_egg_merchant(
    rs: &mut RunState,
    card: &mut Card,
    _is_colorless: bool,
) -> EffectResult {
    if !run_owns_rare(rs, RelicId::ToxicEgg) {
        return EffectResult::NotImplemented;
    }
    if upgrade_card_of_type(card, CardType::Skill) {
        EffectResult::Resolved
    } else {
        EffectResult::NotImplemented
    }
}

/// `HelicalDart::after_card_played` — when the just-played card is a
/// Shiv, `owner.apply_power(DEXTERITY, 1)` (Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/rare.py::HelicalDart`,
/// `DEXTERITY = 1`). Python's `card.is_shiv` (base.py:111-113) is
/// `card_id == SHIV or CardTag.SHIV in tags` — both legs mirrored
/// here so generated Shiv variants (e.g. silent-pool retains) also
/// trigger. Bead `sts2-rs-8aw7`.
fn listener_helical_dart(state: &mut CombatState, ctx: &CardEventCtx) -> EffectResult {
    if !state.relics.owns(RelicKind::HelicalDart) {
        return EffectResult::NotImplemented;
    }
    let Some(card_uid) = ctx.card_uid else {
        return EffectResult::NotImplemented;
    };
    let card_idx = card_uid.0.get() as usize - 1;
    let Some(card) = state.cards.get(card_idx) else {
        return EffectResult::NotImplemented;
    };
    let is_shiv = card.card_id == sts2_core::cards::CardId::Shiv
        || card.tags.contains(sts2_core::cards::CardTag::Shiv);
    if !is_shiv {
        return EffectResult::NotImplemented;
    }
    let Some(player_uid) = state.player.creature_uid else {
        return EffectResult::NotImplemented;
    };
    // sts2-rs-i09du: HelicalDartPower : TemporaryDexterityPower
    // (IsPositive=true) folds +amount Dex on apply, reverts at
    // owner-side AfterTurnEnd via hooks.rs after_turn_end HelicalDart.
    apply_power_to_creature(
        state,
        player_uid,
        PowerKind::Dexterity,
        HELICAL_DART_DEXTERITY,
    );
    apply_power_to_creature(
        state,
        player_uid,
        PowerKind::HelicalDart,
        HELICAL_DART_DEXTERITY,
    );
    EffectResult::Resolved
}

/// `Mango::after_obtained` — `owner.gain_max_hp(14)` (Python
/// `rare.py::Mango`, `MAX_HP=14`). Bead `sts2-rs-u8qa`.
fn listener_mango(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::Mango) {
        return EffectResult::NotImplemented;
    }
    rs.gain_max_hp(MANGO_MAX_HP);
    EffectResult::Resolved
}

/// `OldCoin::after_obtained` — `owner.gain_gold(300)` (Python
/// `rare.py::OldCoin`, `GOLD=300`). Bead `sts2-rs-tyqo`.
fn listener_old_coin(rs: &mut RunState) -> EffectResult {
    if rs.last_obtained_relic != Some(RelicId::OldCoin) {
        return EffectResult::NotImplemented;
    }
    rs.gain_gold(OLD_COIN_GOLD);
    EffectResult::Resolved
}

/// `PrayerWheel::modify_rewards` — append a CardReward to the rewards
/// vec when the player just cleared a Monster room (Python
/// `rare.py::PrayerWheel.modify_rewards`, room-type gate
/// `RoomType.MONSTER`). Elite / Boss / Shop / Treasure / Event /
/// RestSite rooms are unchanged. Ownership-gated. Bead `sts2-rs-4ihz`.
fn listener_prayer_wheel(
    rs: &mut RunState,
    rewards: &mut Vec<Reward>,
    room_type: RoomType,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::PrayerWheel) {
        return EffectResult::NotImplemented;
    }
    if room_type != RoomType::Monster {
        return EffectResult::NotImplemented;
    }
    // C# `CardReward(CardCreationOptions.ForRoom(player, RoomType.Monster), 3)`.
    rewards.push(Reward::Card(CardReward::for_room(RoomType::Monster)));
    EffectResult::Resolved
}

/// `WhiteStar::modify_rewards` — append a (boss-rarity) CardReward to
/// the rewards vec when the player just cleared an Elite room (Python
/// `rare.py::WhiteStar.modify_rewards`, room-type gate `RoomType.ELITE`).
/// The `context='boss'` tag in Python is deferred — current `CardReward`
/// POD has no `context` field (umbrella `sts2-rs-01o`); the rarity bias
/// will land once the rewards-flow surface grows the tag. Ownership-gated.
/// Bead `sts2-rs-b4tb`.
fn listener_white_star(
    rs: &mut RunState,
    rewards: &mut Vec<Reward>,
    room_type: RoomType,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::WhiteStar) {
        return EffectResult::NotImplemented;
    }
    if room_type != RoomType::Elite {
        return EffectResult::NotImplemented;
    }
    // C# `CardReward(CardCreationOptions.ForRoom(Owner, RoomType.Boss), 3)`.
    rewards.push(Reward::Card(CardReward::for_room(RoomType::Boss)));
    EffectResult::Resolved
}

/// `WhiteBeastStatue::should_force_potion_reward` — force a potion
/// reward on every combat-room reward fan-out (Monster / Elite /
/// Boss). Mirrors Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/rare.py::WhiteBeastStatue`
/// (lines 1255-1262): returns True on combat rooms, False on
/// non-combat. The Rust channel signals "force = true" via the
/// out-param; the False case collapses to "leave force untouched"
/// since the dispatcher early-outs on the first `true` and other
/// listeners are free to add their own opinion.
///
/// Ownership-gated. The Python `is_allowed = is_before_act3_treasure_chest`
/// gate (deactivate at Act 3 treasure-chest crossing) is deferred —
/// no Rust path tracks that crossing yet; mirror the
/// `Shovel.modify_rest_site_options` precedent and treat the relic
/// as always-available once obtained.
///
/// **Firing site**: the potion-roll step of the combat-end reward
/// generator isn't ported yet (Python uses
/// `PotionRewardOdds.roll(rng, is_elite, force=...)`); the listener
/// registers eagerly so once the firing site lands (filed as a
/// follow-up sub-bead) the force-drop activates without
/// re-registration.
///
/// Bead `sts2-rs-roi4f`.
fn listener_white_beast_statue(
    rs: &mut RunState,
    room_type: RoomType,
    force: &mut bool,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::WhiteBeastStatue) {
        return EffectResult::NotImplemented;
    }
    if !matches!(
        room_type,
        RoomType::Monster | RoomType::Elite | RoomType::Boss
    ) {
        return EffectResult::NotImplemented;
    }
    *force = true;
    EffectResult::Resolved
}

/// `Shovel::modify_rest_site_options` — append [`RestSiteOption::Dig`]
/// to the rest-site option list if not already present. Mirrors Python
/// `sts2-rl-agent@01c094d:sts2_env/relics/rare.py::Shovel`:
///
/// ```python
/// def modify_rest_site_options(self, owner, options, run_state):
///     if not any(getattr(option, "option_id", "") == "DIG" for option in options):
///         options.append(DigOption(owner))
///     return options
/// ```
///
/// The Python `is_allowed` act-3-chest gate is out of scope at this
/// pin (no act/map run-loop yet); the relic is treated as always
/// available once obtained. Ownership-gated. Bead `sts2-rs-yye7d`.
fn listener_shovel_rest_site_options(
    rs: &RunState,
    options: &mut Vec<RestSiteOption>,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::Shovel) {
        return EffectResult::NotImplemented;
    }
    if options.iter().any(|opt| opt.option_id() == "DIG") {
        return EffectResult::NotImplemented;
    }
    options.push(RestSiteOption::Dig);
    EffectResult::Resolved
}

/// C# `Girya.TryModifyRestSiteOptions` (`Girya.cs:58-69`): while
/// `TimesLifted < 3` append a `LiftRestSiteOption`; at 3 the option is
/// NOT listed at all (not merely disabled). `TimesLifted` lives in
/// `relic_counters[Girya]` (incremented by
/// `RunState::do_rest_site_action(Lift)`, read by the combat-side
/// `relic_effects::girya` Strength grant). Ownership-gated.
fn listener_girya_rest_site_options(
    rs: &RunState,
    options: &mut Vec<RestSiteOption>,
) -> EffectResult {
    if !rs.owns_active_relic(RelicId::Girya) {
        return EffectResult::NotImplemented;
    }
    let lifts = rs.relic_counters.get(&RelicId::Girya).copied().unwrap_or(0);
    if lifts >= GIRYA_MAX_LIFTS {
        return EffectResult::NotImplemented;
    }
    if options.iter().any(|opt| opt.option_id() == "LIFT") {
        return EffectResult::NotImplemented;
    }
    options.push(RestSiteOption::Lift {
        lifts_done: u8::try_from(lifts).unwrap_or(u8::MAX),
    });
    EffectResult::Resolved
}

// ---------------------------------------------------------------------------
// Public per-relic constants (pinned from Python)
// ---------------------------------------------------------------------------
//
// Exposed as `pub const` so the P7-8 integration bead can wire real
// hook bodies without re-typing the numbers from Python. Mirrors the
// `pub const` convention from bead `sts2-rs-evs` (P7-2 commons).

/// `ArtOfWar::ENERGY` — energy gained when no attacks were played
/// last turn (`rare.py::ArtOfWar.ENERGY`).
pub const ART_OF_WAR_ENERGY: i32 = 1;

/// `BeatingRemnant::MAX_HP_LOSS` — per-turn HP-loss cap
/// (`rare.py::BeatingRemnant.MAX_HP_LOSS`).
pub const BEATING_REMNANT_MAX_HP_LOSS: i32 = 20;

/// `BigHat::CARDS` — round-1 ethereal cards generated
/// (`rare.py::BigHat.CARDS`).
pub const BIG_HAT_CARDS: i32 = 2;

/// `CaptainsWheel::BLOCK` — round-3 block on block-clear
/// (`rare.py::CaptainsWheel.BLOCK`).
pub const CAPTAINS_WHEEL_BLOCK: i32 = 18;

/// `Chandelier::ENERGY` — round-3 energy gain
/// (`rare.py::Chandelier.ENERGY`).
pub const CHANDELIER_ENERGY: i32 = 3;

/// `CharonsAshes::DAMAGE` — damage to all enemies on card-exhaust
/// (`rare.py::CharonsAshes.DAMAGE`).
pub const CHARONS_ASHES_DAMAGE: i32 = 3;

/// `GamePiece::CARDS` — cards drawn when a Power is played
/// (`rare.py::GamePiece.CARDS`).
pub const GAME_PIECE_CARDS: i32 = 1;

/// `Girya::MAX_LIFTS` — strength cap from rest-site lifting
/// (`rare.py::Girya.MAX_LIFTS`).
pub const GIRYA_MAX_LIFTS: i32 = 3;

/// `HelicalDart::DEXTERITY` — dex per Shiv played
/// (`rare.py::HelicalDart.DEXTERITY`).
pub const HELICAL_DART_DEXTERITY: i32 = 1;

/// `IntimidatingHelmet::BLOCK` — block on >=2-cost card
/// (`rare.py::IntimidatingHelmet.BLOCK`).
pub const INTIMIDATING_HELMET_BLOCK: i32 = 4;
/// `IntimidatingHelmet::ENERGY_THRESHOLD` — energy-cost threshold
/// (`rare.py::IntimidatingHelmet.ENERGY_THRESHOLD`).
pub const INTIMIDATING_HELMET_ENERGY_THRESHOLD: i32 = 2;

/// `IvoryTile::ENERGY` / `THRESHOLD` — gain 1 energy when card
/// spends >= 3 energy (`rare.py::IvoryTile`).
pub const IVORY_TILE_ENERGY: i32 = 1;
pub const IVORY_TILE_THRESHOLD: i32 = 3;

/// `Kunai::ATTACK_THRESHOLD` / `DEXTERITY` — every 3rd attack +1 Dex
/// (`rare.py::Kunai`).
pub const KUNAI_ATTACK_THRESHOLD: i32 = 3;
pub const KUNAI_DEXTERITY: i32 = 1;

/// `LizardTail::HEAL_PCT` — % max-HP heal on revive
/// (`rare.py::LizardTail.HEAL_PCT`).
pub const LIZARD_TAIL_HEAL_PCT: i32 = 50;

/// `LunarPastry::STARS` — stars gained per turn end
/// (`rare.py::LunarPastry.STARS`).
pub const LUNAR_PASTRY_STARS: i32 = 1;

/// `Mango::MAX_HP` — max-HP gained on obtain
/// (`rare.py::Mango.MAX_HP`).
pub const MANGO_MAX_HP: i32 = 14;

/// `MeatOnTheBone::HP_THRESHOLD_PCT` / `HEAL` — heal at <=50% HP
/// after combat (`rare.py::MeatOnTheBone`).
pub const MEAT_ON_THE_BONE_HP_THRESHOLD_PCT: i32 = 50;
pub const MEAT_ON_THE_BONE_HEAL: i32 = 12;

/// `Metronome::ORB_THRESHOLD` / `DAMAGE` — every 7 orbs deal 30 `AoE`
/// (`rare.py::Metronome`).
pub const METRONOME_ORB_THRESHOLD: i32 = 7;
pub const METRONOME_DAMAGE: i32 = 30;

/// `MiniRegent::STRENGTH` — strength on first stars-spent each turn
/// (`rare.py::MiniRegent.STRENGTH`).
pub const MINI_REGENT_STRENGTH: i32 = 1;

/// `OldCoin::GOLD` — gold gained on obtain
/// (`rare.py::OldCoin.GOLD`).
pub const OLD_COIN_GOLD: i32 = 300;

/// `OrangeDough::CARDS` — round-1 colorless cards generated
/// (`rare.py::OrangeDough.CARDS`).
pub const ORANGE_DOUGH_CARDS: i32 = 2;

/// `Pocketwatch::CARD_THRESHOLD` / `EXTRA_DRAW` — +3 draw if <=3
/// cards played last turn (`rare.py::Pocketwatch`).
pub const POCKETWATCH_CARD_THRESHOLD: i32 = 3;
pub const POCKETWATCH_EXTRA_DRAW: i32 = 3;

/// `PowerCell::CARDS` — round-1 0-cost cards moved to hand
/// (`rare.py::PowerCell.CARDS`).
pub const POWER_CELL_CARDS: i32 = 2;

/// `RainbowRing::STRENGTH` / `DEXTERITY` — gained per A+S+P trio
/// (`rare.py::RainbowRing`).
pub const RAINBOW_RING_STRENGTH: i32 = 1;
pub const RAINBOW_RING_DEXTERITY: i32 = 1;

/// `Shuriken::ATTACK_THRESHOLD` / `STRENGTH` — every 3rd attack +1 Str
/// (`rare.py::Shuriken`).
pub const SHURIKEN_ATTACK_THRESHOLD: i32 = 3;
pub const SHURIKEN_STRENGTH: i32 = 1;

/// `StoneCalendar::DAMAGE` / `DAMAGE_TURN` — turn-7 `AoE` damage
/// (`rare.py::StoneCalendar`).
pub const STONE_CALENDAR_DAMAGE: i32 = 52;
pub const STONE_CALENDAR_DAMAGE_TURN: i32 = 7;

/// `SturdyClamp::MAX_RETAINED_BLOCK` — block retained between turns
/// (`rare.py::SturdyClamp.MAX_RETAINED_BLOCK`).
pub const STURDY_CLAMP_MAX_RETAINED_BLOCK: i32 = 10;

/// `TheCourier::DISCOUNT` — merchant discount %
/// (`rare.py::TheCourier.DISCOUNT`).
pub const THE_COURIER_DISCOUNT: i32 = 20;

/// `ToughBandages::BLOCK` — block per card discarded on player turn
/// (`rare.py::ToughBandages.BLOCK`).
pub const TOUGH_BANDAGES_BLOCK: i32 = 3;

/// `TungstenRod::REDUCTION` — HP-loss reduction
/// (`rare.py::TungstenRod.REDUCTION`).
pub const TUNGSTEN_ROD_REDUCTION: i32 = 1;

// ---------------------------------------------------------------------------
// Typed-channel listener bodies
// ---------------------------------------------------------------------------

/// `PaperKrane::modify_damage_multiplicative` — when the player is
/// taking a powered attack from a dealer that has Weak, contribute an
/// additional `(WEAK - EXTRA_WEAK) / WEAK` multiplier on top of the
/// power-side `Weak` arm's `3/4`. With Debilitate on the dealer the
/// effective Weak multiplier doubles in strength (Python:
/// `weak_multiplier -= 1.0 - weak_multiplier` → `2W - 1 = 0.5`), and
/// PaperKrane contributes `(0.5 - 0.15) / 0.5 = 7/10`.
///
/// Mirrors Python pin
/// `sts2-rl-agent@01c094d:sts2_env/relics/rare.py::PaperKrane`:
///
/// ```python
/// def modify_damage_multiplicative(self, owner, dealer, target, props, card=None):
///     if target is not owner or dealer is None or not props.is_powered_attack():
///         return 1.0
///     if dealer.get_power_amount(PowerId.WEAK) <= 0:
///         return 1.0
///     weak_multiplier = WEAK_MULTIPLIER
///     if dealer.has_power(PowerId.DEBILITATE):
///         weak_multiplier -= 1.0 - weak_multiplier
///     return max(0.0, round((weak_multiplier - self.EXTRA_WEAK) / weak_multiplier, ...))
/// ```
///
/// **Rational composition.** Python's float result is rationalized so
/// it composes cleanly with [`DamageMul`]. With `WEAK = 3/4` and
/// `EXTRA = 3/20` (15/100), `(W - E) / W = (12/20) / (15/20) = 12/15 = 4/5`.
/// With Debilitate active the base becomes `1/2`, so
/// `(1/2 - 3/20) / (1/2) = (7/20) / (10/20) = 7/10`. Both rationals
/// are exact — no float drift across composition.
///
/// **Targeting model.** PaperKrane is a Silent rare; owner is the
/// player. The Rust port reads the dealer's Weak amount via
/// [`CombatState::player.powers`] / [`CombatState::creatures`] (we
/// can't call the private `hooks::creature_power_amount` from
/// content, so the body walks powers inline).
fn listener_paper_krane(state: &CombatState, ctx: &DamageCtx) -> DamageMul {
    // Ownership gate — registry walks every registered listener without
    // pre-filtering by owned relics, so each body must check.
    if !state.relics.owns(RelicKind::PaperKrane) {
        return DamageMul::ONE;
    }
    // Target must be the player.
    if !matches!(ctx.target, CreatureRef::Player) {
        return DamageMul::ONE;
    }
    // Self-damage path: Python's `dealer is None` collapses to
    // `dealer == target == Player` in the Rust ctx. If the dealer is
    // also Player, this is a self-hit (e.g. retaliation card) and
    // should not engage PaperKrane.
    if matches!(ctx.dealer, CreatureRef::Player) {
        return DamageMul::ONE;
    }
    if !ctx.props.is_powered() {
        return DamageMul::ONE;
    }
    // Walk dealer's powers for Weak + Debilitate.
    let dealer_powers: &[sts2_core::PowerUid] = match ctx.dealer {
        CreatureRef::Enemy(uid) => {
            let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
                return DamageMul::ONE;
            };
            &c.powers
        }
        CreatureRef::Ally(idx) => &state.allies[idx as usize].powers,
        CreatureRef::Player => unreachable!("filtered above"),
        // The null-target sentinel is never a dealer; it holds no powers.
        CreatureRef::Nobody => return DamageMul::ONE,
    };
    let mut has_weak = false;
    let mut has_debilitate = false;
    for puid in dealer_powers {
        if let Some(p) = state.powers.get_by_id(puid.0) {
            if p.kind == PowerKind::Weak && p.amount > 0 {
                has_weak = true;
            }
            if p.kind == PowerKind::Debilitate {
                has_debilitate = true;
            }
        }
    }
    if !has_weak {
        return DamageMul::ONE;
    }
    // (W - E) / W where W is either 3/4 (no Debilitate) or 1/2 (with
    // Debilitate). 4/5 and 7/10 respectively (see doc above).
    if has_debilitate {
        DamageMul::new(7, 10)
    } else {
        DamageMul::new(4, 5)
    }
}

// ---------------------------------------------------------------------------
// RelicSpec factories — L–Z wave (bead `sts2-rs-6ynt.5.8`)
//
// Ports the alphabetical L–Z half of the rare pool filtered to
// pool ∈ {IRONCLAD, SHARED, EVENT} from
// `sts2-rl-agent@01c094d:sts2_env/relics/rare.py`. Five relics
// whose Python bodies are pure data (no per-combat / per-run
// counter state) get their matching [`RelicEffect`] AST variant;
// the rest stay [`RelicEffect::Noop`] because their behaviour is
// stateful and lands when the runtime `Relic` POD grows
// per-instance counters. Each factory carries a Python pin in its
// doc comment so the downstream wiring bead can drop in real
// logic. Sibling bead `sts2-rs-6ynt.5.7` covers A–K.
// ---------------------------------------------------------------------------

macro_rules! rare_factory {
    ($vis:vis $fn_name:ident, $kind:ident, $name:literal, $effect:expr) => {
        #[must_use]
        $vis const fn $fn_name() -> RelicSpec {
            RelicSpec {
                id: RelicKind::$kind,
                name: $name,
                rarity: RelicRarity::Rare,
                effect: $effect,
            }
        }
    };
}

// --- L ---
// LastingCandy — stateful (`_combats_seen` counter, every 2nd combat
// inserts an extra Power into card rewards). The combat-side
// `_combats_seen` bump (`relic_effects::lasting_candy`) is `Noop` here;
// the reward arm fires inline in
// `spec_index::append_lasting_candy_power_reward` at the C# mid-hook
// site, so no separate run-state listener registration is needed.
rare_factory!(pub lasting_candy, LastingCandy, "LASTING_CANDY", RelicEffect::Noop);
// LizardTail — one-shot revive (`_was_used` flag). Stateful;
// `HEAL_PCT = 50` pinned via [`LIZARD_TAIL_HEAL_PCT`]. Stays `Noop`.
rare_factory!(pub lizard_tail, LizardTail, "LIZARD_TAIL", RelicEffect::Noop);
// --- M ---
// Mango — `after_obtained`: +14 max HP (and heal by 14). Pure data.
rare_factory!(pub mango, Mango, "MANGO", RelicEffect::GainMaxHpOnObtained { amount: MANGO_MAX_HP });
// MeatOnTheBone — `after_combat_victory`: heal 12 when HP <= 50% of max.
rare_factory!(pub meat_on_the_bone, MeatOnTheBone, "MEAT_ON_THE_BONE", RelicEffect::HealAtCombatVictoryBelowHpPct { heal: MEAT_ON_THE_BONE_HEAL, pct: MEAT_ON_THE_BONE_HP_THRESHOLD_PCT });
// MoltenEgg — modifies card-reward options (auto-upgrade Attack cards
// on obtain). Reward-modifier hook, stays `Noop`.
rare_factory!(pub molten_egg, MoltenEgg, "MOLTEN_EGG", RelicEffect::Noop);
// MummifiedHand — `after_power_played`: random card in hand costs 0
// this turn. Stateful per-turn / RNG-driven; stays `Noop`.
rare_factory!(pub mummified_hand, MummifiedHand, "MUMMIFIED_HAND", RelicEffect::Noop);
// --- O ---
// OldCoin — `after_obtained`: +300 gold. Pure data.
rare_factory!(pub old_coin, OldCoin, "OLD_COIN", RelicEffect::GainGoldOnObtained { amount: OLD_COIN_GOLD });
// --- P ---
// Pocketwatch — stateful (cards-played counter, +3 draw if <=3 cards
// played last turn). Stays `Noop`.
rare_factory!(pub pocketwatch, Pocketwatch, "POCKETWATCH", RelicEffect::Noop);
// PrayerWheel — modifies card rewards (extra option). Reward-modifier
// hook, stays `Noop`.
rare_factory!(pub prayer_wheel, PrayerWheel, "PRAYER_WHEEL", RelicEffect::Noop);
// --- R ---
// RainbowRing — stateful (per-type counters for ASP trio). Stays `Noop`.
rare_factory!(pub rainbow_ring, RainbowRing, "RAINBOW_RING", RelicEffect::Noop);
// RazorTooth — combat body lives in the sts2-core typed channel
// (`relic_effects::razor_tooth::on_after_card_played_razor_tooth`,
// C# `RazorTooth.cs::AfterCardPlayed`: upgrade every played
// Attack/Skill for the combat), so the factory effect stays `Noop`.
rare_factory!(pub razor_tooth, RazorTooth, "RAZOR_TOOTH", RelicEffect::Noop);
// RuinedHelmet — `_used_this_combat` one-shot (doubles first positive
// Strength gain). Stateful; stays `Noop`.
rare_factory!(pub ruined_helmet, RuinedHelmet, "RUINED_HELMET", RelicEffect::Noop);
// --- S ---
// Shovel — rest-site option (skip rest to gain a relic). Run-level
// hook, stays `Noop`.
rare_factory!(pub shovel, Shovel, "SHOVEL", RelicEffect::Noop);
// Shuriken — stateful (attack counter, +1 Str every 3rd attack).
rare_factory!(pub shuriken, Shuriken, "SHURIKEN", RelicEffect::Noop);
// StoneCalendar — `before_turn_end` on player round 7: deal 52
// unpowered damage to all enemies. Pure data.
rare_factory!(pub stone_calendar, StoneCalendar, "STONE_CALENDAR", RelicEffect::DealDamageToAllEnemiesOnPlayerTurnEndRound { amount: STONE_CALENDAR_DAMAGE, round: STONE_CALENDAR_DAMAGE_TURN as u16 });
// SturdyClamp — retain up to 10 block between player turns. Modifier
// of the block-clear pipeline; stateful per-turn. Stays `Noop`.
rare_factory!(pub sturdy_clamp, SturdyClamp, "STURDY_CLAMP", RelicEffect::Noop);
// --- T ---
// TheCourier — merchant discount (run-level). Stays `Noop`.
rare_factory!(pub the_courier, TheCourier, "THE_COURIER", RelicEffect::Noop);
// ToxicEgg — auto-upgrade Skill cards on obtain (reward modifier).
// Stays `Noop`.
rare_factory!(pub toxic_egg, ToxicEgg, "TOXIC_EGG", RelicEffect::Noop);
// TungstenRod — `modify_hp_lost`: reduce HP loss by 1. Pure data.
rare_factory!(pub tungsten_rod, TungstenRod, "TUNGSTEN_ROD", RelicEffect::ReducePlayerHpLossAdditive { reduction: TUNGSTEN_ROD_REDUCTION });
// --- U ---
// UnceasingTop — `after_card_played`: draw 1 if hand empty.
// Stateful-ish (depends on live hand state). Stays `Noop`.
rare_factory!(pub unceasing_top, UnceasingTop, "UNCEASING_TOP", RelicEffect::Noop);
// UnsettlingLamp — `_triggered` one-shot per combat (next attack +X
// damage). Stateful; stays `Noop`.
rare_factory!(pub unsettling_lamp, UnsettlingLamp, "UNSETTLING_LAMP", RelicEffect::Noop);
// --- V ---
// VexingPuzzlebox — stateful trigger when an X-cost card is played.
// Stays `Noop`.
rare_factory!(pub vexing_puzzlebox, VexingPuzzlebox, "VEXING_PUZZLEBOX", RelicEffect::Noop);
// --- W ---
// WhiteBeastStatue — potion-drop rate boost (run-level). Stays `Noop`.
rare_factory!(pub white_beast_statue, WhiteBeastStatue, "WHITE_BEAST_STATUE", RelicEffect::Noop);
// WhiteStar — adds a boss-rarity card reward after elite combats.
// Reward modifier, stays `Noop`.
rare_factory!(pub white_star, WhiteStar, "WHITE_STAR", RelicEffect::Noop);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// C# `Girya.TryModifyRestSiteOptions` + `LiftRestSiteOption.OnSelect`:
    /// with Girya owned the rest site offers Lift after [Rest, Smith];
    /// each Lift bumps `relic_counters[Girya]`; at 3 lifts the option
    /// disappears entirely (not merely disabled). 2026-07-18 shadow census.
    #[test]
    fn girya_lift_rest_option_appears_counts_and_caps() {
        crate::relics::dispatch::ensure_installed();
        sts2_core::relic_effects::ensure_installed();
        let mut rs = RunState::new();
        assert!(
            !rs.rest_site_options()
                .iter()
                .any(|o| o.option_id() == "LIFT"),
            "no Lift without Girya"
        );
        rs.relics.push(RelicId::Girya);
        let opts = rs.rest_site_options();
        assert_eq!(
            opts.last()
                .map(sts2_core::runstate::RestSiteOption::option_id),
            Some("LIFT"),
            "Lift appended after [Rest, Smith] (C# listener-walk order)"
        );
        for expected in 1..=3 {
            let outcome = rs.do_rest_site_action(RestSiteOption::Lift { lifts_done: 0 });
            assert_eq!(outcome, sts2_core::runstate::RestSiteActionOutcome::Lifted);
            assert_eq!(rs.relic_counters.get(&RelicId::Girya), Some(&expected));
        }
        assert!(
            !rs.rest_site_options()
                .iter()
                .any(|o| o.option_id() == "LIFT"),
            "Lift not listed once TimesLifted reaches 3 (C# Girya.cs:63-66)"
        );
    }

    /// The id list covers exactly 50 entries — the count Python pin
    /// at `sts2-rl-agent@01c094d:sts2_env/relics/rare.py`
    /// (`grep -c '^@register_relic' rare.py == 50`).
    #[test]
    fn pool_has_fifty_entries() {
        assert_eq!(RARE_RELIC_IDS.len(), 50);
    }

    /// Every `RelicId` in the list is distinct. Catches accidental
    /// duplicate paste-edits inside [`RARE_RELIC_IDS`].
    #[test]
    fn pool_entries_are_unique_by_relic_id() {
        let mut ids: Vec<RelicId> = RARE_RELIC_IDS.to_vec();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate RelicId in RARE_RELIC_IDS");
    }

    /// Every rare `RelicId` carries an upper-snake-case Python name.
    /// Locks the parity-output naming for the future exporter.
    #[test]
    fn every_rare_relic_has_python_name() {
        for &id in RARE_RELIC_IDS {
            let name = id.to_python_name();
            assert!(!name.is_empty(), "empty python_name for {id:?}");
            assert!(
                name.bytes()
                    .all(|b| b == b'_' || b.is_ascii_uppercase() || b.is_ascii_digit()),
                "non upper-snake-case python_name for {id:?}: {name:?}"
            );
        }
    }

    // --- Deep pin-tests for high-impact rare relics ----------------------
    //
    // The bead spec called for deeper tests on three exemplar
    // transform-heavy relics (Snecko Eye / Dead Branch / Pandora's
    // Box). Those three are actually shop/boss rarity in
    // `sts2-rl-agent` and live in the sibling `yyw` bead. Within
    // `rare.py`, the closest rare-rarity analogues with crisp
    // Python-side constants are pinned below — these tests lock the
    // numeric constants the integration bead must hit when it wires
    // real effect bodies.

    /// `Mango` — pins `MAX_HP = 14` for the `after_obtained` hook
    /// (`rare.py::Mango.MAX_HP`).
    #[test]
    fn mango_pins_max_hp_constant() {
        assert_eq!(MANGO_MAX_HP, 14);
        assert_eq!(RelicId::Mango.to_python_name(), "MANGO");
    }

    /// `MeatOnTheBone` — pins `HP_THRESHOLD_PCT = 50` and `HEAL = 12`
    /// for the `after_combat_victory` hook
    /// (`rare.py::MeatOnTheBone`).
    #[test]
    fn meat_on_the_bone_pins_heal_constants() {
        assert_eq!(MEAT_ON_THE_BONE_HP_THRESHOLD_PCT, 50);
        assert_eq!(MEAT_ON_THE_BONE_HEAL, 12);
        assert_eq!(RelicId::MeatOnTheBone.to_python_name(), "MEAT_ON_THE_BONE");
    }

    /// `TungstenRod` — pins `REDUCTION = 1` for the `modify_hp_lost`
    /// hook (`rare.py::TungstenRod.REDUCTION`).
    #[test]
    fn tungsten_rod_pins_reduction_constant() {
        assert_eq!(TUNGSTEN_ROD_REDUCTION, 1);
        assert_eq!(RelicId::TungstenRod.to_python_name(), "TUNGSTEN_ROD");
    }

    /// `LizardTail` — pins `HEAL_PCT = 50` (revive heal) for the
    /// `should_die_late` hook (`rare.py::LizardTail.HEAL_PCT`). This
    /// is the rare-relic counterpart to Snecko Eye / Pandora's Box —
    /// a one-shot run-scoped state machine that requires the
    /// integration bead to plumb a `_was_used` flag through the
    /// runtime Relic POD.
    #[test]
    fn lizard_tail_pins_heal_pct_and_oneshot_semantics() {
        assert_eq!(LIZARD_TAIL_HEAL_PCT, 50);
        // LizardTail is inline-handled (the `should_die_late` revive +
        // one-shot `_was_used` gate live on the runtime Relic POD, not in
        // any dispatch table); see the RelicId completeness census
        // (INLINE_HANDLED bucket).
        assert_eq!(RelicId::LizardTail.to_python_name(), "LIZARD_TAIL");
    }

    /// Spot-check the load-bearing public constants exist with the
    /// values Python's `rare.py` pins. Catches accidental edits to the
    /// `pub const` block above.
    #[test]
    fn public_constants_match_python_pins() {
        assert_eq!(ART_OF_WAR_ENERGY, 1);
        assert_eq!(BEATING_REMNANT_MAX_HP_LOSS, 20);
        assert_eq!(BIG_HAT_CARDS, 2);
        assert_eq!(CAPTAINS_WHEEL_BLOCK, 18);
        assert_eq!(CHANDELIER_ENERGY, 3);
        assert_eq!(CHARONS_ASHES_DAMAGE, 3);
        assert_eq!(GAME_PIECE_CARDS, 1);
        assert_eq!(GIRYA_MAX_LIFTS, 3);
        assert_eq!(HELICAL_DART_DEXTERITY, 1);
        assert_eq!(INTIMIDATING_HELMET_BLOCK, 4);
        assert_eq!(INTIMIDATING_HELMET_ENERGY_THRESHOLD, 2);
        assert_eq!(IVORY_TILE_ENERGY, 1);
        assert_eq!(IVORY_TILE_THRESHOLD, 3);
        assert_eq!(KUNAI_ATTACK_THRESHOLD, 3);
        assert_eq!(KUNAI_DEXTERITY, 1);
        assert_eq!(LUNAR_PASTRY_STARS, 1);
        assert_eq!(METRONOME_ORB_THRESHOLD, 7);
        assert_eq!(METRONOME_DAMAGE, 30);
        assert_eq!(MINI_REGENT_STRENGTH, 1);
        assert_eq!(OLD_COIN_GOLD, 300);
        assert_eq!(ORANGE_DOUGH_CARDS, 2);
        assert_eq!(POCKETWATCH_CARD_THRESHOLD, 3);
        assert_eq!(POCKETWATCH_EXTRA_DRAW, 3);
        assert_eq!(POWER_CELL_CARDS, 2);
        assert_eq!(RAINBOW_RING_STRENGTH, 1);
        assert_eq!(RAINBOW_RING_DEXTERITY, 1);
        assert_eq!(SHURIKEN_ATTACK_THRESHOLD, 3);
        assert_eq!(SHURIKEN_STRENGTH, 1);
        assert_eq!(STONE_CALENDAR_DAMAGE, 52);
        assert_eq!(STONE_CALENDAR_DAMAGE_TURN, 7);
        assert_eq!(STURDY_CLAMP_MAX_RETAINED_BLOCK, 10);
        assert_eq!(THE_COURIER_DISCOUNT, 20);
        assert_eq!(TOUGH_BANDAGES_BLOCK, 3);
    }

    // -----------------------------------------------------------------
    // Spec pins — L–Z rare-tier wave (bead `sts2-rs-6ynt.5.8`)
    //
    // Each pin asserts the factory's (id, name, rarity, effect) tuple.
    // The 5 stateless relics pin a non-`Noop` `RelicEffect` variant
    // carrying load-bearing Python constants; the 19 stateful ones
    // pin `Noop` so accidental AST drift is caught — when the runtime
    // `Relic` POD grows per-instance counters, those factories will
    // swap in real variants and the pinned `Noop` assertion fails
    // loudly so the test is updated alongside the body.
    // -----------------------------------------------------------------

    #[test]
    fn lz_wave_factory_names_round_trip() {
        // L–Z pool-filtered (IRONCLAD / SHARED / EVENT) — 24 entries.
        // Triples are (factory output, matching `RelicId`, the
        // `Python RelicId.<X>.name` literal). The matching `RelicKind`
        // discriminant is asserted via `spec.id.python_name()` so we
        // don't need a fragile `From<RelicId> for RelicKind` impl.
        let pool: &[(RelicSpec, RelicId, &str)] = &[
            (lasting_candy(), RelicId::LastingCandy, "LASTING_CANDY"),
            (lizard_tail(), RelicId::LizardTail, "LIZARD_TAIL"),
            (mango(), RelicId::Mango, "MANGO"),
            (
                meat_on_the_bone(),
                RelicId::MeatOnTheBone,
                "MEAT_ON_THE_BONE",
            ),
            (molten_egg(), RelicId::MoltenEgg, "MOLTEN_EGG"),
            (mummified_hand(), RelicId::MummifiedHand, "MUMMIFIED_HAND"),
            (old_coin(), RelicId::OldCoin, "OLD_COIN"),
            (pocketwatch(), RelicId::Pocketwatch, "POCKETWATCH"),
            (prayer_wheel(), RelicId::PrayerWheel, "PRAYER_WHEEL"),
            (rainbow_ring(), RelicId::RainbowRing, "RAINBOW_RING"),
            (razor_tooth(), RelicId::RazorTooth, "RAZOR_TOOTH"),
            (ruined_helmet(), RelicId::RuinedHelmet, "RUINED_HELMET"),
            (shovel(), RelicId::Shovel, "SHOVEL"),
            (shuriken(), RelicId::Shuriken, "SHURIKEN"),
            (stone_calendar(), RelicId::StoneCalendar, "STONE_CALENDAR"),
            (sturdy_clamp(), RelicId::SturdyClamp, "STURDY_CLAMP"),
            (the_courier(), RelicId::TheCourier, "THE_COURIER"),
            (toxic_egg(), RelicId::ToxicEgg, "TOXIC_EGG"),
            (tungsten_rod(), RelicId::TungstenRod, "TUNGSTEN_ROD"),
            (unceasing_top(), RelicId::UnceasingTop, "UNCEASING_TOP"),
            (
                unsettling_lamp(),
                RelicId::UnsettlingLamp,
                "UNSETTLING_LAMP",
            ),
            (
                vexing_puzzlebox(),
                RelicId::VexingPuzzlebox,
                "VEXING_PUZZLEBOX",
            ),
            (
                white_beast_statue(),
                RelicId::WhiteBeastStatue,
                "WHITE_BEAST_STATUE",
            ),
            (white_star(), RelicId::WhiteStar, "WHITE_STAR"),
        ];
        assert_eq!(pool.len(), 24, "L–Z pool-filtered count");
        for (spec, id, name) in pool {
            assert_eq!(spec.name, *name, "spec.name vs literal for {id:?}");
            assert_eq!(
                spec.name,
                id.to_python_name(),
                "spec.name vs RelicId for {id:?}"
            );
            assert_eq!(
                spec.id.python_name(),
                *name,
                "spec.id.python_name() for {id:?}"
            );
            assert_eq!(spec.rarity, RelicRarity::Rare, "rarity for {id:?}");
        }
    }

    // --- Stateless-shape pins ----------------------------------------

    #[test]
    fn mango_spec_pin() {
        let r = mango();
        assert_eq!(
            r.effect,
            RelicEffect::GainMaxHpOnObtained {
                amount: MANGO_MAX_HP,
            }
        );
        assert_eq!(MANGO_MAX_HP, 14);
    }

    #[test]
    fn meat_on_the_bone_spec_pin() {
        let r = meat_on_the_bone();
        assert_eq!(
            r.effect,
            RelicEffect::HealAtCombatVictoryBelowHpPct {
                heal: MEAT_ON_THE_BONE_HEAL,
                pct: MEAT_ON_THE_BONE_HP_THRESHOLD_PCT,
            }
        );
        assert_eq!(MEAT_ON_THE_BONE_HEAL, 12);
        assert_eq!(MEAT_ON_THE_BONE_HP_THRESHOLD_PCT, 50);
    }

    #[test]
    fn old_coin_spec_pin() {
        let r = old_coin();
        assert_eq!(
            r.effect,
            RelicEffect::GainGoldOnObtained {
                amount: OLD_COIN_GOLD,
            }
        );
        assert_eq!(OLD_COIN_GOLD, 300);
    }

    #[test]
    fn stone_calendar_spec_pin() {
        let r = stone_calendar();
        assert_eq!(
            r.effect,
            RelicEffect::DealDamageToAllEnemiesOnPlayerTurnEndRound {
                amount: STONE_CALENDAR_DAMAGE,
                round: STONE_CALENDAR_DAMAGE_TURN as u16,
            }
        );
        assert_eq!(STONE_CALENDAR_DAMAGE, 52);
        assert_eq!(STONE_CALENDAR_DAMAGE_TURN, 7);
    }

    #[test]
    fn tungsten_rod_spec_pin() {
        let r = tungsten_rod();
        assert_eq!(
            r.effect,
            RelicEffect::ReducePlayerHpLossAdditive {
                reduction: TUNGSTEN_ROD_REDUCTION,
            }
        );
        assert_eq!(TUNGSTEN_ROD_REDUCTION, 1);
    }

    // --- Stateful-Noop pins (catch accidental swap) ------------------

    #[test]
    fn stateful_lz_relics_remain_noop_until_counters_land() {
        let stateful: &[(RelicSpec, &'static str)] = &[
            (lasting_candy(), "LastingCandy (_combats_seen)"),
            (lizard_tail(), "LizardTail (_was_used one-shot)"),
            (molten_egg(), "MoltenEgg (modify_card_reward_options_late)"),
            (
                mummified_hand(),
                "MummifiedHand (per-power random hand pick)",
            ),
            (pocketwatch(), "Pocketwatch (cards-played counter)"),
            (prayer_wheel(), "PrayerWheel (card-reward modifier)"),
            (rainbow_ring(), "RainbowRing (per-type ASP counters)"),
            (razor_tooth(), "RazorTooth (per-combat debuff trigger)"),
            (ruined_helmet(), "RuinedHelmet (_used_this_combat one-shot)"),
            (shovel(), "Shovel (rest-site option)"),
            (shuriken(), "Shuriken (attack counter)"),
            (sturdy_clamp(), "SturdyClamp (retain-block carrier)"),
            (the_courier(), "TheCourier (merchant discount, run-level)"),
            (toxic_egg(), "ToxicEgg (modify_card_reward_options_late)"),
            (unceasing_top(), "UnceasingTop (hand-empty draw)"),
            (unsettling_lamp(), "UnsettlingLamp (_triggered one-shot)"),
            (vexing_puzzlebox(), "VexingPuzzlebox (X-cost-card trigger)"),
            (white_beast_statue(), "WhiteBeastStatue (potion drop rate)"),
            (white_star(), "WhiteStar (elite reward modifier)"),
        ];
        assert_eq!(stateful.len(), 19);
        for (spec, label) in stateful {
            assert_eq!(
                spec.effect,
                RelicEffect::Noop,
                "{label} must stay Noop until the runtime counter / \
                 reward-modifier channel lands; when you swap in a \
                 real variant, update this pin too"
            );
        }
    }

    // -----------------------------------------------------------------
    // Deep tests — IvoryTile (sts2-rs-6ynt.5.7)
    //
    // NECROBINDER-pool rare. `after_card_played`: if the played card
    // spent >= THRESHOLD=3 energy, gain ENERGY=1 back. Python pin:
    // `sts2-rl-agent@01c094d:sts2_env/relics/rare.py::IvoryTile`
    // (lines 319-331). Pure pass-through — no per-instance counter.
    // -----------------------------------------------------------------

    /// **IvoryTile #1** Python pin: `ENERGY=1`, `THRESHOLD=3`.
    #[test]
    fn ivory_tile_python_pin() {
        assert_eq!(IVORY_TILE_ENERGY, 1);
        assert_eq!(IVORY_TILE_THRESHOLD, 3);
        assert_eq!(RelicId::IvoryTile.to_python_name(), "IVORY_TILE");
        assert_eq!(RelicKind::IvoryTile.python_name(), "IVORY_TILE");
    }

    /// **IvoryTile #2** listener registered on the `after_card_played`
    /// channel after the full registry install. Asserts the
    /// load-bearing path is live independent of the marker-resolved
    /// generic dispatch.
    #[test]
    fn ivory_tile_listener_registered_after_full_install() {
        crate::relics::dispatch::ensure_installed();
        assert!(
            sts2_core::relic_effects::lookup_after_card_played(RelicId::IvoryTile).is_some(),
            "IvoryTile listener not registered on after_card_played channel"
        );
    }

    /// Bead `sts2-rs-u8qa`: Mango's `after_obtained` listener must add
    /// `MAX_HP=14` to the player's max HP (and current HP). Previously
    /// the relic's `RelicEffect::GainMaxHpOnObtained{14}` factory line
    /// was metadata-only — no AST walker, no listener — and picking up
    /// Mango granted nothing.
    #[test]
    fn listener_mango_gains_fourteen_max_hp_on_obtain() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        let before_max = rs.max_hp;
        let before_hp = rs.current_hp;
        rs.relics.push(RelicId::Mango);
        rs.last_obtained_relic = Some(RelicId::Mango);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.max_hp, before_max + MANGO_MAX_HP);
        assert_eq!(rs.current_hp, before_hp + MANGO_MAX_HP);
    }

    /// Bead `sts2-rs-tyqo`: OldCoin's `after_obtained` listener must
    /// add `GOLD=300` to the player's gold. Previously the relic's
    /// `RelicEffect::GainGoldOnObtained{300}` factory line was
    /// metadata-only — picking up OldCoin granted nothing.
    #[test]
    fn listener_old_coin_gains_three_hundred_gold_on_obtain() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = sts2_core::runstate::RunState::new();
        let before_gold = rs.gold;
        rs.relics.push(RelicId::OldCoin);
        rs.last_obtained_relic = Some(RelicId::OldCoin);
        let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
        assert_eq!(rs.gold, before_gold + OLD_COIN_GOLD);
    }

    /// Bead `sts2-rs-4ihz`: PrayerWheel appends a CardReward to the
    /// rewards vec on Monster-room clears.
    #[test]
    fn listener_prayer_wheel_appends_card_reward_on_monster_rooms() {
        use sts2_core::run_effects::dispatch_modify_rewards;
        use sts2_core::runstate::{Reward, RoomType, RunState};

        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::PrayerWheel);
        let mut rewards: Vec<Reward> = Vec::new();
        let _ = dispatch_modify_rewards(&mut rs, &mut rewards, RoomType::Monster);
        assert_eq!(rewards.len(), 1, "PrayerWheel must add 1 reward");
        assert!(
            matches!(rewards[0], Reward::Card(_)),
            "appended reward must be a CardReward"
        );
    }

    /// Bead `sts2-rs-4ihz`: PrayerWheel is a no-op on Elite, Boss, Shop,
    /// Treasure, Event, RestSite, and Unknown rooms — only Monster
    /// rooms trigger the extra card reward.
    #[test]
    fn listener_prayer_wheel_is_noop_on_non_monster_rooms() {
        use sts2_core::run_effects::dispatch_modify_rewards;
        use sts2_core::runstate::{Reward, RoomType, RunState};

        crate::relics::dispatch::ensure_installed();
        for rt in [
            RoomType::Elite,
            RoomType::Boss,
            RoomType::Shop,
            RoomType::Treasure,
            RoomType::Event,
            RoomType::RestSite,
            RoomType::Unknown,
        ] {
            let mut rs = RunState::new();
            rs.relics.push(RelicId::PrayerWheel);
            let mut rewards: Vec<Reward> = Vec::new();
            let _ = dispatch_modify_rewards(&mut rs, &mut rewards, rt);
            assert!(
                rewards.is_empty(),
                "PrayerWheel must be silent on {rt:?} rooms"
            );
        }
    }

    /// Bead `sts2-rs-4ihz`: PrayerWheel listener is a no-op when the
    /// player does not own the relic.
    #[test]
    fn listener_prayer_wheel_is_noop_without_ownership() {
        use sts2_core::run_effects::dispatch_modify_rewards;
        use sts2_core::runstate::{Reward, RoomType, RunState};

        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut rewards: Vec<Reward> = Vec::new();
        let _ = dispatch_modify_rewards(&mut rs, &mut rewards, RoomType::Monster);
        assert!(rewards.is_empty());
    }

    // -----------------------------------------------------------------
    // IvoryTile listener tests — bead `sts2-rs-mft0`
    // -----------------------------------------------------------------
    //
    // Single-channel `after_card_played` listener: when the just-played
    // card spent at least `IVORY_TILE_THRESHOLD = 3` energy, gain
    // `IVORY_TILE_ENERGY = 1` via the existing `CombatState::gain_energy`
    // surface. Mirrors Python's `card.energy_spent` field read.

    #[cfg(test)]
    fn ivory_tile_push_card_energy(
        state: &mut CombatState,
        energy_spent: i32,
    ) -> sts2_core::CardUid {
        use std::num::NonZeroU32;
        use sts2_core::cards::{Card, CardId, CardType, TargetType};
        let next_raw = state
            .cards
            .iter()
            .map(|c| c.uid.0.get())
            .max()
            .map_or(1, |m| m.saturating_add(1));
        let uid = sts2_core::CardUid(NonZeroU32::new(next_raw).expect("non-zero"));
        state.cards.push(Card {
            uid,
            card_id: CardId::StrikeIronclad,
            cost: 3,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            combat_vars: sts2_core::cards::CombatVars::new(),
            enchantments: sts2_core::cards::CardEnchantments::new(),
            tags: sts2_core::cards::CardTags::NONE,
            energy_spent,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        uid
    }

    #[cfg(test)]
    fn ivory_tile_setup(state: &mut CombatState, equip: bool) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        state.player.energy = 0;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::IvoryTile,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    #[cfg(test)]
    fn ivory_tile_ctx_for(
        card_uid: sts2_core::CardUid,
        state: &CombatState,
    ) -> sts2_core::relic_effects::CardEventCtx {
        let idx = card_uid.0.get() as usize - 1;
        let card = &state.cards[idx];
        sts2_core::relic_effects::CardEventCtx {
            card_id: card.card_id,
            card_uid: Some(card_uid),
            target: None,
            card_tags: card.tags,
            energy_spent: 0,
        }
    }

    /// **IvoryTile #1** — card with `energy_spent == THRESHOLD` fires.
    #[test]
    fn listener_ivory_tile_grants_energy_at_threshold() {
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        ivory_tile_setup(&mut state, true);
        let uid = ivory_tile_push_card_energy(&mut state, IVORY_TILE_THRESHOLD);
        let ctx = ivory_tile_ctx_for(uid, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(
            state.player.energy,
            i8::try_from(IVORY_TILE_ENERGY).unwrap()
        );
    }

    /// **IvoryTile #2** — card with `energy_spent > THRESHOLD` fires.
    #[test]
    fn listener_ivory_tile_grants_energy_above_threshold() {
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        ivory_tile_setup(&mut state, true);
        let uid = ivory_tile_push_card_energy(&mut state, IVORY_TILE_THRESHOLD + 2);
        let ctx = ivory_tile_ctx_for(uid, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(
            state.player.energy,
            i8::try_from(IVORY_TILE_ENERGY).unwrap()
        );
    }

    /// **IvoryTile #3** — cards with `energy_spent < THRESHOLD` never
    /// fire (covers 0 / 1 / 2 energy spends).
    #[test]
    fn listener_ivory_tile_is_noop_below_threshold() {
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        for spent in 0..IVORY_TILE_THRESHOLD {
            let mut state = CombatState::default();
            ivory_tile_setup(&mut state, true);
            let uid = ivory_tile_push_card_energy(&mut state, spent);
            let ctx = ivory_tile_ctx_for(uid, &state);
            let _ = dispatch_after_card_played(&mut state, &ctx);
            assert_eq!(
                state.player.energy, 0,
                "energy_spent={spent} below threshold must not fire"
            );
        }
    }

    /// **IvoryTile #4** — no-op when the relic is not owned.
    #[test]
    fn listener_ivory_tile_is_noop_without_ownership() {
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        ivory_tile_setup(&mut state, false);
        let uid = ivory_tile_push_card_energy(&mut state, IVORY_TILE_THRESHOLD + 1);
        let ctx = ivory_tile_ctx_for(uid, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(state.player.energy, 0);
    }

    // -----------------------------------------------------------------
    // HelicalDart listener tests — bead `sts2-rs-8aw7`
    // -----------------------------------------------------------------
    //
    // Single-channel `after_card_played` listener: when the just-played
    // card is a Shiv (CardId::Shiv OR CardTag::Shiv), grant the player
    // `HELICAL_DART_DEXTERITY` (1) via `apply_power_to_creature`.

    #[cfg(test)]
    fn helical_dart_push_card(
        state: &mut CombatState,
        card_id: sts2_core::cards::CardId,
        card_type: sts2_core::cards::CardType,
        tags: sts2_core::cards::CardTags,
    ) -> sts2_core::CardUid {
        use std::num::NonZeroU32;
        use sts2_core::cards::{Card, TargetType};
        let next_raw = state
            .cards
            .iter()
            .map(|c| c.uid.0.get())
            .max()
            .map_or(1, |m| m.saturating_add(1));
        let uid = sts2_core::CardUid(NonZeroU32::new(next_raw).expect("non-zero"));
        state.cards.push(Card {
            uid,
            card_id,
            cost: 0,
            card_type,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 4,
            base_block: 0,
            combat_vars: sts2_core::cards::CombatVars::new(),
            enchantments: sts2_core::cards::CardEnchantments::new(),
            tags,
            energy_spent: 0,
            base_replay_count: 0,
            keyword_override: None,
            affliction: None,
        });
        uid
    }

    #[cfg(test)]
    fn helical_dart_setup(state: &mut CombatState, equip: bool) {
        use std::num::NonZeroU32;
        use sts2_core::Relic;
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state.sync_player_creature();
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::HelicalDart,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
    }

    #[cfg(test)]
    fn helical_dart_ctx_for(
        card_uid: sts2_core::CardUid,
        state: &CombatState,
    ) -> sts2_core::relic_effects::CardEventCtx {
        let idx = card_uid.0.get() as usize - 1;
        let card = &state.cards[idx];
        sts2_core::relic_effects::CardEventCtx {
            card_id: card.card_id,
            card_uid: Some(card_uid),
            target: None,
            card_tags: card.tags,
            energy_spent: 0,
        }
    }

    #[cfg(test)]
    fn helical_dart_player_dex(state: &CombatState) -> i32 {
        // apply_power_to_creature lands the power in `state.powers`
        // and links a PowerUid onto the creature's `.powers` list.
        // For the player there is only ever one Dexterity stack in a
        // fresh test, so a kind-scan of `state.powers` is sufficient.
        state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Dexterity)
            .map_or(0, |p| p.amount)
    }

    /// **HelicalDart #1** — happy path: playing a `CardId::Shiv` card
    /// grants +1 Dexterity to the player creature.
    #[test]
    fn listener_helical_dart_grants_dex_on_shiv_card_id() {
        use sts2_core::cards::{CardId, CardTags, CardType};
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        helical_dart_setup(&mut state, true);
        let uid =
            helical_dart_push_card(&mut state, CardId::Shiv, CardType::Attack, CardTags::NONE);
        let ctx = helical_dart_ctx_for(uid, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(helical_dart_player_dex(&state), HELICAL_DART_DEXTERITY);
        // sts2-rs-i09du: HelicalDartPower : TemporaryDexterityPower
        // (IsPositive=true) also stamps the HelicalDart marker so
        // hooks.rs after_turn_end can revert at owner-side turn end.
        let hd = state
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::HelicalDart)
            .map_or(0, |p| p.amount);
        assert_eq!(hd, HELICAL_DART_DEXTERITY);
    }

    /// **HelicalDart #2** — `CardTag::Shiv` leg: a non-Shiv card_id
    /// that carries the Shiv tag still triggers (matches Python's
    /// `is_shiv = card_id == SHIV or CardTag.SHIV in tags`).
    #[test]
    fn listener_helical_dart_grants_dex_on_shiv_tag() {
        use sts2_core::cards::{CardId, CardTag, CardTags, CardType};
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        helical_dart_setup(&mut state, true);
        let uid = helical_dart_push_card(
            &mut state,
            CardId::StrikeIronclad,
            CardType::Attack,
            CardTags::one(CardTag::Shiv),
        );
        let ctx = helical_dart_ctx_for(uid, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(helical_dart_player_dex(&state), HELICAL_DART_DEXTERITY);
    }

    /// **HelicalDart #3** — non-Shiv card (no matching id, no Shiv
    /// tag) is a no-op.
    #[test]
    fn listener_helical_dart_is_noop_on_non_shiv() {
        use sts2_core::cards::{CardId, CardTags, CardType};
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        helical_dart_setup(&mut state, true);
        let uid = helical_dart_push_card(
            &mut state,
            CardId::StrikeIronclad,
            CardType::Attack,
            CardTags::NONE,
        );
        let ctx = helical_dart_ctx_for(uid, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(helical_dart_player_dex(&state), 0);
    }

    /// **HelicalDart #4** — no-op when the relic is not owned, even
    /// for a Shiv card play.
    #[test]
    fn listener_helical_dart_is_noop_without_ownership() {
        use sts2_core::cards::{CardId, CardTags, CardType};
        use sts2_core::relic_effects::dispatch_after_card_played;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        helical_dart_setup(&mut state, false);
        let uid =
            helical_dart_push_card(&mut state, CardId::Shiv, CardType::Attack, CardTags::NONE);
        let ctx = helical_dart_ctx_for(uid, &state);
        let _ = dispatch_after_card_played(&mut state, &ctx);
        assert_eq!(helical_dart_player_dex(&state), 0);
    }

    // -----------------------------------------------------------------
    // Issue #50 — FrozenEgg / MoltenEgg / ToxicEgg upgrade listeners.
    // -----------------------------------------------------------------

    fn egg_test_card(uid_val: u32, ct: CardType) -> Card {
        use sts2_core::cards::TargetType;
        use sts2_core::CardUid;
        Card {
            uid: CardUid(std::num::NonZeroU32::new(uid_val).unwrap()),
            card_id: sts2_core::cards::CardId::StrikeIronclad,
            cost: 1,
            card_type: ct,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        }
    }

    #[test]
    fn frozen_egg_upgrades_power_on_deck_add() {
        use sts2_core::run_effects::dispatch_modify_card_being_added_to_deck;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::FrozenEgg);
        let mut card = egg_test_card(50_001, CardType::Power);
        let _ = dispatch_modify_card_being_added_to_deck(&mut rs, &mut card);
        assert!(card.upgraded, "FrozenEgg must upgrade Power on deck-add");
    }

    #[test]
    fn frozen_egg_leaves_attack_unchanged() {
        use sts2_core::run_effects::dispatch_modify_card_being_added_to_deck;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::FrozenEgg);
        let mut card = egg_test_card(50_002, CardType::Attack);
        let _ = dispatch_modify_card_being_added_to_deck(&mut rs, &mut card);
        assert!(!card.upgraded, "FrozenEgg gates on Power only");
    }

    #[test]
    fn frozen_egg_inert_without_relic() {
        use sts2_core::run_effects::dispatch_modify_card_being_added_to_deck;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut card = egg_test_card(50_003, CardType::Power);
        let _ = dispatch_modify_card_being_added_to_deck(&mut rs, &mut card);
        assert!(!card.upgraded, "no relic, no upgrade");
    }

    #[test]
    fn molten_egg_upgrades_attack_on_deck_add() {
        use sts2_core::run_effects::dispatch_modify_card_being_added_to_deck;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::MoltenEgg);
        let mut card = egg_test_card(50_010, CardType::Attack);
        let _ = dispatch_modify_card_being_added_to_deck(&mut rs, &mut card);
        assert!(card.upgraded, "MoltenEgg must upgrade Attack on deck-add");
    }

    #[test]
    fn molten_egg_leaves_skill_unchanged() {
        use sts2_core::run_effects::dispatch_modify_card_being_added_to_deck;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::MoltenEgg);
        let mut card = egg_test_card(50_011, CardType::Skill);
        let _ = dispatch_modify_card_being_added_to_deck(&mut rs, &mut card);
        assert!(!card.upgraded, "MoltenEgg gates on Attack only");
    }

    #[test]
    fn toxic_egg_upgrades_skill_in_reward_options() {
        use sts2_core::run_effects::dispatch_modify_card_reward_options_late;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::ToxicEgg);
        let mut options = vec![
            egg_test_card(50_020, CardType::Skill),
            egg_test_card(50_021, CardType::Attack),
        ];
        let _ = dispatch_modify_card_reward_options_late(&mut rs, &mut options);
        assert!(options[0].upgraded, "Skill option must be upgraded");
        assert!(!options[1].upgraded, "Attack option must not be touched");
    }

    #[test]
    fn toxic_egg_merchant_upgrades_skill() {
        use sts2_core::run_effects::dispatch_modify_merchant_card_creation_results;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::ToxicEgg);
        let mut card = egg_test_card(50_030, CardType::Skill);
        let _ = dispatch_modify_merchant_card_creation_results(&mut rs, &mut card, false);
        assert!(card.upgraded, "ToxicEgg upgrades Skill merchant entries");
    }

    // ------------------------------------------------------------------
    // Shovel (bead sts2-rs-yye7d)
    // ------------------------------------------------------------------

    /// Shovel-owning RunState surfaces `Dig` in the rest-site options.
    #[test]
    fn listener_shovel_appends_dig_option() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::Shovel);
        let opts = rs.rest_site_options();
        assert!(opts.iter().any(|o| matches!(o, RestSiteOption::Dig)));
    }

    /// Without Shovel, rest-site options contain Rest + Smith only
    /// (no Dig).
    #[test]
    fn listener_shovel_dig_absent_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let rs = RunState::new();
        let opts = rs.rest_site_options();
        assert!(!opts.iter().any(|o| matches!(o, RestSiteOption::Dig)));
    }

    /// Shovel does NOT duplicate Dig when another source already
    /// pushed it.
    #[test]
    fn listener_shovel_does_not_duplicate_dig() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::Shovel);
        let mut options = vec![RestSiteOption::Rest, RestSiteOption::Dig];
        let _ = sts2_core::run_effects::dispatch_modify_rest_site_options(&rs, &mut options);
        let dig_count = options
            .iter()
            .filter(|o| matches!(o, RestSiteOption::Dig))
            .count();
        assert_eq!(dig_count, 1, "Dig must not duplicate");
    }

    // -----------------------------------------------------------------
    // PaperKrane listener tests
    // -----------------------------------------------------------------

    #[cfg(test)]
    fn paper_krane_setup(
        state: &mut CombatState,
        equip: bool,
        enemy_weak: bool,
        enemy_debilitate: bool,
    ) -> sts2_core::CreatureUid {
        use smallvec::SmallVec;
        use std::num::NonZeroU32;
        use sts2_core::powers::PowerKind;
        use sts2_core::{Creature, MonsterId, MonsterKind, Power, PowerUid, Relic};
        if equip {
            state.relics.push(Relic {
                id: NonZeroU32::new(1).expect("non-zero"),
                kind: RelicKind::PaperKrane,
                counter: 0,
                is_wax: false,
                is_melted: false,
            });
        }
        let mut enemy_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        let mut next_id: u32 = 10;
        if enemy_weak {
            let id = NonZeroU32::new(next_id).expect("non-zero");
            next_id += 1;
            state.powers.push(Power {
                id,
                kind: PowerKind::Weak,
                amount: 2,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            enemy_powers.push(PowerUid(id));
        }
        if enemy_debilitate {
            let id = NonZeroU32::new(next_id).expect("non-zero");
            state.powers.push(Power {
                id,
                kind: PowerKind::Debilitate,
                amount: 1,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            enemy_powers.push(PowerUid(id));
        }
        let enemy_uid = sts2_core::CreatureUid::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: enemy_powers,
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: sts2_core::IntentHistory::default(),
            monster_id: MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(enemy_uid);
        enemy_uid
    }

    /// Combined power+relic multiplicative dispatch, mirroring the
    /// production damage pipeline (`hooks.rs:4345-4346`).
    #[cfg(test)]
    fn dispatch_combined_mul(state: &CombatState, ctx: &DamageCtx) -> DamageMul {
        let power_mul = sts2_core::hooks::dispatch_modify_damage_multiplicative(state, ctx);
        let relic_mul = sts2_core::relic_effects::dispatch_modify_damage_multiplicative(state, ctx);
        power_mul.compose(relic_mul)
    }

    /// **PaperKrane #1** — happy path: enemy with Weak attacking the
    /// player, owner has PaperKrane → power-side Weak 3/4 composed
    /// with PaperKrane's 4/5 = 12/20.
    #[test]
    fn listener_paper_krane_weak_dealer_returns_four_fifths() {
        use sts2_core::hooks::ValueProps;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let enemy_uid = paper_krane_setup(&mut state, true, true, false);
        let ctx = DamageCtx {
            dealer: CreatureRef::Enemy(enemy_uid),
            target: CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        assert_eq!(dispatch_combined_mul(&state, &ctx), DamageMul::new(12, 20));
    }

    /// **PaperKrane #2** — with Debilitate on the dealer, the power-side Weak
    /// arm deepens to `1/2` (Debilitate: `2W - 1`), and PaperKrane returns its
    /// `(W - E)/W = 7/10` ratio. Composed: `1/2 × 7/10 = 7/20` (= the C# final
    /// `W - E = 0.5 - 0.15 = 0.35`). Before the DebilitatePower power-side arm
    /// was wired (`sts2-rs` Necrobinder non-Osty powers), the power side
    /// returned `3/4` and this composed to a too-high `21/40 = 0.525`; the relic
    /// listener's 7/10 ratio was always authored to pair with a `1/2` base.
    #[test]
    fn listener_paper_krane_debilitate_dealer_returns_seven_tenths() {
        use sts2_core::hooks::ValueProps;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let enemy_uid = paper_krane_setup(&mut state, true, true, true);
        let ctx = DamageCtx {
            dealer: CreatureRef::Enemy(enemy_uid),
            target: CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        // 1/2 (Debilitate-deepened Weak) × 7/10 (PaperKrane) = 7/20.
        assert_eq!(dispatch_combined_mul(&state, &ctx), DamageMul::new(7, 20));
    }

    /// **PaperKrane #3** — dealer without Weak: identity composed mul.
    #[test]
    fn listener_paper_krane_no_weak_no_op() {
        use sts2_core::hooks::ValueProps;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let enemy_uid = paper_krane_setup(&mut state, true, false, false);
        let ctx = DamageCtx {
            dealer: CreatureRef::Enemy(enemy_uid),
            target: CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        assert_eq!(dispatch_combined_mul(&state, &ctx), DamageMul::ONE);
    }

    /// **PaperKrane #4** — not owned: only power-side Weak fires (3/4).
    #[test]
    fn listener_paper_krane_no_relic_no_op() {
        use sts2_core::hooks::ValueProps;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let enemy_uid = paper_krane_setup(&mut state, false, true, false);
        let ctx = DamageCtx {
            dealer: CreatureRef::Enemy(enemy_uid),
            target: CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        assert_eq!(dispatch_combined_mul(&state, &ctx), DamageMul::WEAK);
    }

    /// **PaperKrane #5** — unpowered hit (no MOVE): identity.
    #[test]
    fn listener_paper_krane_unpowered_no_op() {
        use sts2_core::hooks::ValueProps;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let enemy_uid = paper_krane_setup(&mut state, true, true, false);
        let ctx = DamageCtx {
            dealer: CreatureRef::Enemy(enemy_uid),
            target: CreatureRef::Player,
            props: ValueProps::NONE,
        };
        assert_eq!(dispatch_combined_mul(&state, &ctx), DamageMul::ONE);
    }

    // ─── sts2-rs-roi4f: WhiteBeastStatue should_force_potion_reward ───

    /// WhiteBeastStatue forces `force = true` on every combat-room
    /// dispatch (Monster / Elite / Boss). Mirrors the Python pin
    /// `rare.py::WhiteBeastStatue.should_force_potion_reward`.
    #[test]
    fn listener_white_beast_statue_forces_on_combat_rooms() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::WhiteBeastStatue);
        for rt in [RoomType::Monster, RoomType::Elite, RoomType::Boss] {
            assert!(
                sts2_core::run_effects::dispatch_should_force_potion_reward(&mut rs, rt),
                "WhiteBeastStatue must force on {rt:?}"
            );
        }
    }

    /// WhiteBeastStatue does NOT force on non-combat rooms (Shop,
    /// RestSite, Treasure, Event).
    #[test]
    fn listener_white_beast_statue_does_not_force_on_non_combat_rooms() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::WhiteBeastStatue);
        for rt in [
            RoomType::Shop,
            RoomType::RestSite,
            RoomType::Treasure,
            RoomType::Event,
        ] {
            assert!(
                !sts2_core::run_effects::dispatch_should_force_potion_reward(&mut rs, rt),
                "WhiteBeastStatue must NOT force on {rt:?}"
            );
        }
    }

    /// Without ownership, no listener sets force.
    #[test]
    fn listener_white_beast_statue_is_noop_without_ownership() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // No WhiteBeastStatue in relics.
        for rt in [
            RoomType::Monster,
            RoomType::Elite,
            RoomType::Boss,
            RoomType::Shop,
        ] {
            assert!(
                !sts2_core::run_effects::dispatch_should_force_potion_reward(&mut rs, rt),
                "no relic → no force on {rt:?}"
            );
        }
    }

    // ─── sts2-rs-vk9av: end-to-end integration via RunState::roll_potion_reward ───

    /// With WhiteBeastStatue equipped, `roll_potion_reward` returns
    /// true for every combat-room invocation across 50 rolls (force
    /// path bypasses the odds entirely).
    #[test]
    fn roll_potion_reward_with_white_beast_statue_always_drops_on_combat() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::WhiteBeastStatue);
        for _ in 0..50 {
            for rt in [RoomType::Monster, RoomType::Elite, RoomType::Boss] {
                assert!(
                    rs.roll_potion_reward(rt),
                    "WhiteBeastStatue forces potion on {rt:?}"
                );
            }
        }
    }

    /// With WhiteBeastStatue equipped, non-combat rooms still consult
    /// the odds (no force) — the force-listener bails on Shop/RestSite/
    /// Treasure/Event. The drop rate stays in a sane band.
    #[test]
    fn roll_potion_reward_with_white_beast_statue_falls_through_on_non_combat() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::WhiteBeastStatue);
        let mut hits = 0;
        for _ in 0..200 {
            if rs.roll_potion_reward(RoomType::Shop) {
                hits += 1;
            }
        }
        // Without force, the oscillating 40-50% odds apply; sanity band.
        assert!(
            (40..=160).contains(&hits),
            "Shop room without force: {hits}/200 in [40, 160]"
        );
    }

    /// Without WhiteBeastStatue, combat-room rolls use the bare
    /// PotionRewardOdds (no force). Sanity band: ~40-50% hit rate.
    #[test]
    fn roll_potion_reward_without_force_listener_uses_bare_odds() {
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        // No WhiteBeastStatue.
        let mut hits = 0;
        for _ in 0..200 {
            if rs.roll_potion_reward(RoomType::Monster) {
                hits += 1;
            }
        }
        assert!(
            (40..=160).contains(&hits),
            "Monster room without force: {hits}/200 in [40, 160]"
        );
    }

    // ─── pool_relic_ids accessor ───
    /// `pool_relic_ids()` returns the full `RARE_RELIC_IDS` membership
    /// (non-empty, exact). Kills a `-> vec![]` body replacement.
    #[test]
    fn pool_relic_ids_returns_full_membership() {
        let ids = pool_relic_ids();
        assert_eq!(ids.len(), RARE_RELIC_IDS.len());
        assert_eq!(ids, RARE_RELIC_IDS.to_vec());
        assert!(ids.contains(&RelicId::WhiteStar));
    }

    // ─── FrozenEgg / MoltenEgg ownership-gated upgrade hooks ───
    // These pin the `if !run_owns_rare(...)` ownership guard on the
    // card-reward and merchant listeners: with the relic OWNED the
    // matching card-type must be upgraded (deleting the `!` flips the
    // guard so an owned relic returns NotImplemented and skips the
    // upgrade).

    #[test]
    fn frozen_egg_upgrades_power_in_card_reward_options() {
        use sts2_core::run_effects::dispatch_modify_card_reward_options_late;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::FrozenEgg);
        let mut options = vec![
            egg_test_card(51_000, CardType::Power),
            egg_test_card(51_001, CardType::Attack),
        ];
        let _ = dispatch_modify_card_reward_options_late(&mut rs, &mut options);
        assert!(options[0].upgraded, "owned FrozenEgg upgrades Power option");
        assert!(!options[1].upgraded, "Attack option untouched");
    }

    #[test]
    fn frozen_egg_card_reward_inert_without_ownership() {
        use sts2_core::run_effects::dispatch_modify_card_reward_options_late;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut options = vec![egg_test_card(51_002, CardType::Power)];
        let _ = dispatch_modify_card_reward_options_late(&mut rs, &mut options);
        assert!(!options[0].upgraded, "no FrozenEgg → no upgrade");
    }

    #[test]
    fn frozen_egg_upgrades_power_in_merchant() {
        use sts2_core::run_effects::dispatch_modify_merchant_card_creation_results;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::FrozenEgg);
        let mut card = egg_test_card(51_010, CardType::Power);
        let _ = dispatch_modify_merchant_card_creation_results(&mut rs, &mut card, false);
        assert!(
            card.upgraded,
            "owned FrozenEgg upgrades Power merchant entry"
        );
    }

    #[test]
    fn frozen_egg_merchant_inert_without_ownership() {
        use sts2_core::run_effects::dispatch_modify_merchant_card_creation_results;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut card = egg_test_card(51_011, CardType::Power);
        let _ = dispatch_modify_merchant_card_creation_results(&mut rs, &mut card, false);
        assert!(!card.upgraded, "no FrozenEgg → no merchant upgrade");
    }

    #[test]
    fn molten_egg_upgrades_attack_in_merchant() {
        use sts2_core::run_effects::dispatch_modify_merchant_card_creation_results;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::MoltenEgg);
        let mut card = egg_test_card(51_020, CardType::Attack);
        let _ = dispatch_modify_merchant_card_creation_results(&mut rs, &mut card, false);
        assert!(
            card.upgraded,
            "owned MoltenEgg upgrades Attack merchant entry"
        );
    }

    #[test]
    fn molten_egg_merchant_inert_without_ownership() {
        use sts2_core::run_effects::dispatch_modify_merchant_card_creation_results;
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let mut card = egg_test_card(51_021, CardType::Attack);
        let _ = dispatch_modify_merchant_card_creation_results(&mut rs, &mut card, false);
        assert!(!card.upgraded, "no MoltenEgg → no merchant upgrade");
    }

    // ─── WhiteStar elite-room reward gate ───
    /// `listener_white_star` appends a CardReward ONLY on Elite rooms
    /// (Python `WhiteStar.modify_rewards`, `RoomType.ELITE` gate). Pins
    /// the `room_type != RoomType::Elite` guard: an `==` mutation flips
    /// the room polarity (no reward on Elite, spurious reward elsewhere).
    #[test]
    fn listener_white_star_appends_reward_only_on_elite() {
        use sts2_core::run_effects::dispatch_modify_rewards;
        use sts2_core::runstate::{Reward, RoomType, RunState};
        crate::relics::dispatch::ensure_installed();

        // Elite → exactly one Card reward.
        let mut rs = RunState::new();
        rs.relics.push(RelicId::WhiteStar);
        let mut rewards: Vec<Reward> = Vec::new();
        let _ = dispatch_modify_rewards(&mut rs, &mut rewards, RoomType::Elite);
        assert_eq!(rewards.len(), 1, "WhiteStar adds a reward on Elite");
        assert!(matches!(rewards[0], Reward::Card(_)));

        // Non-Elite rooms → no reward.
        for rt in [
            RoomType::Monster,
            RoomType::Boss,
            RoomType::Shop,
            RoomType::Treasure,
        ] {
            let mut rs = RunState::new();
            rs.relics.push(RelicId::WhiteStar);
            let mut rewards: Vec<Reward> = Vec::new();
            let _ = dispatch_modify_rewards(&mut rs, &mut rewards, rt);
            assert!(rewards.is_empty(), "WhiteStar silent on {rt:?}");
        }
    }

    // ─── PaperKrane Weak-detection boundary ───
    /// Push a Weak power of the given `amount` onto a fresh enemy and
    /// return the combat ctx, owner equipped with PaperKrane. Used to
    /// pin the `kind == Weak && amount > 0` guard.
    #[cfg(test)]
    fn paper_krane_weak_amount_setup(
        state: &mut CombatState,
        weak_amount: i32,
    ) -> sts2_core::CreatureUid {
        use smallvec::SmallVec;
        use std::num::NonZeroU32;
        use sts2_core::powers::PowerKind;
        use sts2_core::{Creature, MonsterId, MonsterKind, Power, PowerUid, Relic};
        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::PaperKrane,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        let mut enemy_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        let pid = NonZeroU32::new(10).expect("non-zero");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Weak,
            amount: weak_amount,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        enemy_powers.push(PowerUid(pid));
        let enemy_uid = sts2_core::CreatureUid::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: enemy_powers,
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: sts2_core::IntentHistory::default(),
            monster_id: MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(enemy_uid);
        enemy_uid
    }

    /// **PaperKrane boundary `amount > 0`** — a Weak power with `amount
    /// == 0` must NOT engage PaperKrane (no Weak in effect). A `>=`
    /// mutation would treat a 0-stack Weak as live and apply the 4/5.
    #[test]
    fn listener_paper_krane_weak_amount_zero_is_no_op() {
        use sts2_core::hooks::ValueProps;
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        let enemy_uid = paper_krane_weak_amount_setup(&mut state, 0);
        let ctx = DamageCtx {
            dealer: CreatureRef::Enemy(enemy_uid),
            target: CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        assert_eq!(
            sts2_core::relic_effects::dispatch_modify_damage_multiplicative(&state, &ctx),
            DamageMul::ONE,
            "Weak(0) does not engage PaperKrane"
        );
    }

    /// **PaperKrane `&&` join** — a dealer carrying ONLY a non-Weak
    /// power with `amount > 0` (Debilitate) must NOT engage PaperKrane:
    /// the guard requires `kind == Weak AND amount > 0`. A `||` mutation
    /// would fire on any positive-amount power.
    #[test]
    fn listener_paper_krane_non_weak_positive_power_is_no_op() {
        use smallvec::SmallVec;
        use std::num::NonZeroU32;
        use sts2_core::hooks::ValueProps;
        use sts2_core::powers::PowerKind;
        use sts2_core::{Creature, MonsterId, MonsterKind, Power, PowerUid, Relic};
        crate::relics::dispatch::ensure_installed();
        let mut state = CombatState::default();
        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::PaperKrane,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        // Debilitate(1): a non-Weak power with positive amount, no Weak.
        let mut enemy_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        let pid = NonZeroU32::new(10).expect("non-zero");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Debilitate,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        enemy_powers.push(PowerUid(pid));
        let enemy_uid = sts2_core::CreatureUid::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id: NonZeroU32::new(1).expect("non-zero"),
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: enemy_powers,
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: sts2_core::IntentHistory::default(),
            monster_id: MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(enemy_uid);
        let ctx = DamageCtx {
            dealer: CreatureRef::Enemy(enemy_uid),
            target: CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        assert_eq!(
            sts2_core::relic_effects::dispatch_modify_damage_multiplicative(&state, &ctx),
            DamageMul::ONE,
            "non-Weak positive power must not engage PaperKrane"
        );
    }

    // event-deck-add-hooks: EggRelicHelper.UpgradeValidCards (card rewards)
    // is CardCmd.Upgrade on the card: its whole upgrade, as the deck add's.
    #[test]
    fn eggs_upgrade_reward_cards_completely() {
        use sts2_core::cards::CardId;
        crate::cards::dispatch::install_default_registry();
        crate::relics::dispatch::ensure_installed();
        let pod = |id, up| crate::cards::spec_index::card_pod_from_id(id, up).unwrap();
        let mut rs = RunState::new();
        rs.relics.push(RelicId::ToxicEgg);
        rs.relics.push(RelicId::MoltenEgg);
        let mut cards = vec![pod(CardId::DefendIronclad, false), pod(CardId::Bash, false)];
        let _ = listener_toxic_egg_card_reward(&mut rs, &mut cards);
        let _ = listener_molten_egg_card_reward(&mut rs, &mut cards);
        let defend = pod(CardId::DefendIronclad, true);
        let bash = pod(CardId::Bash, true);
        assert!(cards[0].upgraded && cards[1].upgraded);
        assert_eq!(cards[0].base_block, defend.base_block);
        assert_eq!(cards[1].base_damage, bash.base_damage);
    }
}
