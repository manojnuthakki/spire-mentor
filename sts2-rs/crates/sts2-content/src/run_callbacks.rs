//! `ContentCallbacks` factories for the run-loop dispatch table.
//!
//! [`sts2_core::run_manager::RunManager::take_action`] dispatches every
//! Phase variant through [`sts2_core::run_manager::ContentCallbacks`]
//! function pointers — the engine-agnostic dispatch lives in `sts2-core`
//! while the content-binding wires happen here so the `content → core`
//! dep direction stays one-way.
//!
//! [`ironclad_content_callbacks`] is the Act-1→Act-3 Ironclad factory
//! consumed by the `full_run_act1to3_ironclad` acceptance test and any
//! future single-character runtime host. Bead `sts2-rs-81xxo`.
//!
//! ## Partition status
//!
//! - **PARTITION-A (sibling cc_1):** combat-related callbacks —
//!   `enter_combat`, `exit_combat`, `select_encounter`. Wired below.
//! - **PARTITION-B (this commit):** reward-related callbacks —
//!   `generate_card_reward_options`, `generate_boss_relic_pool`,
//!   `generate_treasure_relic_pools`. Wired below.
//! - **PARTITION-C (sibling):** event resolver — `resolve_event_at`
//!   + `event_option_count` (bead `sts2-rs-nzdz1` upgraded the
//!   former single-option `resolve_event` slot to an idx-aware pair).
//!   Wired to [`crate::events::dispatch::resolve_event_at_callback`]
//!   (picks via `pick_event` + resolves the caller-supplied option
//!   index) and [`crate::events::dispatch::event_option_count_callback`].

use sts2_core::act_selection::ActVariant;
use sts2_core::cards::CardId;
use sts2_core::encounters::{
    act1_boss_pool, act1_normal_pool, act2_boss_pool, act2_elite_pool, act2_normal_pool,
    act3_boss_pool, act3_elite_pool, act3_normal_pool, EncounterSpec,
};
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{ContentCallbacks, RunManager};
use sts2_core::runstate::{CardRewardGenerationOptions, PinnedActBoss, RoomType, RunState};
use sts2_core::{RelicId, RelicRarity};

/// Ironclad Act-1→Act-3 callback bundle. Wires PARTITION-A
/// (combat-related: `enter_combat`, `exit_combat`,
/// `select_encounter`) plus PARTITION-B (reward-related:
/// `generate_card_reward_options`, `generate_boss_relic_pool`,
/// `generate_treasure_relic_pools`) plus PARTITION-C (event resolver:
/// `resolve_event_at` + `event_option_count`). Beads `sts2-rs-81xxo`
/// (initial wiring) + `sts2-rs-nzdz1` (multi-option idx plumbing).
#[must_use]
pub fn ironclad_content_callbacks() -> ContentCallbacks {
    ContentCallbacks {
        enter_combat: Some(crate::run_bridge::enter_combat),
        exit_combat: Some(crate::run_bridge::exit_combat),
        select_encounter: Some(select_encounter_act1_to_3),
        // EncounterDef-based builder: tier-by-floor selection + roster
        // randomizers + per-monster HP rolls for Act 1. Returns `None`
        // for Acts 2–3 so `select_encounter` (above) still handles them.
        build_combat_for_room: Some(build_combat_for_room),
        generate_card_reward_options: Some(generate_card_reward_options),
        generate_noncombat_card_reward_options: Some(
            crate::cards::spec_index::roll_noncombat_card_reward_options,
        ),
        generate_boss_relic_pool: Some(generate_boss_relic_pool),
        setup_boss_relic_ancient: Some(setup_boss_relic_ancient),
        generate_treasure_relic_pools: Some(generate_treasure_relic_pools),
        resolve_event_at: Some(crate::events::dispatch::resolve_event_at_callback),
        event_option_count: Some(crate::events::dispatch::event_option_count_callback),
        // Stocked merchant inventory: fills the payload-less slots core's
        // `shop::generate_shop_inventory` rolls (price + rarity only) with
        // concrete cards/relics/potions from the run's pools, so the shop
        // is buyable instead of perpetually sold-out. Bead `sts2-rs-64mq2`
        // (follow-up to the `sts2-rs-if6ry` structure-only slice).
        generate_shop_inventory: Some(generate_shop_inventory),
        // Courier restock: re-roll a bought slot on the run's Shops/Rewards
        // streams read-for-read with the initial merchant roll, so a later
        // shop visit stays aligned (parity sweep 2026-07-07 run-shop-gold).
        restock_shop_entry: Some(crate::cards::spec_index::restock_shop_entry),
        // Bead `sts2-rs-01o`: roll one random out-of-combat potion for
        // unspecified `Reward::Potion` sentinels (Lost Coffer / event
        // "procure 1 random potion") drained by the run loop.
        roll_potion_reward_kind: Some(roll_out_of_combat_potion_kind),
        roll_neows_bones_curse: Some(crate::relics::neow_relics::roll_neows_bones_curse),
        // Pre-roll the act's boss at map-generation time so the workbench
        // map widget can label the boss from floor 1 (matching the C#
        // game) and so the boss-room dispatch consumes the same pick.
        pre_roll_act_boss: Some(pre_roll_act_boss),
        // Bead `sts2-rs-jenwp`: consume the run-level `Rewards` stream for a
        // Monster/Elite win (potion-odds → gold → potion-contents) in C#'s
        // `RewardsSet` order + apply gold/potion, BEFORE the card reward
        // rolls off the same cursor.
        apply_combat_gold_and_potion: Some(apply_combat_gold_and_potion),
        // sts2-agent fork: the opt-in rewards screen rolls the same rewards
        // without applying them; Entropic Brew out of combat rolls its fill.
        roll_combat_gold_and_potion: Some(roll_combat_gold_and_potion),
        roll_out_of_combat_potion: Some(roll_out_of_combat_potion),
        // Interactive deck-transform resolve (NewLeaf): roll the
        // replacement off the NICHE stream after the player's pick —
        // C# `NewLeaf.AfterObtained` → select → `CardCmd.TransformToRandom
        // (item, RunState.Rng.Niche)` — as a spec-populated POD. Bead
        // 6ynt.4.deck-add / GH #34.
        roll_transform_replacement: Some(roll_transform_replacement),
        // Bead `sts2-rs-ck9z6` (GH #166): event→combat bridge. Build the
        // nested combat for an event fight (BattlewornDummy / TheLanternKey /
        // FakeMerchant) and resume the parent event after victory.
        build_event_combat: Some(crate::run_bridge::build_event_combat),
        resume_event_after_combat: Some(
            crate::events::dispatch::resume_event_after_combat_callback,
        ),
    }
}

/// Roll the replacement card for one interactive deck transform
/// (NewLeaf's post-pick `CardCmd.TransformToRandom(item, Rng.Niche)`):
/// the character transform pool (C/U/R, non-MP-only, `!= original`)
/// indexed by one `niche` read — or, when the pending entry carries an
/// `event_stream_roll`, by a read off that event's own non-shared stream
/// (WhisperingHollow Hug passes `base.Rng` to `TransformToRandom`) —
/// materialized through [`crate::cards::spec_index::card_pod_from_id`]
/// so the POD carries its real spec stats (cost / base_damage /
/// base_block), not `pod_defaults()`. `None` only on an empty pool or
/// unmapped spec (unreachable for shipped characters).
fn roll_transform_replacement(
    rs: &mut RunState,
    original: sts2_core::cards::CardId,
    event_stream: Option<(&'static str, u32)>,
) -> Option<sts2_core::cards::Card> {
    let character = rs.character_id;
    let new_id = match event_stream {
        Some((name, skip)) => crate::cards::spec_index::roll_transform_replacement_event_stream(
            rs, character, original, name, skip,
        )?,
        None => crate::cards::spec_index::roll_transform_replacement_basic_niche(
            rs, character, original,
        )?,
    };
    crate::cards::spec_index::card_pod_from_id(new_id, false)
}

/// Pre-roll the boss encounter for the act the player just entered and
/// stash it on [`RunState::pinned_act_boss`]. Mirrors the C# game's
/// "boss locked at act start" behavior — the boss-room dispatch in
/// [`build_combat_for_room`] (all acts; [`select_encounter_act1_to_3`]
/// honors the pin too for the legacy direct-call path) consumes the pin
/// instead of rolling fresh, so total `run_manager` advancement is preserved and
/// only the *timing* of the boss roll shifts from boss entry to act
/// start.
///
/// Act 1 draws from the live [`crate::encounters::act1_overgrowth::
/// ACT1_ENCOUNTERS`] Boss-tier subset (3 entries: Vantom, Ceremonial
/// Beast, The Kin). Acts 2-3 draw from
/// [`sts2_core::encounters::act2_boss_pool`] / `act3_boss_pool` (3
/// entries each). No-op for out-of-range acts.
pub fn pre_roll_act_boss(rs: &mut RunState) {
    use crate::encounters::EncounterTier;
    // GH #544: act 1 has two models, so this MUST be the table the run is
    // actually walking. Both act-1 boss pools hold exactly 3 entries, so the
    // `next_int(0, len - 1)` below draws the same width either way and the
    // `run_manager` stream stays byte-identical — the bug this fixes was a
    // WRONG PIN, never a desync, which is why `--underdocks-parity` (which
    // compares generated rooms) stayed green while the pin was Overgrowth's.
    let act1_encounters = act_encounter_table(1, rs.act1).unwrap_or(&[]);
    let act_index = rs.act_index;
    // Idempotent: if an earlier call (e.g. a re-init path) already
    // pinned this act's boss, don't burn fresh RNG re-rolling it.
    if rs
        .pinned_act_boss
        .as_ref()
        .is_some_and(|p| p.act_index == act_index)
    {
        return;
    }
    // C#-faithful boss identity: `RunManager.GenerateRooms` already rolled this
    // act's boss off the shared `up_front` stream at run start
    // (`up_front.NextItem(AllBossEncounters)`), stored on
    // `RunState::generated_rooms` (`run_room_gen`). Prefer it over the
    // `run_manager` roll below so the boss FOUGHT matches the boss LABEL shown
    // on the map (`full_map().boss`). The `run_manager` ints are still consumed
    // (stream alignment for the not-yet-migrated per-node encounter rolls); only
    // the chosen index is overridden. When `generated_rooms` is empty (raw-core
    // test paths that skip the PyO3 ctor), fall back to the `run_manager` roll.
    // Bead `sts2-rs-jenwp` (M3).
    let gen_boss: Option<&'static str> = (act_index as usize)
        .checked_sub(1)
        .and_then(|i| rs.generated_rooms.get(i))
        .map(|g| g.boss_id);
    let pin = match act_index {
        1 => {
            let pool: Vec<_> = act1_encounters
                .iter()
                .enumerate()
                .filter(|(_, e)| e.tier == EncounterTier::Boss)
                .collect();
            if pool.is_empty() {
                return;
            }
            let len = i32::try_from(pool.len()).unwrap_or(1);
            let idx = rs.rng.run_manager.next_int(0, len - 1) as usize;
            let per_encounter_seed = rs.rng.run_manager.next_int(0, i32::MAX);
            // Override with the `up_front`-generated boss (matched by id) when
            // available; else honor the `run_manager` roll.
            let (def_index, def) = gen_boss
                .and_then(|bid| pool.iter().find(|(_, e)| e.id == bid).copied())
                .unwrap_or(pool[idx]);
            // Lead monster identifies the boss for display lookups; for
            // multi-monster boss encounters (e.g. The Kin) it's the
            // first roster entry, which the workbench renders.
            let lead_kind = def
                .monsters
                .first()
                .map_or(sts2_core::MonsterKind::Vantom, |(k, _)| *k);
            PinnedActBoss {
                act_index,
                encounter_index: def_index,
                per_encounter_seed,
                lead_kind,
                display_name: def.display,
            }
        }
        2 | 3 => {
            let pool: &'static [EncounterSpec] = if act_index == 2 {
                act2_boss_pool()
            } else {
                act3_boss_pool()
            };
            if pool.is_empty() {
                return;
            }
            let len = i32::try_from(pool.len()).unwrap_or(1);
            let idx = rs.rng.run_manager.next_int(0, len - 1) as usize;
            let per_encounter_seed = rs.rng.run_manager.next_int(0, i32::MAX);
            // Override with the `up_front`-generated boss (matched by snake_case
            // name) when available; else honor the `run_manager` roll. The act
            // 2/3 boss pools are ordered differently from C#'s `AllBossEncounters`,
            // so match by id, not index.
            let idx = gen_boss
                .and_then(|bid| pool.iter().position(|s| s.name.eq_ignore_ascii_case(bid)))
                .unwrap_or(idx);
            let spec = &pool[idx];
            let lead_kind = spec
                .members
                .first()
                .map_or(sts2_core::MonsterKind::Vantom, |m| m.kind);
            PinnedActBoss {
                act_index,
                encounter_index: idx,
                per_encounter_seed,
                lead_kind,
                // EncounterSpec carries a snake_case `name` (e.g.
                // "the_insatiable_boss"); the workbench resolves the
                // display title from `lead_kind` instead, so the
                // `display_name` here is a fallback.
                display_name: spec.name,
            }
        }
        _ => return,
    };
    rs.pinned_act_boss = Some(pin);
}

/// Re-derive the CURRENT act's pinned boss identity from the
/// `up_front`-generated boss id (`RunState::generated_rooms`), preserving the
/// `run_manager`-rolled `per_encounter_seed`. Consumes NO RNG.
///
/// Act 1's [`pre_roll_act_boss`] fires during the PyO3 `RunManager`
/// construction, BEFORE `generate_all_rooms` has populated `generated_rooms`, so
/// the act-1 pin is initially the `run_manager`-rolled boss. Acts 2/3 pre-roll
/// at act-advance time (after generation), so their override lands directly.
/// Call this once right after `generated_rooms` is populated to align act 1's
/// boss FIGHT with the boss LABEL surfaced on the map. No-op when no pin exists,
/// no generated boss is available, or the id doesn't resolve to a content
/// encounter (e.g. raw-core test paths). Bead `sts2-rs-jenwp` (M3).
pub fn repin_act_boss_from_generated(rs: &mut RunState) {
    use crate::encounters::EncounterTier;
    // GH #544: search the act-1 table this run walks. Against the Overgrowth
    // table an Underdocks boss id simply is not found, so this early-returned
    // and left the pre-roll's Overgrowth pin in place — the map then displayed
    // (and `pinned_boss_encounter_id` reported) a boss the run never fights.
    let act1_encounters = act_encounter_table(1, rs.act1).unwrap_or(&[]);
    let act_index = rs.act_index;
    let Some(boss_id) = (act_index as usize)
        .checked_sub(1)
        .and_then(|i| rs.generated_rooms.get(i))
        .map(|g| g.boss_id)
    else {
        return;
    };
    let Some(per_encounter_seed) = rs
        .pinned_act_boss
        .as_ref()
        .filter(|p| p.act_index == act_index)
        .map(|p| p.per_encounter_seed)
    else {
        return;
    };
    let pin = match act_index {
        1 => {
            let Some((def_index, def)) = act1_encounters
                .iter()
                .enumerate()
                .find(|(_, e)| e.tier == EncounterTier::Boss && e.id == boss_id)
            else {
                return;
            };
            PinnedActBoss {
                act_index,
                encounter_index: def_index,
                per_encounter_seed,
                lead_kind: def
                    .monsters
                    .first()
                    .map_or(sts2_core::MonsterKind::Vantom, |(k, _)| *k),
                display_name: def.display,
            }
        }
        2 | 3 => {
            let pool: &'static [EncounterSpec] = if act_index == 2 {
                act2_boss_pool()
            } else {
                act3_boss_pool()
            };
            let Some(idx) = pool
                .iter()
                .position(|s| s.name.eq_ignore_ascii_case(boss_id))
            else {
                return;
            };
            let spec = &pool[idx];
            PinnedActBoss {
                act_index,
                encounter_index: idx,
                per_encounter_seed,
                lead_kind: spec
                    .members
                    .first()
                    .map_or(sts2_core::MonsterKind::Vantom, |m| m.kind),
                display_name: spec.name,
            }
        }
        _ => return,
    };
    rs.pinned_act_boss = Some(pin);
}

/// Order in which the relic grab-bag rarity buckets are shuffled off the
/// `up_front` stream. C# `SharedRelicPool.GenerateAllRelics` lists relics
/// starting with `Akabeko` (Uncommon), then `AmethystAubergine` (Common),
/// then `ArtOfWar` (Rare), then `BeltBuckle` (Shop); `RelicGrabBag.Populate`
/// buckets into a dict keyed in that first-seen order and shuffles
/// `_deques.Values` in iteration (= insertion) order. Rust's `BTreeMap` keys
/// on the `RelicRarity` discriminant (Common→Uncommon→…), so we must drive the
/// shuffle by this explicit list to consume `up_front` in C# order.
const GRAB_BAG_SHUFFLE_ORDER: [sts2_core::runstate::RelicRarity; 4] = {
    use sts2_core::runstate::RelicRarity;
    [
        RelicRarity::Uncommon,
        RelicRarity::Common,
        RelicRarity::Rare,
        RelicRarity::Shop,
    ]
};

/// Canonical, C#-faithful run-setup that EVERY real run requires: populate the
/// relic grab bags (player + shared) and pre-roll all acts' room contents
/// (event-pool shuffle, weak/regular/elite encounters, bosses, ancients) off
/// the named `up_front` RNG stream — exactly as C# `RunManager.InitializeNewRun`
/// + `GenerateRooms` do at run start.
///
/// The bare `new_run_manager_for_character*` factories deliberately OMIT this so
/// lightweight consumers (feature/view/projection probes, unit tests) don't pay
/// for full init + room generation. Any driver that plays a *full run* — the
/// arena, the labeler, the feature exporter, the calibrator, the PyO3 binding —
/// MUST call this exactly once, after construction (and after `with_neow_start`,
/// matching the PyO3 order, so the Neow offers are unaffected). Skipping it
/// leaves every grab-bag-fed relic source empty (only the Neow relic lands) and
/// serves events in a fixed, unshuffled order — the class of silent
/// run-setup bug that survived all of M0–M5 (Addendum 23).
///
/// Consumes only the `up_front` stream (map LAYOUT uses the separate per-act
/// `map_rng` salt), so it is C#-faithful AND cannot disturb lazily-generated act
/// maps. Idempotent in practice — call once at construction.
///
/// RNG order (verified read-for-read against the live C# headless engine via
/// `sts2-pycli/parity_walk.py`; beads `sts2-rs-mcesz` / `7e04v` / `jenwp`):
/// C# `InitializeNewRun` shuffles TWO bags off the *same* `up_front` stream —
/// first the shared-only `SharedRelicGrabBag` (`RunManager.cs:311`, ~111 reads
/// for the 30/25/35/25 shared buckets), then a `NextInt(2)` setup read
/// (counter 112, a single-element-range read not visible in the IL decomp),
/// then the player's shared+character bag (`:314`). Relic rewards draw from the
/// player bag, but the shared bag's shuffle must run first or the player Common
/// shuffle starts ~111 reads too early. The shuffled shared bag is retained
/// (treasure-room chest pulls from it via `SharedRelicGrabBag.PullFromFront`).
/// Then `GenerateRooms` pre-rolls all acts' contents (reads 231..413).
pub fn initialize_run_setup(rm: &mut sts2_core::run_manager::RunManager) {
    let cid = rm.run_state.character_id;
    let (mut by_rarity, fallback, boss) = default_relic_grab_bag_for(cid);

    // (1) shared-only bag first (its shuffle consumes ~111 `up_front` reads).
    let mut shared = crate::relics::grab_bag::shared_grab_bag_buckets();
    for rarity in GRAB_BAG_SHUFFLE_ORDER {
        if let Some(bucket) = shared.get_mut(&rarity) {
            rm.run_state.rng.up_front.shuffle(bucket);
        }
    }
    rm.run_state.shared_relic_grab_bag_by_rarity = shared;

    // (2) one setup `NextInt(2)` between the two populates (counter 112).
    rm.run_state.rng.up_front.next_int(0, 1);

    // (3) player shared+character bag.
    for rarity in GRAB_BAG_SHUFFLE_ORDER {
        if let Some(bucket) = by_rarity.get_mut(&rarity) {
            rm.run_state.rng.up_front.shuffle(bucket);
        }
    }
    rm.run_state
        .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);

    // (4) GenerateRooms: pre-roll every act's room contents off `up_front`
    // (events shuffle, encounters, boss, ancient). A10 DoubleBoss threads
    // `has_double_boss` so the final act rolls its second boss too.
    let has_double_boss = rm.run_state.has_double_boss;
    // GH #544: which Act-1 model the run walks. Decided before this point
    // (`RunManager::with_act1` / `with_rolled_act1`) on its own
    // `act_selection` stream, so reading it here costs no `up_front` reads —
    // but it changes act 1's encounter table AND the size of its event pool,
    // which shifts the cursor for acts 2 and 3 as well.
    let act1 = rm.run_state.act1;
    rm.run_state.generated_rooms = sts2_core::run_room_gen::generate_all_rooms(
        &mut rm.run_state.rng.up_front,
        has_double_boss,
        act1,
    );

    // (5) Act 1's boss was pinned at construction (before rooms existed);
    // re-derive its identity from the freshly generated boss so the boss FOUGHT
    // matches the map LABEL. Consumes no RNG.
    repin_act_boss_from_generated(&mut rm.run_state);
}

/// Roll a single random out-of-combat potion kind on the **Rewards** RNG
/// stream, mirroring C# `PotionReward.Populate`
/// (`PotionReward.cs:56-61`: `rng = _rngOverride ?? PlayerRng.Rewards` ->
/// `PotionFactory.CreateRandomPotionOutOfCombat`). Wired as
/// [`sts2_core::run_manager::ContentCallbacks::roll_potion_reward_kind`]
/// so the core run loop can materialize potion-reward sentinels (LostCoffer,
/// event potion rewards) without depending on the content potion pools.
///
/// NOT the stream PhialHolster uses — its `AfterObtained` explicitly passes
/// `Rng.CombatPotionGeneration` (PhialHolster.cs:29) and has its own
/// listener. This previously read `CombatPotionGeneration` too, which left
/// the Rewards cursor 2 reads short of C# after a LostCoffer pick (RP0:
/// every later Rewards roll shifted — gold 116 vs 115). Bead `sts2-rs-01o`.
fn roll_out_of_combat_potion_kind(rs: &mut RunState) -> Option<sts2_core::potions::PotionKind> {
    let character = rs.character_id;
    crate::potions::all::create_random_potions_out_of_combat(&mut rs.rng.rewards, character, 1)
        .into_iter()
        .next()
}

/// Apply a Monster/Elite combat win's gold + potion rewards, consuming the
/// run-level `Rewards` stream in C#'s `RewardsSet.GenerateRewardsFor` order
/// so the subsequent card reward (`generate_card_reward_options`) rolls off
/// the correctly-advanced cursor. Wired as
/// [`sts2_core::run_manager::ContentCallbacks::apply_combat_gold_and_potion`]
/// and invoked by `RunManager::exit_combat_and_transition` immediately
/// before `enter_card_reward_phase`.
///
/// C# (`RewardsSet.cs:167-174`, `RoomType.Monster`) constructs
/// `[GoldReward, (PotionReward?), CardReward]` then `Populate()`s them in
/// list order, all on `player.PlayerRng.Rewards`:
///   1. **potion-odds** — `RollForPotionAndAddTo` → `PotionRewardOdds.Roll`
///      reads one `NextFloat` (pity `CurrentValue`, init 0.4, ±0.1; the
///      drop fires for a Monster when `roll < CurrentValue`). This happens
///      at reward-set CONSTRUCTION, before the gold/card populate.
///   2. **gold** — `GoldReward.Populate` → `Rewards.NextInt(min, max+1)`
///      (Monster 10–20). Awarded eagerly here via [`RunState::gain_gold`]
///      to mirror C# auto-claiming gold on the rewards screen (and to run
///      the gold-gain hooks, like `PlayerCmd.GainGold`).
///   3. **potion-contents** — `PotionReward.Populate` →
///      `CreateRandomPotionOutOfCombat(player, Rewards)`, only when (1)
///      dropped. Note this reads `Rewards`, NOT the
///      `CombatPotionGeneration` stream that
///      [`roll_out_of_combat_potion_kind`] uses for event/Lost-Coffer
///      potion sentinels.
///   4. card reward — rolled later by `generate_card_reward_options`.
///
/// `GoldProportion` (C# `CombatRoom.GoldProportion = 1 - escaped/total`,
/// `CombatRoom.cs:216`) is read from `rs.last_combat_gold_proportion`, which
/// `exit_combat` set from the combat's `escaped_creatures`. A combat where
/// every monster fled rounds the bounds to 0 → `roll_encounter_gold` returns
/// `None` → no gold (and no `Rewards` read), matching C#. Bead `sts2-rs-RP15`.
///
/// Invoked for Monster/Elite rooms from `exit_combat_and_transition` and
/// for Boss rooms from `do_boss_relic_pick` (C# fires
/// `RewardsCmd.OfferForRoomEnd` for boss rooms too, so a boss kill
/// consumes this same gold + potion-odds sequence after the RNG-free,
/// pre-shuffled boss-relic choice). The Elite-only 5th reward (a relic,
/// `RollRarity` off `Rewards`) is appended separately by the run loop
/// AFTER the card reward, matching C#'s populate order. Bead
/// `sts2-rs-jenwp` (M4 live reward stream).
/// The rewards-screen twin of [`apply_combat_gold_and_potion`]
/// (`RunState::interactive_rewards`, sts2-agent fork): the same `Rewards`
/// reads in the same order (potion odds, gold, potion contents) and the same
/// `modify_rewards` gold bonuses, returned instead of applied. The potion is
/// returned whatever the belt holds: the screen lists it, unclaimable while
/// the belt is full, as C# lists a TooFull `PotionReward`.
fn roll_combat_gold_and_potion(
    rs: &mut RunState,
    room_type: RoomType,
) -> sts2_core::run_manager::RoomRewardRoll {
    let mut out = sts2_core::run_manager::RoomRewardRoll::default();
    let potion_drops = rs.roll_potion_reward(room_type);
    let gold_proportion = rs.last_combat_gold_proportion;
    if let Some(amount) = rs.roll_encounter_gold(room_type, gold_proportion) {
        out.gold.push(amount);
    }
    if potion_drops {
        let character = rs.character_id;
        out.potion = crate::potions::all::create_random_potions_out_of_combat(
            &mut rs.rng.rewards,
            character,
            1,
        )
        .into_iter()
        .next();
    }
    out.gold.extend(
        rs.offer_rewards(room_type, Vec::new())
            .iter()
            .filter_map(|r| match r {
                sts2_core::runstate::Reward::Gold { amount } if *amount > 0 => Some(*amount),
                _ => None,
            }),
    );
    out
}

/// One random potion off the run's `CombatPotionGeneration` stream (C#
/// `EntropicBrew.OnUse` out of combat:
/// `PotionFactory.CreateRandomPotionOutOfCombat(owner,
/// RunState.Rng.CombatPotionGeneration)`). sts2-agent fork.
fn roll_out_of_combat_potion(rs: &mut RunState) -> Option<sts2_core::potions::PotionKind> {
    let character = rs.character_id;
    crate::potions::all::create_random_potions_out_of_combat(
        &mut rs.rng.combat_potion_generation,
        character,
        1,
    )
    .into_iter()
    .next()
}

fn apply_combat_gold_and_potion(rs: &mut RunState, room_type: RoomType) {
    // (1) potion-odds NextFloat + pity update; `true` ⇒ a potion drops.
    let potion_drops = rs.roll_potion_reward(room_type);
    // (2) gold NextInt → apply immediately, scaled by the escaped-creature
    // GoldProportion that `exit_combat` stashed (`sts2-rs-RP15`).
    let gold_proportion = rs.last_combat_gold_proportion;
    if let Some(amount) = rs.roll_encounter_gold(room_type, gold_proportion) {
        rs.gain_gold(amount);
    }
    // (3) potion-contents off the SAME `Rewards` stream (only on a drop).
    if potion_drops {
        let character = rs.character_id;
        if let Some(kind) = crate::potions::all::create_random_potions_out_of_combat(
            &mut rs.rng.rewards,
            character,
            1,
        )
        .into_iter()
        .next()
        {
            // Reward claim: on a full belt this queues an interactive
            // discard-to-claim swap (or counts the drop) instead of
            // silently evaporating — C# leaves the TooFull `PotionReward`
            // claimable on the reward screen (GH #252).
            let _ = rs.claim_potion_reward(kind);
        }
    }
    // (4) `modify_rewards` relic bonuses (C# `RelicModel.TryModifyRewards`,
    // applied to the combat reward set). The hook bus exists + is tested but
    // was never wired into the LIVE combat-reward path, so AmethystAubergine's
    // `GoldReward(15)` per Monster/Elite (`AmethystAubergine.cs:42`) was never
    // granted — the rs-lower half of the gold class (IC:1009 elite @266:
    // C# +15). The headless auto-collects gold rewards, so credit the appended
    // `Reward::Gold` here. Listeners are pure pushes (no RNG), so this consumes
    // no `Rewards` reads and stays stream-faithful. Card/Relic bonuses from the
    // same hook (PrayerWheel / WhiteStar / LavaRock / BlackStar / Wongo) still
    // need their own presentation wiring — tracked separately; ignoring them
    // here is the current (unmodeled) behavior, so no regression. Bead
    // `sts2-rs-RP16`.
    let bonus_gold: i32 = rs
        .offer_rewards(room_type, Vec::new())
        .iter()
        .filter_map(|r| match r {
            sts2_core::runstate::Reward::Gold { amount } => Some(*amount),
            _ => None,
        })
        .sum();
    if bonus_gold > 0 {
        rs.gain_gold(bonus_gold);
    }
}

/// Seeded encounter picker for Acts 1–3. Mirrors Python
/// `RunManager._enter_combat` at
/// `sts2-rl-agent@01c094d:sts2_env/run/run_manager.py:519-533`:
///
/// ```python
/// setup_fn = self._rng.choice(pool)
/// encounter_rng = Rng(self._rng.next_int(0, INT_MAX))
/// setup_fn(self._combat, encounter_rng)
/// ```
///
/// where `self._rng = Rng(seed + RUN_MANAGER_RNG_SEED_OFFSET=9999)`.
/// We drive both rolls off [`sts2_core::RunRngSet::run_manager`] so
/// RNG offsets stay aligned with Python even though the Rust encounter
/// setup currently uses pinned HP values and never consumes the
/// per-encounter `Rng`. Bead `sts2-rs-lakuh`.
///
/// Act-1 has no dedicated elite pool at the current pin — elites in
/// Act-1 fall back to the normal pool so the dispatch table doesn't
/// panic mid-run. The dedicated `act1_elite_pool` lands in a follow-up
/// (P9.1.f-equivalent); this fallback is documented at the Python pin
/// as "Act-1 elites reuse normal pool" until the elite registry ships.
///
/// Unknown act/room-type combos fall back to `act1_normal_pool()` for
/// the same panic-safety reason; in practice the dispatch only ever
/// passes (1..=3, Monster|Elite|Boss).
fn select_encounter_act1_to_3(
    rs: &mut RunState,
    act_index: u8,
    room_type: RoomType,
) -> &'static EncounterSpec {
    // Boss-room entry: if [`pre_roll_act_boss`] already pinned this
    // act's boss at act start (the C#-faithful timing), consume the
    // stored index without burning fresh RNG — the two `run_manager`
    // ints were already taken at pre-roll time. As of M3b all acts (1-3)
    // route boss rooms through `build_combat_for_room` instead, so this
    // branch is never reached in the live dispatch (build_combat short-
    // circuits before the fallback to select_encounter); we honor the
    // pin here too for the legacy direct-call test path.
    if room_type == RoomType::Boss {
        if let Some(pin) = rs.pinned_act_boss.as_ref() {
            if pin.act_index == act_index {
                let pool: &'static [EncounterSpec] = match act_index {
                    2 => act2_boss_pool(),
                    3 => act3_boss_pool(),
                    1 => act1_boss_pool(),
                    _ => act1_normal_pool(),
                };
                let idx = pin.encounter_index.min(pool.len().saturating_sub(1));
                return &pool[idx];
            }
        }
    }
    let pool: &'static [EncounterSpec] = match (act_index, room_type) {
        (1, RoomType::Monster) => act1_normal_pool(),
        (1, RoomType::Elite) => act1_normal_pool(),
        (1, RoomType::Boss) => act1_boss_pool(),
        (2, RoomType::Monster) => act2_normal_pool(),
        (2, RoomType::Elite) => act2_elite_pool(),
        (2, RoomType::Boss) => act2_boss_pool(),
        (3, RoomType::Monster) => act3_normal_pool(),
        (3, RoomType::Elite) => act3_elite_pool(),
        (3, RoomType::Boss) => act3_boss_pool(),
        _ => act1_normal_pool(),
    };
    // Python `self._rng.choice(pool)` → `next_int(0, len-1)` inclusive.
    let len = i32::try_from(pool.len()).unwrap_or(1);
    let idx = rs.rng.run_manager.next_int(0, len - 1) as usize;
    // Python then immediately rolls `self._rng.next_int(0, INT_MAX)`
    // to seed the per-encounter Rng. Consume it here too so downstream
    // run_manager rolls (combat_seed, future gold rolls, the next
    // encounter pick) stay byte-aligned with Python. The resulting
    // value is discarded — Rust's encounter setup uses pinned HP at
    // the current pin and never consumes the per-encounter Rng.
    let _per_encounter_seed = rs.rng.run_manager.next_int(0, i32::MAX);
    &pool[idx]
}

/// Per-act [`EncounterDef`] table — Act 1
/// [`crate::encounters::act1_overgrowth::ACT1_ENCOUNTERS`] or
/// [`crate::encounters::act1_underdocks::ACT1_UNDERDOCKS_ENCOUNTERS`]
/// depending on the run's act-1 variant, Act 2
/// [`crate::encounters::act2_hive::ACT2_ENCOUNTERS`], Act 3
/// [`crate::encounters::act3_glory::ACT3_ENCOUNTERS`]. Returns `None` for
/// out-of-range acts so the builder defers to the legacy fallback.
///
/// Act 1 takes the variant because this table is what the tier fallback
/// pools come from: on a raw-core path with no generated room list, an
/// Underdocks run must still draw Underdocks encounters. GH #544.
fn act_encounter_table(
    act_index: u8,
    act1: ActVariant,
) -> Option<&'static [crate::encounters::EncounterDef]> {
    match act_index {
        1 => Some(crate::encounters::act1_encounters(act1)),
        2 => Some(crate::encounters::act2_hive::ACT2_ENCOUNTERS),
        3 => Some(crate::encounters::act3_glory::ACT3_ENCOUNTERS),
        _ => None,
    }
}

/// EncounterDef-based combat builder for Acts 1–3 (bead `sts2-rs-jenwp`
/// M3b). Resolves the encounter IDENTITY from the `up_front`-generated
/// per-act room list (`RunState::generated_rooms[act-1]`, byte-matched to
/// C# `RoomSet` by `--encounter-parity`) at the C# room cursor —
/// `normalEncounters[normalEncountersVisited]` (Monster) /
/// `eliteEncounters[eliteEncountersVisited]` (Elite) / `Boss` — then looks
/// the id up across all three per-act EncounterDef tables via
/// [`crate::encounters::find`], rolls the encounter's roster randomizer +
/// per-monster starting HP, and builds the combat via
/// [`crate::run_bridge::enter_combat_from_roster`].
///
/// RNG discipline (M3b step-c — now C#-faithful): the live full-run path
/// consumes ZERO `run_manager` ints. Identity comes from the generated
/// `RoomSet` cursor (Monster/Elite) or the pre-rolled boss pin; the roster
/// composition rolls off the per-encounter `Rng`
/// (`Rng::with_name(Seed + TotalFloor, enc.id)`) and per-monster HP off the
/// run-level `niche` stream — exactly C#'s two encounter-generation streams.
/// A single `run_manager` pool-index roll survives only in the raw-core
/// fallback (no generated list), purely for deterministic test variety. The
/// former per-encounter-seed `run_manager` roll (Python's `encounter_rng`)
/// is removed.
///
/// Returns `None` for non-combat room types and out-of-range acts, so the
/// legacy [`crate::run_bridge::enter_combat`] + [`select_encounter_act1_to_3`]
/// path remains the fallback there.
fn build_combat_for_room(
    rs: &mut RunState,
    act_index: u8,
    room_type: RoomType,
) -> Option<sts2_core::CombatState> {
    use crate::encounters::{EncounterDef, EncounterTier};
    use crate::monster_dispatch::MonsterSpawnArgs;
    use sts2_core::dotnet_rng::Rng;
    use sts2_core::MonsterKind;

    let table = act_encounter_table(act_index, rs.act1)?;
    let tier = match room_type {
        RoomType::Monster => {
            if rs.monster_combats_this_act < 3 {
                EncounterTier::Weak
            } else {
                EncounterTier::Normal
            }
        }
        RoomType::Elite => EncounterTier::Elite,
        RoomType::Boss => EncounterTier::Boss,
        // Non-combat room types never enter combat; defer defensively.
        _ => return None,
    };

    // Fallback pool for the chosen tier (used only when no generated list
    // exists — raw-core test paths), with a Normal fallback for any empty
    // tier so the picker never panics.
    let pool: Vec<&'static EncounterDef> = {
        let exact: Vec<&'static EncounterDef> = table.iter().filter(|e| e.tier == tier).collect();
        if exact.is_empty() {
            table
                .iter()
                .filter(|e| e.tier == EncounterTier::Normal)
                .collect()
        } else {
            exact
        }
    };
    if pool.is_empty() {
        return None;
    }

    // The `up_front`-generated room list for this act (id-matched to C#).
    let gen_act = (act_index as usize)
        .checked_sub(1)
        .and_then(|i| rs.generated_rooms.get(i));

    // Boss-room entry: consume the pre-rolled pin (set at act start by
    // [`pre_roll_act_boss`], which already took its `run_manager` ints)
    // instead of burning fresh RNG. Identity comes from the generated
    // `boss_id` (resolved across all act tables — the acts-2/3 pin's
    // `encounter_index` references the EncounterSpec pool, not this
    // EncounterDef table, so we never index the table by it). Falls back to
    // a fresh `run_manager` roll only when no pin exists (raw-core paths).
    // Resolve the encounter IDENTITY. C# walks a precomputed `RoomSet`
    // cursor — `normalEncounters[normalEncountersVisited]` (Monster) /
    // `eliteEncounters[eliteEncountersVisited]` (Elite) — with NO RNG roll
    // for identity; the `up_front`-generated room list mirrors that cursor,
    // so the live full-run path consumes ZERO `run_manager` ints here. Only
    // the raw-core test paths (empty generated list) fall back to a single
    // `run_manager` pool-index roll for deterministic variety. The former
    // per-encounter-seed `run_manager` roll (Python's `encounter_rng`) is
    // gone entirely: the roster + HP now ride C#'s real streams (the
    // per-encounter `Rng` + `niche`), so there is nothing left to seed off
    // `run_manager`. Bead `sts2-rs-jenwp` (M3b step-c).
    let enc: &'static EncounterDef = if room_type == RoomType::Boss {
        // A10 DoubleBoss: standing on the second-boss map node fights the
        // act's SECOND boss. Mirrors C# `RoomSet.NextBossEncounter`
        // (`RoomSet.cs:70-79`: `bossEncountersVisited != 0 && SecondBoss
        // != null` → `SecondBoss`); the second visit IS the second-boss
        // node, which `CombatManager.cs:801` keys off the map coord the
        // same way. Falls back to the first boss when the generated id is
        // missing (raw-core paths). Bead sts2-rs-40ikw.2.
        let at_second_boss = rs.at_second_boss_point();
        let gen_boss: Option<&'static EncounterDef> = gen_act.and_then(|g| {
            let id = if at_second_boss {
                g.second_boss_id.unwrap_or(g.boss_id)
            } else {
                g.boss_id
            };
            crate::encounters::find(id)
        });
        match rs.pinned_act_boss.as_ref() {
            // Pre-rolled pin (the live path): identity comes from the
            // generated `boss_id`; consume no RNG.
            Some(pin) if pin.act_index == act_index => gen_boss
                .or_else(|| {
                    // Raw-core fallback: act 1's pin indexes this table.
                    table
                        .get(pin.encounter_index)
                        .filter(|e| e.tier == EncounterTier::Boss)
                })
                .unwrap_or(pool[0]),
            _ => {
                // No pin (raw-core path): pick within the boss pool.
                let len = i32::try_from(pool.len()).unwrap_or(1);
                let idx = rs.rng.run_manager.next_int(0, len - 1) as usize;
                gen_boss.unwrap_or(pool[idx])
            }
        }
    } else {
        let gen_enc = gen_act.and_then(|g| {
            let (list, cursor) = match room_type {
                RoomType::Elite => (&g.elite_encounters, rs.elite_combats_this_act as usize),
                _ => (&g.normal_encounters, rs.monster_combats_this_act as usize),
            };
            if list.is_empty() {
                return None;
            }
            let id = list[cursor % list.len()];
            crate::encounters::find(id)
        });
        gen_enc.unwrap_or_else(|| {
            // Raw-core fallback: no generated list, pick within the tier pool.
            let len = i32::try_from(pool.len()).unwrap_or(1);
            let idx = rs.rng.run_manager.next_int(0, len - 1) as usize;
            pool[idx]
        })
    };

    // Record the chosen encounter's stable index (harvest-only metadata,
    // AZ Milestone 1 Task 2) — `enc` is the only place downstream of this
    // point that still remembers which `EncounterDef` was rolled; the
    // `CombatState` built below carries no such back-reference. Does not
    // affect engine behavior or determinism (excluded from `run_hash`).
    rs.current_encounter_idx = Some(crate::encounters::encounter_index(enc));

    // Roster composition rolls off the PER-ENCOUNTER `Rng`; per-monster HP
    // off `niche` — C#'s two real streams.
    // `EncounterModel.GenerateMonstersWithSlots` (`EncounterModel.cs:201`)
    // seeds the roster Rng `new Rng(Seed + TotalFloor + hash(Id.Entry))` ==
    // `Rng::with_name(Seed + TotalFloor, enc.id)` (`with_name` adds
    // `deterministic_hash_code(name)`; `Id.Entry == Slugify(className) ==
    // EncounterDef.id`); `CombatState.CreateCreature` then rolls each enemy's
    // HP off `RunState.Rng.Niche`.
    //
    // C#'s `TotalFloor == MapPointHistory.Sum(Count)` counts each **Ancient
    // event** as a room in the history, but Rust's `total_floor` only advances
    // on map-node entry (`add_visited_coord`) and Ancient events are not map
    // nodes (Neow is pre-map; the act-2/3 boss-relic Ancients are modelled as
    // `Phase::BossRelic`). So C#'s floor leads Rust's `total_floor` by the
    // number of Ancient events entered so far — which is exactly `act_index`:
    // 1 in act 1 (Neow), 2 in act 2 (Neow + the act-2 Darv/Orobas/… ancient),
    // 3 in act 3. Add `act_index` to recover C#'s floor (verified: C# floor
    // 2/3/4 == Rust total_floor 1/2/3 + 1 across the first three act-1 combats
    // — randomizer encounters like SLIMES_WEAK desync without it, bead
    // `sts2-rs-1hiht`; and act-2 BOWLBUGS_WEAK picked the wrong even-slot bug
    // — Egg vs C#'s Nectar — with a constant `+ 1`, fixed by `+ act_index`,
    // IC:1000 @289).
    //
    // The roster-parity oracle's dump (`dump_roster_for_encounter`) keeps its
    // own `+ 1` for the throwaway entered-room it never actually enters; it
    // validates roster GEN at floor 1 (Rust-fresh vs C#-fresh, both Neow-less),
    // which is independent of this live-floor selection.
    //
    // The two-pass `roll_unique_max_hps` mirrors C#'s creation-then-finalize
    // split (side-wide RAW exclusion in pass 1; even-unique Decimillipede
    // fix-up off the mixed teammate set in pass 2). Bead `sts2-rs-q6bix` /
    // `8mpo1`.
    let mut enc_rng = Rng::with_name(
        rs.rng
            .seed
            .wrapping_add(rs.total_floor + i32::from(act_index)),
        enc.id,
    );
    let roster_kinds = enc.roster(&mut enc_rng);
    let kinds: Vec<MonsterKind> = roster_kinds.iter().map(|&(k, _)| k).collect();
    let hps =
        sts2_core::monsters::roll_unique_max_hps(&kinds, &mut rs.rng.niche, rs.ascension_level);
    let roster: Vec<(MonsterKind, MonsterSpawnArgs, i32)> = roster_kinds
        .iter()
        .zip(hps)
        .map(|(&(kind, args), hp)| (kind, args, hp))
        .collect();

    // Advance the C# `RoomSet` cursor for the entered room type (pull-then-
    // increment: the pull above used the pre-increment cursor).
    match room_type {
        RoomType::Monster => rs.monster_combats_this_act += 1,
        RoomType::Elite => rs.elite_combats_this_act += 1,
        _ => {}
    }
    // Thread the entering room's type onto the combat so round-1 draw
    // modifiers (BoomingConch's elite +2) and the boss-gated hooks
    // (SwordOfStone elite-victory counter, StoneCracker boss gate) see the
    // right value — set inside `enter_combat_from_roster` BEFORE the opening
    // draw.
    let mut combat = crate::run_bridge::enter_combat_from_roster(rs, &roster, room_type);
    // GH #552: stamp the encounter's slot model so a mid-combat summon lands
    // in the turn position C# gives it (`CombatState.SortEnemiesBySlotName` +
    // the summoners' free-slot search). Inert for encounters with no `Slots`,
    // and a no-op for the initial roster — every shipped `GenerateMonsters` is
    // already slot-ascending, so this only ever informs a LATER add.
    combat.assign_encounter_slots(enc.member_slots());
    Some(combat)
}

/// Spawn `encounter_id`'s roster off the C# `niche` stream and return each
/// enemy's `(monster_id, max_hp, current_hp)` in spawn order — the
/// roster-parity oracle (M3b).
///
/// On a freshly-started run the `niche` cursor is at 0, so this isolates a
/// single combat's HP roll exactly the way the FIRST combat of a run rolls
/// it — letting `run_parity.py --roster-parity` compare against the C#
/// ground truth (`enter_room` + `get_state` on a fresh run, niche@0)
/// per-encounter, without a full lockstep replay.
///
/// Mirrors C# `CombatState.CreateCreature` →
/// `Creature.SetUniqueMonsterHpValue(RunState.Rng.Niche)`
/// (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Combat/CombatState.cs:388`):
/// HP is rolled off the run-level `niche` stream via the already-faithful
/// [`sts2_core::MonsterKind::starting_hp_unique`]. This deliberately differs
/// from [`build_combat_for_room`], which still rolls HP off the per-encounter
/// `Rng` seeded from `run_manager` (the Python pin) — swapping that live path
/// onto `niche` (and dropping the `run_manager` rolls) is the deferred M3b
/// sub-part this oracle exists to gate. Reuses
/// [`crate::run_bridge::enter_combat_from_roster`] so on-spawn buffs +
/// `finalize_spawn_max_hp` (Decimillipede) are applied, matching what C#
/// `get_state` reports.
///
/// MUTATES `rs.rng.niche` (and runs combat setup on a throwaway state) —
/// call once per fresh `RunManager`. Returns `None` for an unknown
/// encounter id.
pub fn dump_roster_for_encounter(
    rs: &mut RunState,
    encounter_id: &str,
) -> Option<Vec<(&'static str, i32, i32)>> {
    use crate::monster_dispatch::MonsterSpawnArgs;
    use sts2_core::MonsterKind;

    let enc = crate::encounters::find(encounter_id)?;
    // Roster composition (randomizer rows sub-select) rolls off the
    // PER-ENCOUNTER Rng, NOT niche. C#
    // `EncounterModel.GenerateMonstersWithSlots`
    // (`../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models/EncounterModel.cs:201`)
    // seeds it `new Rng((uint)(runState.Rng.Seed + TotalFloor + GetDeterministicHashCode(Id.Entry)))`
    // where `Id.Entry == Slugify(className) == EncounterDef.id`. That is
    // exactly `Rng::with_name(Seed + TotalFloor, id)` (with_name adds
    // `deterministic_hash_code(name)` to the base). niche is consumed ONLY
    // by the per-creature HP roll below — keeping the two streams separate
    // is what makes the randomizer rows' HP line up (rolling the roster off
    // niche shifted the HP cursor by the roster reads).
    //
    // `TotalFloor == MapPointHistory.Sum(Count)` AT generation time. The C#
    // oracle (`run_parity.py::_cs_roster`) does `start` (floor 0) then
    // `enter_room` — which pushes the room into `MapPointHistory` BEFORE
    // `CombatRoom` calls `GenerateMonstersWithSlots`, so the FIRST combat
    // generates at `TotalFloor == 1`. This throwaway dump on a fresh
    // `RunManager` never advances `rs.total_floor` (it stays 0), so we add
    // the entered-room's `+1` to match. Empirically verified: at +1 the
    // roster-parity sweep is kinds 50/50 (vs 43/50 at +0). The live
    // `build_combat_for_room` path will instead read the run's real
    // post-entry `total_floor` when step-c wires it onto this stream.
    let mut enc_rng =
        sts2_core::dotnet_rng::Rng::with_name(rs.rng.seed.wrapping_add(rs.total_floor + 1), enc.id);
    let roster_kinds = enc.roster(&mut enc_rng);
    // Per-monster HP off niche via the shared two-pass roller — C#'s
    // creation-then-finalize split (raw side-wide exclusion in pass 1, the
    // even-unique Decimillipede / Door-sentinel fix-up off the mixed
    // teammate set in pass 2). `enter_combat_from_roster` does NOT itself
    // finalize, so the roller must hand it the post-fix-up HPs. This is the
    // SAME roller the live `build_combat_for_room` path uses, so the oracle
    // gates exactly what the live run produces.
    let kinds: Vec<MonsterKind> = roster_kinds.iter().map(|&(k, _)| k).collect();
    let hps =
        sts2_core::monsters::roll_unique_max_hps(&kinds, &mut rs.rng.niche, rs.ascension_level);
    let roster: Vec<(MonsterKind, MonsterSpawnArgs, i32)> = roster_kinds
        .iter()
        .zip(hps)
        .map(|(&(kind, args), hp)| (kind, args, hp))
        .collect();
    // Roster-HP oracle dump: room type is irrelevant to the HP roll it
    // gates, so pass `Unknown` (no draw-modifier influence).
    let state = crate::run_bridge::enter_combat_from_roster(
        rs,
        &roster,
        sts2_core::runstate::RoomType::Unknown,
    );
    let out = state
        .enemy_uids
        .iter()
        .filter_map(|uid| {
            let c = state.creatures.iter().find(|c| c.id == uid.0)?;
            Some((c.species?.python_name(), c.max_hp, c.current_hp))
        })
        .collect();
    Some(out)
}

// ── starter-deck loader (sts2-rs-rkz98) ─────────────────────────────

/// Populate `rs.master_deck` with the character's starter deck (via
/// `crate::starters::starter_deck_for`) iff it is currently empty.
/// Idempotent: subsequent calls leave a populated deck alone.
///
/// `RunManager::new` doesn't load the starter deck because the deck
/// factory lives in `sts2-content` and the `content → core` dep
/// direction forbids `sts2-core` from calling into it. This bridges
/// that gap from the content side. Bead `sts2-rs-rkz98`.
pub fn populate_starter_deck(rs: &mut RunState, character: CharacterId) {
    // Idempotency / custom-deck guard: bail only if a real (non-Curse)
    // card is already present. Ascension-injected curses (AscendersBane,
    // pushed by `RunManager::with_ascension` at asc>=5 *before* this
    // runs) must NOT short-circuit population — the starter has to be
    // seeded ahead of them. Guarding on bare `is_empty()` was the
    // `sts2-rs-r36am` bug: at asc>=5 the lone AscendersBane made the
    // deck non-empty, so the 10-card starter was silently dropped and
    // every high-ascension run fought Act-1 with a 1-card deck.
    if rs
        .master_deck
        .iter()
        .any(|c| c.card_type != sts2_core::cards::CardType::Curse)
    {
        return;
    }
    let starter = crate::starters::starter_deck_for(character);
    // Insert the starter ahead of any pre-existing ascension curse so
    // the deck order is [starter.., AscendersBane] — matching the C#
    // pin's "append curse after the base deck" order
    // (sts2-rl-agent@01c094d run_state.py:1520-1532), which the shuffle
    // RNG consumes positionally.
    let curses = std::mem::take(&mut rs.master_deck);
    for spec in &starter {
        rs.master_deck
            .push(crate::cards::status::card_from_spec(spec));
    }
    rs.master_deck.extend(curses);
}

/// Top-level Ironclad run factory. Constructs a `RunManager` with all
/// callbacks wired (`ironclad_content_callbacks`) and the Ironclad
/// starter deck loaded into `run_state.master_deck`. This is the
/// canonical entry point for the `full_run_act1to3_ironclad`
/// acceptance test (ta5z8) and any future Ironclad-only runtime host.
/// Bead `sts2-rs-rkz98`.
/// Build an Ironclad `RunManager` from a C# game-style **string** seed
/// (10-char alphanumeric, e.g. `"3ACCFREXKR"` as emitted by the scraped
/// run files at `../sts2_run_scraper/data/files/the-ironclad/*.run`).
///
/// Pipeline mirrors the C# game exactly:
/// 1. `canonicalize_seed` — uppercase, fold `O→0` and `I→1`, trim. Same
///    rule as `SeedHelper.CanonicalizeSeed` in C#.
/// 2. `deterministic_hash_code` — port of
///    `StringHelper.GetDeterministicHashCode`. Returns `i32`.
/// 3. `RunManager::new(int_seed, Ironclad)` — `RunRngSet::new(int_seed)`
///    derives every per-stream RNG from the same seed via
///    `deterministic_hash_code(stream_name)`.
///
/// Match-up with the C# init at `StartRunLobby.cs:407`:
///
/// [decompiled C# omitted — see the cited path]
///
/// Bead `sts2-rs-8xlqe` (corpus replay follow-up).
#[must_use]
pub fn new_ironclad_run_manager_from_string_seed(seed: &str) -> RunManager {
    let canonical = sts2_core::canonicalize_seed(seed);
    let int_seed = sts2_core::deterministic_hash_code(&canonical);
    new_ironclad_run_manager(int_seed)
}

/// Build an Ironclad `RunManager` at the given `ascension_level`.
/// Mirrors [`new_ironclad_run_manager`] but routes through
/// [`RunManager::with_ascension`] so ascension-side scaling
/// (A3 Poverty, A4 TightBelt, A5 AscendersBane, A7 Scarcity, …) is
/// applied before the starter deck is populated. The starter deck
/// is added on top of any ascension-injected cards (e.g. the A5
/// AscendersBane curse).
#[must_use]
pub fn new_ironclad_run_manager_with_ascension(seed: i32, ascension_level: u8) -> RunManager {
    crate::cards::dispatch::ensure_installed();

    let mut rm = RunManager::with_ascension(seed, CharacterId::Ironclad, ascension_level)
        .with_content_callbacks(ironclad_content_callbacks());
    populate_starter_deck(&mut rm.run_state, CharacterId::Ironclad);
    rm
}

#[must_use]
pub fn new_ironclad_run_manager(seed: i32) -> RunManager {
    new_run_manager_for_character(seed, CharacterId::Ironclad)
}

/// Build a `RunManager` for `character` with the canonical content
/// callbacks + starter deck loaded.
///
/// `ironclad_content_callbacks` is misnamed for historical reasons —
/// its three reward-generation callbacks (`generate_card_reward_options`,
/// `generate_boss_relic_pool`, `generate_treasure_relic_pools`) all
/// dispatch internally on `rs.character_id`, so the same bundle is
/// reusable across every playable character. Combat enter/exit hooks
/// (`run_bridge::enter_combat` / `exit_combat`) are likewise
/// character-agnostic — `enter_combat` reads `rs.player_class` for the
/// post-combat heal lookup, etc. This factory keeps the constructor
/// signature consistent across characters; per-character divergence
/// (if it ever materialises) can fork a `character_content_callbacks(c)`
/// dispatch without touching the call sites.
///
/// **Starter-deck status:** Ironclad / Silent / Necrobinder / Regent
/// have proper starter-deck factories in `sts2-content`. Defect falls
/// back to the Ironclad starter deck because the Defect starter-card
/// factory isn't ported yet (see `starters::starter_deck_for` comments).
/// Callers that need a real Defect starter should file a follow-up
/// bead under `bay` (P6 content port).
///
/// Bead `sts2-rs-livck` (G5 — PyO3 character-callback wiring).
#[must_use]
pub fn new_run_manager_for_character(seed: i32, character: CharacterId) -> RunManager {
    // Bead `sts2-rs-fyp7w`: install the card-effects dispatch registry
    // before constructing the run. Without this, every Strike / Defend
    // / Bash dispatches as `EffectResult::NotImplemented` — energy and
    // hand still cycle (per Python parity), but no damage / block / debuff
    // actually applies. `ensure_installed` is idempotent.
    crate::cards::dispatch::ensure_installed();

    let mut rm =
        RunManager::new(seed, character).with_content_callbacks(ironclad_content_callbacks());
    populate_starter_deck(&mut rm.run_state, character);
    rm
}

/// Character-parameterised counterpart of
/// [`new_ironclad_run_manager_from_string_seed`]. Canonicalises the
/// C#-game-style 10-char alphanumeric seed (e.g. `"3ACCFREXKR"`) the
/// same way Ironclad does, then defers to
/// [`new_run_manager_for_character`]. Bead `sts2-rs-livck`.
#[must_use]
pub fn new_run_manager_for_character_from_string_seed(
    seed: &str,
    character: CharacterId,
) -> RunManager {
    let canonical = sts2_core::canonicalize_seed(seed);
    let int_seed = sts2_core::deterministic_hash_code(&canonical);
    new_run_manager_for_character(int_seed, character)
}

/// Character-parameterised counterpart of
/// [`new_ironclad_run_manager_with_ascension`]. Routes through
/// [`RunManager::with_ascension`] before loading the starter deck so
/// ascension-side scaling (A3/A4/A5/A7/…) is applied first. Bead
/// `sts2-rs-livck`.
#[must_use]
pub fn new_run_manager_for_character_with_ascension(
    seed: i32,
    character: CharacterId,
    ascension_level: u8,
) -> RunManager {
    crate::cards::dispatch::ensure_installed();

    let mut rm = RunManager::with_ascension(seed, character, ascension_level)
        .with_content_callbacks(ironclad_content_callbacks());
    populate_starter_deck(&mut rm.run_state, character);
    rm
}

/// Test-only Ironclad run factory that bypasses the canonical starter
/// deck and loads `deck` into `run_state.master_deck` instead.
///
/// Bead `sts2-rs-el3lx`: the `full_run_act1to3_ironclad` acceptance
/// test requires the run to reach `Phase::BossRelic`, which is gated on
/// winning a non-final boss combat. With the canonical 10-card starter
/// (5×Strike / 4×Defend / 1×Bash) plus pickup-only rewards, the
/// pick-first / hp-aware test driver cannot beat Vantom (Act-1 boss,
/// 173 HP + 18 dmg/turn + 2 spawned 21-HP adds) under any seed in a
/// 500-seed sweep. This factory lets the integration test stack a
/// stronger deck so the strict reading of the /goal ("every Phase
/// variant visited end-to-end") is satisfiable without weakening the
/// boss or adding MCTS to the test driver.
///
/// Behaviorally identical to [`new_ironclad_run_manager`] in every
/// respect other than the master-deck contents. Not used outside tests
/// at the moment but kept `pub` (not `pub(crate)`) because the
/// acceptance test lives in `sts2-cli` and crosses the crate boundary.
#[must_use]
pub fn new_ironclad_run_manager_with_deck(seed: i32, deck: Vec<CardId>) -> RunManager {
    new_ironclad_run_manager_with_deck_and_hp(seed, deck, None)
}

/// Like [`new_ironclad_run_manager_with_deck`] but also overrides
/// `run_state.max_hp` / `current_hp` to `max_hp_override` when `Some`.
/// Used by the `full_run_act1to3_ironclad` acceptance test (bead
/// `sts2-rs-el3lx`) so a non-MCTS driver with a stacked deck can
/// survive Vantom (Act-1 boss, 173 HP + 18 dmg/turn + 21-HP adds) and
/// reach `Phase::BossRelic` end-to-end.
#[must_use]
pub fn new_ironclad_run_manager_with_deck_and_hp(
    seed: i32,
    deck: Vec<CardId>,
    max_hp_override: Option<i32>,
) -> RunManager {
    crate::cards::dispatch::ensure_installed();

    let mut rm = RunManager::new(seed, CharacterId::Ironclad)
        .with_content_callbacks(ironclad_content_callbacks());
    for card_id in deck {
        let pod = crate::cards::spec_index::card_pod_from_id(card_id, false)
            .unwrap_or_else(|| panic!("no spec registered for {card_id:?}"));
        rm.run_state.master_deck.push(pod);
    }
    if let Some(hp) = max_hp_override {
        rm.run_state.max_hp = hp;
        rm.run_state.current_hp = hp;
    }
    // Seed `boss_relic_pool` so `generate_boss_relic_pool` returns a
    // non-empty pick list when the BossRelic phase fires. No Boss-rarity
    // relics are content-side at the current pin, so we use Starter
    // relics as filler — the dispatch path doesn't gate on rarity, just
    // on `run_state.player_relics.contains(id)` already-owned filtering.
    // Bead `sts2-rs-el3lx`.
    if rm.run_state.boss_relic_pool.is_empty() {
        rm.run_state.boss_relic_pool = vec![
            sts2_core::RelicId::BlackBlood,
            sts2_core::RelicId::DivineRight,
            sts2_core::RelicId::DivineDestiny,
        ];
    }
    rm
}

// ── PARTITION-B: reward-related callbacks ────────────────────────────

/// Roll a 3-card reward option list from the run's current character
/// pool. Bridges
/// [`crate::cards::spec_index::roll_card_reward_cards`] (bead
/// `sts2-rs-8qrhw`) into the `Vec<CardId>` shape that
/// [`sts2_core::run_manager::ContentCallbacks::generate_card_reward_options`]
/// expects. Drops `Card`-level upgrade / enchantment state since
/// the dispatch layer only stores `card_id` on the master deck (a
/// follow-up will upgrade the dispatch to carry the full `Card`
/// POD). Bead `sts2-rs-81xxo`.
pub fn generate_card_reward_options(rs: &mut RunState) -> Vec<CardId> {
    let character = rs.character_id;
    // Card-reward rarity odds are room-type-scoped. Mirrors C#
    // `CardCreationOptions.ForRoom` (`CardCreationOptions.cs:48-75`):
    // Monster → RegularEncounter, Elite → EliteEncounter, Boss →
    // BossEncounter (all-Rare). Without this every post-combat reward
    // rolled "regular" odds, so elite kills under-weighted rares and
    // boss kills offered mostly-common cards instead of the expected 3
    // rares (GH #25).
    let context = match rs.current_room.map(|r| r.room_type) {
        Some(RoomType::Elite) => "elite",
        Some(RoomType::Boss) => "boss",
        _ => "regular",
    };
    let opts = CardRewardGenerationOptions {
        context,
        ..CardRewardGenerationOptions::default()
    };
    let cards = crate::cards::spec_index::roll_card_reward_cards(rs, opts, character);
    // The callback signature carries bare `CardId`s; stash the rolled/
    // relic-stamped upgrade flags (SilverCrucible's
    // `TryModifyCardRewardOptionsLate`, act-2+ upgrade rolls) in the
    // RunState side channel so `RunManager::do_card_reward_pick` builds the
    // picked card with the right upgrade state.
    rs.card_reward_upgraded_flags = cards.iter().map(|c| c.upgraded).collect();
    cards.into_iter().map(|c| c.card_id).collect()
}

/// Offer up to 3 boss-relic choices from `run_state.boss_relic_pool`
/// (populated by `populate_relic_grab_bag` — bead `sts2-rs-l9vkh`).
/// Mirrors Python's BOSS_RELIC phase walk in `run_manager.py:685-694`:
/// returns the first `count` entries the player doesn't already own.
/// Pool is **not** drained here — `do_boss_relic_pick` drains on
/// pick. Bead `sts2-rs-81xxo`.
pub fn generate_boss_relic_pool(rs: &mut RunState) -> Vec<RelicId> {
    // C#-faithful: the boss relic is the NEXT act's Ancient event
    // (`Orobas`/`Pael`/`Tezcatara` act 2; `Nonupeipe`/`Tanx`/`Vakuu` act 3;
    // `Darv` shared), each offering 3 relics rolled off a per-event RNG
    // (bead `sts2-rs-40ikw`, task #19). Falls back to the flat boss pool for
    // raw-core runs that never generated rooms (no ancient routed).
    let offers = rs.roll_boss_relic_offers();
    if offers.is_empty() {
        rs.offer_boss_relic_choice(3)
    } else {
        offers
    }
}

/// Apply the `Rewards`-stream side effect of presenting the boss-relic /
/// next-act Ancient option list. C# `AncientEventModel.GenerateInitialOptions`
/// calls each offered relic's `SetupForPlayer` as the options are built; the
/// only one that touches RNG is `DustyTome.SetupForPlayer`, which rolls one
/// `PlayerRng.Rewards.NextItem(items)` over the player's unlocked Ancient-rarity
/// cards to fix the card it will add when obtained (`DustyTome.cs:55`). That
/// single read happens whenever DustyTome is *offered* (regardless of pick), so
/// the shared `Rewards` cursor advances on every Darv act-transition — a read
/// Rust previously deferred to `DustyTome::after_obtained` (and only on pick),
/// leaving the stream one short for the rest of any run that saw Darv offer but
/// not take DustyTome. The chosen card is stashed on
/// [`RunState::dusty_tome_setup_card`] for `listener_dusty_tome` to consume.
///
/// Invoked at `Phase::BossRelic` entry — after the boss reward's
/// gold/potion/card reads, before the relic pick — so the read order matches
/// C# read-for-read. Bead `sts2-rs-40ikw` follow-up (act-2/3 reward parity).
pub fn setup_boss_relic_ancient(rs: &mut RunState, offers: &[RelicId]) {
    if offers.contains(&RelicId::DustyTome) {
        // Mirror `DustyTome.SetupForPlayer` (`DustyTome.cs:52-55`): one `Rewards`
        // NextItem over the run character's OWN Ancient-rarity pool, EXCLUDING
        // `ArchaicTooth.TranscendenceCards`. `random_ancient_card_for_dusty_tome`
        // consumes exactly that read; the picked id is what `listener_dusty_tome`
        // adds on obtain. (Was `random_card_from_rarity(.., None, ..)`, which
        // unioned every character's Ancient cards and skipped the transcendence
        // filter — granting e.g. Silent's WraithForm to a Defect run.)
        let character = rs.character_id;
        rs.dusty_tome_setup_card =
            crate::cards::spec_index::random_ancient_card_for_dusty_tome(rs, character);
    }
}

/// Snapshot the run's per-rarity relic buckets into the
/// `(common, uncommon, rare)` tuple the TreasureRoom driver (bead
/// `sts2-rs-7epcg`) consumes. Reads
/// [`sts2_core::runstate::RunState::relic_grab_bag_by_rarity`]
/// (populated by `populate_relic_grab_bag_by_rarity` — bead
/// `sts2-rs-l9vkh`). Missing buckets return empty `Vec`s; the
/// treasure driver falls through the standard fallback chain on its
/// own roll. Bead `sts2-rs-81xxo`.
pub fn generate_treasure_relic_pools(
    rs: &mut RunState,
) -> (Vec<RelicId>, Vec<RelicId>, Vec<RelicId>) {
    let common = rs
        .relic_grab_bag_by_rarity
        .get(&RelicRarity::Common)
        .cloned()
        .unwrap_or_default();
    let uncommon = rs
        .relic_grab_bag_by_rarity
        .get(&RelicRarity::Uncommon)
        .cloned()
        .unwrap_or_default();
    let rare = rs
        .relic_grab_bag_by_rarity
        .get(&RelicRarity::Rare)
        .cloned()
        .unwrap_or_default();
    (common, uncommon, rare)
}

/// Roll a full, stocked merchant inventory. Delegates to the C#-faithful
/// [`crate::cards::spec_index::generate_merchant_inventory`], which walks
/// `MerchantInventory.CreateForNormalMerchant`'s exact two-stream
/// (`Shops` + `Rewards`) consumption order. Card / colorless / potion
/// identities and prices are seed-exact to the headless C# `dump_shop`
/// oracle; relic slots consume their RNG reads to keep the streams
/// aligned but leave `relic_id = None` pending the per-relic
/// `MerchantCost` / `IsAllowedInShops` / back-of-bag data port.
///
/// Previously (bead `sts2-rs-64mq2`) this filled slots off the `rewards`
/// stream with the Python-pin single-stream roller, which stocked a
/// buyable shop but was not seed-exact. Replaced for M4 whole-run RNG
/// parity (`run_parity.py --shop-parity`).
pub fn generate_shop_inventory(rs: &mut RunState) -> sts2_core::shop::ShopInventory {
    let character = rs.character_id;
    crate::cards::spec_index::generate_merchant_inventory(rs, character)
}

/// Default relic grab-bag pools for an Ironclad run. Back-compat shim
/// over [`default_relic_grab_bag_for`]; new callers that know the run's
/// character should call that directly. See it for the full contract.
#[must_use]
pub fn default_relic_grab_bag() -> (
    std::collections::BTreeMap<RelicRarity, Vec<RelicId>>,
    Vec<RelicId>,
    Vec<RelicId>,
) {
    default_relic_grab_bag_for(CharacterId::Ironclad)
}

/// C#-faithful relic grab-bag pools for `character`, as
/// `(by_rarity, fallback, boss_pool)` ready for
/// [`sts2_core::runstate::RunState::populate_relic_grab_bag_by_rarity`].
///
/// The per-rarity buckets mirror `RelicGrabBag.Populate`: `SharedRelicPool`
/// relics of each grab-bag rarity followed by `character`'s own pool relics
/// of that rarity, both in C# declaration order (see
/// [`crate::relics::grab_bag`]). This is the *pre-shuffle* deque order; the
/// caller shuffles each bucket with the run RNG in C# Dictionary insertion
/// order (Uncommon → Common → Rare → Shop). Getting composition *and* order
/// right is what makes the workbench's seeded `SmallCapsule` roll match the
/// C# walker (bead `sts2-rs-7e04v`, follow-up to `sts2-rs-mcesz` / GH #70).
///
/// The boss pool stays the mixed shop+boss pool
/// ([`crate::relics::shop_event`]); C#-exact boss-pool composition is out of
/// scope here. Already-owned relics are filtered at draw time by
/// `pop_relic_from_grab_bag`, so full pools are returned as-is. Bead
/// `sts2-rs-64mq2` (original non-empty-pool seed), `sts2-rs-7e04v`
/// (character-aware C# composition).
#[must_use]
pub fn default_relic_grab_bag_for(
    character: CharacterId,
) -> (
    std::collections::BTreeMap<RelicRarity, Vec<RelicId>>,
    Vec<RelicId>,
    Vec<RelicId>,
) {
    let by_rarity = crate::relics::grab_bag::grab_bag_buckets(character);
    let boss = crate::relics::shop_event::pool_relic_ids();
    (by_rarity, Vec::new(), boss)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sts2_core::potions::CharacterId;
    use sts2_core::run_manager::RunManager;

    /// An Ironclad run in a won fight of `room`, holding `relic` with the
    /// combat-side `counter` the fight left it at (the combat bus has run its
    /// own `AfterCombatVictory`), resolved through the deployed exit path.
    fn won_fight_with(
        relic: sts2_core::RelicId,
        kind: sts2_core::RelicKind,
        counter: i32,
        elite: bool,
    ) -> RunManager {
        use sts2_core::runstate::RoomType;
        crate::relics::dispatch::ensure_installed();
        let mut rm = new_ironclad_run_manager(0);
        rm.run_state.relics.push(relic);
        rm.run_state.relic_counters.insert(relic, counter + 1);
        let mut combat = sts2_core::CombatState::default();
        combat.player.current_hp = 80;
        combat.player.max_hp = 80;
        combat.is_elite = elite;
        combat.relics.push(sts2_core::Relic {
            id: std::num::NonZeroU32::new(1).expect("non-zero"),
            kind,
            counter,
            is_wax: false,
            is_melted: false,
        });
        let room = if elite {
            RoomType::Elite
        } else {
            RoomType::Monster
        };
        rm.debug_enter_combat(combat, room);
        let _ = rm.resolve_combat_fast(0.0, true);
        rm
    }

    /// C# `SwordOfStone.AfterCombatVictory` (`SwordOfStone.cs:40-51`): the
    /// fifth elite replaces it with Sword of Jade (`RelicCmd.Replace`). The
    /// combat side counts the elites down; the run side now fires its
    /// victory channel at the combat's exit and replaces the relic once the
    /// flushed count reaches 0 (S2's rest: the engine never did).
    #[test]
    fn sword_of_stone_becomes_sword_of_jade_after_its_fifth_elite() {
        use sts2_core::{RelicId, RelicKind};
        let rm = won_fight_with(RelicId::SwordOfStone, RelicKind::SwordOfStone, 0, true);
        assert!(rm.run_state.relics.contains(&RelicId::SwordOfJade));
        assert!(!rm.run_state.relics.contains(&RelicId::SwordOfStone));
        // Four elites in: still Sword of Stone, its count kept.
        let rm = won_fight_with(RelicId::SwordOfStone, RelicKind::SwordOfStone, 1, true);
        assert!(rm.run_state.relics.contains(&RelicId::SwordOfStone));
        assert_eq!(
            rm.run_state.relic_counters.get(&RelicId::SwordOfStone),
            Some(&1)
        );
    }

    /// War Hammer (`WarHammer.cs:19-31`) is registered on the same channel,
    /// but its fix isn't in the approved fork batch: the combat exit doesn't
    /// fire it, so an elite victory upgrades nothing and draws nothing from
    /// Niche, as before the channel fired. Once approved, it joins
    /// `LIVE_COMBAT_VICTORY_RUN_LISTENERS` and this test turns around.
    #[test]
    fn war_hammer_stays_inactive_until_approved() {
        use sts2_core::{RelicId, RelicKind};
        let rm = won_fight_with(RelicId::WarHammer, RelicKind::WarHammer, 0, false);
        let niche_before = rm.run_state.rng.niche.counter();
        let rm = won_fight_with(RelicId::WarHammer, RelicKind::WarHammer, 0, true);
        assert_eq!(
            rm.run_state
                .master_deck
                .iter()
                .filter(|c| c.upgraded)
                .count(),
            0
        );
        assert_eq!(
            rm.run_state.rng.niche.counter(),
            niche_before,
            "no Niche draw"
        );
    }

    /// Guardrail #4 (Addendum 23): the shared run setup that EVERY full-run
    /// driver must call. The bare factory leaves the relic/room pools empty;
    /// `initialize_run_setup` must stock them all — the exact omission that
    /// silently crippled M0–M5 (relic-less + static-event harness).
    #[test]
    fn initialize_run_setup_stocks_pools_and_generates_rooms() {
        let mut rm = new_run_manager_for_character(42, CharacterId::Ironclad).with_neow_start();
        // Precondition: the bare factory + neow start leaves the grab bag empty.
        assert!(
            rm.run_state
                .relic_grab_bag_by_rarity
                .values()
                .all(Vec::is_empty),
            "bare factory must leave the player grab bag empty (init is opt-in)"
        );

        initialize_run_setup(&mut rm);

        let rs = &rm.run_state;
        assert!(
            rs.relic_grab_bag_by_rarity.values().any(|b| !b.is_empty()),
            "player relic grab bag stocked"
        );
        assert!(
            rs.shared_relic_grab_bag_by_rarity
                .values()
                .any(|b| !b.is_empty()),
            "shared relic grab bag stocked (treasure chest pulls from it)"
        );
        assert!(!rs.boss_relic_pool.is_empty(), "boss relic pool stocked");
        assert_eq!(rs.generated_rooms.len(), 3, "all three acts generated");
        for (i, g) in rs.generated_rooms.iter().enumerate() {
            assert!(
                !g.events_shuffled.is_empty(),
                "act {i} event pool shuffled (no static-order fallback)"
            );
            assert!(!g.boss_id.is_empty(), "act {i} boss rolled");
        }
        // Task #19: every act has an Ancient routed (act 1 = Neow; acts 2/3
        // one of their pool + the shared Darv subset).
        use sts2_core::run_room_gen::AncientId;
        assert_eq!(rs.generated_rooms[0].ancient, Some(AncientId::Neow));
        for i in [1usize, 2] {
            let a = rs.generated_rooms[i].ancient.expect("act ancient routed");
            let valid = match i {
                1 => [
                    AncientId::Orobas,
                    AncientId::Pael,
                    AncientId::Tezcatara,
                    AncientId::Darv,
                ]
                .contains(&a),
                _ => [
                    AncientId::Nonupeipe,
                    AncientId::Tanx,
                    AncientId::Vakuu,
                    AncientId::Darv,
                ]
                .contains(&a),
            };
            assert!(valid, "act {i} ancient {a:?} not in its pool");
        }
    }

    /// Bead `sts2-rs-RP16`: the combat gold-reward path credits the
    /// `modify_rewards` relic gold bonus. C# `AmethystAubergine.TryModifyRewards`
    /// adds a fixed `GoldReward(15)` per Monster/Elite; the headless auto-collects
    /// it. Base Monster gold is `[10, 20]`, so with the relic the player gains
    /// `[25, 35]`; without it only `[10, 20]` — the disjoint ranges prove the
    /// +15 is credited.
    #[test]
    fn apply_combat_gold_credits_amethyst_aubergine_bonus() {
        crate::relics::dispatch::ensure_installed();
        let mut rm = new_run_manager_for_character(42, CharacterId::Ironclad).with_neow_start();
        initialize_run_setup(&mut rm);
        let rs = &mut rm.run_state;
        rs.last_combat_gold_proportion = 1.0;

        // Control: no AmethystAubergine ⇒ base Monster gold only.
        rs.gold = 0;
        apply_combat_gold_and_potion(rs, RoomType::Monster);
        let base_only = rs.gold;
        assert!(
            (10..=20).contains(&base_only),
            "base Monster gold in [10,20], got {base_only}"
        );

        // With the relic ⇒ base + 15.
        rs.relics.push(RelicId::AmethystAubergine);
        rs.gold = 0;
        apply_combat_gold_and_potion(rs, RoomType::Monster);
        assert!(
            (25..=35).contains(&rs.gold),
            "base+15 bonus in [25,35], got {}",
            rs.gold
        );
    }

    /// Task #19 end-to-end: `generate_boss_relic_pool` returns the NEXT
    /// act's Ancient-event offers (3 distinct relics) for an initialized
    /// run, not the flat shop_event stand-in. Bead `sts2-rs-40ikw`.
    #[test]
    fn boss_relic_pool_routes_through_next_act_ancient() {
        use sts2_core::run_room_gen::AncientId;
        let mut rm = new_run_manager_for_character(42, CharacterId::Ironclad).with_neow_start();
        initialize_run_setup(&mut rm);
        // Standing in act 1 (act_index = 1), the boss relic is act 2's Ancient.
        rm.run_state.act_index = 1;
        let act2_ancient = rm.run_state.generated_rooms[1].ancient.unwrap();
        assert_ne!(act2_ancient, AncientId::Neow);
        let offers = generate_boss_relic_pool(&mut rm.run_state);
        assert_eq!(offers.len(), 3, "boss relic offers 3 from {act2_ancient:?}");
        assert!(
            offers[0] != offers[1] && offers[1] != offers[2] && offers[0] != offers[2],
            "distinct offers, got {offers:?}"
        );
        // Deterministic for the same state.
        let again = generate_boss_relic_pool(&mut rm.run_state);
        assert_eq!(offers, again, "offer roll deterministic");
    }

    /// Same seed → byte-identical setup. Guards against a non-deterministic
    /// init creeping in (e.g. a wall-clock / process-RNG shuffle like the
    /// stopgap `stock_relic_pools` used before unification).
    #[test]
    fn initialize_run_setup_is_deterministic_for_a_seed() {
        let build = || {
            let mut rm = new_run_manager_for_character(7, CharacterId::Ironclad).with_neow_start();
            initialize_run_setup(&mut rm);
            rm
        };
        let a = build();
        let b = build();
        let bosses = |rm: &RunManager| -> Vec<&'static str> {
            rm.run_state
                .generated_rooms
                .iter()
                .map(|g| g.boss_id)
                .collect()
        };
        assert_eq!(bosses(&a), bosses(&b), "boss roll is seed-deterministic");
        assert_eq!(
            a.run_state.generated_rooms[0].events_shuffled,
            b.run_state.generated_rooms[0].events_shuffled,
            "event shuffle is seed-deterministic"
        );
        assert_eq!(
            a.run_state.relic_grab_bag_by_rarity, b.run_state.relic_grab_bag_by_rarity,
            "grab-bag order is seed-deterministic"
        );
    }

    #[test]
    fn factory_wires_three_combat_callbacks() {
        let cb = ironclad_content_callbacks();
        assert!(cb.enter_combat.is_some(), "enter_combat wired");
        assert!(cb.exit_combat.is_some(), "exit_combat wired");
        assert!(cb.select_encounter.is_some(), "select_encounter wired");
    }

    #[test]
    fn factory_wires_partition_b_reward_callbacks() {
        let cb = ironclad_content_callbacks();
        assert!(cb.generate_card_reward_options.is_some());
        assert!(cb.generate_boss_relic_pool.is_some());
        assert!(cb.generate_treasure_relic_pools.is_some());
    }

    #[test]
    fn resolve_event_slot_wired_to_dispatch_callback() {
        // Bead sts2-rs-nzdz1: both event-related callbacks must be
        // wired — `resolve_event_at` (multi-option idx-aware resolver)
        // and `event_option_count` (used by `get_available_actions`
        // to enumerate one EventChoice per option).
        let cb = ironclad_content_callbacks();
        assert!(cb.resolve_event_at.is_some());
        assert!(cb.event_option_count.is_some());
    }

    #[test]
    fn resolve_event_at_callback_drives_event_phase_to_resolution() {
        // End-to-end smoke: a fresh RunState with empty visited_event_ids
        // gets driven through pick→enter→resolve; current_event must be
        // cleared (finished-outcome path) and exactly one event recorded
        // as visited. Mirrors the dispatch-side test but routed through
        // the factory wiring so a future re-route gets caught here too.
        let cb = ironclad_content_callbacks();
        let resolve = cb.resolve_event_at.expect("event resolver wired");
        let mut rs = RunState::new();
        let before = rs.visited_event_ids.len();
        assert!(resolve(&mut rs, 0));
        assert!(rs.current_event.is_none());
        assert_eq!(rs.visited_event_ids.len(), before + 1);
    }

    #[test]
    fn neow_with_full_callbacks_enumerates_three_choices() {
        // Bead sts2-rs-nzdz1 acceptance: with the Ironclad callback
        // bundle wired (which provides `event_option_count`), a
        // RunManager started via `with_neow_start()` exposes one
        // EventChoice per Neow option (positive_1 / positive_2 /
        // cursed).
        use sts2_core::run_manager::{RunAction, RunManager};
        let rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(ironclad_content_callbacks())
            .with_neow_start();
        let actions = rm.get_available_actions();
        assert_eq!(
            actions,
            vec![
                RunAction::EventChoice { idx: 0 },
                RunAction::EventChoice { idx: 1 },
                RunAction::EventChoice { idx: 2 },
            ]
        );
    }

    #[test]
    fn neow_choice_at_idx_2_offers_cursed_relic_reward() {
        // Bead sts2-rs-nzdz1 acceptance: resolving Neow at idx=2 must
        // route through the cursed-option handler, which queues the
        // rolled cursed relic onto `state.relic_rewards` (the actual
        // grant happens later when the player picks the reward at the
        // CardReward phase — wiring that into Phase::CardReward post-
        // event is the next gap, but the multi-option idx plumbing is
        // verified here by observing that the idx=2 handler ran, not
        // idx=0/1.
        use sts2_core::run_manager::{ActionResult, Phase, RunAction, RunManager};
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(ironclad_content_callbacks())
            .with_neow_start();
        let cursed = rm
            .run_state
            .neow_offers
            .as_ref()
            .expect("neow_start populated offers")
            .cursed;
        let positive_1 = rm.run_state.neow_offers.as_ref().unwrap().positive_1;
        let before = rm.run_state.relic_rewards.len();
        let result = rm.take_action(RunAction::EventChoice { idx: 2 });
        assert_eq!(result, ActionResult::Advanced);
        // Bead sts2-rs-5pjsc: with the cursed relic queued onto
        // relic_rewards, the engine now routes to Phase::CardReward
        // (relic-reward sub-state) instead of straight to MapChoice
        // so the agent can PickRelicReward / SkipRelic to drain it.
        assert_eq!(rm.phase(), Phase::CardReward);
        assert_eq!(
            rm.run_state.relic_rewards.len(),
            before + 1,
            "idx=2 must queue exactly one relic reward",
        );
        let queued = rm
            .run_state
            .relic_rewards
            .last()
            .expect("just queued")
            .relic_id;
        // The cursed handler routes through `resolve_neow_relic`,
        // which maps the rolled `NeowRelic` to a `RelicId` (or a
        // placeholder for the 5-relic gap). What matters for the idx-
        // routing assertion is that the queued id is the CURSED one,
        // not positive_1's id.
        let cursed_id = crate::events::act3_glory::resolve_neow_relic(cursed);
        let positive_1_id = crate::events::act3_glory::resolve_neow_relic(positive_1);
        assert_eq!(
            queued, cursed_id,
            "idx=2 must use cursed slot, not positive_1"
        );
        if cursed_id != positive_1_id {
            assert_ne!(queued, positive_1_id);
        }
    }

    #[test]
    fn neow_choice_drains_through_card_reward_to_owned_relic() {
        // Bead sts2-rs-5pjsc: end-to-end Neow → relic in inventory.
        // Verifies the full path: EventChoice{idx:2} → cursed handler
        // queues relic onto relic_rewards → engine routes to
        // Phase::CardReward → PickRelicReward drains queue → relic
        // lands in state.relics → advance_after_card_reward returns
        // the run to Phase::MapChoice.
        use sts2_core::run_manager::{ActionResult, Phase, RunAction, RunManager};
        // Install relic `after_obtained` listeners (production RunManager
        // construction does this) so the test is deterministic regardless of
        // suite order, and force the cursed offer to a relic with NO
        // reward-queuing `after_obtained`. The seed-0 cursed roll
        // (HeftyTablet / NeowsBones) queues an extra reward once the registry
        // is installed by a sibling test, which left this test passing only
        // in isolation. Bead `sts2-rs-ll6p9`.
        crate::relics::dispatch::ensure_installed();
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(ironclad_content_callbacks())
            .with_neow_start();
        {
            let mut offers = rm.run_state.neow_offers.expect("neow offers rolled");
            offers.cursed = sts2_core::NeowRelic::CursedPearl;
            rm.run_state.neow_offers = Some(offers);
        }
        let cursed_id =
            crate::events::act3_glory::resolve_neow_relic(rm.run_state.neow_offers.unwrap().cursed);
        let before_relic_count = rm.run_state.relics.len();

        // Step 1: pick the cursed Neow option.
        assert_eq!(
            rm.take_action(RunAction::EventChoice { idx: 2 }),
            ActionResult::Advanced
        );
        assert_eq!(rm.phase(), Phase::CardReward);
        assert_eq!(
            rm.get_available_actions(),
            vec![RunAction::PickRelicReward, RunAction::SkipRelic]
        );

        // Step 2: accept the queued relic.
        assert_eq!(
            rm.take_action(RunAction::PickRelicReward),
            ActionResult::Advanced
        );
        assert_eq!(rm.phase(), Phase::MapChoice);
        assert_eq!(rm.run_state.relics.len(), before_relic_count + 1);
        assert!(rm.run_state.relics.contains(&cursed_id));
        assert!(rm.run_state.relic_rewards.is_empty());
    }

    #[test]
    fn shop_inventory_is_stocked_with_cards_and_potions() {
        // Bead sts2-rs-64mq2: the content roller fills the payload-less
        // slots core rolls, so the shop is buyable. Cards + potions stock
        // from the character/potion pools (no grab bag needed); relic
        // slots fill only when `relic_grab_bag_by_rarity` is populated
        // (a separate run-init step) — they degrade to empty here.
        let mut rm = new_run_manager_for_character(0, CharacterId::Ironclad);
        let inv = generate_shop_inventory(&mut rm.run_state);

        assert_eq!(inv.cards.len(), 5, "five character-card slots");
        assert!(
            inv.cards.iter().all(|c| c.card.is_some()),
            "every character-card slot is stocked"
        );
        assert!(
            inv.colorless_cards.iter().all(|c| c.card.is_some()),
            "every colorless-card slot is stocked"
        );
        assert!(
            inv.potions.iter().all(|p| p.potion_kind.is_some()),
            "every potion slot is stocked"
        );
        assert!(inv.cards.iter().all(|c| c.price > 0), "cards priced");
        assert!(
            !inv.removal_used && inv.removal_cost > 0,
            "removal slot live"
        );
    }

    #[test]
    fn card_reward_options_use_boss_context_for_boss_rooms() {
        // GH #25: a Boss-room card reward must roll boss-context rarity
        // odds (common=0 / uncommon=0 / rare=1), so every offered card is
        // Rare. Mirrors C# `CardCreationOptions.ForRoom(Boss) =>
        // BossEncounter`. Boss odds ignore the pity counter and are
        // deterministic in rarity, so this holds for any RNG seed.
        use crate::cards::spec_index::rarity_for_card_id;
        use crate::cards::types::CardRarity;
        use sts2_core::runstate::{Room, RoomType, RunState};
        let mut rs = RunState::new();
        rs.character_id = CharacterId::Ironclad;
        rs.current_room = Some(Room::new(RoomType::Boss));
        let opts = generate_card_reward_options(&mut rs);
        assert_eq!(opts.len(), 3, "boss reward still offers 3 cards");
        for cid in opts {
            assert_eq!(
                rarity_for_card_id(cid),
                Some(CardRarity::Rare),
                "boss-context reward card {cid:?} should be Rare",
            );
        }
    }

    #[test]
    fn card_reward_options_use_regular_context_for_monster_rooms() {
        // The Monster-room path keeps regular-context odds — a sanity
        // counterweight to the boss test so a future regression that
        // hard-codes "boss" everywhere is caught. Over many monster
        // rewards at least one non-Rare card must appear (regular rare
        // odds are ~3-4%).
        use crate::cards::spec_index::rarity_for_card_id;
        use crate::cards::types::CardRarity;
        use sts2_core::runstate::{Room, RoomType, RunState};
        let mut rs = RunState::new();
        rs.character_id = CharacterId::Ironclad;
        rs.current_room = Some(Room::new(RoomType::Monster));
        let mut saw_non_rare = false;
        for _ in 0..40 {
            for cid in generate_card_reward_options(&mut rs) {
                if rarity_for_card_id(cid) != Some(CardRarity::Rare) {
                    saw_non_rare = true;
                }
            }
        }
        assert!(
            saw_non_rare,
            "monster-context rewards should not be all-Rare",
        );
    }

    #[test]
    fn default_relic_grab_bag_has_nonempty_reward_tiers() {
        use sts2_core::runstate::RelicRarity;
        let (by_rarity, _fallback, boss) = default_relic_grab_bag();
        for tier in [
            RelicRarity::Common,
            RelicRarity::Uncommon,
            RelicRarity::Rare,
        ] {
            assert!(
                by_rarity.get(&tier).is_some_and(|v| !v.is_empty()),
                "{tier:?} relic pool should be non-empty"
            );
        }
        assert!(!boss.is_empty(), "boss relic pool should be non-empty");
    }

    #[test]
    fn neow_direct_grant_skips_the_relic_reward_step() {
        // With `direct_event_relic_grant` set (the workbench), choosing a
        // Neow blessing grants its relic immediately — no queued
        // relic-reward, no `PickRelicReward` confirmation — matching C#.
        use sts2_core::run_manager::{ActionResult, Phase, RunAction, RunManager};
        // Deterministic + order-independent: install the relic registry
        // (matches production) and force the cursed offer to a relic with no
        // reward-queuing `after_obtained`. Bead `sts2-rs-ll6p9`.
        crate::relics::dispatch::ensure_installed();
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(ironclad_content_callbacks())
            .with_neow_start();
        rm.run_state.direct_event_relic_grant = true;
        {
            let mut offers = rm.run_state.neow_offers.expect("neow offers rolled");
            offers.cursed = sts2_core::NeowRelic::CursedPearl;
            rm.run_state.neow_offers = Some(offers);
        }
        let cursed_id =
            crate::events::act3_glory::resolve_neow_relic(rm.run_state.neow_offers.unwrap().cursed);
        let before = rm.run_state.relics.len();

        assert_eq!(
            rm.take_action(RunAction::EventChoice { idx: 2 }),
            ActionResult::Advanced
        );
        // Relic is owned right away; no relic-reward queued, no CardReward
        // pick-relic detour.
        assert!(rm.run_state.relics.contains(&cursed_id));
        assert_eq!(rm.run_state.relics.len(), before + 1);
        assert!(rm.run_state.relic_rewards.is_empty());
        assert_ne!(rm.phase(), Phase::CardReward);
        assert!(!rm
            .get_available_actions()
            .iter()
            .any(|a| matches!(a, RunAction::PickRelicReward)));
    }

    #[test]
    fn neow_lost_coffer_grants_card_reward_and_procures_potion() {
        // Bead `sts2-rs-01o`: the reported bug — picking Lost Coffer from
        // Neow promises "1 card reward + procure 1 potion" but granted
        // neither, because its `after_obtained` listener pushed both onto
        // the write-only `pending_rewards` queue. Now the run loop drains
        // it: a card-reward screen opens and a potion lands in the belt.
        use sts2_core::run_manager::{ActionResult, Phase, RunAction, RunManager};
        use sts2_core::NeowRelic;
        // Register relic `after_obtained` listeners (the py layer does
        // this at RunManager construction). Without it Lost Coffer's
        // listener never fires.
        crate::relics::dispatch::ensure_installed();
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(ironclad_content_callbacks())
            .with_neow_start();
        rm.run_state.direct_event_relic_grant = true;
        // Force the positive_2 offer to Lost Coffer regardless of seed.
        let mut offers = rm.run_state.neow_offers.expect("neow offers rolled");
        offers.positive_2 = NeowRelic::LostCoffer;
        rm.run_state.neow_offers = Some(offers);

        // idx 1 == positive_2 (idx 0/1/2 == positive_1/positive_2/cursed).
        assert_eq!(
            rm.take_action(RunAction::EventChoice { idx: 1 }),
            ActionResult::Advanced
        );

        assert!(
            rm.run_state.relics.contains(&RelicId::LostCoffer),
            "Lost Coffer obtained"
        );
        assert_eq!(
            rm.phase(),
            Phase::CardReward,
            "the promised card reward surfaces instead of dropping to the map"
        );
        assert!(
            rm.get_available_actions()
                .iter()
                .any(|a| matches!(a, RunAction::PickCardReward { .. })),
            "a card-reward pick is offered"
        );
        assert_eq!(
            rm.run_state.potions.len(),
            1,
            "the promised potion is procured into the run belt"
        );
    }

    #[test]
    fn neow_small_capsule_chains_follow_up_reward_directly_under_workbench_flag() {
        // Bead `sts2-rs-l0wbw` (GH #70): picking SmallCapsule from Neow
        // fires `listener_small_capsule` (shop_event.rs:2092), which
        // queues a free random Common via `offer_random_relic_rewards`.
        // C# `RewardsSet.Offer()` under `TestMode.IsOn`
        // (`MegaCrit.Sts2.Core.Rewards/RewardsSet.cs:125-136`) auto-resolves
        // every reward in the set — no UI, no "Take / Skip" decision.
        // Under `direct_event_relic_grant` (the workbench's mode), Rust
        // must match: drain the follow-up reward directly via
        // `obtain_relic` so the run lands at `Phase::MapChoice` with
        // BOTH SmallCapsule and its rolled follow-up already in
        // `relics`, with no phantom `PickRelicReward / SkipRelic`
        // surfaced.
        use sts2_core::run_manager::{ActionResult, Phase, RunAction, RunManager};
        use sts2_core::NeowRelic;
        crate::relics::dispatch::ensure_installed();
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(ironclad_content_callbacks())
            .with_neow_start();
        rm.run_state.direct_event_relic_grant = true;
        // SmallCapsule's after-obtain rolls from the per-rarity grab bag;
        // populate it (production path is `RunManager::initialize_run`).
        let (by_rarity, fallback, boss) = default_relic_grab_bag();
        rm.run_state
            .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
        // Force positive_1 to SmallCapsule regardless of seed roll so
        // the after-obtain chain is exercised deterministically.
        let mut offers = rm.run_state.neow_offers.expect("neow offers rolled");
        offers.positive_1 = NeowRelic::SmallCapsule;
        rm.run_state.neow_offers = Some(offers);
        let before_relics = rm.run_state.relics.len();

        assert_eq!(
            rm.take_action(RunAction::EventChoice { idx: 0 }),
            ActionResult::Advanced
        );

        // SmallCapsule itself owned (direct grant).
        assert!(
            rm.run_state.relics.contains(&RelicId::SmallCapsule),
            "SmallCapsule directly granted"
        );
        // Follow-up Common drained inline — relics gained TWO entries
        // (SmallCapsule + 1 rolled), and the relic-reward queue is empty.
        assert_eq!(
            rm.run_state.relics.len(),
            before_relics + 2,
            "SmallCapsule + follow-up Common both granted",
        );
        assert!(
            rm.run_state.relic_rewards.is_empty(),
            "follow-up reward auto-drained, not left for a PickRelicReward"
        );
        // Phase advanced past CardReward straight to MapChoice — no
        // phantom Take/Skip decision exposed to the agent.
        assert_eq!(
            rm.phase(),
            Phase::MapChoice,
            "no PickRelicReward detour; matches C# headless flow"
        );
        assert!(
            !rm.get_available_actions()
                .iter()
                .any(|a| matches!(a, RunAction::PickRelicReward | RunAction::SkipRelic)),
            "no PickRelicReward / SkipRelic available"
        );
    }

    #[test]
    fn neows_bones_deferred_relic_grant_drains_chained_reward_and_lands_curse() {
        // Parity `deck_size_minus1_step2` (SEED A5P41 Ironclad A5): NeowsBones
        // grants relic#1 = LostCoffer (whose `AfterObtained` surfaces a card
        // pick), so relic#2 = SmallCapsule is deferred onto
        // `pending_post_pick_relic_grants` and the 1 curse onto
        // `pending_neows_bones_curses` — faithful to `NeowsBones.cs:40-71`,
        // which adds the `Niche` curse only AFTER the full `RewardsSet.Offer()`
        // (including SmallCapsule's nested `OfferCustom(new RelicReward)`)
        // returns.
        //
        // The bug: after the LostCoffer card pick resolves,
        // `drain_post_pick_relic_grants_if_settled` granted SmallCapsule, whose
        // `listener_small_capsule` pushed a fresh PotionBelt RelicReward onto
        // `relic_rewards` (push-only, NOT auto-applied). That path had no
        // follow-up drain, so PotionBelt lingered, `grant_batch_settled()`
        // stayed false forever, and the immediately following
        // `drain_post_pick_deck_cards_if_settled` could NEVER roll the deferred
        // curse — stranding BOTH the relic and the curse (deck off-by-one).
        //
        // The fix mirrors `RewardsSet.Offer()` under `TestMode.IsOn`
        // (`MegaCrit.Sts2.Core.Rewards/RewardsSet.cs:125-136`): under
        // `direct_event_relic_grant`, auto-collect the chained reward inline so
        // `relic_rewards` empties, the batch settles, and the curse lands.
        use sts2_core::run_manager::{ActionResult, Phase, RunAction, RunManager};
        use sts2_core::NeowRelic;
        crate::relics::dispatch::ensure_installed();
        let mut rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(ironclad_content_callbacks())
            .with_neow_start();
        rm.run_state.direct_event_relic_grant = true;
        // SmallCapsule's after-obtain rolls from the per-rarity grab bag.
        let (by_rarity, fallback, boss) = default_relic_grab_bag();
        rm.run_state
            .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);

        // Grant relic#1 = LostCoffer through the real Neow event so its
        // `AfterObtained` surfaces the card pick via the run loop (exactly
        // like NeowsBones' first deferred grant). Force the offer regardless
        // of seed.
        let mut offers = rm.run_state.neow_offers.expect("neow offers rolled");
        offers.positive_2 = NeowRelic::LostCoffer;
        rm.run_state.neow_offers = Some(offers);
        // idx 1 == positive_2.
        assert_eq!(
            rm.take_action(RunAction::EventChoice { idx: 1 }),
            ActionResult::Advanced
        );
        assert_eq!(
            rm.phase(),
            Phase::CardReward,
            "LostCoffer's card reward is open"
        );
        assert!(
            rm.run_state.relics.contains(&RelicId::LostCoffer),
            "LostCoffer obtained"
        );

        // Now reproduce NeowsBones' deferred state: relic#2 = SmallCapsule is
        // queued onto `pending_post_pick_relic_grants` and the 1 curse onto
        // `pending_neows_bones_curses` while the LostCoffer pick is still open
        // (apply_neows_bones, neow_relics.rs:248-288).
        rm.run_state
            .pending_post_pick_relic_grants
            .push(RelicId::SmallCapsule);
        rm.run_state.pending_neows_bones_curses = 1;

        // Deck size right before the pick: resolving it adds the picked card
        // AND, once the deferred grant batch settles, the deferred curse.
        let deck_before_pick = rm.run_state.master_deck.len();
        let relics_before_pick = rm.run_state.relics.len();

        // Resolve the LostCoffer card pick — this is the settle point that
        // drains the deferred SmallCapsule grant.
        let pick = rm
            .get_available_actions()
            .into_iter()
            .find_map(|a| match a {
                RunAction::PickCardReward { idx: Some(i) } => Some(i),
                _ => None,
            })
            .expect("a card-reward option to pick");
        assert_eq!(
            rm.take_action(RunAction::PickCardReward { idx: Some(pick) }),
            ActionResult::Advanced
        );

        // SmallCapsule was granted from the deferred queue.
        assert!(
            rm.run_state.relics.contains(&RelicId::SmallCapsule),
            "deferred SmallCapsule granted after the card pick settled"
        );
        // Its chained reward (PotionBelt) auto-drained inline — relic_rewards
        // is empty, so `grant_batch_settled()` can become true.
        assert!(
            rm.run_state.relic_rewards.is_empty(),
            "SmallCapsule's chained relic reward auto-collected, not stranded \
             on relic_rewards (the bug)"
        );
        // SmallCapsule's after-obtain chains exactly one random Common
        // (PotionBelt for SEED A5P41) which is auto-collected inline — so the
        // relic count grew by TWO (SmallCapsule + its chained reward), not one.
        assert_eq!(
            rm.run_state.relics.len(),
            relics_before_pick + 2,
            "SmallCapsule + its single chained Common both obtained (C# \
             TestMode auto-Offer), not just SmallCapsule with the chain \
             stranded on relic_rewards (the bug)"
        );
        // The deferred grant queue is empty and the curse rolled + landed.
        assert!(
            rm.run_state.pending_post_pick_relic_grants.is_empty(),
            "no relic grants left deferred"
        );
        assert_eq!(
            rm.run_state.pending_neows_bones_curses, 0,
            "the deferred NeowsBones curse was rolled, not stranded"
        );
        assert_eq!(
            rm.run_state.master_deck.len(),
            deck_before_pick + 2,
            "picked card + the deferred curse both land (no off-by-one strand)"
        );
    }

    #[test]
    fn card_reward_returns_three_cards_for_ironclad() {
        let mut rs = RunState::new();
        rs.character_id = CharacterId::Ironclad;
        let cards = generate_card_reward_options(&mut rs);
        assert_eq!(cards.len(), 3);
    }

    #[test]
    fn card_reward_stays_in_ironclad_plus_colorless_pool() {
        use crate::cards::spec_index::{COLORLESS_SPECS, IRONCLAD_SPECS};
        let mut rs = RunState::new();
        rs.character_id = CharacterId::Ironclad;
        let mut allowed: std::collections::BTreeSet<CardId> =
            IRONCLAD_SPECS.iter().map(|f| f().id).collect();
        for f in COLORLESS_SPECS {
            allowed.insert(f().id);
        }
        for id in generate_card_reward_options(&mut rs) {
            assert!(allowed.contains(&id), "leaked card id: {id:?}");
        }
    }

    #[test]
    fn boss_relic_pool_returns_up_to_three_unowned() {
        let mut rs = RunState::new();
        rs.relics.push(RelicId::Anchor); // owned — must be skipped
        rs.boss_relic_pool = vec![
            RelicId::Anchor,
            RelicId::BloodVial,
            RelicId::Strawberry,
            RelicId::Whetstone,
            RelicId::Akabeko,
        ];
        let offer = generate_boss_relic_pool(&mut rs);
        assert_eq!(offer.len(), 3);
        assert!(!offer.contains(&RelicId::Anchor));
    }

    #[test]
    fn boss_relic_pool_empty_when_unpopulated() {
        let mut rs = RunState::new();
        assert!(generate_boss_relic_pool(&mut rs).is_empty());
    }

    /// `setup_boss_relic_ancient` mirrors `DustyTome.SetupForPlayer`: exactly
    /// one `Rewards` read + a stashed Ancient card WHEN DustyTome is offered,
    /// and zero reads otherwise. This is the shared-`Rewards`-stream alignment
    /// fix — C# always consumes that read at Darv option-generation time, even
    /// when DustyTome is not the relic ultimately picked.
    #[test]
    fn setup_boss_relic_ancient_consumes_one_rewards_read_iff_dusty_tome_offered() {
        // No DustyTome → no read, no stashed card.
        let mut rs = RunState::new();
        let before = rs.rng.rewards.counter();
        setup_boss_relic_ancient(&mut rs, &[RelicId::Anchor, RelicId::BloodVial]);
        assert_eq!(
            rs.rng.rewards.counter(),
            before,
            "no read without DustyTome"
        );
        assert!(rs.dusty_tome_setup_card.is_none());

        // DustyTome offered → exactly one Rewards read, card stashed.
        let mut rs = RunState::new();
        let before = rs.rng.rewards.counter();
        setup_boss_relic_ancient(&mut rs, &[RelicId::BlackStar, RelicId::DustyTome]);
        assert_eq!(
            rs.rng.rewards.counter(),
            before + 1,
            "DustyTome setup must consume exactly one Rewards read"
        );
        assert!(
            rs.dusty_tome_setup_card.is_some(),
            "the rolled Ancient card must be stashed for listener_dusty_tome"
        );
    }

    /// The M3b roster-parity oracle helper spawns the encounter's kinds off
    /// the `niche` stream and rolls per-monster HP via the C#-faithful
    /// `starting_hp_unique` (distinct within a same-kind group). `…wb8lk.11`:
    /// v0.107.1 `AXEBOTS_NORMAL` is now a SINGLE Axebot, so the same-kind
    /// distinct-HP property is exercised against `SCROLLS_OF_BITING_NORMAL`
    /// (four `ScrollOfBiting`). Unknown encounter ids return `None`.
    #[test]
    fn dump_roster_for_encounter_spawns_kinds_and_distinct_niche_hp() {
        let mut rs = RunState::new();
        let roster = dump_roster_for_encounter(&mut rs, "AXEBOTS_NORMAL")
            .expect("AXEBOTS_NORMAL is a known encounter");
        let kinds: Vec<&str> = roster.iter().map(|&(k, _, _)| k).collect();
        assert_eq!(kinds, vec!["Axebot"], "v0.107.1: single Axebot");
        for &(_, max_hp, cur_hp) in &roster {
            assert_eq!(max_hp, cur_hp, "monsters spawn at full HP");
            assert!(max_hp > 0, "HP rolled positive");
        }
        // Same-kind monsters get distinct HP off the shared `niche` `taken`
        // set (C# `SetUniqueMonsterHpValue` excludes already-taken values).
        let scrolls = dump_roster_for_encounter(&mut rs, "SCROLLS_OF_BITING_NORMAL")
            .expect("SCROLLS_OF_BITING_NORMAL is a known encounter");
        assert!(scrolls.len() >= 2, "scrolls roster has same-kind dupes");
        let hps: std::collections::BTreeSet<i32> = scrolls.iter().map(|&(_, m, _)| m).collect();
        assert_eq!(hps.len(), scrolls.len(), "same-kind HP must be distinct");
        assert!(
            dump_roster_for_encounter(&mut rs, "NOT_A_REAL_ENCOUNTER").is_none(),
            "unknown encounter id returns None"
        );
    }

    // ── Multi-character factory wiring (sts2-rs-livck) ───────────────

    /// Each playable character routes through
    /// `new_run_manager_for_character` cleanly: the run constructs with
    /// the right starter relic + non-empty master deck + the expected
    /// max-HP per `character_starter`.
    #[test]
    fn new_run_manager_for_character_wires_starter_relic_and_deck() {
        use sts2_core::{character_starter, RelicId};
        for character in [
            CharacterId::Ironclad,
            CharacterId::Silent,
            CharacterId::Defect,
            CharacterId::Necrobinder,
            CharacterId::Regent,
        ] {
            let rm = new_run_manager_for_character(0, character);
            let expected = character_starter(character);
            assert_eq!(
                rm.run_state.max_hp, expected.hp,
                "{character:?}: max_hp matches character_starter row"
            );
            assert!(
                !rm.run_state.master_deck.is_empty(),
                "{character:?}: master_deck populated from starter factory"
            );
            // The starter relic itself is granted by `with_neow_start`
            // or the post-Neow leg in the canonical flow; the
            // RelicId::is in the `character_starter` row is the
            // contract this test pins down (downstream Neow hands out
            // the matching offer pool).
            let expected_relic = expected.starter_relic;
            match character {
                CharacterId::Ironclad => assert_eq!(expected_relic, RelicId::BurningBlood),
                CharacterId::Silent => assert_eq!(expected_relic, RelicId::RingOfTheSnake),
                CharacterId::Defect => assert_eq!(expected_relic, RelicId::CrackedCore),
                CharacterId::Necrobinder => assert_eq!(expected_relic, RelicId::BoundPhylactery),
                CharacterId::Regent => assert_eq!(expected_relic, RelicId::DivineRight),
            }
        }
    }

    /// `RunManager: Clone` mutation independence (bead `sts2-rs-40ikw.1`,
    /// P0 of the full-run-AI plan). The tier-2/3 rollout machinery and
    /// the offline labeler clone a live, fully-wired manager and drive
    /// the *clone* through candidate futures — so the clone must be a
    /// deep copy: driving it forward must leave the original untouched
    /// (hash, deck, gold, phase, RNG cursor), and the driven clone must
    /// observably diverge (so the test can't pass vacuously on a no-op
    /// driver).
    #[test]
    fn run_manager_clone_is_independent_deep_copy() {
        use sts2_core::full_run::{drive_full_run, FullRunConfig, RandomPolicy};
        use sts2_core::hashing::run_hash;

        let original = new_run_manager_for_character(7, CharacterId::Ironclad);
        let hash_before = run_hash(&original);
        let deck_before = original.run_state.master_deck.len();
        let gold_before = original.run_state.gold;
        let phase_before = original.phase();
        let rng_counter_before = original.run_state.rng.up_front.counter();

        let mut clone = original.clone();
        assert_eq!(
            run_hash(&clone),
            hash_before,
            "freshly cloned manager hashes identically to the original"
        );

        // Drive the CLONE forward a bounded number of actions.
        let mut policy = RandomPolicy::from_run_seed(7);
        let cfg = FullRunConfig {
            max_steps: 40,
            ..FullRunConfig::default()
        };
        let _report = drive_full_run(&mut clone, &mut policy, &cfg);

        // The clone must have observably diverged — otherwise the
        // "original untouched" assertions below would be vacuous.
        assert_ne!(
            run_hash(&clone),
            hash_before,
            "driven clone diverged from the pre-drive snapshot"
        );

        // The ORIGINAL is byte-for-byte where it started.
        assert_eq!(
            run_hash(&original),
            hash_before,
            "original run_hash unchanged after driving the clone"
        );
        assert_eq!(
            original.run_state.master_deck.len(),
            deck_before,
            "original master deck length unchanged"
        );
        assert_eq!(
            original.run_state.gold, gold_before,
            "original gold unchanged"
        );
        assert_eq!(original.phase(), phase_before, "original phase unchanged");
        assert_eq!(
            original.run_state.rng.up_front.counter(),
            rng_counter_before,
            "original up_front RNG cursor unchanged"
        );
    }

    /// String-seed factory (C# game-style 10-char alphanumeric seed)
    /// also accepts every playable character; canonicalised int seed
    /// matches across runs.
    #[test]
    fn new_run_manager_for_character_from_string_seed_routes_per_character() {
        let a = new_run_manager_for_character_from_string_seed("3ACCFREXKR", CharacterId::Silent);
        let b = new_run_manager_for_character_from_string_seed("3ACCFREXKR", CharacterId::Defect);
        // Same canonical seed → identical RunRngSet seeding regardless
        // of character (per-character divergence only kicks in at the
        // content-callback dispatch level, not the seed plumbing).
        assert_eq!(a.run_state.character_id, CharacterId::Silent);
        assert_eq!(b.run_state.character_id, CharacterId::Defect);
        // Both runs land with a non-empty starter deck (Silent has its
        // own factory; Defect falls back to the Ironclad starter deck
        // until `sts2-content::starters::starter_deck_for(Defect)` ships
        // real content).
        assert!(!a.run_state.master_deck.is_empty());
        assert!(!b.run_state.master_deck.is_empty());
        // Pin the per-character starter relic row — what the Neow leg
        // ultimately grants — so future char-config edits surface here.
        use sts2_core::{character_starter, RelicId};
        assert_eq!(
            character_starter(CharacterId::Silent).starter_relic,
            RelicId::RingOfTheSnake
        );
        assert_eq!(
            character_starter(CharacterId::Defect).starter_relic,
            RelicId::CrackedCore
        );
    }

    /// Ascension routing preserves character: A1+ runs for Silent get
    /// the Silent `max_hp` baseline (70), not Ironclad's 80.
    #[test]
    fn new_run_manager_for_character_with_ascension_preserves_character_hp() {
        let silent_a5 = new_run_manager_for_character_with_ascension(0, CharacterId::Silent, 5);
        assert_eq!(
            silent_a5.run_state.character_id,
            CharacterId::Silent,
            "ascension factory must preserve character routing"
        );
        // Silent's max_hp baseline is 70 (vs Ironclad's 80). A1+ doesn't
        // change max_hp directly — that's A14 territory — so the
        // baseline is what gets pinned here.
        assert_eq!(silent_a5.run_state.max_hp, 70);
    }

    /// Regression for `sts2-rs-r36am`: at ascension >= 5 the
    /// AscendersBane curse must be *appended* to the full 10-card
    /// starter (→ 11 cards), not *replace* it. The prior
    /// `populate_starter_deck` `is_empty()` guard short-circuited on the
    /// lone curse and dropped the entire starter, leaving high-ascension
    /// runs with a 1-card deck.
    #[test]
    fn ascension_appends_ascenders_bane_without_dropping_starter() {
        use sts2_core::cards::{CardId, CardType};
        let count_curses = |rm: &RunManager| {
            rm.run_state
                .master_deck
                .iter()
                .filter(|c| c.card_type == CardType::Curse)
                .count()
        };
        let count_bane = |rm: &RunManager| {
            rm.run_state
                .master_deck
                .iter()
                .filter(|c| c.card_id == CardId::AscendersBane)
                .count()
        };

        // asc 0 / asc 4: no curse, plain 10-card starter.
        for asc in [0u8, 4] {
            let rm = new_ironclad_run_manager_with_ascension(0, asc);
            assert_eq!(
                rm.run_state.master_deck.len(),
                10,
                "asc {asc}: starter must be 10 cards with no curse"
            );
            assert_eq!(count_curses(&rm), 0, "asc {asc}: no curse below A5");
        }

        // asc 5 / asc 10: 10 starter + exactly one AscendersBane = 11,
        // and the curse sits at the END (append order, for shuffle-RNG
        // parity with the C# pin).
        for asc in [5u8, 10] {
            let rm = new_ironclad_run_manager_with_ascension(0, asc);
            assert_eq!(
                rm.run_state.master_deck.len(),
                11,
                "asc {asc}: 10 starter + 1 AscendersBane"
            );
            assert_eq!(count_bane(&rm), 1, "asc {asc}: exactly one AscendersBane");
            assert_eq!(count_curses(&rm), 1, "asc {asc}: only the curse is a Curse");
            assert_eq!(
                rm.run_state.master_deck.last().map(|c| c.card_id),
                Some(CardId::AscendersBane),
                "asc {asc}: curse appended after the starter"
            );
        }
    }

    /// PARITY127 (Silent A5): the interactive deck-transform offer list
    /// must mirror C# `CardSelectCmd.FromDeckForTransformation`
    /// (`CardSelectCmd.cs:306` = `Cards.Where(c => c.Type != Quest &&
    /// c.IsTransformable)`). In the Deck pile `IsTransformable`
    /// (`CardModel.cs:576-587`) reduces to `IsRemovable`
    /// (`CardModel.cs:574` = `!Keywords.Contains(Eternal)`), so the Eternal
    /// Ascender's Bane (`AscendersBane.cs:12-17`) appended at A5+ is NOT
    /// offered. Over-offering it shifts every later deck index by one, so a
    /// fixed-index pick lands on the wrong card. The eligible-index list must
    /// therefore exclude exactly the Ascender's Bane position.
    #[test]
    fn transform_offer_excludes_eternal_ascenders_bane_at_a5() {
        use sts2_core::cards::CardId;

        let mut rm = new_run_manager_for_character_with_ascension(0, CharacterId::Silent, 5);
        // Queue a pending interactive transform (NewLeaf / AromaOfChaos shape,
        // not basic_only) so the front entry drives the offer list.
        rm.run_state
            .pending_deck_transforms
            .push(sts2_core::PendingDeckTransform::default());

        let bane_pos = rm
            .run_state
            .master_deck
            .iter()
            .position(|c| c.card_id == CardId::AscendersBane)
            .expect("A5 starter deck must contain Ascender's Bane");

        let eligible = rm.transform_eligible_deck_indices();

        assert!(
            !eligible.contains(&bane_pos),
            "Eternal Ascender's Bane (deck position {bane_pos}) must NOT be a \
             transform target (C# IsTransformable == IsRemovable in the Deck pile)"
        );
        // Every OTHER master-deck position stays eligible: only the lone
        // Eternal curse is dropped, so the offer list is deck_len - 1.
        assert_eq!(
            eligible.len(),
            rm.run_state.master_deck.len() - 1,
            "exactly one card (Ascender's Bane) excluded from the transform offer"
        );
        for (i, c) in rm.run_state.master_deck.iter().enumerate() {
            if c.card_id != CardId::AscendersBane {
                assert!(
                    eligible.contains(&i),
                    "removable card at position {i} ({:?}) must stay eligible",
                    c.card_id
                );
            }
        }
    }

    #[test]
    fn treasure_pools_snapshot_per_rarity_buckets() {
        let mut rs = RunState::new();
        rs.relic_grab_bag_by_rarity.insert(
            RelicRarity::Common,
            vec![RelicId::Anchor, RelicId::BloodVial],
        );
        rs.relic_grab_bag_by_rarity
            .insert(RelicRarity::Uncommon, vec![RelicId::Strawberry]);
        rs.relic_grab_bag_by_rarity
            .insert(RelicRarity::Rare, vec![RelicId::Akabeko]);
        let (common, uncommon, rare) = generate_treasure_relic_pools(&mut rs);
        assert_eq!(common, vec![RelicId::Anchor, RelicId::BloodVial]);
        assert_eq!(uncommon, vec![RelicId::Strawberry]);
        assert_eq!(rare, vec![RelicId::Akabeko]);
    }

    #[test]
    fn treasure_pools_missing_buckets_return_empty() {
        let mut rs = RunState::new();
        let (c, u, r) = generate_treasure_relic_pools(&mut rs);
        assert!(c.is_empty() && u.is_empty() && r.is_empty());
    }

    /// Helper: assert `spec` is `&pool[i]` for some `i` in range.
    fn assert_in_pool(spec: &EncounterSpec, pool: &'static [EncounterSpec]) {
        assert!(
            pool.iter().any(|p| std::ptr::eq(p, spec)),
            "{:?} not in pool of len {}",
            spec.name,
            pool.len()
        );
    }

    #[test]
    fn select_encounter_act1_monster_picks_from_normal_pool() {
        let mut rs = RunState::new();
        let spec = select_encounter_act1_to_3(&mut rs, 1, RoomType::Monster);
        assert_in_pool(spec, act1_normal_pool());
    }

    #[test]
    fn select_encounter_act1_boss_picks_from_boss_pool() {
        let mut rs = RunState::new();
        let spec = select_encounter_act1_to_3(&mut rs, 1, RoomType::Boss);
        assert_in_pool(spec, act1_boss_pool());
    }

    #[test]
    fn select_encounter_act1_elite_falls_back_to_normal_pool() {
        // No act1_elite_pool at the pin — fallback to act1_normal_pool.
        let mut rs = RunState::new();
        let spec = select_encounter_act1_to_3(&mut rs, 1, RoomType::Elite);
        assert_in_pool(spec, act1_normal_pool());
    }

    /// Union of every `MonsterKind` an Act-1 encounter of `tier` can
    /// spawn, sampling randomizer rows across many seeds so randomized
    /// rosters (e.g. `SLIMES_WEAK`'s medium-slime draw) are fully covered.
    fn kinds_for_tier(
        tier: crate::encounters::EncounterTier,
    ) -> std::collections::HashSet<sts2_core::MonsterKind> {
        use crate::encounters::act1_overgrowth::ACT1_ENCOUNTERS;
        use sts2_core::dotnet_rng::Rng;
        let mut set = std::collections::HashSet::new();
        for enc in ACT1_ENCOUNTERS.iter().filter(|e| e.tier == tier) {
            for seed in 0..64_i32 {
                let mut rng = Rng::new(seed);
                for (kind, _) in enc.roster(&mut rng) {
                    set.insert(kind);
                }
            }
        }
        set
    }

    #[test]
    fn build_combat_for_room_act1_weak_then_normal_by_monster_fight_index() {
        use sts2_core::MonsterKind;
        let weak = kinds_for_tier(crate::encounters::EncounterTier::Weak);
        let normal = kinds_for_tier(crate::encounters::EncounterTier::Normal);

        let mut rs = RunState::new();
        // The first three Monster fights of the act draw from the Weak
        // tier; every spawned species must be a Weak-tier monster, and
        // the per-act counter bumps each fight.
        for fight in 0..3u32 {
            let combat = build_combat_for_room(&mut rs, 1, RoomType::Monster)
                .expect("act-1 monster room builds a combat");
            let kinds: Vec<MonsterKind> =
                combat.creatures.iter().filter_map(|c| c.species).collect();
            assert!(
                !kinds.is_empty(),
                "fight {fight}: combat should spawn enemies"
            );
            for k in &kinds {
                assert!(
                    weak.contains(k),
                    "fight {fight}: {k:?} is not a Weak-tier species"
                );
            }
            assert_eq!(rs.monster_combats_this_act, fight + 1);
        }
        // The fourth Monster fight switches to the Normal tier.
        let combat = build_combat_for_room(&mut rs, 1, RoomType::Monster).unwrap();
        let kinds: Vec<MonsterKind> = combat.creatures.iter().filter_map(|c| c.species).collect();
        assert!(
            kinds.iter().any(|k| normal.contains(k)),
            "4th monster fight should pull a Normal-tier encounter, got {kinds:?}"
        );
    }

    #[test]
    fn build_combat_for_room_builds_acts_2_and_3_and_defers_noncombat() {
        // M3b: the builder now covers Acts 2–3 (previously deferred). With
        // no generated room list (raw `RunState::new`), it falls back to the
        // per-act EncounterDef tier pool and still builds a combat.
        let mut rs = RunState::new();
        assert!(
            build_combat_for_room(&mut rs, 2, RoomType::Monster).is_some(),
            "act-2 monster room should build from the ACT2 table"
        );
        let mut rs = RunState::new();
        assert!(
            build_combat_for_room(&mut rs, 3, RoomType::Boss).is_some(),
            "act-3 boss room should build from the ACT3 boss pool"
        );
        // Non-combat rooms and out-of-range acts still defer to the fallback.
        let mut rs = RunState::new();
        assert!(build_combat_for_room(&mut rs, 1, RoomType::RestSite).is_none());
        assert!(build_combat_for_room(&mut rs, 4, RoomType::Monster).is_none());
    }

    #[test]
    fn build_combat_for_room_act2_routes_to_generated_identity() {
        // M3b: with a populated `generated_rooms`, an Act-2 monster room must
        // spawn the C#-generated encounter at the room cursor — NOT a fresh
        // roll from the Weak fallback pool. `HUNTER_KILLER_NORMAL` is a
        // Normal-tier single-`HunterKiller` encounter whose species never
        // appears in the Act-2 Weak fallback pool, so seeing it proves the
        // generated path was taken (cursor 0 < 3 would otherwise pick a Weak
        // encounter).
        use sts2_core::run_room_gen::GeneratedActRooms;
        use sts2_core::MonsterKind;

        let mut rs = RunState::new();
        rs.act_index = 2;
        rs.monster_combats_this_act = 0;
        rs.generated_rooms = vec![
            GeneratedActRooms::default(), // act 1 placeholder
            GeneratedActRooms {
                normal_encounters: vec!["HUNTER_KILLER_NORMAL"],
                elite_encounters: vec![],
                boss_id: "KAISER_CRAB_BOSS",
                second_boss_id: None,
                events_shuffled: vec![],
                ancient: None,
            },
        ];

        let combat = build_combat_for_room(&mut rs, 2, RoomType::Monster)
            .expect("act-2 monster room builds a combat");
        let kinds: Vec<MonsterKind> = combat.creatures.iter().filter_map(|c| c.species).collect();
        assert_eq!(
            kinds,
            vec![MonsterKind::HunterKiller],
            "act-2 monster room routed to the generated HUNTER_KILLER_NORMAL, got {kinds:?}"
        );
        // Cursor advanced (pull-then-increment).
        assert_eq!(rs.monster_combats_this_act, 1);
    }

    /// A10 DoubleBoss: standing on the second-boss map node routes the
    /// Boss room to `second_boss_id` (C# `RoomSet.NextBossEncounter`,
    /// `RoomSet.cs:70-79`); standing anywhere else (incl. the first boss
    /// node) keeps `boss_id`.
    #[test]
    fn build_combat_for_room_second_boss_node_fights_second_boss() {
        use sts2_core::run_room_gen::GeneratedActRooms;

        let mut rs = RunState::new();
        rs.act_index = 3;
        rs.has_double_boss = true;
        rs.generated_rooms = vec![
            GeneratedActRooms::default(), // act 1
            GeneratedActRooms::default(), // act 2
            GeneratedActRooms {
                normal_encounters: vec![],
                elite_encounters: vec![],
                boss_id: "AEONGLASS_BOSS",
                second_boss_id: Some("QUEEN_BOSS"),
                events_shuffled: vec![],
                ancient: None,
            },
        ];
        // Lay out the A10 act-3 map (boss + attached second-boss node).
        rs.generate_map();
        let (boss_coord, second_coord) = {
            let map = rs.current_act_map.as_ref().expect("act-3 map");
            (
                map.point(map.boss.expect("boss")).coord,
                map.point(map.second_boss.expect("second boss")).coord,
            )
        };

        // First boss node → boss_id.
        let mut room = sts2_core::runstate::Room::new(RoomType::Boss);
        room.map_coord = Some(boss_coord);
        rs.current_room = Some(room);
        let first =
            build_combat_for_room(&mut rs, 3, RoomType::Boss).expect("first boss combat builds");
        let first_kinds: Vec<sts2_core::MonsterKind> =
            first.creatures.iter().filter_map(|c| c.species).collect();
        assert!(
            first_kinds.contains(&sts2_core::MonsterKind::Aeonglass),
            "first boss node fights the generated boss_id (Aeonglass), got {first_kinds:?}"
        );

        // Second boss node → second_boss_id.
        let mut room = sts2_core::runstate::Room::new(RoomType::Boss);
        room.map_coord = Some(second_coord);
        rs.current_room = Some(room);
        let second =
            build_combat_for_room(&mut rs, 3, RoomType::Boss).expect("second boss combat builds");
        let second_kinds: Vec<sts2_core::MonsterKind> =
            second.creatures.iter().filter_map(|c| c.species).collect();
        assert!(
            second_kinds.contains(&sts2_core::MonsterKind::Queen),
            "second boss node fights second_boss_id (Queen), got {second_kinds:?}"
        );
    }

    #[test]
    fn select_encounter_act2_and_act3_route_to_their_pools() {
        let mut rs = RunState::new();
        assert_in_pool(
            select_encounter_act1_to_3(&mut rs, 2, RoomType::Monster),
            act2_normal_pool(),
        );
        assert_in_pool(
            select_encounter_act1_to_3(&mut rs, 2, RoomType::Elite),
            act2_elite_pool(),
        );
        assert_in_pool(
            select_encounter_act1_to_3(&mut rs, 2, RoomType::Boss),
            act2_boss_pool(),
        );
        assert_in_pool(
            select_encounter_act1_to_3(&mut rs, 3, RoomType::Monster),
            act3_normal_pool(),
        );
        assert_in_pool(
            select_encounter_act1_to_3(&mut rs, 3, RoomType::Elite),
            act3_elite_pool(),
        );
        assert_in_pool(
            select_encounter_act1_to_3(&mut rs, 3, RoomType::Boss),
            act3_boss_pool(),
        );
    }

    /// Legacy direct-call boss-pin path: when a matching `pinned_act_boss`
    /// is present and the room is a Boss, `select_encounter_act1_to_3`
    /// returns the PINNED pool entry WITHOUT burning any `run_manager` RNG.
    ///
    /// Kills both `==`→`!=` mutants in the pin branch (550:18
    /// `room_type == Boss` and 552:30 `pin.act_index == act_index`): under
    /// either `!=`, the matching Boss room would skip the pin and fall to
    /// the `match` fallback, which (a) burns 2 `run_manager` rolls and
    /// (b) re-derives the index from RNG. We pin act-2 (pool len 3) at
    /// index 2 and assert both the exact returned entry AND a zero-RNG
    /// counter delta, so the fallback path is distinguishable.
    #[test]
    fn select_encounter_boss_pin_returns_pinned_entry_without_rng() {
        let mut rs = RunState::new();
        rs.act_index = 2;
        rs.pinned_act_boss = Some(PinnedActBoss {
            act_index: 2,
            encounter_index: 2,
            per_encounter_seed: 7,
            lead_kind: sts2_core::MonsterKind::Vantom,
            display_name: "PINNED",
        });
        let before = rs.rng.run_manager.counter();
        let spec = select_encounter_act1_to_3(&mut rs, 2, RoomType::Boss);
        let after = rs.rng.run_manager.counter();
        assert!(
            std::ptr::eq(spec, &act2_boss_pool()[2]),
            "matching boss pin must return pool[encounter_index], got {:?}",
            spec.name
        );
        assert_eq!(
            after, before,
            "matching boss pin must consume zero run_manager RNG"
        );
    }

    /// A pin whose `act_index` does NOT match the requested act must be
    /// ignored: the call falls through to the RNG-rolled fallback (which
    /// burns 2 `run_manager` ints). Locks the `pin.act_index == act_index`
    /// guard from the *other* direction — confirms the branch is a real
    /// equality check, not always-taken.
    #[test]
    fn select_encounter_boss_pin_for_other_act_is_ignored() {
        let mut rs = RunState::new();
        rs.act_index = 2;
        // Pin belongs to act 3, but we query act 2 → no match → fallback.
        rs.pinned_act_boss = Some(PinnedActBoss {
            act_index: 3,
            encounter_index: 0,
            per_encounter_seed: 7,
            lead_kind: sts2_core::MonsterKind::Vantom,
            display_name: "WRONG_ACT",
        });
        let before = rs.rng.run_manager.counter();
        let spec = select_encounter_act1_to_3(&mut rs, 2, RoomType::Boss);
        let after = rs.rng.run_manager.counter();
        assert_in_pool(spec, act2_boss_pool());
        assert_eq!(
            after - before,
            2,
            "non-matching pin act must fall through to the 2-roll RNG fallback"
        );
    }

    // ── Seeded-selector parity tests (bead `sts2-rs-lakuh`) ─────────

    /// Same master seed → same encounter sequence across walks.
    #[test]
    fn select_encounter_is_deterministic_under_same_seed() {
        use sts2_core::RunRngSet;
        let mut a = RunState::new();
        a.rng = RunRngSet::new(12345);
        let mut b = RunState::new();
        b.rng = RunRngSet::new(12345);
        for room_type in [RoomType::Monster, RoomType::Elite, RoomType::Boss] {
            for act in 1u8..=3 {
                let pa = select_encounter_act1_to_3(&mut a, act, room_type);
                let pb = select_encounter_act1_to_3(&mut b, act, room_type);
                assert!(
                    std::ptr::eq(pa, pb),
                    "diverged at act={act} room={room_type:?}"
                );
            }
        }
    }

    /// Different seeds explore the pool. With ≥2 act-2 normal picks
    /// across N=64 distinct master seeds, we expect at least 2 distinct
    /// EncounterSpec names (the act-2 normal pool has 11 entries, so a
    /// "first only" regression would collapse this to 1).
    #[test]
    fn select_encounter_seeded_picks_explore_pool() {
        use sts2_core::RunRngSet;
        let mut names = std::collections::HashSet::new();
        for seed in 0..64_i32 {
            let mut rs = RunState::new();
            rs.rng = RunRngSet::new(seed);
            let spec = select_encounter_act1_to_3(&mut rs, 2, RoomType::Monster);
            names.insert(spec.name);
        }
        assert!(
            names.len() >= 2,
            "encounter selector collapsed to a single spec across 64 seeds — \
             smells like a `select_first` regression. picks={names:?}"
        );
    }

    /// Each pick must consume exactly 2 run_manager rolls (the pool
    /// pick + the per-encounter seed). Locks the RNG offset so future
    /// rolls (combat_seed, gold rolls, …) stay byte-aligned with the
    /// Python pin.
    #[test]
    fn select_encounter_advances_run_manager_rng_by_two() {
        let mut rs = RunState::new();
        let before = rs.rng.run_manager.counter();
        let _ = select_encounter_act1_to_3(&mut rs, 1, RoomType::Monster);
        let after = rs.rng.run_manager.counter();
        assert_eq!(after - before, 2, "expected 2 rolls per encounter pick");
    }

    #[test]
    fn factory_plugs_into_run_manager_builder_chain() {
        // Smoke test: confirm the factory composes with RunManager.
        let rm = RunManager::new(0, CharacterId::Ironclad)
            .with_content_callbacks(ironclad_content_callbacks());
        assert!(rm.content_callbacks().enter_combat.is_some());
        assert!(rm.content_callbacks().select_encounter.is_some());
    }

    // ── Mutation-coverage tests (run_callbacks.rs survivors) ─────────
    //
    // The tests below pin function contracts the broader full-run tests
    // exercise but never *assert*, so each listed cargo-mutants survivor
    // flips an assertion here.

    use sts2_core::run_room_gen::GeneratedActRooms;

    /// `roll_transform_replacement` rolls a real replacement card off the
    /// character's transform pool (kills `-> Option<Card> with None` at 107).
    /// For Ironclad with `original = Bash`, the basic-niche path returns
    /// `Some(card)` whose id differs from the original and whose POD carries
    /// real spec stats (so it is not a `pod_defaults` shell).
    #[test]
    fn roll_transform_replacement_returns_a_real_card() {
        let mut rs = RunState::new();
        rs.character_id = CharacterId::Ironclad;
        let original = CardId::Bash;
        let card = roll_transform_replacement(&mut rs, original, None)
            .expect("Ironclad transform pool is non-empty → Some(card)");
        assert_ne!(
            card.card_id, original,
            "transform must differ from original"
        );
    }

    /// `pre_roll_act_boss` must actually PIN a boss for act 1 (kills
    /// `replace pre_roll_act_boss with ()` + `delete match arm 1`). With no
    /// generated rooms, the act-1 Boss-tier subset of `ACT1_ENCOUNTERS` is
    /// rolled off `run_manager`; the `lead_kind` must be one of the three
    /// act-1 bosses (Vantom / CeremonialBeast / KinFollower-lead Kin).
    #[test]
    fn pre_roll_act_boss_pins_act1_boss() {
        use sts2_core::MonsterKind;
        let mut rs = RunState::new();
        rs.act_index = 1;
        assert!(rs.pinned_act_boss.is_none());
        pre_roll_act_boss(&mut rs);
        let pin = rs.pinned_act_boss.as_ref().expect("act-1 boss pinned");
        assert_eq!(pin.act_index, 1);
        assert!(
            matches!(
                pin.lead_kind,
                MonsterKind::Vantom | MonsterKind::CeremonialBeast | MonsterKind::KinFollower
            ),
            "act-1 boss lead must be a real act-1 boss, got {:?}",
            pin.lead_kind
        );
    }

    /// `pre_roll_act_boss` honors the `up_front`-generated boss id over the
    /// `run_manager` roll (kills the act-1 `e.id == bid` `==`→`!=` mutant and
    /// the `e.tier == Boss` filter mutant). With `boss_id = THE_KIN_BOSS`, the
    /// pin's `lead_kind` is the Kin lead (`KinFollower`) regardless of the roll.
    #[test]
    fn pre_roll_act_boss_act1_overrides_with_generated_boss() {
        use sts2_core::MonsterKind;
        let mut rs = RunState::new();
        rs.act_index = 1;
        rs.generated_rooms = vec![GeneratedActRooms {
            normal_encounters: vec![],
            elite_encounters: vec![],
            boss_id: "THE_KIN_BOSS",
            second_boss_id: None,
            events_shuffled: vec![],
            ancient: None,
        }];
        pre_roll_act_boss(&mut rs);
        let pin = rs.pinned_act_boss.as_ref().expect("act-1 boss pinned");
        assert_eq!(
            pin.lead_kind,
            MonsterKind::KinFollower,
            "generated THE_KIN_BOSS must override the run_manager roll"
        );
        assert_eq!(pin.display_name, "The Kin");
    }

    /// Idempotency guard (kills `pin.act_index == act_index`→`!=` at 142:38):
    /// a pre-existing pin for the SAME act is left untouched and burns no RNG;
    /// a pin for a DIFFERENT act is replaced.
    #[test]
    fn pre_roll_act_boss_is_idempotent_for_pinned_act() {
        let mut rs = RunState::new();
        rs.act_index = 1;
        let sentinel = PinnedActBoss {
            act_index: 1,
            encounter_index: 99,
            per_encounter_seed: 1234,
            lead_kind: sts2_core::MonsterKind::Vantom,
            display_name: "SENTINEL",
        };
        rs.pinned_act_boss = Some(sentinel);
        let before = rs.rng.run_manager.counter();
        pre_roll_act_boss(&mut rs);
        assert_eq!(
            rs.rng.run_manager.counter(),
            before,
            "matching pin must not burn RNG"
        );
        assert_eq!(
            rs.pinned_act_boss.as_ref().unwrap().display_name,
            "SENTINEL",
            "matching pin must be preserved untouched"
        );
        // A pin for a different act IS replaced (proves the guard is act-scoped,
        // so `==`→`!=` — which would always early-return — fails).
        rs.pinned_act_boss = Some(PinnedActBoss {
            act_index: 2,
            encounter_index: 99,
            per_encounter_seed: 1234,
            lead_kind: sts2_core::MonsterKind::Vantom,
            display_name: "SENTINEL",
        });
        pre_roll_act_boss(&mut rs);
        assert_ne!(
            rs.pinned_act_boss.as_ref().unwrap().display_name,
            "SENTINEL",
            "a stale (different-act) pin must be re-rolled for act 1"
        );
        assert_eq!(rs.pinned_act_boss.as_ref().unwrap().act_index, 1);
    }

    /// Act 2/3 arm (kills `delete match arm 2 | 3` and the `act_index == 2`
    /// pool selector at 194:63). With a generated `kaiser_crab_boss`
    /// (index 2 of the act-2 boss pool), the pin must reference act-2 pool
    /// index 2 — proving both the arm ran and the act-2 (not act-3) pool was
    /// chosen.
    #[test]
    fn pre_roll_act_boss_act2_uses_act2_pool_and_generated_index() {
        let mut rs = RunState::new();
        rs.act_index = 2;
        rs.generated_rooms = vec![
            GeneratedActRooms::default(), // act 1
            GeneratedActRooms {
                normal_encounters: vec![],
                elite_encounters: vec![],
                boss_id: "kaiser_crab_boss",
                second_boss_id: None,
                events_shuffled: vec![],
                ancient: None,
            },
        ];
        pre_roll_act_boss(&mut rs);
        let pin = rs.pinned_act_boss.as_ref().expect("act-2 boss pinned");
        assert_eq!(pin.act_index, 2);
        assert_eq!(
            pin.encounter_index, 2,
            "kaiser_crab_boss is act-2 boss pool index 2"
        );
        // The act-2 pool index 2 lead is the Kaiser Crab pool entry's first member.
        let expected = act2_boss_pool()[2].members[0].kind;
        assert_eq!(pin.lead_kind, expected);
    }

    /// `next_int(0, len - 1)` must use `len - 1`, not `len / 1` (kills the
    /// `- with /` arithmetic mutant at 203:58). For the 3-entry act-2 boss
    /// pool, `len / 1 == 3` so `next_int(0, 3)` could yield index 3 →
    /// out-of-bounds panic on `&pool[idx]`. We run many seeds with NO
    /// generated override (so the raw `run_manager` roll is used) and require
    /// every pin to land on a valid in-range index without panicking.
    #[test]
    fn pre_roll_act_boss_act2_index_in_range_across_seeds() {
        use sts2_core::RunRngSet;
        let pool_len = act2_boss_pool().len();
        for seed in 0..200_i32 {
            let mut rs = RunState::new();
            rs.rng = RunRngSet::new(seed);
            rs.act_index = 2;
            // No generated_rooms → raw run_manager roll exercises `len - 1`.
            pre_roll_act_boss(&mut rs);
            let pin = rs.pinned_act_boss.as_ref().expect("pinned");
            assert!(
                pin.encounter_index < pool_len,
                "seed {seed}: index {} out of range (len {pool_len})",
                pin.encounter_index
            );
        }
    }

    /// `repin_act_boss_from_generated` re-derives the act-1 pin from the
    /// generated `boss_id`, preserving the `per_encounter_seed` and consuming
    /// no RNG (kills `replace ... with ()`, `delete match arm 1`, the
    /// `p.act_index == act_index` filter at 260:33, and the
    /// `tier == Boss && id == boss_id` guard mutants at 270).
    #[test]
    fn repin_act_boss_from_generated_act1_rederives_from_generated_id() {
        use sts2_core::MonsterKind;
        let mut rs = RunState::new();
        rs.act_index = 1;
        rs.generated_rooms = vec![GeneratedActRooms {
            normal_encounters: vec![],
            elite_encounters: vec![],
            boss_id: "CEREMONIAL_BEAST_BOSS",
            second_boss_id: None,
            events_shuffled: vec![],
            ancient: None,
        }];
        // Seed a stale pin for act 1 (wrong boss) with a known seed to preserve.
        rs.pinned_act_boss = Some(PinnedActBoss {
            act_index: 1,
            encounter_index: 0,
            per_encounter_seed: 777,
            lead_kind: MonsterKind::Vantom,
            display_name: "WRONG",
        });
        let before = rs.rng.run_manager.counter();
        repin_act_boss_from_generated(&mut rs);
        assert_eq!(
            rs.rng.run_manager.counter(),
            before,
            "repin consumes no RNG"
        );
        let pin = rs.pinned_act_boss.as_ref().expect("repinned");
        assert_eq!(pin.lead_kind, MonsterKind::CeremonialBeast);
        assert_eq!(pin.display_name, "Ceremonial Beast");
        assert_eq!(pin.per_encounter_seed, 777, "preserves the pinned seed");
    }

    /// GH #544: the act-1 boss pin must come from the act-1 table THIS RUN
    /// walks. Against the Overgrowth table an Underdocks boss id is simply not
    /// found, so `repin` early-returned and the pre-roll's Overgrowth pin
    /// survived — the map then displayed, and `pinned_boss_encounter_id`
    /// reported, a boss the run never fights (`Vantom` for a run whose boss is
    /// `LAGAVULIN_MATRIARCH_BOSS`). The FIGHT was always right, because
    /// `build_combat_for_room` resolves through `act_encounter_table`; only
    /// the pin was wrong, which is why `--underdocks-parity` stayed green.
    #[test]
    fn repin_act_boss_from_generated_act1_uses_the_runs_own_act1_table() {
        use sts2_core::act_selection::ActVariant;
        use sts2_core::MonsterKind;
        let mut rs = RunState::new();
        rs.act_index = 1;
        rs.act1 = ActVariant::Underdocks;
        rs.generated_rooms = vec![GeneratedActRooms {
            normal_encounters: vec![],
            elite_encounters: vec![],
            boss_id: "LAGAVULIN_MATRIARCH_BOSS",
            second_boss_id: None,
            events_shuffled: vec![],
            ancient: None,
        }];
        // The exact stale state the bug left behind: an Overgrowth pin.
        rs.pinned_act_boss = Some(PinnedActBoss {
            act_index: 1,
            encounter_index: 0,
            per_encounter_seed: 777,
            lead_kind: MonsterKind::Vantom,
            display_name: "Vantom",
        });
        repin_act_boss_from_generated(&mut rs);
        let pin = rs.pinned_act_boss.as_ref().expect("repinned");
        assert_ne!(
            pin.display_name, "Vantom",
            "the Overgrowth pin must not survive an Underdocks repin"
        );
        assert_eq!(pin.display_name, "Lagavulin Matriarch");
        assert_eq!(pin.per_encounter_seed, 777, "preserves the pinned seed");
    }

    /// The pre-roll draws act 1's boss from the run's own act-1 table, and
    /// costs the SAME two `run_manager` reads either way.
    ///
    /// The read count is the load-bearing half: both act-1 boss pools hold
    /// exactly three entries, so `next_int(0, len - 1)` draws the same width
    /// and the stream stays byte-identical. If a future act-1 model ships a
    /// different boss count this test fails, which is the warning that the
    /// pin fix has become a desync.
    #[test]
    fn pre_roll_act_boss_draws_from_the_runs_own_act1_table_at_equal_cost() {
        use crate::encounters::{act1_overgrowth, act1_underdocks, EncounterTier};
        use sts2_core::act_selection::ActVariant;
        // Name the two tables CONCRETELY rather than via `act1_encounters`.
        // Deriving the expectation from the same helper under test makes the
        // assertion self-consistent under the very bug it guards: point the
        // helper at Overgrowth for both variants and a helper-derived
        // expectation still agrees with itself.
        let cases = [
            (ActVariant::Overgrowth, act1_overgrowth::ACT1_ENCOUNTERS),
            (
                ActVariant::Underdocks,
                act1_underdocks::ACT1_UNDERDOCKS_ENCOUNTERS,
            ),
        ];
        let mut costs = Vec::new();
        for (variant, table) in cases {
            let bosses: Vec<&str> = table
                .iter()
                .filter(|e| e.tier == EncounterTier::Boss)
                .map(|e| e.id)
                .collect();
            assert_eq!(bosses.len(), 3, "{variant:?} act-1 boss pool");

            let mut rs = RunState::new();
            rs.act_index = 1;
            rs.act1 = variant;
            let before = rs.rng.run_manager.counter();
            pre_roll_act_boss(&mut rs);
            costs.push(rs.rng.run_manager.counter() - before);

            let pin = rs.pinned_act_boss.as_ref().expect("pinned");
            let picked = table[pin.encounter_index];
            assert!(
                bosses.contains(&picked.id),
                "{variant:?} pinned {} which is not in its own boss pool",
                picked.id
            );
        }
        assert_eq!(costs[0], costs[1], "both act-1 models cost the same reads");
    }

    /// Repin no-ops when the pin belongs to a DIFFERENT act (kills the
    /// `p.act_index == act_index`→`!=` mutant: under `!=` it would wrongly
    /// proceed using a foreign-act seed).
    #[test]
    fn repin_act_boss_no_op_for_mismatched_pin_act() {
        let mut rs = RunState::new();
        rs.act_index = 1;
        rs.generated_rooms = vec![GeneratedActRooms {
            normal_encounters: vec![],
            elite_encounters: vec![],
            boss_id: "CEREMONIAL_BEAST_BOSS",
            second_boss_id: None,
            events_shuffled: vec![],
            ancient: None,
        }];
        rs.pinned_act_boss = Some(PinnedActBoss {
            act_index: 2, // belongs to a different act
            encounter_index: 0,
            per_encounter_seed: 777,
            lead_kind: sts2_core::MonsterKind::Vantom,
            display_name: "WRONG",
        });
        repin_act_boss_from_generated(&mut rs);
        assert_eq!(
            rs.pinned_act_boss.as_ref().unwrap().display_name,
            "WRONG",
            "no act-1 pin exists → repin must leave the foreign pin untouched"
        );
    }

    /// `repin_act_boss_from_generated` act-2/3 arm (kills `delete match arm
    /// 2 | 3` and the `act_index == 2` pool selector at 286:63). A generated
    /// `kaiser_crab_boss` re-derives to act-2 boss pool index 2.
    #[test]
    fn repin_act_boss_from_generated_act2_uses_act2_pool() {
        let mut rs = RunState::new();
        rs.act_index = 2;
        rs.generated_rooms = vec![
            GeneratedActRooms::default(),
            GeneratedActRooms {
                normal_encounters: vec![],
                elite_encounters: vec![],
                boss_id: "kaiser_crab_boss",
                second_boss_id: None,
                events_shuffled: vec![],
                ancient: None,
            },
        ];
        rs.pinned_act_boss = Some(PinnedActBoss {
            act_index: 2,
            encounter_index: 0,
            per_encounter_seed: 555,
            lead_kind: sts2_core::MonsterKind::Vantom,
            display_name: "WRONG",
        });
        repin_act_boss_from_generated(&mut rs);
        let pin = rs.pinned_act_boss.as_ref().expect("repinned");
        assert_eq!(pin.encounter_index, 2);
        assert_eq!(pin.lead_kind, act2_boss_pool()[2].members[0].kind);
        assert_eq!(pin.per_encounter_seed, 555);
    }

    /// `select_encounter_act1_to_3` honors a matching boss pin (kills the
    /// `room_type == Boss` guard at 550, the `pin.act_index == act_index`
    /// guard at 552, and EACH act-1/2/3 boss-pool match-arm deletion at
    /// 556/554/555). For every act, a matching pin at index 1 must return
    /// that act's boss pool entry 1 — deleting any arm drops through to the
    /// `_ => act1_normal_pool()` fallback, so the returned spec would no
    /// longer be in the act's boss pool.
    #[test]
    fn select_encounter_boss_honors_matching_pin() {
        let pools: [(u8, &'static [EncounterSpec]); 3] = [
            (1, act1_boss_pool()),
            (2, act2_boss_pool()),
            (3, act3_boss_pool()),
        ];
        for (act, pool) in pools {
            let idx = 1usize.min(pool.len() - 1);
            let mut rs = RunState::new();
            rs.pinned_act_boss = Some(PinnedActBoss {
                act_index: act,
                encounter_index: idx,
                per_encounter_seed: 0,
                lead_kind: sts2_core::MonsterKind::Vantom,
                display_name: "x",
            });
            let spec = select_encounter_act1_to_3(&mut rs, act, RoomType::Boss);
            assert!(
                std::ptr::eq(spec, &pool[idx]),
                "act {act}: matching pin must return that act's boss pool entry {idx}"
            );
        }
    }

    /// A boss pin for a DIFFERENT act is ignored — the picker falls through to
    /// the seeded pool roll (kills `pin.act_index == act_index`→`!=`, which
    /// would mis-index act 1's pin into the act-3 boss pool). The returned spec
    /// must be a valid act-3 boss entry.
    #[test]
    fn select_encounter_boss_ignores_mismatched_pin_act() {
        let mut rs = RunState::new();
        rs.pinned_act_boss = Some(PinnedActBoss {
            act_index: 1, // mismatched vs the act-3 query below
            encounter_index: 0,
            per_encounter_seed: 0,
            lead_kind: sts2_core::MonsterKind::Vantom,
            display_name: "x",
        });
        let spec = select_encounter_act1_to_3(&mut rs, 3, RoomType::Boss);
        assert_in_pool(spec, act3_boss_pool());
    }

    /// Act-1 Monster and Elite both route to the act-1 NORMAL pool (kills the
    /// `delete match arm (1, RoomType::Monster)` and `(1, RoomType::Elite)`
    /// mutants — deleting either would drop through to the `_` arm, which also
    /// returns `act1_normal_pool()`, so to make the arms observable we assert
    /// the RNG-advance contract still holds AND the pool is normal). The
    /// distinct assertion that kills the arm deletion is that BOTH room types
    /// resolve to the normal pool deterministically.
    #[test]
    fn select_encounter_act1_monster_and_elite_use_normal_pool() {
        let mut rs = RunState::new();
        let m = select_encounter_act1_to_3(&mut rs, 1, RoomType::Monster);
        assert_in_pool(m, act1_normal_pool());
        let mut rs = RunState::new();
        let e = select_encounter_act1_to_3(&mut rs, 1, RoomType::Elite);
        assert_in_pool(e, act1_normal_pool());
    }

    /// `build_combat_for_room` tier selection: the first three act-1 Monster
    /// fights are Weak, the 4th flips to Normal (kills `< with <=` at 641 —
    /// under `<=` the 4th fight at counter==3 would still draw the Weak pool).
    /// Uses the raw fallback path (no generated rooms) so `tier` drives the
    /// pool. The act-1 Weak and Normal species sets overlap, so the
    /// distinguishing signal is a Normal-ONLY species: across a seed sweep the
    /// 4th fight must AT LEAST ONCE spawn a species that no Weak encounter can
    /// produce — impossible under the `<=` mutant (4th stays Weak).
    #[test]
    fn build_combat_for_room_weak_normal_boundary_is_strict_lt() {
        use sts2_core::MonsterKind;
        use sts2_core::RunRngSet;
        let weak = kinds_for_tier(crate::encounters::EncounterTier::Weak);
        let normal = kinds_for_tier(crate::encounters::EncounterTier::Normal);
        let normal_only: std::collections::HashSet<MonsterKind> =
            normal.difference(&weak).copied().collect();
        assert!(
            !normal_only.is_empty(),
            "test premise: there exist Normal-only act-1 species"
        );

        let mut saw_normal_only_on_fourth = false;
        for seed in 0..64_i32 {
            let mut rs = RunState::new();
            rs.rng = RunRngSet::new(seed);
            // Fights 1-3: every spawned species must be Weak-tier.
            for _ in 0..3 {
                let combat = build_combat_for_room(&mut rs, 1, RoomType::Monster).unwrap();
                for k in combat.creatures.iter().filter_map(|c| c.species) {
                    assert!(
                        weak.contains(&k),
                        "seed {seed}: fights 1-3 must be Weak-tier, got {k:?}"
                    );
                }
            }
            // 4th fight (counter==3) is Normal tier under `< 3`.
            let combat = build_combat_for_room(&mut rs, 1, RoomType::Monster).unwrap();
            let kinds: Vec<MonsterKind> =
                combat.creatures.iter().filter_map(|c| c.species).collect();
            if kinds.iter().any(|k| normal_only.contains(k)) {
                saw_normal_only_on_fourth = true;
                break;
            }
        }
        assert!(
            saw_normal_only_on_fourth,
            "the 4th monster fight never escaped the Weak species set across 64 seeds — \
             boundary is not strictly `< 3` (a `<= 3` mutant keeps the 4th fight Weak)"
        );
    }

    /// Elite room → Elite tier (kills `delete match arm RoomType::Elite` at
    /// 647 and the `delete match arm RoomType::Elite` cursor/counter arms at
    /// 732 & 801, plus the `+= with *=` at 801:54). Routes through a generated
    /// elite encounter and asserts the cursor advances additively.
    #[test]
    fn build_combat_for_room_elite_routes_and_advances_cursor() {
        use sts2_core::MonsterKind;
        let mut rs = RunState::new();
        rs.act_index = 2;
        rs.elite_combats_this_act = 1; // pull-then-increment from a non-1 base
        rs.generated_rooms = vec![
            GeneratedActRooms::default(),
            GeneratedActRooms {
                normal_encounters: vec![],
                // cursor 1 (elite_combats_this_act) indexes the 2nd entry.
                elite_encounters: vec!["HUNTER_KILLER_NORMAL", "HUNTER_KILLER_NORMAL"],
                boss_id: "kaiser_crab_boss",
                second_boss_id: None,
                events_shuffled: vec![],
                ancient: None,
            },
        ];
        let combat = build_combat_for_room(&mut rs, 2, RoomType::Elite)
            .expect("act-2 elite room builds a combat");
        let kinds: Vec<MonsterKind> = combat.creatures.iter().filter_map(|c| c.species).collect();
        assert_eq!(kinds, vec![MonsterKind::HunterKiller]);
        // `+= 1` from 1 → 2 (a `*= 1` mutant would leave it at 1).
        assert_eq!(
            rs.elite_combats_this_act, 2,
            "elite cursor must advance additively (1 += 1 == 2)"
        );
    }

    /// The generated-cursor index uses `cursor % list.len()` (kills `% with /`
    /// at 738). With a 2-entry elite list and `cursor = 1`, `1 % 2 == 1`
    /// picks the 2nd entry; a `1 / 2 == 0` mutant would pick the 1st. The two
    /// list entries are DISTINCT encounters so the choice is observable.
    #[test]
    fn build_combat_for_room_cursor_uses_modulo() {
        use sts2_core::MonsterKind;
        let mut rs = RunState::new();
        rs.act_index = 2;
        rs.monster_combats_this_act = 1; // cursor 1
        rs.generated_rooms = vec![
            GeneratedActRooms::default(),
            GeneratedActRooms {
                // index 0 = AXEBOTS_NORMAL (Axebot), index 1 = HUNTER_KILLER_NORMAL.
                normal_encounters: vec!["AXEBOTS_NORMAL", "HUNTER_KILLER_NORMAL"],
                elite_encounters: vec![],
                boss_id: "kaiser_crab_boss",
                second_boss_id: None,
                events_shuffled: vec![],
                ancient: None,
            },
        ];
        let combat = build_combat_for_room(&mut rs, 2, RoomType::Monster).unwrap();
        let kinds: Vec<MonsterKind> = combat.creatures.iter().filter_map(|c| c.species).collect();
        assert_eq!(
            kinds,
            vec![MonsterKind::HunterKiller],
            "cursor 1 % 2 == 1 must select the 2nd entry (HUNTER_KILLER_NORMAL)"
        );
    }

    /// Boss-room pin path resolves identity from the generated `boss_id`
    /// (kills the `room_type == Boss` `==`→`!=` at 719 — under `!=` a Boss
    /// room never reaches this branch). A matching act-2 pin + generated
    /// `kaiser_crab_boss` must fight Kaiser Crab.
    #[test]
    fn build_combat_for_room_boss_pin_uses_generated_identity() {
        use sts2_core::MonsterKind;
        let mut rs = RunState::new();
        rs.act_index = 2;
        rs.generated_rooms = vec![
            GeneratedActRooms::default(),
            GeneratedActRooms {
                normal_encounters: vec![],
                elite_encounters: vec![],
                boss_id: "KAISER_CRAB_BOSS",
                second_boss_id: None,
                events_shuffled: vec![],
                ancient: None,
            },
        ];
        rs.pinned_act_boss = Some(PinnedActBoss {
            act_index: 2,
            encounter_index: 0,
            per_encounter_seed: 0,
            lead_kind: MonsterKind::Vantom,
            display_name: "x",
        });
        let combat = build_combat_for_room(&mut rs, 2, RoomType::Boss).unwrap();
        let kinds: Vec<MonsterKind> = combat.creatures.iter().filter_map(|c| c.species).collect();
        assert!(
            kinds.contains(&MonsterKind::Crusher) && kinds.contains(&MonsterKind::Rocket),
            "act-2 boss pin must fight the generated KAISER_CRAB_BOSS (Crusher + Rocket), got {kinds:?}"
        );
    }

    /// The boss-pin match-guard `Some(pin) if pin.act_index == act_index`
    /// (714) decides whether the RNG-free pinned path or the RNG-rolling
    /// fallback is taken. With NO generated boss (raw-core), the pinned arm
    /// consumes ZERO `run_manager` ints while the fallback consumes ONE.
    ///
    /// * Matching pin → 0 rolls. Kills `guard → false` and `== → !=`
    ///   (both would drop to the `_` arm and burn a roll).
    /// * Mismatched pin → 1 roll. Kills `guard → true` (which would wrongly
    ///   take the RNG-free arm for a foreign-act pin).
    #[test]
    fn build_combat_for_room_boss_guard_gates_rng_consumption() {
        // First, obtain a VALID act-1 boss `encounter_index` by pre-rolling
        // against a generated boss, then drop the generated list so the live
        // call falls onto the `table.get(encounter_index)` pinned path.
        let mut rs = RunState::new();
        rs.act_index = 1;
        rs.generated_rooms = vec![GeneratedActRooms {
            normal_encounters: vec![],
            elite_encounters: vec![],
            boss_id: "THE_KIN_BOSS",
            second_boss_id: None,
            events_shuffled: vec![],
            ancient: None,
        }];
        pre_roll_act_boss(&mut rs);
        let valid_pin = rs.pinned_act_boss.clone().expect("pinned");
        rs.generated_rooms.clear(); // gen_boss → None now

        // Matching pin → RNG-free pinned arm.
        let before = rs.rng.run_manager.counter();
        let combat = build_combat_for_room(&mut rs, 1, RoomType::Boss).expect("boss combat");
        assert_eq!(
            rs.rng.run_manager.counter(),
            before,
            "a matching boss pin must consume ZERO run_manager rolls"
        );
        let kinds: Vec<sts2_core::MonsterKind> =
            combat.creatures.iter().filter_map(|c| c.species).collect();
        assert!(
            kinds.contains(&sts2_core::MonsterKind::KinFollower)
                || kinds.contains(&sts2_core::MonsterKind::KinPriest),
            "pinned act-1 boss must fight The Kin, got {kinds:?}"
        );

        // Mismatched pin (foreign act) → fallback arm consumes one roll.
        let mut rs2 = RunState::new();
        rs2.act_index = 1;
        let mut foreign = valid_pin;
        foreign.act_index = 2; // mismatched vs the act-1 query
        rs2.pinned_act_boss = Some(foreign);
        let before = rs2.rng.run_manager.counter();
        let _ = build_combat_for_room(&mut rs2, 1, RoomType::Boss).expect("boss combat");
        assert_eq!(
            rs2.rng.run_manager.counter() - before,
            1,
            "a non-matching boss pin must fall to the fallback (one run_manager roll)"
        );
    }

    /// The no-pin boss fallback rolls `next_int(0, len - 1)` over the boss
    /// pool (kills `- with /` at 725:62 — `len / 1 == len` would let
    /// `next_int(0, len)` return `len`, indexing `pool[len]` out of bounds and
    /// panicking). With no pin and no generated boss, every seed must build a
    /// valid combat without panic and spawn a real act-1 boss species.
    #[test]
    fn build_combat_for_room_boss_no_pin_fallback_index_in_range() {
        use sts2_core::MonsterKind;
        use sts2_core::RunRngSet;
        for seed in 0..200_i32 {
            let mut rs = RunState::new();
            rs.rng = RunRngSet::new(seed);
            rs.act_index = 1;
            // No pin, no generated_rooms → the `_ =>` boss fallback rolls
            // `next_int(0, len - 1)` and indexes `pool[idx]`.
            assert!(rs.pinned_act_boss.is_none());
            let combat = build_combat_for_room(&mut rs, 1, RoomType::Boss)
                .unwrap_or_else(|| panic!("seed {seed}: boss fallback must build a combat"));
            let kinds: Vec<MonsterKind> =
                combat.creatures.iter().filter_map(|c| c.species).collect();
            assert!(
                kinds.iter().any(|k| matches!(
                    k,
                    MonsterKind::Vantom | MonsterKind::CeremonialBeast | MonsterKind::KinFollower
                )),
                "seed {seed}: boss fallback must spawn an act-1 boss species, got {kinds:?}"
            );
        }
    }

    /// The enc-RNG seed adds `total_floor + act_index` (kills `+ with *` and
    /// `+ with -` at 784). For a randomizer encounter, a different seed yields
    /// a different roster; `total_floor + act_index` (= 0 + 2 = 2) must differ
    /// from `total_floor * act_index` (= 0) and `total_floor - act_index`
    /// (= -2) at least somewhere. We compare the rosters `build_combat_for_room`
    /// produces against rosters generated with the mutant seeds and require
    /// the live builder to match the `+` formula.
    #[test]
    fn build_combat_for_room_seed_uses_total_floor_plus_act_index() {
        use crate::encounters::find;
        use sts2_core::dotnet_rng::Rng;
        // SLIMES_WEAK is an act-1 randomizer encounter (roster varies by seed).
        let enc = find("SLIMES_WEAK").expect("SLIMES_WEAK encounter exists");

        let mut rs = RunState::new();
        rs.act_index = 1;
        rs.monster_combats_this_act = 0; // Weak tier
        rs.generated_rooms = vec![GeneratedActRooms {
            normal_encounters: vec!["SLIMES_WEAK"],
            elite_encounters: vec![],
            boss_id: "VANTOM_BOSS",
            second_boss_id: None,
            events_shuffled: vec![],
            ancient: None,
        }];
        let seed = rs.rng.seed;
        let combat = build_combat_for_room(&mut rs, 1, RoomType::Monster).unwrap();
        let live_kinds: Vec<_> = combat.creatures.iter().filter_map(|c| c.species).collect();

        // Reproduce the roster the `+ act_index` formula prescribes.
        let roster_for = |base: i32| -> Vec<sts2_core::MonsterKind> {
            let mut rng = Rng::with_name(base, enc.id);
            enc.roster(&mut rng).iter().map(|&(k, _)| k).collect()
        };
        // total_floor == 0, act_index == 1.
        let plus = roster_for(seed.wrapping_add(0_i32.wrapping_add(1)));
        let times = roster_for(seed.wrapping_add(0_i32.wrapping_mul(1)));
        let minus = roster_for(seed.wrapping_add(0_i32.wrapping_sub(1)));
        assert_eq!(
            live_kinds, plus,
            "live builder must use total_floor + act_index"
        );
        // The `+` result must be distinguishable from at least one mutant to
        // make the mutation observable; with act_index 1 the difference is
        // base+1 vs base+0 vs base-1.
        assert!(
            plus != times || plus != minus,
            "SLIMES_WEAK roster too seed-insensitive to distinguish the seed mutation; \
             test premise broken (live={live_kinds:?})"
        );
    }

    /// `dump_roster_for_encounter` seeds the enc-RNG with `total_floor + 1`
    /// (kills `+ with *` and `+ with -` at 871). On a fresh state
    /// (`total_floor == 0`) the roster must match `Rng::with_name(seed + 1)`,
    /// not `seed + 0` or `seed - 1`, for a randomizer encounter.
    #[test]
    fn dump_roster_for_encounter_seed_uses_total_floor_plus_one() {
        use crate::encounters::find;
        use sts2_core::dotnet_rng::Rng;
        let enc = find("SLIMES_WEAK").expect("SLIMES_WEAK exists");
        let mut rs = RunState::new();
        let seed = rs.rng.seed;
        let dumped = dump_roster_for_encounter(&mut rs, "SLIMES_WEAK").expect("known encounter");
        let dumped_ids: Vec<&'static str> = dumped.iter().map(|&(id, _, _)| id).collect();

        let kinds_for = |base: i32| -> Vec<&'static str> {
            let mut rng = Rng::with_name(base, enc.id);
            enc.roster(&mut rng)
                .iter()
                .map(|&(k, _)| k.python_name())
                .collect()
        };
        let plus1 = kinds_for(seed.wrapping_add(1));
        let plus0 = kinds_for(seed);
        let minus1 = kinds_for(seed.wrapping_sub(1));
        assert_eq!(
            dumped_ids, plus1,
            "dump must use total_floor + 1 for the enc seed"
        );
        assert!(
            plus1 != plus0 || plus1 != minus1,
            "SLIMES_WEAK roster too seed-insensitive to observe the +1 (ids={dumped_ids:?})"
        );
    }

    /// `generate_card_reward_options` is room-type-scoped: a Boss room rolls
    /// the all-Rare BossEncounter context (kills `delete match arm
    /// Some(RoomType::Elite)` only indirectly — but primarily pins that the
    /// `current_room` room-type drives the context). We assert Boss context
    /// yields all-Rare cards while the default (regular) context, at the same
    /// seed, does not — proving room-type selection is live.
    #[test]
    fn card_reward_context_is_room_type_scoped() {
        use crate::cards::spec_index::rarity_for_card_id;
        use crate::cards::CardRarity;

        let all_rare = |rs: &mut RunState| -> bool {
            generate_card_reward_options(rs)
                .iter()
                .all(|id| rarity_for_card_id(*id) == Some(CardRarity::Rare))
        };

        // Boss room → BossEncounter context → all 3 cards Rare.
        let mut rs = RunState::new();
        rs.character_id = CharacterId::Ironclad;
        let mut boss_room = sts2_core::runstate::Room::new(RoomType::Boss);
        boss_room.room_type = RoomType::Boss;
        rs.current_room = Some(boss_room);
        assert!(all_rare(&mut rs), "boss room reward must be all-Rare");

        // Regular room (no current_room) at many seeds is NOT reliably all-Rare;
        // a single all-Rare regular roll is astronomically unlikely, so a sweep
        // proves the context actually differs.
        use sts2_core::RunRngSet;
        let mut any_non_rare = false;
        for seed in 0..32_i32 {
            let mut rs = RunState::new();
            rs.character_id = CharacterId::Ironclad;
            rs.rng = RunRngSet::new(seed);
            rs.current_room = None; // → "regular" context
            if !all_rare(&mut rs) {
                any_non_rare = true;
                break;
            }
        }
        assert!(
            any_non_rare,
            "regular-context rewards should include non-Rare cards (context not applied?)"
        );
    }

    /// Elite room → EliteEncounter context (kills `delete match arm
    /// Some(RoomType::Elite)` at 1160). The elite context's rare-rate is
    /// elevated vs regular, so across a seed sweep elite rewards must yield a
    /// strictly higher Rare count than regular rewards at the same seeds —
    /// deleting the Elite arm collapses elite to regular, equalizing the
    /// counts.
    #[test]
    fn card_reward_elite_context_raises_rare_rate() {
        use crate::cards::spec_index::rarity_for_card_id;
        use crate::cards::CardRarity;
        use sts2_core::RunRngSet;

        let rare_count = |room: Option<RoomType>| -> usize {
            let mut total = 0usize;
            for seed in 0..200_i32 {
                let mut rs = RunState::new();
                rs.character_id = CharacterId::Ironclad;
                rs.rng = RunRngSet::new(seed);
                rs.current_room = room.map(|rt| {
                    let mut r = sts2_core::runstate::Room::new(rt);
                    r.room_type = rt;
                    r
                });
                total += generate_card_reward_options(&mut rs)
                    .iter()
                    .filter(|id| rarity_for_card_id(**id) == Some(CardRarity::Rare))
                    .count();
            }
            total
        };
        let regular = rare_count(None);
        let elite = rare_count(Some(RoomType::Elite));
        assert!(
            elite > regular,
            "elite context must raise the Rare count vs regular (elite={elite}, regular={regular})"
        );
    }
}
